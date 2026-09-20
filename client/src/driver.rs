//! Network glue under the UIs: performs actions, feeds stream events into the
//! session

use proto::{ErrorCode, RoomSnapshot};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::api::{Api, ClientError};
use crate::config::{LocalState, Paths};
use crate::session::{Action, Connection, Session};
use crate::sse::{self, StreamEvent};

#[derive(Debug)]
#[allow(
    clippy::large_enum_variant,
    reason = "channel messages, consumed immediately"
)]
pub enum AppMsg {
    Stream(StreamEvent),
    Ack(Result<(), ClientError>),
    Resync(Result<RoomSnapshot, ClientError>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Exit {
    Normal,
    Outdated { min: String },
    Fatal(String),
}

#[derive(Debug)]
pub struct Driver {
    pub session: Session,
    api: Api,
    paths: Paths,
    tx: mpsc::Sender<AppMsg>,
    pub rx: mpsc::Receiver<AppMsg>,
    stream: Option<JoinHandle<()>>,
    pub exit: Option<Exit>,
}

impl Driver {
    pub fn new(api: Api, session: Session, paths: Paths) -> Self {
        let (tx, rx) = mpsc::channel(256);
        let mut d = Self {
            session,
            api,
            paths,
            tx,
            rx,
            stream: None,
            exit: None,
        };
        d.remember_room();
        d.restart_stream();
        d
    }

    fn restart_stream(&mut self) {
        if let Some(h) = self.stream.take() {
            h.abort();
        }
        let (stx, mut srx) = mpsc::channel::<StreamEvent>(64);
        let handle = sse::spawn(
            self.api.clone(),
            self.session.room_id.clone(),
            self.session.last_event_id,
            stx,
        );
        let tx = self.tx.clone();
        tokio::spawn(async move {
            while let Some(ev) = srx.recv().await {
                if tx.send(AppMsg::Stream(ev)).await.is_err() {
                    break;
                }
            }
        });
        self.stream = Some(handle);
    }

    fn update_state(&self, f: impl FnOnce(&mut LocalState)) {
        let mut state = self.paths.load_state();
        f(&mut state);
        if let Err(e) = self.paths.save_state(&state) {
            tracing::warn!(error = %e, "could not save state.toml");
        }
    }

    fn remember_room(&self) {
        let room = self.session.room_id.clone();
        let server = self.api.base().to_owned();
        self.update_state(|s| {
            s.last_room_id = Some(room);
            s.last_server = Some(server);
        });
    }

    pub fn perform(&mut self, action: Action) {
        let api = self.api.clone();
        let room = self.session.room_id.clone();
        let tx = self.tx.clone();
        let fut: std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<(), ClientError>> + Send>,
        > = match action {
            Action::None => return,
            Action::Quit => {
                self.exit = Some(Exit::Normal);
                return;
            }
            Action::Move(req) => Box::pin(async move { api.play(&room, req).await.map(|_| ()) }),
            Action::Chat(text) => Box::pin(async move { api.chat(&room, &text).await.map(|_| ()) }),
            Action::Resign => Box::pin(async move { api.resign(&room).await.map(|_| ()) }),
            Action::Ready => Box::pin(async move { api.ready(&room).await.map(|_| ()) }),
        };
        tokio::spawn(async move {
            let _ = tx.send(AppMsg::Ack(fut.await)).await;
        });
    }

    fn resync(&self) {
        let api = self.api.clone();
        let room = self.session.room_id.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let r = api.snapshot(&room).await;
            let _ = tx.send(AppMsg::Resync(r)).await;
        });
    }

    pub fn handle(&mut self, msg: AppMsg) {
        match msg {
            AppMsg::Stream(StreamEvent::Connected) => {
                if self.session.connection == Connection::Reconnecting {
                    self.session.notice("reconnected");
                }
                self.session.connection = Connection::Connected;
            }
            AppMsg::Stream(StreamEvent::Disconnected { reason, retry_in }) => {
                tracing::warn!(%reason, ?retry_in, "event stream disconnected");
                if self.session.connection == Connection::Connected {
                    self.session.notice("connection lost, reconnecting...");
                }
                self.session.connection = Connection::Reconnecting;
            }
            AppMsg::Stream(StreamEvent::Event { seq, event }) => self.session.apply(seq, event),
            AppMsg::Stream(StreamEvent::Fatal(err)) => {
                self.exit = Some(match err {
                    ClientError::Outdated { min } => Exit::Outdated { min },
                    other => Exit::Fatal(other.to_string()),
                });
            }
            AppMsg::Ack(Ok(())) => {}
            AppMsg::Ack(Err(err)) | AppMsg::Resync(Err(err)) => self.on_error(err),
            AppMsg::Resync(Ok(snapshot)) => {
                self.session.notice("resynchronised");
                self.session.set_snapshot(snapshot);
            }
        }
    }

    fn on_error(&mut self, err: ClientError) {
        match err {
            ClientError::Outdated { min } => self.exit = Some(Exit::Outdated { min }),
            ClientError::Api(e) => {
                let msg = match e.code {
                    ErrorCode::SeqMismatch => {
                        self.resync();
                        "board changed, resynchronising".to_owned()
                    }
                    ErrorCode::Forbidden => match e.forbidden {
                        Some(kind) => format!("forbidden ({})", kind.label()),
                        None => e.message.clone(),
                    },
                    _ => e.message.clone(),
                };
                self.session.error(msg);
            }
            other => self.session.error(other.to_string()),
        }
    }

    pub fn shutdown(&mut self) {
        if let Some(h) = self.stream.take() {
            h.abort();
        }
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub fn last_room(paths: &Paths, server: &str) -> Option<String> {
    let LocalState {
        last_room_id,
        last_server,
        ..
    } = paths.load_state();
    match last_server {
        Some(s) if s == server => last_room_id,
        _ => None,
    }
}
