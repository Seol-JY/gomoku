//! ratatui front end. One `select!`, one draw per iteration

pub mod board;
pub mod chat;
pub mod layout;

use std::time::Duration;

use anyhow::{Context, Result};
use futures::StreamExt;
use proto::Color;
use ratatui::Frame;
use ratatui::crossterm::event::{
    Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color as TColor, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};
use rules::{Pos, SIZE};
use unicode_width::UnicodeWidthStr;

use crate::driver::{Driver, Exit};
use crate::session::{Action, Connection, Session};

const DIM: Style = Style::new().fg(TColor::DarkGray);
const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);
const HOT: Style = Style::new().fg(TColor::Green).add_modifier(Modifier::BOLD);
const WARN: Style = Style::new().fg(TColor::Red);
const PROMPT: Style = Style::new().fg(TColor::Cyan).add_modifier(Modifier::BOLD);

#[derive(Debug, Default)]
struct Input {
    text: String,
    /// char index
    cursor: usize,
}

impl Input {
    fn insert(&mut self, c: char) {
        let idx = self.byte_idx();
        self.text.insert(idx, c);
        self.cursor += 1;
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.cursor -= 1;
        let idx = self.byte_idx();
        self.text.remove(idx);
    }

    fn delete(&mut self) {
        if self.cursor < self.text.chars().count() {
            let idx = self.byte_idx();
            self.text.remove(idx);
        }
    }

    fn byte_idx(&self) -> usize {
        self.text
            .char_indices()
            .nth(self.cursor)
            .map_or(self.text.len(), |(i, _)| i)
    }

    fn take(&mut self) -> String {
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }

    fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    fn cursor_width(&self) -> usize {
        self.text
            .chars()
            .take(self.cursor)
            .collect::<String>()
            .width()
    }
}

/// What the typed text points at on the board, before Enter
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Preview {
    None,
    Column(u8),
    Cell(Pos),
}

fn preview(text: &str) -> Preview {
    let t = text.trim();
    let mut chars = t.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphabetic() => {
            let x = c.to_ascii_uppercase() as u8 - b'A';
            if x < SIZE {
                Preview::Column(x)
            } else {
                Preview::None
            }
        }
        _ => Pos::from_notation(t).map_or(Preview::None, Preview::Cell),
    }
}

#[derive(Debug, Default)]
struct App {
    input: Input,
    /// lines scrolled up from the bottom; 0 follows
    scroll: usize,
    cursor: Option<Pos>,
}

impl App {
    fn target(&self) -> Preview {
        match preview(&self.input.text) {
            Preview::None if self.input.is_empty() => {
                self.cursor.map_or(Preview::None, Preview::Cell)
            }
            p => p,
        }
    }

    fn move_cursor(&mut self, dx: i8, dy: i8, session: &Session) {
        let from = self
            .cursor
            .or_else(|| session.last_move())
            .unwrap_or_else(|| Pos::new(7, 7).expect("centre"));
        let x = (from.x() as i8 + dx).clamp(0, SIZE as i8 - 1) as u8;
        let y = (from.y() as i8 + dy).clamp(0, SIZE as i8 - 1) as u8;
        self.cursor = Pos::new(x, y);
    }
}

pub async fn run(mut driver: Driver) -> Result<Exit> {
    let mut terminal = ratatui::try_init().context("cannot initialise the terminal")?;
    let result = event_loop(&mut terminal, &mut driver).await;
    ratatui::restore();
    driver.shutdown();
    result
}

async fn event_loop(terminal: &mut ratatui::DefaultTerminal, driver: &mut Driver) -> Result<Exit> {
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    let mut app = App::default();

    loop {
        tokio::select! {
            ev = events.next() => match ev {
                Some(Ok(Event::Key(key))) => handle_key(key, driver, &mut app),
                Some(Ok(_)) => {}
                Some(Err(e)) => return Err(e).context("terminal event stream failed"),
                None => return Ok(Exit::Normal),
            },
            msg = driver.rx.recv() => match msg {
                Some(m) => driver.handle(m),
                None => return Ok(Exit::Fatal("event channel closed".into())),
            },
            _ = tick.tick() => {}
        }
        if let Some(exit) = driver.exit.clone() {
            return Ok(exit);
        }
        terminal.draw(|f| draw(f, &driver.session, &mut app))?;
    }
}

fn handle_key(key: KeyEvent, driver: &mut Driver, app: &mut App) {
    if key.kind != KeyEventKind::Press {
        return; // Windows also reports Release/Repeat
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let empty = app.input.is_empty();
    match key.code {
        KeyCode::Char('c') if ctrl => driver.exit = Some(Exit::Normal),
        KeyCode::Esc => driver.exit = Some(Exit::Normal),
        KeyCode::Char('u') if ctrl => {
            app.input.take();
        }
        KeyCode::Enter => {
            let action = if empty {
                match app.cursor {
                    Some(p) if driver.session.is_playing() => {
                        driver.session.interpret(&p.to_string())
                    }
                    _ if driver.session.is_finished() => {
                        app.cursor = None;
                        driver.session.ready()
                    }
                    _ => Action::None,
                }
            } else {
                let line = app.input.take();
                driver.session.interpret(&line)
            };
            driver.perform(action);
            app.scroll = 0;
        }
        KeyCode::Backspace => app.input.backspace(),
        KeyCode::Delete => app.input.delete(),
        KeyCode::Tab => app.cursor = driver.session.last_move().or(app.cursor),
        KeyCode::Left if empty => app.move_cursor(-1, 0, &driver.session),
        KeyCode::Right if empty => app.move_cursor(1, 0, &driver.session),
        KeyCode::Up if empty => app.move_cursor(0, -1, &driver.session),
        KeyCode::Down if empty => app.move_cursor(0, 1, &driver.session),
        KeyCode::Left => app.input.cursor = app.input.cursor.saturating_sub(1),
        KeyCode::Right => {
            app.input.cursor = (app.input.cursor + 1).min(app.input.text.chars().count());
        }
        KeyCode::Home => app.input.cursor = 0,
        KeyCode::End => app.input.cursor = app.input.text.chars().count(),
        KeyCode::Up => app.scroll += 1,
        KeyCode::Down => app.scroll = app.scroll.saturating_sub(1),
        KeyCode::PageUp => app.scroll += 10,
        KeyCode::PageDown => app.scroll = app.scroll.saturating_sub(10),
        KeyCode::Char(c) if !ctrl => {
            app.input.insert(c);
            if let Preview::Cell(p) = preview(&app.input.text) {
                app.cursor = Some(p);
            }
        }
        _ => {}
    }
}

fn draw(frame: &mut Frame<'_>, session: &Session, app: &mut App) {
    let area = frame.area();
    let mode = layout::Mode::choose(area.width, area.height);
    let Some(areas) = layout::split(area, mode) else {
        let msg = Paragraph::new("Please enlarge your terminal\n(at least 20 columns)")
            .wrap(Wrap { trim: true });
        frame.render_widget(msg, area);
        return;
    };

    let (cursor, column) = match app.target() {
        Preview::None => (None, None),
        Preview::Column(x) => (None, Some(x)),
        Preview::Cell(p) => (Some(p), None),
    };
    frame.render_widget(
        board::BoardWidget {
            snapshot: session.snapshot.as_ref(),
            compact: mode.compact(),
            cursor,
            column,
        },
        areas.board,
    );
    draw_chat(frame, areas.chat, session, app);
}

fn draw_chat(frame: &mut Frame<'_>, area: Rect, session: &Session, app: &mut App) {
    let title = match &session.invite_code {
        Some(code) => format!(" gomoku  code {code} "),
        None => " gomoku ".to_owned(),
    };
    let block = Block::bordered().title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 || inner.width < 4 {
        return;
    }

    let status_lines = status_lines(session);
    let [status, log, prompt] = Layout::new(
        Direction::Vertical,
        [
            Constraint::Length(status_lines.len() as u16),
            Constraint::Min(1),
            Constraint::Length(1),
        ],
    )
    .areas(inner);

    frame.render_widget(Paragraph::new(status_lines), status);

    let lines = chat::render_log(&session.log, log.width as usize);
    let visible = log.height as usize;
    let max_up = lines.len().saturating_sub(visible);
    app.scroll = app.scroll.min(max_up);
    let end = lines.len() - app.scroll;
    let start = end.saturating_sub(visible);
    frame.render_widget(Paragraph::new(lines[start..end].to_vec()), log);

    draw_prompt(frame, prompt, session, app);
}

fn draw_prompt(frame: &mut Frame<'_>, prompt: Rect, session: &Session, app: &App) {
    let prefix = "> ";
    let prompt_style = if session.is_my_turn() { HOT } else { PROMPT };
    let avail = (prompt.width as usize).saturating_sub(prefix.len() + 1);
    let cursor_w = app.input.cursor_width();
    let skip_w = cursor_w.saturating_sub(avail);
    let mut shown = String::new();
    let mut w = 0usize;
    for c in app.input.text.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if w < skip_w {
            w += cw;
            continue;
        }
        shown.push(c);
    }
    let mut spans = vec![Span::styled(prefix, prompt_style), Span::raw(shown)];
    if app.input.is_empty() {
        let hint = if session.is_finished() && session.my_color().is_some() {
            if session.i_am_ready() {
                format!("waiting for {}...", session.opponent_name())
            } else {
                "Enter to play again".to_owned()
            }
        } else {
            match (app.cursor, session.is_my_turn()) {
                (Some(p), true) => format!("{p}  Enter plays here, arrows move"),
                (Some(p), false) => format!("{p}  arrows move the cursor"),
                (None, true) => "type h8 or use the arrow keys".to_owned(),
                (None, false) => String::new(),
            }
        };
        spans.push(Span::styled(hint, DIM));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), prompt);
    let cx = prompt.x + prefix.len() as u16 + (cursor_w - skip_w) as u16;
    frame.set_cursor_position(Position::new(
        cx.min(prompt.right().saturating_sub(1)),
        prompt.y,
    ));
}

const BADGE: Style = Style::new()
    .fg(TColor::Black)
    .bg(TColor::Green)
    .add_modifier(Modifier::BOLD);
const CODE: Style = Style::new()
    .fg(TColor::Black)
    .bg(TColor::Yellow)
    .add_modifier(Modifier::BOLD);

fn status_lines(session: &Session) -> Vec<Line<'static>> {
    let sep = Span::styled("  │  ", DIM);
    let me = match session.my_color() {
        Some(Color::Black) => Span::styled(format!("{}: Black (X)", session.my_name), BOLD),
        Some(Color::White) => Span::styled(
            format!("{}: White (O)", session.my_name),
            BOLD.fg(TColor::Yellow),
        ),
        None => Span::styled(format!("{}: spectating", session.my_name), BOLD),
    };
    let snap = session.snapshot.as_ref();
    let status = snap.map(|s| s.status);
    let turn = match status {
        Some(proto::RoomStatus::Playing) if session.is_my_turn() => {
            Span::styled(" >> YOUR MOVE << ", BADGE)
        }
        Some(proto::RoomStatus::Playing) => Span::styled(
            format!(
                "{} is thinking...",
                session
                    .opponent()
                    .map_or("opponent", |p| p.display_name.as_str())
            ),
            DIM,
        ),
        Some(proto::RoomStatus::Finished) => {
            Span::styled(session.turn_text(), BOLD.fg(TColor::Magenta))
        }
        _ => Span::styled(session.turn_text(), DIM),
    };
    let first = vec![me, sep.clone(), turn];

    let mut second: Vec<Span<'static>> = Vec::new();
    match session.opponent() {
        Some(opp) => {
            second.push(Span::raw(format!("{} ", opp.display_name)));
            second.push(if opp.online {
                Span::styled("online", Style::new().fg(TColor::Green))
            } else {
                Span::styled("offline", WARN)
            });
        }
        None => second.push(Span::styled("no opponent yet", DIM)),
    }
    if let Some(last) = snap.and_then(|s| s.moves.last().copied()) {
        let who = snap
            .and_then(|s| s.seats.by_color(last.color))
            .map_or_else(|| last.color.to_string(), |p| p.display_name.clone());
        let coord = last
            .pos()
            .map_or_else(|| last.coord().to_string(), |p| p.to_string());
        second.push(sep.clone());
        second.push(Span::styled("last: ", DIM));
        second.push(Span::styled(
            format!("{coord} ({who})"),
            if session.is_my_turn() {
                BOLD
            } else {
                Style::new()
            },
        ));
    }
    second.push(sep.clone());
    let conn_style = match session.connection {
        Connection::Connected => Style::new().fg(TColor::Green),
        Connection::Connecting => Style::new().fg(TColor::Yellow),
        Connection::Reconnecting => WARN.add_modifier(Modifier::BOLD),
    };
    second.push(Span::styled(session.connection.label(), conn_style));
    if session.is_finished() && session.my_color().is_some() {
        let hint = match (session.i_am_ready(), session.opponent_ready()) {
            (false, false) => "Enter: play again".to_owned(),
            (false, true) => format!("{} is ready. Enter: play again", session.opponent_name()),
            (true, _) => format!("waiting for {}...", session.opponent_name()),
        };
        second.push(sep);
        second.push(Span::styled(hint, Style::new().fg(TColor::Yellow)));
    }
    let mut lines = vec![Line::from(first), Line::from(second)];

    if status == Some(proto::RoomStatus::Waiting)
        && let Some(code) = &session.invite_code
    {
        lines.push(Line::from(vec![
            Span::styled("invite code ", DIM),
            Span::styled(format!("  {code}  "), CODE),
        ]));
        lines.push(Line::from(Span::styled(
            format!("your friend runs:  gomoku join {code}"),
            DIM,
        )));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use proto::{MoveRecord, PlayerInfo, Role, RoomSnapshot, RoomStatus, Seats};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn sample_session() -> Session {
        let moves = [(7, 7), (8, 8), (6, 7), (7, 8), (7, 5), (3, 3)];
        let mut game = rules::Game::new();
        let mut records = Vec::new();
        for (n, (x, y)) in moves.iter().enumerate() {
            let color = Color::from(game.turn());
            game.play(Pos::new(*x, *y).unwrap()).unwrap();
            records.push(MoveRecord {
                n: n as u32 + 1,
                x: *x,
                y: *y,
                color,
            });
        }
        let snap = RoomSnapshot {
            room_id: "room".into(),
            invite_code: Some("ABC234".into()),
            status: RoomStatus::Playing,
            seq: 6,
            board: game.board().to_compact(),
            moves: records,
            turn: Some(Color::Black),
            seats: Seats {
                black: Some(PlayerInfo {
                    user_id: "me".into(),
                    display_name: "seol".into(),
                    role: Role::Black,
                    online: true,
                }),
                white: Some(PlayerInfo {
                    user_id: "opp".into(),
                    display_name: "bob".into(),
                    role: Role::White,
                    online: true,
                }),
            },
            spectators: 0,
            forbidden: vec![],
            last_move: Some(proto::Coord { x: 3, y: 3 }),
            result: None,
            ready: vec![],
            games_played: 0,
            created_at: String::new(),
        };
        let mut s = Session::new("room".into(), "me".into(), "seol".into(), None);
        s.connection = Connection::Connected;
        s.set_snapshot(snap);
        s
    }

    fn render(app: &mut App, session: &Session, w: u16, h: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| draw(f, session, app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..h {
            for x in 0..w {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn frame_shows_board_status_and_cursor_hint() {
        let session = sample_session();
        let mut app = App {
            cursor: Some(Pos::new(7, 7).unwrap()),
            ..App::default()
        };
        let frame = render(&mut app, &session, 80, 24);
        println!("{frame}");
        assert!(frame.contains("gomoku  code ABC234"));
        assert!(frame.contains("seol: Black (X)"));
        assert!(frame.contains(">> YOUR MOVE <<"));
        assert!(frame.contains("last: D4 (bob)"));
        assert!(frame.contains(" 8 . . . . . . X X"));
        assert!(frame.contains("H8  Enter plays here"));

        let mut waiting = sample_session();
        let snap = waiting.snapshot.as_mut().unwrap();
        snap.status = RoomStatus::Waiting;
        snap.turn = None;
        snap.seats.white = None;
        let frame = render(&mut App::default(), &waiting, 80, 24);
        println!("{frame}");
        assert!(frame.contains("invite code   ABC234  "));
        assert!(frame.contains("your friend runs:  gomoku join ABC234"));

        app.input = Input {
            text: "k9".into(),
            cursor: 2,
        };
        assert_eq!(app.target(), Preview::Cell(Pos::new(10, 8).unwrap()));
        app.input = Input {
            text: "k".into(),
            cursor: 1,
        };
        assert_eq!(app.target(), Preview::Column(10));
    }

    #[test]
    fn preview_parses_partial_input() {
        assert_eq!(preview(""), Preview::None);
        assert_eq!(preview("h"), Preview::Column(7));
        assert_eq!(preview("H"), Preview::Column(7));
        assert_eq!(preview("z"), Preview::None);
        assert_eq!(preview("h8"), Preview::Cell(Pos::new(7, 7).unwrap()));
        assert_eq!(preview("h 15"), Preview::Cell(Pos::new(7, 14).unwrap()));
        assert_eq!(preview("h16"), Preview::None);
        assert_eq!(preview("hello"), Preview::None);
        assert_eq!(preview("/quit"), Preview::None);
    }
}
