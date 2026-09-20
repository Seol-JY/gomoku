use proto::{Color, ForbiddenKind, RoomSnapshot, RoomStatus};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color as TColor, Modifier, Style};
use ratatui::widgets::Widget;
use rules::{Pos, SIZE};

use super::layout::{BOARD_COMPACT_W, BOARD_H, BOARD_W};

/// Renju star points: orientation anchors on an otherwise uniform grid
pub const STARS: [(u8, u8); 5] = [(3, 3), (11, 3), (7, 7), (3, 11), (11, 11)];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellKind {
    Empty,
    Star,
    Black,
    White,
    Forbidden(ForbiddenKind),
}

impl CellKind {
    pub const fn ch(self) -> char {
        match self {
            Self::Empty => '.',
            Self::Star => '+',
            Self::Black => 'X',
            Self::White => 'O',
            Self::Forbidden(_) => '#',
        }
    }

    fn style(self) -> Style {
        match self {
            Self::Empty | Self::Star => Style::new().fg(TColor::DarkGray),
            Self::Black => Style::new().fg(TColor::White).add_modifier(Modifier::BOLD),
            Self::White => Style::new().fg(TColor::Yellow).add_modifier(Modifier::BOLD),
            Self::Forbidden(_) => Style::new().fg(TColor::Red),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellView {
    pub kind: CellKind,
    pub last: bool,
    pub winning: bool,
}

const EMPTY: CellView = CellView {
    kind: CellKind::Empty,
    last: false,
    winning: false,
};

type Grid = [[CellView; SIZE as usize]; SIZE as usize];

pub fn grid(snapshot: Option<&RoomSnapshot>) -> Grid {
    let mut g = [[EMPTY; SIZE as usize]; SIZE as usize];
    for (x, y) in STARS {
        g[y as usize][x as usize].kind = CellKind::Star;
    }
    let Some(snap) = snapshot else { return g };
    for (i, ch) in snap.board.chars().enumerate() {
        let Some(p) = Pos::from_index(i) else { break };
        let cell = &mut g[p.y() as usize][p.x() as usize];
        match ch {
            'X' => cell.kind = CellKind::Black,
            'O' => cell.kind = CellKind::White,
            _ => {}
        }
    }
    if snap.status == RoomStatus::Playing && snap.turn == Some(Color::Black) {
        for f in &snap.forbidden {
            if let Some(p) = Pos::new(f.x, f.y) {
                let cell = &mut g[p.y() as usize][p.x() as usize];
                if matches!(cell.kind, CellKind::Empty | CellKind::Star) {
                    cell.kind = CellKind::Forbidden(f.kind);
                }
            }
        }
    }
    if let Some(last) = snap.last_move.and_then(proto::Coord::to_pos) {
        g[last.y() as usize][last.x() as usize].last = true;
    }
    if let Some(result) = &snap.result {
        for c in &result.line {
            if let Some(p) = c.to_pos() {
                g[p.y() as usize][p.x() as usize].winning = true;
            }
        }
    }
    g
}

/// Plain-text rows for `--plain` mode; last move in `[ ]`
pub fn text_rows(snapshot: Option<&RoomSnapshot>, compact: bool) -> Vec<String> {
    let g = grid(snapshot);
    let mut rows = Vec::with_capacity(BOARD_H as usize);
    let mut header = String::from("   ");
    for x in 0..SIZE {
        header.push((b'A' + x) as char);
        if !compact && x + 1 < SIZE {
            header.push(' ');
        }
    }
    rows.push(header);
    for (y, cells) in g.iter().enumerate() {
        let mut row = format!("{:>2} ", y + 1);
        for (x, cell) in cells.iter().copied().enumerate() {
            let ch = cell.kind.ch();
            if compact {
                row.push(ch);
            } else if cell.last {
                if x > 0 {
                    row.pop();
                    row.push('[');
                }
                row.push(ch);
                if x + 1 < SIZE as usize {
                    row.push(']');
                }
            } else {
                row.push(ch);
                if x + 1 < SIZE as usize {
                    row.push(' ');
                }
            }
        }
        rows.push(row);
    }
    rows
}

const LABEL: Style = Style::new().fg(TColor::DarkGray);
const LABEL_HOT: Style = Style::new().fg(TColor::Cyan).add_modifier(Modifier::BOLD);
const BAND: Style = Style::new().fg(TColor::Cyan);
const WIN: Style = Style::new().fg(TColor::Green).add_modifier(Modifier::BOLD);

/// Fixed-size board centred in its area, with an optional crosshair
#[derive(Debug)]
pub struct BoardWidget<'a> {
    pub snapshot: Option<&'a RoomSnapshot>,
    pub compact: bool,
    pub cursor: Option<Pos>,
    /// Column highlight while only a letter typed
    pub column: Option<u8>,
}

impl BoardWidget<'_> {
    fn col_x(&self, x0: u16, x: u8) -> u16 {
        let step = if self.compact { 1 } else { 2 };
        x0 + 3 + u16::from(x) * step
    }

    fn hot_col(&self, x: u8) -> bool {
        self.column == Some(x) || self.cursor.is_some_and(|c| c.x() == x)
    }

    fn hot_row(&self, y: u8) -> bool {
        self.cursor.is_some_and(|c| c.y() == y)
    }
}

impl Widget for BoardWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let w = if self.compact {
            BOARD_COMPACT_W
        } else {
            BOARD_W
        };
        let x0 = area.x + area.width.saturating_sub(w) / 2;
        let y0 = area.y + area.height.saturating_sub(BOARD_H) / 2;
        let g = grid(self.snapshot);

        if y0 < area.bottom() {
            for x in 0..SIZE {
                let col = self.col_x(x0, x);
                if col < area.right() {
                    let style = if self.hot_col(x) { LABEL_HOT } else { LABEL };
                    buf.set_string(col, y0, ((b'A' + x) as char).to_string(), style);
                }
            }
        }
        for y in 0..SIZE {
            let row = y0 + 1 + u16::from(y);
            if row >= area.bottom() {
                break;
            }
            if x0 + 2 < area.right() {
                let style = if self.hot_row(y) { LABEL_HOT } else { LABEL };
                buf.set_string(x0, row, format!("{:>2}", y + 1), style);
            }
            for x in 0..SIZE {
                let col = self.col_x(x0, x);
                if col >= area.right() {
                    break;
                }
                let cell = g[y as usize][x as usize];
                let mut style = cell.kind.style();
                if matches!(cell.kind, CellKind::Empty | CellKind::Star)
                    && (self.hot_col(x) || self.hot_row(y))
                {
                    style = style.patch(BAND);
                }
                if cell.winning {
                    style = style.patch(WIN);
                }
                if cell.last {
                    style = style.add_modifier(Modifier::REVERSED | Modifier::BOLD);
                }
                if self.cursor.is_some_and(|c| c.x() == x && c.y() == y) {
                    style = style.bg(TColor::Cyan).fg(TColor::Black);
                }
                buf.set_string(col, row, cell.kind.ch().to_string(), style);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_rows_have_expected_widths() {
        let rows = text_rows(None, false);
        assert_eq!(rows.len(), 16);
        assert!(
            rows.iter().all(|r| r.chars().count() == BOARD_W as usize),
            "{rows:?}"
        );
        assert!(rows[8].contains('+'), "centre star point: {}", rows[8]);
        let rows = text_rows(None, true);
        assert!(
            rows.iter()
                .all(|r| r.chars().count() == BOARD_COMPACT_W as usize),
            "{rows:?}"
        );
    }
}
