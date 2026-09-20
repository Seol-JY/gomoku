use core::fmt;

use crate::{CELLS, Pos, SIZE};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Stone {
    Black,
    White,
}

impl Stone {
    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::Black => Self::White,
            Self::White => Self::Black,
        }
    }

    pub const fn ascii(self) -> char {
        match self {
            Self::Black => 'X',
            Self::White => 'O',
        }
    }
}

impl fmt::Display for Stone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Black => "Black",
            Self::White => "White",
        })
    }
}

/// Copy (225 bytes): cheaper than borrowing through the three-recursion
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Board {
    cells: [Option<Stone>; CELLS],
}

impl Default for Board {
    fn default() -> Self {
        Self::new()
    }
}

impl Board {
    pub const fn new() -> Self {
        Self {
            cells: [None; CELLS],
        }
    }

    pub const fn get(&self, pos: Pos) -> Option<Stone> {
        self.cells[pos.index()]
    }

    #[allow(clippy::option_option)]
    pub(crate) const fn get_i8(&self, x: i8, y: i8) -> Option<Option<Stone>> {
        match Pos::from_i8(x, y) {
            Some(p) => Some(self.cells[p.index()]),
            None => None,
        }
    }

    pub const fn is_empty(&self, pos: Pos) -> bool {
        self.cells[pos.index()].is_none()
    }

    pub const fn set(&mut self, pos: Pos, stone: Stone) {
        self.cells[pos.index()] = Some(stone);
    }

    pub const fn clear(&mut self, pos: Pos) {
        self.cells[pos.index()] = None;
    }

    pub fn count(&self) -> usize {
        self.cells.iter().filter(|c| c.is_some()).count()
    }

    pub fn is_full(&self) -> bool {
        self.cells.iter().all(Option::is_some)
    }

    pub fn iter(&self) -> impl Iterator<Item = (Pos, Option<Stone>)> + '_ {
        self.cells
            .iter()
            .enumerate()
            .map(|(i, c)| (Pos::from_index(i).expect("index in range"), *c))
    }

    pub const fn cells(&self) -> &[Option<Stone>; CELLS] {
        &self.cells
    }

    /// 225 chars of `.`/`X`/`O`, row-major, row 1 first
    pub fn to_compact(&self) -> String {
        self.cells
            .iter()
            .map(|c| match c {
                None => '.',
                Some(s) => s.ascii(),
            })
            .collect()
    }

    /// Whitespace ignored; accepts `.`/`_`/`+`, `X`/`x`/`●`, `O`/`o`/`○`
    pub fn from_compact(s: &str) -> Result<Self, String> {
        let mut board = Self::new();
        let mut i = 0usize;
        for ch in s.chars() {
            if ch.is_whitespace() {
                continue;
            }
            if i >= CELLS {
                return Err(format!("too many cells (more than {CELLS})"));
            }
            let stone = match ch {
                '.' | '_' | '+' => None,
                'X' | 'x' | '●' => Some(Stone::Black),
                'O' | 'o' | '○' => Some(Stone::White),
                other => return Err(format!("unexpected character {other:?} at cell {i}")),
            };
            board.cells[i] = stone;
            i += 1;
        }
        if i != CELLS {
            return Err(format!("expected {CELLS} cells, got {i}"));
        }
        Ok(board)
    }

    /// Test helper; short or missing rows = empty
    pub fn from_diagram(s: &str) -> Result<Self, String> {
        let mut board = Self::new();
        let rows: Vec<&str> = s.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        if rows.len() > SIZE as usize {
            return Err(format!("too many rows: {}", rows.len()));
        }
        for (y, row) in rows.iter().enumerate() {
            let mut x = 0usize;
            for ch in row.chars() {
                if ch.is_whitespace() {
                    continue;
                }
                if x >= SIZE as usize {
                    return Err(format!("row {} too long", y + 1));
                }
                let stone = match ch {
                    '.' | '_' | '+' => None,
                    'X' | 'x' | '●' => Some(Stone::Black),
                    'O' | 'o' | '○' => Some(Stone::White),
                    other => return Err(format!("unexpected character {other:?}")),
                };
                board.cells[y * SIZE as usize + x] = stone;
                x += 1;
            }
        }
        Ok(board)
    }
}

impl fmt::Debug for Board {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Board {{")?;
        for y in 0..SIZE {
            write!(f, "  {:>2} ", y + 1)?;
            for x in 0..SIZE {
                let p = Pos::new(x, y).expect("in range");
                let c = match self.get(p) {
                    None => '.',
                    Some(s) => s.ascii(),
                };
                write!(f, "{c} ")?;
            }
            writeln!(f)?;
        }
        write!(f, "     ")?;
        for x in 0..SIZE {
            write!(f, "{} ", (b'A' + x) as char)?;
        }
        writeln!(f)?;
        write!(f, "}}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_roundtrip() {
        let mut b = Board::new();
        b.set(Pos::new(7, 7).unwrap(), Stone::Black);
        b.set(Pos::new(0, 14).unwrap(), Stone::White);
        let s = b.to_compact();
        assert_eq!(s.len(), CELLS);
        assert_eq!(Board::from_compact(&s).unwrap(), b);
    }

    #[test]
    fn diagram_short_rows() {
        let b = Board::from_diagram(
            "
            X
            .O
            ",
        )
        .unwrap();
        assert_eq!(b.get(Pos::new(0, 0).unwrap()), Some(Stone::Black));
        assert_eq!(b.get(Pos::new(1, 1).unwrap()), Some(Stone::White));
        assert_eq!(b.count(), 2);
    }
}
