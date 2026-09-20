//! Room runtime: authoritative state behind an async mutex + broadcast fan-out
//! Mutate under the lock, write through to PostgreSQL, then broadcast
//! A restart loses only the ready flags and presence

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use proto::{
    Ack, Color, Coord, EndReason, Envelope, ErrorCode, ForbiddenPoint, GameResult, JoinResponse,
    MAX_CHAT_LEN, MoveRecord, MoveRequest, PlayerInfo, Role, RoomId, RoomSnapshot, RoomStatus,
    Seats, ServerEvent, StateCause, UserId,
};
use rules::{Game, GameStatus, MoveError, Pos};
use sqlx::PgPool;
use tokio::sync::{Mutex, broadcast};

use crate::auth::AuthUser;
use crate::db;
use crate::error::{AppError, AppResult};
use crate::time;

/// Short bursts only: lagged receivers get a snapshot
const BROADCAST_CAPACITY: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub user_id: UserId,
    pub display_name: String,
    pub role: Role,
    pub joined_at: DateTime<Utc>,
}

#[derive(Debug)]
pub struct RoomState {
    pub id: RoomId,
    pub invite_code: String,
    pub invite_expires_at: DateTime<Utc>,
    pub status: RoomStatus,
    pub created_by: UserId,
    pub created_at: DateTime<Utc>,
    pub members: Vec<Member>,
    pub game: Game,
    /// Echoed by clients as `expect_seq`
    pub state_seq: u64,
    /// SSE id
    pub event_seq: u64,
    pub result: Option<GameResult>,
    pub ready: Vec<UserId>,
    pub games_played: u32,
    /// Live SSE connections per user
    pub online: HashMap<UserId, u32>,
}

impl RoomState {
    pub fn member(&self, user_id: &str) -> Option<&Member> {
        self.members.iter().find(|m| m.user_id == user_id)
    }

    pub fn seat(&self, color: Color) -> Option<&Member> {
        self.members.iter().find(|m| m.role.color() == Some(color))
    }

    pub fn player_color(&self, user_id: &str) -> Option<Color> {
        self.member(user_id).and_then(|m| m.role.color())
    }

    pub fn opponent_of(&self, user_id: &str) -> Option<&Member> {
        let color = self.player_color(user_id)?;
        self.seat(color.opposite())
    }

    fn player_info(&self, m: &Member) -> PlayerInfo {
        PlayerInfo {
            user_id: m.user_id.clone(),
            display_name: m.display_name.clone(),
            role: m.role,
            online: self.online.get(&m.user_id).is_some_and(|n| *n > 0),
        }
    }

    pub fn move_records(&self) -> Vec<MoveRecord> {
        self.game
            .moves()
            .iter()
            .enumerate()
            .map(|(i, p)| MoveRecord {
                n: i as u32 + 1,
                x: p.x(),
                y: p.y(),
                color: if i % 2 == 0 {
                    Color::Black
                } else {
                    Color::White
                },
            })
            .collect()
    }

    fn forbidden(&self) -> Vec<ForbiddenPoint> {
        if self.status != RoomStatus::Playing {
            return Vec::new();
        }
        let mut pts: Vec<ForbiddenPoint> = self
            .game
            .forbidden_points()
            .into_iter()
            .map(|(p, k)| ForbiddenPoint {
                x: p.x(),
                y: p.y(),
                kind: k.into(),
            })
            .collect();
        pts.sort_by_key(|p| (p.y, p.x));
        pts
    }

    pub fn snapshot(&self) -> RoomSnapshot {
        let turn = if self.status == RoomStatus::Playing {
            Some(self.game.turn().into())
        } else {
            None
        };
        RoomSnapshot {
            room_id: self.id.clone(),
            invite_code: Some(self.invite_code.clone()),
            status: self.status,
            seq: self.state_seq,
            board: self.game.board().to_compact(),
            moves: self.move_records(),
            turn,
            seats: Seats {
                black: self.seat(Color::Black).map(|m| self.player_info(m)),
                white: self.seat(Color::White).map(|m| self.player_info(m)),
            },
            spectators: self
                .members
                .iter()
                .filter(|m| m.role == Role::Spectator)
                .count() as u32,
            forbidden: self.forbidden(),
            last_move: self.game.last_move().map(Coord::from),
            result: self.result.clone(),
            ready: self.ready.clone(),
            games_played: self.games_played,
            created_at: time::to_rfc3339(self.created_at),
        }
    }
}

pub struct RoomHub {
    pub id: RoomId,
    state: Mutex<RoomState>,
    tx: broadcast::Sender<Envelope>,
    db: PgPool,
}

impl std::fmt::Debug for RoomHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RoomHub")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl RoomHub {
    pub fn new(state: RoomState, db: PgPool) -> Arc<Self> {
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        Arc::new(Self {
            id: state.id.clone(),
            state: Mutex::new(state),
            tx,
            db,
        })
    }

    /// Board replayed from the move log
    pub async fn load(db: PgPool, row: db::RoomRow) -> anyhow::Result<Arc<Self>> {
        let members = db::members_of_room(&db, &row.id)
            .await?
            .into_iter()
            .map(|m| Member {
                user_id: m.user_id,
                display_name: m.display_name,
                role: m.role,
                joined_at: m.joined_at,
            })
            .collect();
        let moves = db::moves_of_room(&db, &row.id).await?;
        let positions: Vec<Pos> = moves
            .iter()
            .map(|m| Pos::new(m.x, m.y).ok_or_else(|| anyhow::anyhow!("bad move")))
            .collect::<Result<_, _>>()?;
        let game = Game::from_moves(positions.iter().copied())
            .map_err(|(i, e)| anyhow::anyhow!("replaying move {}: {e}", i + 1))?;
        let event_seq = db::last_event_seq(&db, &row.id).await? as u64;

        let result = if row.status == RoomStatus::Finished {
            let reason = match row.end_reason.as_deref() {
                Some("resign") => EndReason::Resign,
                Some("draw") => EndReason::Draw,
                Some("timeout") => EndReason::Timeout,
                _ => EndReason::Five,
            };
            let winner = match row.winner.as_deref() {
                Some("black") => Some(Color::Black),
                Some("white") => Some(Color::White),
                _ => None,
            };
            let line = match game.status() {
                GameStatus::Won { line, .. } if reason == EndReason::Five => {
                    line.iter().copied().map(Coord::from).collect()
                }
                _ => Vec::new(),
            };
            Some(GameResult {
                winner,
                reason,
                line,
            })
        } else {
            None
        };

        let state = RoomState {
            id: row.id,
            invite_code: row.invite_code,
            invite_expires_at: row.invite_expires_at,
            status: row.status,
            created_by: row.created_by,
            created_at: row.created_at,
            members,
            game,
            state_seq: row.state_seq as u64,
            event_seq,
            result,
            ready: Vec::new(),
            games_played: row.games_played as u32,
            online: HashMap::new(),
        };
        Ok(Self::new(state, db))
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Envelope> {
        self.tx.subscribe()
    }

    pub async fn snapshot(&self) -> RoomSnapshot {
        self.state.lock().await.snapshot()
    }

    pub async fn invite(&self) -> (String, DateTime<Utc>) {
        let st = self.state.lock().await;
        (st.invite_code.clone(), st.invite_expires_at)
    }

    pub async fn event_seq(&self) -> u64 {
        self.state.lock().await.event_seq
    }

    /// Presence not persisted: ephemeral, meaningless on replay
    async fn emit(&self, st: &mut RoomState, event: ServerEvent) -> AppResult<u64> {
        st.event_seq += 1;
        let seq = st.event_seq;
        if !matches!(event, ServerEvent::Presence { .. }) {
            let payload = serde_json::to_value(&event).map_err(AppError::internal)?;
            db::insert_event(
                &self.db,
                &st.id,
                seq as i64,
                event.kind(),
                &payload,
                time::now(),
            )
            .await?;
        }
        // no receivers = fine
        let _ = self.tx.send(Envelope {
            seq,
            room_id: st.id.clone(),
            event,
        });
        Ok(seq)
    }

    async fn emit_state(&self, st: &mut RoomState, cause: StateCause) -> AppResult<u64> {
        let snapshot = st.snapshot();
        self.emit(st, ServerEvent::State { cause, snapshot }).await
    }

    /// Existing member => reconnect; otherwise take the free seat
    pub async fn join_by_code(&self, user: &AuthUser) -> AppResult<JoinResponse> {
        let mut st = self.state.lock().await;
        if let Some(m) = st.member(&user.user_id) {
            return Ok(JoinResponse {
                room_id: st.id.clone(),
                my_role: m.role,
                snapshot: st.snapshot(),
            });
        }
        if time::now() > st.invite_expires_at {
            return Err(AppError::new(
                ErrorCode::InviteExpired,
                "this invite code has expired",
            ));
        }
        if st.status != RoomStatus::Waiting {
            return Err(AppError::new(ErrorCode::RoomFull, "both seats are taken"));
        }
        let free = [Color::Black, Color::White]
            .into_iter()
            .find(|c| st.seat(*c).is_none())
            .ok_or_else(|| AppError::new(ErrorCode::RoomFull, "both seats are taken"))?;
        let role = Role::from_color(free);
        let now = time::now();
        db::insert_membership(&self.db, &st.id, &user.user_id, role, now).await?;
        st.members.push(Member {
            user_id: user.user_id.clone(),
            display_name: user.display_name.clone(),
            role,
            joined_at: now,
        });
        st.status = RoomStatus::Playing;
        st.state_seq += 1;
        db::update_room_status(&self.db, &st.id, st.status, st.state_seq as i64).await?;

        let info = st
            .member(&user.user_id)
            .map(|m| st.player_info(m))
            .expect("just pushed");
        self.emit(&mut st, ServerEvent::Joined { user: info })
            .await?;
        self.emit_state(&mut st, StateCause::Start).await?;
        Ok(JoinResponse {
            room_id: st.id.clone(),
            my_role: role,
            snapshot: st.snapshot(),
        })
    }

    /// Reconnect
    pub async fn join_member(&self, user: &AuthUser) -> AppResult<JoinResponse> {
        let st = self.state.lock().await;
        let m = st.member(&user.user_id).ok_or_else(|| {
            AppError::new(ErrorCode::NotAMember, "you are not a member of this room")
        })?;
        Ok(JoinResponse {
            room_id: st.id.clone(),
            my_role: m.role,
            snapshot: st.snapshot(),
        })
    }

    fn require_player(st: &RoomState, user_id: &str) -> AppResult<Color> {
        match st.member(user_id) {
            None => Err(AppError::new(
                ErrorCode::NotAMember,
                "you are not a member of this room",
            )),
            Some(m) => m
                .role
                .color()
                .ok_or_else(|| AppError::new(ErrorCode::NotAPlayer, "spectators cannot do that")),
        }
    }

    fn require_playing(st: &RoomState) -> AppResult<()> {
        match st.status {
            RoomStatus::Playing => Ok(()),
            RoomStatus::Waiting => Err(AppError::new(
                ErrorCode::NotPlaying,
                "waiting for an opponent",
            )),
            RoomStatus::Finished => Err(AppError::new(ErrorCode::NotPlaying, "the game is over")),
        }
    }

    pub async fn play(&self, user: &AuthUser, req: MoveRequest) -> AppResult<Ack> {
        let mut st = self.state.lock().await;
        let color = Self::require_player(&st, &user.user_id)?;
        Self::require_playing(&st)?;
        if Color::from(st.game.turn()) != color {
            return Err(AppError::new(ErrorCode::NotYourTurn, "it is not your turn"));
        }
        if req.expect_seq != st.state_seq {
            return Err(
                AppError::new(ErrorCode::SeqMismatch, "your view of the game is stale")
                    .with_current_seq(st.state_seq),
            );
        }
        let pos = Pos::new(req.x, req.y)
            .ok_or_else(|| AppError::bad_request("coordinate out of range"))?;
        st.game.play(pos).map_err(|e| match e {
            MoveError::Occupied => AppError::new(ErrorCode::Occupied, "that point is occupied"),
            MoveError::GameOver => AppError::new(ErrorCode::NotPlaying, "the game is over"),
            MoveError::Forbidden(kind) => {
                AppError::new(ErrorCode::Forbidden, format!("forbidden for Black: {kind}"))
                    .with_forbidden(kind.into())
            }
        })?;
        st.state_seq += 1;
        let seq = st.game.moves().len() as i32;
        let now = time::now();
        let new_move = db::NewMove {
            seq,
            user_id: &user.user_id,
            x: pos.x(),
            y: pos.y(),
            played_at: now,
        };
        db::insert_move(&self.db, &st.id, new_move, st.state_seq as i64).await?;

        let record = MoveRecord {
            n: seq as u32,
            x: pos.x(),
            y: pos.y(),
            color,
        };
        let finished = match st.game.status().clone() {
            GameStatus::Playing => None,
            GameStatus::Won { winner, line } => Some(GameResult {
                winner: Some(winner.into()),
                reason: EndReason::Five,
                line: line.into_iter().map(Coord::from).collect(),
            }),
            GameStatus::Draw => Some(GameResult {
                winner: None,
                reason: EndReason::Draw,
                line: Vec::new(),
            }),
        };
        if let Some(result) = finished {
            self.finish(&mut st, result, now).await?;
            self.emit_state(&mut st, StateCause::Move { record })
                .await?;
            let snapshot = st.snapshot();
            let result = st.result.clone().expect("set by finish");
            self.emit(&mut st, ServerEvent::Gameover { result, snapshot })
                .await?;
        } else {
            self.emit_state(&mut st, StateCause::Move { record })
                .await?;
        }
        Ok(Ack { seq: st.state_seq })
    }

    async fn finish(
        &self,
        st: &mut RoomState,
        result: GameResult,
        now: DateTime<Utc>,
    ) -> AppResult<()> {
        st.status = RoomStatus::Finished;
        st.games_played += 1;
        let winner = result.winner.map(|c| match c {
            Color::Black => "black",
            Color::White => "white",
        });
        let reason = match result.reason {
            EndReason::Five => "five",
            EndReason::Resign => "resign",
            EndReason::Draw => "draw",
            EndReason::Timeout => "timeout",
        };
        db::update_room_finished(
            &self.db,
            &st.id,
            st.state_seq as i64,
            winner,
            reason,
            now,
            st.games_played as i32,
        )
        .await?;
        st.result = Some(result);
        Ok(())
    }

    pub async fn chat(&self, user: &AuthUser, text: &str) -> AppResult<Ack> {
        let text = text.trim();
        if text.is_empty() {
            return Err(AppError::bad_request("empty message"));
        }
        if text.chars().count() > MAX_CHAT_LEN {
            return Err(AppError::bad_request(format!(
                "message longer than {MAX_CHAT_LEN} characters"
            )));
        }
        let mut st = self.state.lock().await;
        if st.member(&user.user_id).is_none() {
            return Err(AppError::new(
                ErrorCode::NotAMember,
                "you are not a member of this room",
            ));
        }
        let ev = ServerEvent::Chat {
            from: user.user_id.clone(),
            display_name: user.display_name.clone(),
            text: text.to_owned(),
            at: time::now_rfc3339(),
        };
        self.emit(&mut st, ev).await?;
        Ok(Ack { seq: st.state_seq })
    }

    pub async fn resign(&self, user: &AuthUser) -> AppResult<Ack> {
        let mut st = self.state.lock().await;
        let color = Self::require_player(&st, &user.user_id)?;
        Self::require_playing(&st)?;
        st.state_seq += 1;
        let now = time::now();
        let result = GameResult {
            winner: Some(color.opposite()),
            reason: EndReason::Resign,
            line: Vec::new(),
        };
        self.finish(&mut st, result, now).await?;
        let snapshot = st.snapshot();
        let result = st.result.clone().expect("set by finish");
        self.emit(&mut st, ServerEvent::Gameover { result, snapshot })
            .await?;
        Ok(Ack { seq: st.state_seq })
    }

    /// Enter after a game; both ready => same room restarts
    /// with the loser as Black (draw: colours swap)
    pub async fn ready(&self, user: &AuthUser) -> AppResult<Ack> {
        let mut st = self.state.lock().await;
        let color = Self::require_player(&st, &user.user_id)?;
        if st.status != RoomStatus::Finished {
            return Err(AppError::new(
                ErrorCode::NotPlaying,
                "the game is not over yet",
            ));
        }
        if !st.ready.contains(&user.user_id) {
            st.ready.push(user.user_id.clone());
            let ev = ServerEvent::Ready {
                from: user.user_id.clone(),
                display_name: user.display_name.clone(),
            };
            self.emit(&mut st, ev).await?;
        }
        let Some(opponent) = st.opponent_of(&user.user_id).cloned() else {
            return Ok(Ack { seq: st.state_seq });
        };
        if !st.ready.contains(&opponent.user_id) {
            return Ok(Ack { seq: st.state_seq });
        }

        let winner = st.result.as_ref().and_then(|r| r.winner);
        let me_black = match winner {
            Some(w) => w != color,
            None => color == Color::White,
        };
        let (black, white) = if me_black {
            (user.user_id.clone(), opponent.user_id.clone())
        } else {
            (opponent.user_id.clone(), user.user_id.clone())
        };
        for m in &mut st.members {
            if m.user_id == black {
                m.role = Role::Black;
            } else if m.user_id == white {
                m.role = Role::White;
            }
        }
        st.game = Game::new();
        st.result = None;
        st.ready.clear();
        st.status = RoomStatus::Playing;
        st.state_seq += 1;
        db::restart_game(
            &self.db,
            &st.id,
            st.state_seq as i64,
            st.games_played as i32,
            &black,
            &white,
        )
        .await?;
        self.emit_state(&mut st, StateCause::Start).await?;
        Ok(Ack { seq: st.state_seq })
    }

    /// Drop the guard when the stream ends
    pub async fn connected(self: &Arc<Self>, user: &AuthUser) -> PresenceGuard {
        let mut st = self.state.lock().await;
        let count = st.online.entry(user.user_id.clone()).or_insert(0);
        *count += 1;
        if *count == 1 {
            let ev = ServerEvent::Presence {
                user_id: user.user_id.clone(),
                display_name: user.display_name.clone(),
                online: true,
            };
            if let Err(e) = self.emit(&mut st, ev).await {
                tracing::warn!(error = %e, "presence emit failed");
            }
        }
        PresenceGuard {
            hub: Arc::clone(self),
            user: user.clone(),
        }
    }

    async fn disconnected(&self, user: AuthUser) {
        let mut st = self.state.lock().await;
        let gone = match st.online.get_mut(&user.user_id) {
            Some(n) if *n > 1 => {
                *n -= 1;
                false
            }
            Some(_) => {
                st.online.remove(&user.user_id);
                true
            }
            None => false,
        };
        if gone {
            let ev = ServerEvent::Presence {
                user_id: user.user_id,
                display_name: user.display_name,
                online: false,
            };
            if let Err(e) = self.emit(&mut st, ev).await {
                tracing::warn!(error = %e, "presence emit failed");
            }
        }
    }

    pub async fn replay(&self, after: u64, limit: i64) -> AppResult<Vec<Envelope>> {
        let rows = db::events_after(&self.db, &self.id, after as i64, limit).await?;
        let mut out = Vec::with_capacity(rows.len());
        for (seq, payload) in rows {
            let event: ServerEvent = serde_json::from_value(payload).map_err(AppError::internal)?;
            out.push(Envelope {
                seq: seq as u64,
                room_id: self.id.clone(),
                event,
            });
        }
        Ok(out)
    }
}

#[derive(Debug)]
pub struct PresenceGuard {
    hub: Arc<RoomHub>,
    user: AuthUser,
}

impl Drop for PresenceGuard {
    fn drop(&mut self) {
        let hub = Arc::clone(&self.hub);
        let user = self.user.clone();
        // Drop = sync; hand off to the runtime
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move { hub.disconnected(user).await });
        }
    }
}

pub fn random_color() -> Color {
    if rand::random::<bool>() {
        Color::Black
    } else {
        Color::White
    }
}

pub fn new_room_state(creator: Member, second: Option<Member>) -> RoomState {
    let now = time::now();
    let mut members = vec![creator];
    let status = if second.is_some() {
        RoomStatus::Playing
    } else {
        RoomStatus::Waiting
    };
    members.extend(second);
    RoomState {
        id: crate::ids::new_id(),
        invite_code: crate::ids::new_invite_code(),
        invite_expires_at: now + chrono::Duration::seconds(proto::INVITE_TTL_SECS),
        status,
        created_by: members[0].user_id.clone(),
        created_at: now,
        members,
        game: Game::new(),
        state_seq: 0,
        event_seq: 0,
        result: None,
        ready: Vec::new(),
        games_played: 0,
        online: HashMap::new(),
    }
}
