//! HTTP handlers; endpoint list = `router()`, request/response types in `proto`

use std::convert::Infallible;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use axum::{Router, extract::Request, middleware::Next, response::Response};
use futures::Stream;
use proto::{
    Ack, ChatRequest, CreateRoomResponse, Envelope, ErrorCode, Health, JoinByCodeRequest,
    JoinResponse, MAX_NAME_LEN, MoveRequest, RegisterRequest, RegisterResponse, RoomSnapshot,
    ServerEvent, StateCause, UpdateMeRequest, UserInfo,
};
use tokio::sync::broadcast::error::RecvError;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::{db, ids, time};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/users/anonymous", post(register_anonymous))
        .route("/users/me", get(me).patch(update_me))
        .route("/rooms", post(create_room))
        .route("/rooms/join", post(join_by_code))
        .route("/rooms/{id}", get(room_snapshot))
        .route("/rooms/{id}/join", post(join_by_id))
        .route("/rooms/{id}/events", get(room_events))
        .route("/rooms/{id}/move", post(play_move))
        .route("/rooms/{id}/chat", post(chat))
        .route("/rooms/{id}/resign", post(resign))
        .route("/rooms/{id}/ready", post(ready))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            version_gate,
        ))
        .with_state(state)
}

/// UC-08. No header (curl, tests) passes
async fn version_gate(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, AppError> {
    if let Some(v) = req
        .headers()
        .get(proto::CLIENT_VERSION_HEADER)
        .and_then(|v| v.to_str().ok())
    {
        let min = &state.config.min_client_version;
        if !proto::version_at_least(v, min) {
            return Err(AppError::new(
                ErrorCode::ClientOutdated,
                format!("client {v} is too old; at least {min} is required"),
            )
            .with_min_client_version(min));
        }
    }
    Ok(next.run(req).await)
}

/// Liveness; no DB access => DB outage never restarts pods
async fn healthz(State(state): State<AppState>) -> Json<Health> {
    Json(health(&state, true))
}

/// Readiness; database unreachable => 503
async fn readyz(State(state): State<AppState>) -> (StatusCode, Json<Health>) {
    let ok = state.db_ready().await;
    let status = if ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(health(&state, ok)))
}

fn health(state: &AppState, ok: bool) -> Health {
    Health {
        ok,
        version: env!("CARGO_PKG_VERSION").to_owned(),
        protocol: proto::PROTOCOL_VERSION,
        min_client_version: state.config.min_client_version.clone(),
        latest_client_version: state.config.latest_client_version.clone(),
    }
}

fn clean_name(raw: Option<&str>) -> AppResult<Option<String>> {
    let Some(raw) = raw else { return Ok(None) };
    let name: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        return Ok(None);
    }
    if name.chars().count() > MAX_NAME_LEN {
        return Err(AppError::bad_request(format!(
            "display name longer than {MAX_NAME_LEN} characters"
        )));
    }
    if name.chars().any(char::is_control) {
        return Err(AppError::bad_request(
            "display name contains control characters",
        ));
    }
    Ok(Some(name))
}

async fn register_anonymous(
    State(state): State<AppState>,
    body: Option<Json<RegisterRequest>>,
) -> AppResult<Json<RegisterResponse>> {
    let req = body.map(|Json(b)| b).unwrap_or_default();
    let display_name = clean_name(req.display_name.as_deref())?.unwrap_or_else(ids::new_guest_name);
    let user_id = ids::new_id();
    let token = ids::new_token();
    let now = time::now();
    db::insert_user(&state.db, &user_id, &display_name, now).await?;
    db::insert_credential(
        &state.db,
        &ids::hash_token(&token),
        &user_id,
        Some("cli"),
        now,
    )
    .await?;
    tracing::info!(%user_id, %display_name, "registered anonymous user");
    Ok(Json(RegisterResponse {
        user_id,
        token,
        display_name,
    }))
}

async fn me(user: AuthUser) -> Json<UserInfo> {
    Json(UserInfo {
        user_id: user.user_id,
        display_name: user.display_name,
    })
}

async fn update_me(
    State(state): State<AppState>,
    user: AuthUser,
    Json(req): Json<UpdateMeRequest>,
) -> AppResult<Json<UserInfo>> {
    let name = clean_name(Some(&req.display_name))?
        .ok_or_else(|| AppError::bad_request("empty display name"))?;
    db::update_display_name(&state.db, &user.user_id, &name).await?;
    Ok(Json(UserInfo {
        user_id: user.user_id,
        display_name: name,
    }))
}

async fn create_room(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<Json<CreateRoomResponse>> {
    let (hub, response) = state.create_room(&user).await?;
    tracing::info!(room = %hub.id, code = %response.invite_code, by = %user.user_id, "room created");
    Ok(Json(response))
}

async fn join_by_code(
    State(state): State<AppState>,
    user: AuthUser,
    Json(req): Json<JoinByCodeRequest>,
) -> AppResult<Json<JoinResponse>> {
    let code = proto::normalize_invite_code(&req.invite_code)
        .ok_or_else(|| AppError::new(ErrorCode::InviteInvalid, "malformed invite code"))?;
    let hub = state.room_by_code(&code).await?;
    let res = hub.join_by_code(&user).await?;
    tracing::info!(room = %hub.id, user = %user.user_id, role = %res.my_role, "joined by code");
    Ok(Json(res))
}

async fn join_by_id(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> AppResult<Json<JoinResponse>> {
    let hub = state.room(&id).await?;
    Ok(Json(hub.join_member(&user).await?))
}

async fn room_snapshot(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> AppResult<Json<RoomSnapshot>> {
    let hub = state.room(&id).await?;
    hub.join_member(&user).await?;
    Ok(Json(hub.snapshot().await))
}

async fn play_move(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<MoveRequest>,
) -> AppResult<Json<Ack>> {
    let hub = state.room(&id).await?;
    Ok(Json(hub.play(&user, req).await?))
}

async fn chat(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(req): Json<ChatRequest>,
) -> AppResult<Json<Ack>> {
    let hub = state.room(&id).await?;
    Ok(Json(hub.chat(&user, &req.text).await?))
}

async fn resign(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> AppResult<Json<Ack>> {
    let hub = state.room(&id).await?;
    Ok(Json(hub.resign(&user).await?))
}

async fn ready(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> AppResult<Json<Ack>> {
    let hub = state.room(&id).await?;
    Ok(Json(hub.ready(&user).await?))
}

fn sse_event(env: &Envelope) -> Result<Event, AppError> {
    Event::default()
        .id(env.seq.to_string())
        .event(env.event.kind())
        .json_data(&env.event)
        .map_err(AppError::internal)
}

fn snapshot_envelope(room_id: &str, seq: u64, snapshot: RoomSnapshot) -> Envelope {
    Envelope {
        seq,
        room_id: room_id.to_owned(),
        event: ServerEvent::State {
            cause: StateCause::Snapshot,
            snapshot,
        },
    }
}

/// `GET /rooms/{id}/events`
///
/// Subscribe before choosing the initial payload => nothing emitted in between lost
/// Usable `Last-Event-ID` => replay persisted events; otherwise a fresh snapshot
/// Lagged receiver => snapshot, not the missed events
async fn room_events(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> AppResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    let hub = state.room(&id).await?;
    hub.join_member(&user).await?;
    let last_id: Option<u64> = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse().ok());

    let mut rx = hub.subscribe();
    let guard = hub.connected(&user).await;
    let current = hub.event_seq().await;
    let max_replay = state.config.max_replay;
    let mut shutdown = state.shutdown_signal();
    let room_id = hub.id.clone();
    tracing::debug!(room = %room_id, user = %user.user_id, ?last_id, current, "sse connected");

    let stream = async_stream::stream! {
        let _guard = guard;
        let mut last_sent: u64 = 0;

        // initial catch-up
        let mut initial: Vec<Envelope> = Vec::new();
        match last_id {
            Some(last) if last <= current => match hub.replay(last, max_replay).await {
                Ok(events) if (events.len() as i64) < max_replay => {
                    initial = events;
                    last_sent = last;
                }
                Ok(_) | Err(_) => {
                    initial.push(snapshot_envelope(&room_id, current, hub.snapshot().await));
                }
            },
            _ => initial.push(snapshot_envelope(&room_id, current, hub.snapshot().await)),
        }
        if initial.is_empty() {
            // nothing missed; still confirm with current state
            initial.push(snapshot_envelope(&room_id, current, hub.snapshot().await));
        }
        for env in initial {
            last_sent = last_sent.max(env.seq);
            match sse_event(&env) {
                Ok(ev) => yield Ok(ev),
                Err(e) => tracing::error!(error = %e, "encoding sse event"),
            }
        }

        // live; shutdown ends the stream (pod exit), clients reconnect
        loop {
            let next = tokio::select! {
                r = rx.recv() => r,
                _ = shutdown.changed() => break,
            };
            match next {
                Ok(env) => {
                    if env.seq <= last_sent {
                        continue;
                    }
                    last_sent = env.seq;
                    match sse_event(&env) {
                        Ok(ev) => yield Ok(ev),
                        Err(e) => tracing::error!(error = %e, "encoding sse event"),
                    }
                }
                Err(RecvError::Lagged(n)) => {
                    tracing::warn!(room = %room_id, lagged = n, "sse receiver lagged; sending snapshot");
                    let seq = hub.event_seq().await;
                    let env = snapshot_envelope(&room_id, seq, hub.snapshot().await);
                    last_sent = last_sent.max(seq);
                    if let Ok(ev) = sse_event(&env) {
                        yield Ok(ev);
                    }
                }
                Err(RecvError::Closed) => break,
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(state.config.keepalive.max(Duration::from_secs(1)))
            .text("keepalive"),
    ))
}
