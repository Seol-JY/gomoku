//! All PostgreSQL queries. Static SQL only: sqlx 0.9 accepts nothing else without
//! `AssertSqlSafe`; ids = UUID in the database, opaque strings elsewhere; a
//! non-UUID string matches nothing (callers report "not found")

use std::time::Duration;

use anyhow::Context;
use chrono::{DateTime, Utc};
use proto::{Role, RoomStatus};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::types::Json;
use sqlx::{PgPool, Row};
use uuid::Uuid;

pub async fn connect(url: &str) -> anyhow::Result<PgPool> {
    let opts: PgConnectOptions = url.parse().context("invalid database url")?;
    connect_with(opts)
        .await
        .with_context(|| format!("connecting to {}", redact(url)))
}

/// Tests build options for throwaway databases
pub async fn connect_with(opts: PgConnectOptions) -> anyhow::Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(Duration::from_secs(5))
        .connect_with(opts)
        .await
        .context("connecting to PostgreSQL")?;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("running migrations")?;
    Ok(pool)
}

pub fn redact(url: &str) -> String {
    match (url.find("://"), url.rfind('@')) {
        (Some(s), Some(at)) if at > s + 3 => {
            let creds = &url[s + 3..at];
            match creds.find(':') {
                Some(c) => format!("{}{}:***{}", &url[..s + 3], &creds[..c], &url[at..]),
                None => url.to_owned(),
            }
        }
        _ => url.to_owned(),
    }
}

fn uuid(s: &str) -> Option<Uuid> {
    Uuid::parse_str(s).ok()
}

/// Server-generated ids => infallible parse
fn own_uuid(s: &str) -> Uuid {
    Uuid::parse_str(s).expect("server-generated id is a uuid")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserRow {
    pub id: String,
    pub display_name: String,
    pub created_at: DateTime<Utc>,
}

fn user_from_row(r: &sqlx::postgres::PgRow) -> UserRow {
    UserRow {
        id: r.get::<Uuid, _>("id").to_string(),
        display_name: r.get("display_name"),
        created_at: r.get("created_at"),
    }
}

pub async fn insert_user(
    pool: &PgPool,
    id: &str,
    display_name: &str,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO users (id, display_name, created_at) VALUES ($1, $2, $3)")
        .bind(own_uuid(id))
        .bind(display_name)
        .bind(now)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn insert_credential(
    pool: &PgPool,
    token_hash: &str,
    user_id: &str,
    device: Option<&str>,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO credentials (token_hash, user_id, device, created_at, last_used_at) \
         VALUES ($1, $2, $3, $4, $4)",
    )
    .bind(token_hash)
    .bind(own_uuid(user_id))
    .bind(device)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Expired credentials excluded
pub async fn user_by_token_hash(
    pool: &PgPool,
    token_hash: &str,
    now: DateTime<Utc>,
) -> sqlx::Result<Option<UserRow>> {
    let row = sqlx::query(
        "SELECT u.id, u.display_name, u.created_at \
         FROM credentials c JOIN users u ON u.id = c.user_id \
         WHERE c.token_hash = $1 AND (c.expires_at IS NULL OR c.expires_at > $2)",
    )
    .bind(token_hash)
    .bind(now)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(user_from_row))
}

pub async fn touch_credential(
    pool: &PgPool,
    token_hash: &str,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query("UPDATE credentials SET last_used_at = $2 WHERE token_hash = $1")
        .bind(token_hash)
        .bind(now)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_display_name(
    pool: &PgPool,
    user_id: &str,
    display_name: &str,
) -> sqlx::Result<()> {
    sqlx::query("UPDATE users SET display_name = $2 WHERE id = $1")
        .bind(own_uuid(user_id))
        .bind(display_name)
        .execute(pool)
        .await?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomRow {
    pub id: String,
    pub invite_code: String,
    pub invite_expires_at: DateTime<Utc>,
    pub status: RoomStatus,
    pub state_seq: i64,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub winner: Option<String>,
    pub end_reason: Option<String>,
    pub games_played: i32,
}

fn room_from_row(r: &sqlx::postgres::PgRow) -> anyhow::Result<RoomRow> {
    let status: String = r.get("status");
    Ok(RoomRow {
        id: r.get::<Uuid, _>("id").to_string(),
        invite_code: r.get("invite_code"),
        invite_expires_at: r.get("invite_expires_at"),
        status: RoomStatus::parse(&status)
            .with_context(|| format!("bad room status {status:?}"))?,
        state_seq: r.get("state_seq"),
        created_by: r.get::<Uuid, _>("created_by").to_string(),
        created_at: r.get("created_at"),
        winner: r.get("winner"),
        end_reason: r.get("end_reason"),
        games_played: r.get("games_played"),
    })
}

pub async fn room_by_id(pool: &PgPool, id: &str) -> anyhow::Result<Option<RoomRow>> {
    let Some(id) = uuid(id) else {
        return Ok(None);
    };
    let row = sqlx::query(
        "SELECT id, invite_code, invite_expires_at, status, state_seq, created_by, created_at, \
         winner, end_reason, games_played FROM rooms WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(room_from_row).transpose()
}

pub async fn room_by_invite_code(pool: &PgPool, code: &str) -> anyhow::Result<Option<RoomRow>> {
    let row = sqlx::query(
        "SELECT id, invite_code, invite_expires_at, status, state_seq, created_by, created_at, \
         winner, end_reason, games_played FROM rooms WHERE invite_code = $1",
    )
    .bind(code)
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(room_from_row).transpose()
}

/// One transaction
pub async fn insert_room(
    pool: &PgPool,
    room: &RoomRow,
    members: &[(String, Role, DateTime<Utc>)],
) -> sqlx::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO rooms (id, invite_code, invite_expires_at, visibility, status, state_seq, \
         created_by, created_at) VALUES ($1, $2, $3, 'private', $4, $5, $6, $7)",
    )
    .bind(own_uuid(&room.id))
    .bind(&room.invite_code)
    .bind(room.invite_expires_at)
    .bind(room.status.as_str())
    .bind(room.state_seq)
    .bind(own_uuid(&room.created_by))
    .bind(room.created_at)
    .execute(&mut *tx)
    .await?;
    for (user_id, role, joined_at) in members {
        sqlx::query(
            "INSERT INTO memberships (room_id, user_id, role, joined_at) VALUES ($1, $2, $3, $4)",
        )
        .bind(own_uuid(&room.id))
        .bind(own_uuid(user_id))
        .bind(role.as_str())
        .bind(joined_at)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}

pub async fn insert_membership(
    pool: &PgPool,
    room_id: &str,
    user_id: &str,
    role: Role,
    joined_at: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO memberships (room_id, user_id, role, joined_at) VALUES ($1, $2, $3, $4)",
    )
    .bind(own_uuid(room_id))
    .bind(own_uuid(user_id))
    .bind(role.as_str())
    .bind(joined_at)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn update_room_status(
    pool: &PgPool,
    room_id: &str,
    status: RoomStatus,
    state_seq: i64,
) -> sqlx::Result<()> {
    sqlx::query("UPDATE rooms SET status = $2, state_seq = $3 WHERE id = $1")
        .bind(own_uuid(room_id))
        .bind(status.as_str())
        .bind(state_seq)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_room_finished(
    pool: &PgPool,
    room_id: &str,
    state_seq: i64,
    winner: Option<&str>,
    end_reason: &str,
    finished_at: DateTime<Utc>,
    games_played: i32,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE rooms SET status = 'finished', state_seq = $2, winner = $3, end_reason = $4, \
         finished_at = $5, games_played = $6 WHERE id = $1",
    )
    .bind(own_uuid(room_id))
    .bind(state_seq)
    .bind(winner)
    .bind(end_reason)
    .bind(finished_at)
    .bind(games_played)
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberRow {
    pub user_id: String,
    pub display_name: String,
    pub role: Role,
    pub joined_at: DateTime<Utc>,
}

pub async fn members_of_room(pool: &PgPool, room_id: &str) -> anyhow::Result<Vec<MemberRow>> {
    let rows = sqlx::query(
        "SELECT m.user_id, u.display_name, m.role, m.joined_at \
         FROM memberships m JOIN users u ON u.id = m.user_id \
         WHERE m.room_id = $1 ORDER BY m.joined_at",
    )
    .bind(own_uuid(room_id))
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|r| {
            let role: String = r.get("role");
            Ok(MemberRow {
                user_id: r.get::<Uuid, _>("user_id").to_string(),
                display_name: r.get("display_name"),
                role: Role::parse(&role).with_context(|| format!("bad role {role:?}"))?,
                joined_at: r.get("joined_at"),
            })
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoveRow {
    pub seq: i32,
    pub x: u8,
    pub y: u8,
}

pub async fn moves_of_room(pool: &PgPool, room_id: &str) -> sqlx::Result<Vec<MoveRow>> {
    let rows = sqlx::query("SELECT seq, x, y FROM moves WHERE room_id = $1 ORDER BY seq")
        .bind(own_uuid(room_id))
        .fetch_all(pool)
        .await?;
    Ok(rows
        .iter()
        .map(|r| MoveRow {
            seq: r.get("seq"),
            x: r.get::<i16, _>("x") as u8,
            y: r.get::<i16, _>("y") as u8,
        })
        .collect())
}

#[derive(Debug, Clone, Copy)]
pub struct NewMove<'a> {
    pub seq: i32,
    pub user_id: &'a str,
    pub x: u8,
    pub y: u8,
    pub played_at: DateTime<Utc>,
}

/// Move + `state_seq` bump in one transaction
pub async fn insert_move(
    pool: &PgPool,
    room_id: &str,
    m: NewMove<'_>,
    state_seq: i64,
) -> sqlx::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO moves (room_id, seq, user_id, x, y, played_at) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(own_uuid(room_id))
    .bind(m.seq)
    .bind(own_uuid(m.user_id))
    .bind(i16::from(m.x))
    .bind(i16::from(m.y))
    .bind(m.played_at)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE rooms SET state_seq = $2 WHERE id = $1")
        .bind(own_uuid(room_id))
        .bind(state_seq)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

/// Same room, next game: clears moves and result, swaps seats, all in one transaction
pub async fn restart_game(
    pool: &PgPool,
    room_id: &str,
    state_seq: i64,
    games_played: i32,
    black_user: &str,
    white_user: &str,
) -> sqlx::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM moves WHERE room_id = $1")
        .bind(own_uuid(room_id))
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE rooms SET status = 'playing', state_seq = $2, games_played = $3, \
         winner = NULL, end_reason = NULL, finished_at = NULL WHERE id = $1",
    )
    .bind(own_uuid(room_id))
    .bind(state_seq)
    .bind(games_played)
    .execute(&mut *tx)
    .await?;
    for (user, role) in [(black_user, Role::Black), (white_user, Role::White)] {
        sqlx::query("UPDATE memberships SET role = $3 WHERE room_id = $1 AND user_id = $2")
            .bind(own_uuid(room_id))
            .bind(own_uuid(user))
            .bind(role.as_str())
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await
}

pub async fn insert_event(
    pool: &PgPool,
    room_id: &str,
    seq: i64,
    kind: &str,
    payload: &serde_json::Value,
    created_at: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO events (room_id, seq, kind, payload, created_at) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(own_uuid(room_id))
    .bind(seq)
    .bind(kind)
    .bind(Json(payload))
    .bind(created_at)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn last_event_seq(pool: &PgPool, room_id: &str) -> sqlx::Result<i64> {
    let row = sqlx::query("SELECT COALESCE(MAX(seq), 0) AS seq FROM events WHERE room_id = $1")
        .bind(own_uuid(room_id))
        .fetch_one(pool)
        .await?;
    Ok(row.get("seq"))
}

/// Oldest first, capped at `limit`
pub async fn events_after(
    pool: &PgPool,
    room_id: &str,
    after: i64,
    limit: i64,
) -> sqlx::Result<Vec<(i64, serde_json::Value)>> {
    let rows = sqlx::query(
        "SELECT seq, payload FROM events WHERE room_id = $1 AND seq > $2 ORDER BY seq LIMIT $3",
    )
    .bind(own_uuid(room_id))
    .bind(after)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|r| {
            (
                r.get("seq"),
                r.get::<Json<serde_json::Value>, _>("payload").0,
            )
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_passwords() {
        assert_eq!(
            redact("postgres://u:secret@h:5432/db"),
            "postgres://u:***@h:5432/db"
        );
        assert_eq!(redact("postgres://u@h/db"), "postgres://u@h/db");
        assert_eq!(redact("postgres://h/db"), "postgres://h/db");
    }
}
