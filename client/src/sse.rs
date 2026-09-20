//! SSE task: owns the connection, reconnects with backoff, forwards parsed events

use std::time::Duration;

use eventsource_stream::Eventsource;
use futures::StreamExt;
use proto::ServerEvent;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::api::{Api, ClientError};

#[derive(Debug)]
#[allow(
    clippy::large_enum_variant,
    reason = "channel messages, consumed immediately"
)]
pub enum StreamEvent {
    Connected,
    Disconnected {
        reason: String,
        retry_in: Duration,
    },
    Event {
        seq: Option<u64>,
        event: ServerEvent,
    },
    /// Not recoverable by retrying (auth, membership, outdated)
    Fatal(ClientError),
}

const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// +-25% jitter from clock noise; avoids a rand dependency
fn jitter(d: Duration) -> Duration {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |t| t.subsec_nanos());
    let factor = 0.75 + f64::from(nanos % 1000) / 2000.0; // 0.75 ..= 1.25
    d.mul_f64(factor)
}

/// Abort the handle to stop (room switch)
pub fn spawn(
    api: Api,
    room_id: String,
    last_event_id: Option<u64>,
    tx: mpsc::Sender<StreamEvent>,
) -> JoinHandle<()> {
    tokio::spawn(run(api, room_id, last_event_id, tx))
}

async fn run(api: Api, room_id: String, mut last_id: Option<u64>, tx: mpsc::Sender<StreamEvent>) {
    let mut backoff = BACKOFF_MIN;
    loop {
        match connect_once(&api, &room_id, &mut last_id, &tx, &mut backoff).await {
            Ok(()) => {}
            Err(Stop::ChannelClosed) => return,
            Err(Stop::Fatal(e)) => {
                let _ = tx.send(StreamEvent::Fatal(e)).await;
                return;
            }
        }
        let wait = jitter(backoff);
        tracing::info!(room = %room_id, ?wait, "reconnecting event stream");
        tokio::time::sleep(wait).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

enum Stop {
    ChannelClosed,
    Fatal(ClientError),
}

/// `Ok` = stream ended, retry appropriate
async fn connect_once(
    api: &Api,
    room_id: &str,
    last_id: &mut Option<u64>,
    tx: &mpsc::Sender<StreamEvent>,
    backoff: &mut Duration,
) -> Result<(), Stop> {
    let resp = match api.events_request(room_id, *last_id).send().await {
        Ok(r) => r,
        Err(e) => {
            let e = ClientError::from(e);
            report_disconnect(tx, e.to_string(), *backoff).await?;
            return Ok(());
        }
    };
    if !resp.status().is_success() {
        let err = Api::error_from(resp).await;
        if err.is_fatal_for_stream() {
            return Err(Stop::Fatal(err));
        }
        report_disconnect(tx, err.to_string(), *backoff).await?;
        return Ok(());
    }

    tx.send(StreamEvent::Connected)
        .await
        .map_err(|_| Stop::ChannelClosed)?;
    *backoff = BACKOFF_MIN;

    let mut stream = resp.bytes_stream().eventsource();
    while let Some(item) = stream.next().await {
        match item {
            Ok(frame) => {
                let seq = frame.id.trim().parse::<u64>().ok();
                if frame.data.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<ServerEvent>(&frame.data) {
                    Ok(event) => {
                        if seq.is_some() {
                            *last_id = seq;
                        }
                        tx.send(StreamEvent::Event { seq, event })
                            .await
                            .map_err(|_| Stop::ChannelClosed)?;
                    }
                    Err(e) => {
                        tracing::warn!(kind = %frame.event, error = %e, "unparseable server event");
                    }
                }
            }
            Err(e) => {
                report_disconnect(tx, e.to_string(), *backoff).await?;
                return Ok(());
            }
        }
    }
    report_disconnect(tx, "stream ended".to_owned(), *backoff).await?;
    Ok(())
}

async fn report_disconnect(
    tx: &mpsc::Sender<StreamEvent>,
    reason: String,
    retry_in: Duration,
) -> Result<(), Stop> {
    tx.send(StreamEvent::Disconnected { reason, retry_in })
        .await
        .map_err(|_| Stop::ChannelClosed)
}
