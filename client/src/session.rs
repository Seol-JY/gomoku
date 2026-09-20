//! Terminal-independent session state. Never touches the network

use proto::{Color, EndReason, MoveRequest, RoomSnapshot, RoomStatus, ServerEvent, StateCause};
use rules::{MoveError, Pos};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LogEntry {
    Chat {
        from: String,
        text: String,
        mine: bool,
    },
    Notice(String),
    Error(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Connection {
    Connecting,
    Connected,
    Reconnecting,
}

impl Connection {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Connecting => "connecting...",
            Self::Connected => "connected",
            Self::Reconnecting => "reconnecting...",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Move(MoveRequest),
    Chat(String),
    Resign,
    /// Enter after a finished game
    Ready,
    Quit,
}

pub const HELP: &[&str] = &[
    "Type a coordinate like h8 to play; anything else is chat.",
    "Empty input + arrow keys move the cursor, Enter plays there.",
    "After a game, Enter starts the next one (loser plays Black).",
    "/resign   resign        /help   this text        /quit   leave (rejoin later with `gomoku rejoin`)",
];

#[derive(Debug)]
pub struct Session {
    pub room_id: String,
    pub my_user_id: String,
    pub my_name: String,
    pub invite_code: Option<String>,
    pub snapshot: Option<RoomSnapshot>,
    pub log: Vec<LogEntry>,
    pub connection: Connection,
    pub last_event_id: Option<u64>,
    announced_result: bool,
}

impl Session {
    pub fn new(
        room_id: String,
        my_user_id: String,
        my_name: String,
        invite_code: Option<String>,
    ) -> Self {
        Self {
            room_id,
            my_user_id,
            my_name,
            invite_code,
            snapshot: None,
            log: Vec::new(),
            connection: Connection::Connecting,
            last_event_id: None,
            announced_result: false,
        }
    }

    pub fn notice(&mut self, text: impl Into<String>) {
        self.log.push(LogEntry::Notice(text.into()));
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.log.push(LogEntry::Error(text.into()));
    }

    pub fn my_color(&self) -> Option<Color> {
        self.snapshot.as_ref()?.role_of(&self.my_user_id)?.color()
    }

    pub fn opponent(&self) -> Option<&proto::PlayerInfo> {
        let snap = self.snapshot.as_ref()?;
        let mine = self.my_color()?;
        snap.seats.by_color(mine.opposite())
    }

    pub fn opponent_name(&self) -> String {
        self.opponent()
            .map_or("opponent".to_owned(), |p| p.display_name.clone())
    }

    pub fn status(&self) -> Option<RoomStatus> {
        self.snapshot.as_ref().map(|s| s.status)
    }

    pub fn is_playing(&self) -> bool {
        self.status() == Some(RoomStatus::Playing)
    }

    pub fn is_finished(&self) -> bool {
        self.status() == Some(RoomStatus::Finished)
    }

    pub fn is_my_turn(&self) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|s| s.is_turn_of(&self.my_user_id))
    }

    pub fn last_move(&self) -> Option<Pos> {
        self.snapshot.as_ref()?.last_move?.to_pos()
    }

    pub fn i_am_ready(&self) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|s| s.ready.contains(&self.my_user_id))
    }

    pub fn opponent_ready(&self) -> bool {
        match (self.snapshot.as_ref(), self.opponent()) {
            (Some(s), Some(o)) => s.ready.contains(&o.user_id),
            _ => false,
        }
    }

    /// Result from my point of view, e.g. `You win by five in a row.`
    pub fn result_text(&self) -> Option<String> {
        let snap = self.snapshot.as_ref()?;
        let result = snap.result.as_ref()?;
        Some(result_text(result, self.my_color()))
    }

    pub fn turn_text(&self) -> String {
        let Some(snap) = &self.snapshot else {
            return "loading...".into();
        };
        match snap.status {
            RoomStatus::Waiting => "waiting for an opponent".into(),
            RoomStatus::Finished => self.result_text().unwrap_or_else(|| "game over".into()),
            RoomStatus::Playing if self.is_my_turn() => "your move".into(),
            RoomStatus::Playing => format!("{} is thinking...", self.opponent_name()),
        }
    }

    pub fn interpret(&mut self, raw: &str) -> Action {
        let line = raw.trim();
        if line.is_empty() {
            return if self.is_finished() {
                self.ready()
            } else {
                Action::None
            };
        }
        if let Some(cmd) = line.strip_prefix('/') {
            return self.command(cmd);
        }
        if Pos::looks_like_notation(line) {
            return match Pos::from_notation(line) {
                Ok(pos) => self.try_move(pos),
                Err(e) => {
                    self.error(format!("{line}: {e}"));
                    Action::None
                }
            };
        }
        let mut text = line.to_owned();
        if text.chars().count() > proto::MAX_CHAT_LEN {
            text = text.chars().take(proto::MAX_CHAT_LEN).collect();
        }
        Action::Chat(text)
    }

    /// Enter after a finished game. Idempotent
    pub fn ready(&mut self) -> Action {
        if !self.is_finished() || self.my_color().is_none() {
            return Action::None;
        }
        if self.i_am_ready() {
            return Action::None;
        }
        Action::Ready
    }

    fn command(&mut self, cmd: &str) -> Action {
        let name = cmd
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        match name.as_str() {
            "resign" => {
                if !self.is_playing() {
                    self.notice("no game in progress");
                    return Action::None;
                }
                Action::Resign
            }
            "help" | "?" => {
                for l in HELP {
                    self.notice(*l);
                }
                Action::None
            }
            "quit" | "exit" | "q" => Action::Quit,
            other => {
                self.error(format!("unknown command /{other} (try /help)"));
                Action::None
            }
        }
    }

    /// Local pre-check only; the server stays authoritative
    fn try_move(&mut self, pos: Pos) -> Action {
        let Some(snap) = &self.snapshot else {
            self.notice("still loading, try again");
            return Action::None;
        };
        match snap.status {
            RoomStatus::Waiting => {
                self.notice("opponent has not joined yet");
                return Action::None;
            }
            RoomStatus::Finished => {
                self.notice("game over: press Enter to play again");
                return Action::None;
            }
            RoomStatus::Playing => {}
        }
        let Some(mine) = self.my_color() else {
            self.notice("spectators cannot move");
            return Action::None;
        };
        if snap.turn != Some(mine) {
            self.notice("not your turn");
            return Action::None;
        }
        match snap.to_game() {
            Ok(game) => {
                if let Err(e) = game.validate(pos) {
                    let msg = match e {
                        MoveError::Occupied => format!("{pos} is occupied"),
                        MoveError::GameOver => "game over".to_owned(),
                        MoveError::Forbidden(kind) => {
                            format!("{pos}: forbidden ({})", kind.label())
                        }
                    };
                    self.error(msg);
                    return Action::None;
                }
            }
            Err((i, e)) => {
                tracing::warn!(index = i, error = %e, "local replay of move list failed");
            }
        }
        Action::Move(MoveRequest {
            x: pos.x(),
            y: pos.y(),
            expect_seq: snap.seq,
        })
    }

    pub fn set_snapshot(&mut self, snapshot: RoomSnapshot) {
        if snapshot.invite_code.is_some() {
            self.invite_code.clone_from(&snapshot.invite_code);
        }
        check_forbidden(&snapshot);
        self.snapshot = Some(snapshot);
    }

    pub fn apply(&mut self, seq: Option<u64>, event: ServerEvent) {
        if let Some(s) = seq {
            self.last_event_id = Some(s);
        }
        let me = self.my_user_id.clone();
        match event {
            ServerEvent::Joined { user } => {
                if user.user_id != me {
                    self.notice(format!("{} joined", user.display_name));
                }
            }
            ServerEvent::Presence {
                user_id,
                display_name,
                online,
            } => {
                if user_id != me {
                    let state = if online { "is back" } else { "is offline" };
                    self.notice(format!("{display_name} {state}"));
                }
                if let Some(snap) = &mut self.snapshot {
                    for seat in [&mut snap.seats.black, &mut snap.seats.white]
                        .into_iter()
                        .flatten()
                    {
                        if seat.user_id == user_id {
                            seat.online = online;
                        }
                    }
                }
            }
            ServerEvent::State { cause, snapshot } => {
                if cause == StateCause::Start {
                    let mine = snapshot
                        .role_of(&me)
                        .map_or("watching".to_owned(), |r| r.to_string());
                    let n = snapshot.games_played + 1;
                    self.notice(format!("Game {n}: you are {mine}."));
                }
                if snapshot.status != RoomStatus::Finished {
                    self.announced_result = false;
                }
                self.set_snapshot(snapshot);
            }
            ServerEvent::Chat {
                from,
                display_name,
                text,
                ..
            } => {
                self.log.push(LogEntry::Chat {
                    from: display_name,
                    text,
                    mine: from == me,
                });
            }
            ServerEvent::Gameover { result, snapshot } => {
                if !self.announced_result {
                    self.announced_result = true;
                    let mine = snapshot.role_of(&me).and_then(proto::Role::color);
                    self.notice(result_text(&result, mine));
                    if mine.is_some() {
                        self.notice("Press Enter to play again (loser plays Black).");
                    }
                }
                self.set_snapshot(snapshot);
            }
            ServerEvent::Ready { from, display_name } => {
                if let Some(snap) = &mut self.snapshot
                    && !snap.ready.contains(&from)
                {
                    snap.ready.push(from.clone());
                }
                if from != me {
                    self.notice(format!("{display_name} is ready."));
                }
            }
        }
    }
}

pub fn result_text(result: &proto::GameResult, me: Option<Color>) -> String {
    let reason = match result.reason {
        EndReason::Five => "five in a row",
        EndReason::Resign => "resignation",
        EndReason::Draw => "a full board",
        EndReason::Timeout => "timeout",
    };
    match (result.winner, me) {
        (None, _) => "Draw.".to_owned(),
        (Some(w), Some(m)) if w == m => format!("You win by {reason}."),
        (Some(_), Some(_)) => format!("You lose by {reason}."),
        (Some(w), None) => format!("{w} wins by {reason}."),
    }
}

/// Server list vs local rules; mismatch = rules bug, logged
fn check_forbidden(snapshot: &RoomSnapshot) {
    if snapshot.turn != Some(Color::Black) || snapshot.status != RoomStatus::Playing {
        return;
    }
    let Some(local) = snapshot.local_forbidden() else {
        return;
    };
    let mut server = snapshot.forbidden.clone();
    server.sort_by_key(|p| (p.y, p.x));
    if server != local {
        tracing::warn!(
            room = %snapshot.room_id,
            seq = snapshot.seq,
            server = ?server,
            local = ?local,
            "forbidden points differ between server and local rules"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proto::{GameResult, MoveRecord, PlayerInfo, Role, Seats};

    fn player(id: &str, role: Role) -> PlayerInfo {
        PlayerInfo {
            user_id: id.into(),
            display_name: id.to_uppercase(),
            role,
            online: true,
        }
    }

    fn snapshot(moves: &[(u8, u8)], status: RoomStatus) -> RoomSnapshot {
        let mut game = rules::Game::new();
        let mut records = Vec::new();
        for (n, (x, y)) in moves.iter().enumerate() {
            let pos = Pos::new(*x, *y).unwrap();
            let color = Color::from(game.turn());
            game.play(pos).unwrap();
            records.push(MoveRecord {
                n: n as u32 + 1,
                x: *x,
                y: *y,
                color,
            });
        }
        RoomSnapshot {
            room_id: "room".into(),
            invite_code: Some("ABC234".into()),
            status,
            seq: moves.len() as u64,
            board: game.board().to_compact(),
            moves: records,
            turn: (status == RoomStatus::Playing).then(|| Color::from(game.turn())),
            seats: Seats {
                black: Some(player("me", Role::Black)),
                white: Some(player("opp", Role::White)),
            },
            spectators: 0,
            forbidden: vec![],
            last_move: None,
            result: None,
            ready: vec![],
            games_played: 0,
            created_at: String::new(),
        }
    }

    fn session(snap: RoomSnapshot) -> Session {
        let mut s = Session::new("room".into(), "me".into(), "ME".into(), None);
        s.set_snapshot(snap);
        s
    }

    #[test]
    fn classifies_input() {
        let mut s = session(snapshot(&[], RoomStatus::Playing));
        assert_eq!(
            s.interpret("hello there"),
            Action::Chat("hello there".into())
        );
        assert_eq!(s.interpret("   "), Action::None);
        assert_eq!(s.interpret("/quit"), Action::Quit);
        assert_eq!(s.interpret("/resign"), Action::Resign);
        assert_eq!(
            s.interpret("h8"),
            Action::Move(MoveRequest {
                x: 7,
                y: 7,
                expect_seq: 0
            })
        );
        assert_eq!(s.interpret("/bogus"), Action::None);
        assert!(matches!(s.log.last(), Some(LogEntry::Error(_))));
    }

    #[test]
    fn rejects_locally() {
        let mut s = session(snapshot(&[(7, 7)], RoomStatus::Playing));
        assert_eq!(s.interpret("i9"), Action::None);
        assert_eq!(
            s.log.last(),
            Some(&LogEntry::Notice("not your turn".into()))
        );

        let mut s = session(snapshot(&[(7, 7), (0, 0)], RoomStatus::Playing));
        assert_eq!(s.interpret("h8"), Action::None);
        assert!(matches!(s.log.last(), Some(LogEntry::Error(m)) if m.contains("occupied")));

        // F8 G8 + H6 H7, then H8 = 3-3
        let mut s = session(snapshot(
            &[
                (5, 7),
                (0, 0),
                (6, 7),
                (1, 0),
                (7, 5),
                (2, 0),
                (7, 6),
                (3, 0),
            ],
            RoomStatus::Playing,
        ));
        assert_eq!(s.interpret("h8"), Action::None);
        assert!(matches!(s.log.last(), Some(LogEntry::Error(m)) if m.contains("3-3")));

        let mut s = session(snapshot(&[], RoomStatus::Waiting));
        assert_eq!(s.interpret("h8"), Action::None);
        assert_eq!(s.interpret("/resign"), Action::None);
        assert_eq!(s.interpret(""), Action::None);
    }

    #[test]
    fn ready_after_game_over() {
        let mut snap = snapshot(&[], RoomStatus::Finished);
        snap.result = Some(GameResult {
            winner: Some(Color::White),
            reason: EndReason::Resign,
            line: vec![],
        });
        let mut s = session(snap);
        assert_eq!(s.result_text().unwrap(), "You lose by resignation.");
        assert_eq!(s.interpret("h8"), Action::None);
        assert_eq!(s.interpret(""), Action::Ready);
        s.apply(
            None,
            ServerEvent::Ready {
                from: "me".into(),
                display_name: "ME".into(),
            },
        );
        assert!(s.i_am_ready());
        assert_eq!(s.interpret(""), Action::None);
        assert!(!s.opponent_ready());
        s.apply(
            None,
            ServerEvent::Ready {
                from: "opp".into(),
                display_name: "OPP".into(),
            },
        );
        assert!(s.opponent_ready());
        assert!(matches!(s.log.last(), Some(LogEntry::Notice(m)) if m == "OPP is ready."));
    }
}
