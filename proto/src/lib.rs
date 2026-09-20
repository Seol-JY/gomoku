//! Wire types shared by server and client: a field rename breaks both at compile time
//! SSE frame: `id` = event seq, `event` = `ServerEvent::kind()`, `data` = `ServerEvent` JSON
//! Auth: `Authorization: Bearer <token>` on every request, SSE included

#![forbid(unsafe_code)]

use std::fmt;

use rules::{Forbidden, Game, MoveError, Pos, Stone};
use serde::{Deserialize, Serialize};

/// Bump on incompatible wire changes
pub const PROTOCOL_VERSION: u32 = 1;

pub const CLIENT_VERSION_HEADER: &str = "x-gomoku-client-version";

/// No 0/O or 1/I/L: codes read aloud
pub const INVITE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
pub const INVITE_CODE_LEN: usize = 6;
pub const INVITE_TTL_SECS: i64 = 24 * 60 * 60;

pub const MAX_CHAT_LEN: usize = 500;
pub const MAX_NAME_LEN: usize = 24;

pub type UserId = String;
pub type RoomId = String;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Color {
    Black,
    White,
}

impl Color {
    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::Black => Self::White,
            Self::White => Self::Black,
        }
    }
}

impl From<Stone> for Color {
    fn from(s: Stone) -> Self {
        match s {
            Stone::Black => Self::Black,
            Stone::White => Self::White,
        }
    }
}

impl From<Color> for Stone {
    fn from(c: Color) -> Self {
        match c {
            Color::Black => Self::Black,
            Color::White => Self::White,
        }
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Black => "Black",
            Self::White => "White",
        })
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "snake_case")]
pub enum ForbiddenKind {
    Overline,
    DoubleFour,
    DoubleThree,
}

impl ForbiddenKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Overline => Forbidden::Overline.label(),
            Self::DoubleFour => Forbidden::DoubleFour.label(),
            Self::DoubleThree => Forbidden::DoubleThree.label(),
        }
    }
}

impl From<Forbidden> for ForbiddenKind {
    fn from(f: Forbidden) -> Self {
        match f {
            Forbidden::Overline => Self::Overline,
            Forbidden::DoubleFour => Self::DoubleFour,
            Forbidden::DoubleThree => Self::DoubleThree,
        }
    }
}

impl From<ForbiddenKind> for Forbidden {
    fn from(f: ForbiddenKind) -> Self {
        match f {
            ForbiddenKind::Overline => Self::Overline,
            ForbiddenKind::DoubleFour => Self::DoubleFour,
            ForbiddenKind::DoubleThree => Self::DoubleThree,
        }
    }
}

impl fmt::Display for ForbiddenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// 0-based column `x`, row `y`
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Coord {
    pub x: u8,
    pub y: u8,
}

impl Coord {
    pub const fn to_pos(self) -> Option<Pos> {
        Pos::new(self.x, self.y)
    }
}

impl From<Pos> for Coord {
    fn from(p: Pos) -> Self {
        Self { x: p.x(), y: p.y() }
    }
}

impl TryFrom<Coord> for Pos {
    type Error = ();

    fn try_from(c: Coord) -> Result<Self, ()> {
        c.to_pos().ok_or(())
    }
}

impl fmt::Display for Coord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.to_pos() {
            Some(p) => write!(f, "{p}"),
            None => write!(f, "({},{})", self.x, self.y),
        }
    }
}

/// `Spectator` reserved now: spectating later = issuing a code, not a schema change
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Black,
    White,
    Spectator,
}

impl Role {
    pub const fn color(self) -> Option<Color> {
        match self {
            Self::Black => Some(Color::Black),
            Self::White => Some(Color::White),
            Self::Spectator => None,
        }
    }

    pub const fn from_color(c: Color) -> Self {
        match c {
            Color::Black => Self::Black,
            Color::White => Self::White,
        }
    }

    pub const fn is_player(self) -> bool {
        !matches!(self, Self::Spectator)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Black => "black",
            Self::White => "white",
            Self::Spectator => "spectator",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "black" => Some(Self::Black),
            "white" => Some(Self::White),
            "spectator" => Some(Self::Spectator),
            _ => None,
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Black => "Black",
            Self::White => "White",
            Self::Spectator => "Spectator",
        })
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "snake_case")]
pub enum RoomStatus {
    Waiting,
    Playing,
    Finished,
}

impl RoomStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Playing => "playing",
            Self::Finished => "finished",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "waiting" => Some(Self::Waiting),
            "playing" => Some(Self::Playing),
            "finished" => Some(Self::Finished),
            _ => None,
        }
    }
}

/// Only `Private` exists today
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    Private,
    Public,
}

impl Visibility {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Public => "public",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    /// Overline also wins for White
    Five,
    Resign,
    Draw,
    /// Reserved; no time control yet
    Timeout,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct PlayerInfo {
    pub user_id: UserId,
    pub display_name: String,
    pub role: Role,
    /// At least one live event stream
    pub online: bool,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub struct MoveRecord {
    /// 1-based
    pub n: u32,
    pub x: u8,
    pub y: u8,
    pub color: Color,
}

impl MoveRecord {
    pub const fn coord(&self) -> Coord {
        Coord {
            x: self.x,
            y: self.y,
        }
    }

    pub const fn pos(&self) -> Option<Pos> {
        Pos::new(self.x, self.y)
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub struct ForbiddenPoint {
    pub x: u8,
    pub y: u8,
    pub kind: ForbiddenKind,
}

impl ForbiddenPoint {
    pub const fn coord(&self) -> Coord {
        Coord {
            x: self.x,
            y: self.y,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct GameResult {
    /// `None` = draw
    pub winner: Option<Color>,
    pub reason: EndReason,
    /// Winning stones when `reason == Five`
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub line: Vec<Coord>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, Default)]
pub struct Seats {
    pub black: Option<PlayerInfo>,
    pub white: Option<PlayerInfo>,
}

impl Seats {
    pub fn by_color(&self, c: Color) -> Option<&PlayerInfo> {
        match c {
            Color::Black => self.black.as_ref(),
            Color::White => self.white.as_ref(),
        }
    }

    pub fn by_user(&self, user_id: &str) -> Option<&PlayerInfo> {
        [self.black.as_ref(), self.white.as_ref()]
            .into_iter()
            .flatten()
            .find(|p| p.user_id == user_id)
    }

    pub fn is_full(&self) -> bool {
        self.black.is_some() && self.white.is_some()
    }
}

/// Full state on every `state` event; 225 bytes = immunity to lost or lagged events
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct RoomSnapshot {
    pub room_id: RoomId,
    /// Members only
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invite_code: Option<String>,
    pub status: RoomStatus,
    /// State version; echoed in `MoveRequest::expect_seq`
    pub seq: u64,
    /// 225 chars `.`/`X`/`O`, row-major, row 1 first
    pub board: String,
    /// Current game only; board derivable from it
    pub moves: Vec<MoveRecord>,
    /// `None` once finished
    pub turn: Option<Color>,
    pub seats: Seats,
    pub spectators: u32,
    /// Server-computed; non-empty only on Black's turn
    pub forbidden: Vec<ForbiddenPoint>,
    pub last_move: Option<Coord>,
    pub result: Option<GameResult>,
    /// Players who pressed Enter for the next game (finished rooms only)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ready: Vec<UserId>,
    /// Games finished in this room so far
    #[serde(default)]
    pub games_played: u32,
    /// RFC 3339
    pub created_at: String,
}

impl RoomSnapshot {
    pub fn to_game(&self) -> Result<Game, (usize, MoveError)> {
        Game::from_moves(self.moves.iter().filter_map(MoveRecord::pos))
    }

    /// Local recomputation, compared with `forbidden` to catch rule bugs early
    pub fn local_forbidden(&self) -> Option<Vec<ForbiddenPoint>> {
        let game = self.to_game().ok()?;
        let mut pts: Vec<ForbiddenPoint> = game
            .forbidden_points()
            .into_iter()
            .map(|(p, k)| ForbiddenPoint {
                x: p.x(),
                y: p.y(),
                kind: k.into(),
            })
            .collect();
        pts.sort_by_key(|p| (p.y, p.x));
        Some(pts)
    }

    pub fn role_of(&self, user_id: &str) -> Option<Role> {
        self.seats.by_user(user_id).map(|p| p.role)
    }

    pub fn is_turn_of(&self, user_id: &str) -> bool {
        match (self.turn, self.role_of(user_id)) {
            (Some(t), Some(r)) => r.color() == Some(t),
            _ => false,
        }
    }
}

/// `POST /users/anonymous`
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, Default)]
pub struct RegisterRequest {
    /// Missing or empty = server-generated name
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

/// Response to `POST /users/anonymous`
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct RegisterResponse {
    pub user_id: UserId,
    /// Shown once, server keeps only the hash
    pub token: String,
    pub display_name: String,
}

/// `PATCH /users/me`
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct UpdateMeRequest {
    pub display_name: String,
}

/// `GET /users/me` and response to `PATCH /users/me`
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct UserInfo {
    pub user_id: UserId,
    pub display_name: String,
}

/// Response to `POST /rooms`
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct CreateRoomResponse {
    pub room_id: RoomId,
    pub invite_code: String,
    /// Black/White assigned by the server
    pub my_role: Role,
    /// RFC 3339
    pub expires_at: String,
    pub snapshot: RoomSnapshot,
}

/// `POST /rooms/join`
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct JoinByCodeRequest {
    pub invite_code: String,
}

/// Response to `POST /rooms/join` and `POST /rooms/{id}/join`
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct JoinResponse {
    pub room_id: RoomId,
    pub my_role: Role,
    pub snapshot: RoomSnapshot,
}

/// `POST /rooms/{id}/move`
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub struct MoveRequest {
    pub x: u8,
    pub y: u8,
    /// Snapshot `seq` the client saw; stale => `seq_mismatch`, also dedups retried POSTs
    pub expect_seq: u64,
}

/// `POST /rooms/{id}/chat`
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct ChatRequest {
    pub text: String,
}

/// State via the event stream only, never via the POST response
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub struct Ack {
    pub seq: u64,
}

/// `GET /healthz`
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct Health {
    pub ok: bool,
    pub version: String,
    pub protocol: u32,
    /// Below this: 426
    pub min_client_version: String,
    /// Newest released client; below this: update hint only
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_client_version: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    BadRequest,
    Unauthorized,
    NotFound,
    InviteInvalid,
    InviteExpired,
    RoomFull,
    NotAMember,
    NotAPlayer,
    NotYourTurn,
    NotPlaying,
    SeqMismatch,
    Occupied,
    Forbidden,
    ClientOutdated,
    Internal,
}

/// HTTP status derives from `code`
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
    /// With `ClientOutdated`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_client_version: Option<String>,
    /// With `Forbidden`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forbidden: Option<ForbiddenKind>,
    /// With `SeqMismatch`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_seq: Option<u64>,
}

impl ApiError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            min_client_version: None,
            forbidden: None,
            current_seq: None,
        }
    }

    pub const fn http_status(&self) -> u16 {
        match self.code {
            ErrorCode::BadRequest | ErrorCode::InviteInvalid => 400,
            ErrorCode::Unauthorized => 401,
            ErrorCode::NotAMember | ErrorCode::NotAPlayer | ErrorCode::NotYourTurn => 403,
            ErrorCode::NotFound => 404,
            ErrorCode::InviteExpired => 410,
            ErrorCode::RoomFull | ErrorCode::NotPlaying | ErrorCode::SeqMismatch => 409,
            ErrorCode::Occupied | ErrorCode::Forbidden => 422,
            ErrorCode::ClientOutdated => 426,
            ErrorCode::Internal => 500,
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for ApiError {}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StateCause {
    /// Initial connect or resync
    Snapshot,
    Start,
    Move {
        record: MoveRecord,
    },
}

/// SSE `data:` payload; `kind()` = `event:` name
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServerEvent {
    /// First join only; reconnects show up as `Presence`
    Joined { user: PlayerInfo },
    Presence {
        user_id: UserId,
        display_name: String,
        online: bool,
    },
    /// After every change and on (re)connect
    State {
        cause: StateCause,
        snapshot: RoomSnapshot,
    },
    Chat {
        from: UserId,
        display_name: String,
        text: String,
        at: String,
    },
    /// `snapshot.result` set
    Gameover {
        result: GameResult,
        snapshot: RoomSnapshot,
    },
    /// Player ready for the next game; both ready => `state`/`Start`
    Ready { from: UserId, display_name: String },
}

impl ServerEvent {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Joined { .. } => "joined",
            Self::Presence { .. } => "presence",
            Self::State { .. } => "state",
            Self::Chat { .. } => "chat",
            Self::Gameover { .. } => "gameover",
            Self::Ready { .. } => "ready",
        }
    }

    pub const fn snapshot(&self) -> Option<&RoomSnapshot> {
        match self {
            Self::State { snapshot, .. } | Self::Gameover { snapshot, .. } => Some(snapshot),
            _ => None,
        }
    }
}

/// `seq` = SSE `id`
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct Envelope {
    pub seq: u64,
    pub room_id: RoomId,
    pub event: ServerEvent,
}

/// Suffixes after `-`/`+` ignored
pub fn parse_semver(s: &str) -> Option<(u64, u64, u64)> {
    let core = s.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next()?;
    let mut it = core.split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next().unwrap_or("0").parse().ok()?;
    let patch = it.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

/// Malformed = too old
pub fn version_at_least(client: &str, min: &str) -> bool {
    match (parse_semver(client), parse_semver(min)) {
        (Some(c), Some(m)) => c >= m,
        _ => false,
    }
}

/// Shape only, not existence
pub fn is_valid_invite_code(code: &str) -> bool {
    code.len() == INVITE_CODE_LEN && code.bytes().all(|b| INVITE_ALPHABET.contains(&b))
}

/// Trim, uppercase, drop `-`/spaces; O/I/L without unambiguous target => rejected
pub fn normalize_invite_code(input: &str) -> Option<String> {
    let mut out = String::with_capacity(INVITE_CODE_LEN);
    for ch in input.trim().chars() {
        if ch == '-' || ch == ' ' {
            continue;
        }
        let up = ch.to_ascii_uppercase();
        let mapped = match up {
            'O' => '0',
            'I' | 'L' => '1',
            other => other,
        };
        if mapped == '0' || mapped == '1' {
            return None;
        }
        out.push(mapped);
    }
    is_valid_invite_code(&out).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_kind_matches_serde_tag() {
        let ev = ServerEvent::Ready {
            from: "u".into(),
            display_name: "n".into(),
        };
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["kind"], ev.kind());
        let back: ServerEvent = serde_json::from_value(v).unwrap();
        assert_eq!(back, ev);
    }

    #[test]
    fn semver_compare() {
        assert!(version_at_least("0.1.0", "0.1.0"));
        assert!(version_at_least("0.2.0", "0.1.9"));
        assert!(version_at_least("1.0.0-beta.1", "0.9.0"));
        assert!(!version_at_least("0.0.9", "0.1.0"));
        assert!(!version_at_least("garbage", "0.1.0"));
    }

    #[test]
    fn invite_codes() {
        assert!(is_valid_invite_code("ABC234"));
        assert!(!is_valid_invite_code("ABC10L"));
        assert_eq!(normalize_invite_code(" abc-234 "), Some("ABC234".into()));
        assert_eq!(normalize_invite_code("ABC10L"), None);
        assert_eq!(normalize_invite_code("ABC"), None);
    }

    #[test]
    fn snapshot_roundtrip_and_local_rules() {
        let snap = RoomSnapshot {
            room_id: "r".into(),
            invite_code: Some("ABC234".into()),
            status: RoomStatus::Playing,
            seq: 3,
            board: ".".repeat(rules::CELLS),
            moves: vec![
                MoveRecord {
                    n: 1,
                    x: 7,
                    y: 7,
                    color: Color::Black,
                },
                MoveRecord {
                    n: 2,
                    x: 0,
                    y: 0,
                    color: Color::White,
                },
            ],
            turn: Some(Color::Black),
            seats: Seats::default(),
            spectators: 0,
            forbidden: vec![],
            last_move: Some(Coord { x: 0, y: 0 }),
            result: None,
            ready: vec![],
            games_played: 0,
            created_at: "2026-01-01T00:00:00Z".into(),
        };
        let json = serde_json::to_string(&snap).unwrap();
        let back: RoomSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back, snap);
        let game = snap.to_game().unwrap();
        assert_eq!(game.moves().len(), 2);
        assert_eq!(snap.local_forbidden().unwrap(), vec![]);
    }

    #[test]
    fn error_status_codes() {
        assert_eq!(
            ApiError::new(ErrorCode::ClientOutdated, "").http_status(),
            426
        );
        assert_eq!(ApiError::new(ErrorCode::SeqMismatch, "").http_status(), 409);
    }
}
