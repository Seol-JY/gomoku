//! Shared application state: database pool, config and the room registry

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use proto::{CreateRoomResponse, ErrorCode, Role, RoomId};
use sqlx::PgPool;
use tokio::sync::{Mutex, watch};

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::room::{Member, RoomHub, RoomState, new_room_state, random_color};
use crate::{db, time};

/// Bump when the protocol breaks
pub const DEFAULT_MIN_CLIENT_VERSION: &str = "0.1.0";

#[derive(Debug, Clone)]
pub struct Config {
    pub min_client_version: String,
    /// Advertised to clients for the soft update hint
    pub latest_client_version: Option<String>,
    pub keepalive: Duration,
    /// Beyond this: snapshot instead of replay on reconnect
    pub max_replay: i64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            min_client_version: DEFAULT_MIN_CLIENT_VERSION.to_owned(),
            latest_client_version: None,
            keepalive: Duration::from_secs(15),
            max_replay: 500,
        }
    }
}

#[derive(Clone, Debug)]
pub struct AppState {
    pub db: PgPool,
    pub config: Arc<Config>,
    rooms: Arc<Mutex<HashMap<RoomId, Arc<RoomHub>>>>,
    shutdown_tx: Arc<watch::Sender<bool>>,
    shutdown_rx: watch::Receiver<bool>,
}

impl AppState {
    pub fn new(db: PgPool, config: Config) -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        Self {
            db,
            config: Arc::new(config),
            rooms: Arc::new(Mutex::new(HashMap::new())),
            shutdown_tx: Arc::new(shutdown_tx),
            shutdown_rx,
        }
    }

    /// Ends every SSE stream => graceful shutdown completes; clients reconnect
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    pub fn shutdown_signal(&self) -> watch::Receiver<bool> {
        self.shutdown_rx.clone()
    }

    pub async fn db_ready(&self) -> bool {
        sqlx::query("SELECT 1").execute(&self.db).await.is_ok()
    }

    /// Lazily loaded from the database
    pub async fn room(&self, id: &str) -> AppResult<Arc<RoomHub>> {
        let mut rooms = self.rooms.lock().await;
        if let Some(hub) = rooms.get(id) {
            return Ok(Arc::clone(hub));
        }
        let row = db::room_by_id(&self.db, id)
            .await?
            .ok_or_else(|| AppError::not_found("room"))?;
        let hub = RoomHub::load(self.db.clone(), row).await?;
        rooms.insert(id.to_owned(), Arc::clone(&hub));
        Ok(hub)
    }

    pub async fn room_by_code(&self, code: &str) -> AppResult<Arc<RoomHub>> {
        let row = db::room_by_invite_code(&self.db, code)
            .await?
            .ok_or_else(|| AppError::new(ErrorCode::InviteInvalid, "unknown invite code"))?;
        self.room(&row.id).await
    }

    async fn register(&self, hub: Arc<RoomHub>) {
        self.rooms.lock().await.insert(hub.id.clone(), hub);
    }

    /// Random colour for the creator: Black favoured, human choice = disputes
    pub async fn create_room(
        &self,
        creator: &AuthUser,
    ) -> AppResult<(Arc<RoomHub>, CreateRoomResponse)> {
        let color = random_color();
        let member = Member {
            user_id: creator.user_id.clone(),
            display_name: creator.display_name.clone(),
            role: Role::from_color(color),
            joined_at: time::now(),
        };
        let state = new_room_state(member, None);
        let hub = self.persist_new_room(state).await?;
        let snapshot = hub.snapshot().await;
        let (invite_code, expires_at) = hub.invite().await;
        let response = CreateRoomResponse {
            room_id: hub.id.clone(),
            invite_code,
            my_role: Role::from_color(color),
            expires_at: time::to_rfc3339(expires_at),
            snapshot,
        };
        Ok((hub, response))
    }

    async fn persist_new_room(&self, mut state: RoomState) -> AppResult<Arc<RoomHub>> {
        // retry on invite-code collision
        for _attempt in 0..5 {
            let row = db::RoomRow {
                id: state.id.clone(),
                invite_code: state.invite_code.clone(),
                invite_expires_at: state.invite_expires_at,
                status: state.status,
                state_seq: state.state_seq as i64,
                created_by: state.created_by.clone(),
                created_at: state.created_at,
                winner: None,
                end_reason: None,
                games_played: 0,
            };
            let members: Vec<(String, Role, DateTime<Utc>)> = state
                .members
                .iter()
                .map(|m| (m.user_id.clone(), m.role, m.joined_at))
                .collect();
            match db::insert_room(&self.db, &row, &members).await {
                Ok(()) => {
                    let hub = RoomHub::new(state, self.db.clone());
                    self.register(Arc::clone(&hub)).await;
                    return Ok(hub);
                }
                Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
                    state.invite_code = crate::ids::new_invite_code();
                }
                Err(e) => return Err(e.into()),
            }
        }
        Err(AppError::internal(
            "could not allocate a unique invite code",
        ))
    }
}
