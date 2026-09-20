use core::fmt;

use crate::SIZE;

/// Always in bounds by construction
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct Pos {
    x: u8,
    y: u8,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PosParseError {
    Format,
    Column,
    Row,
}

impl fmt::Display for PosParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Format => f.write_str("expected a coordinate like H8"),
            Self::Column => f.write_str("column must be A..O"),
            Self::Row => f.write_str("row must be 1..15"),
        }
    }
}

impl core::error::Error for PosParseError {}

impl Pos {
    pub const fn new(x: u8, y: u8) -> Option<Self> {
        if x < SIZE && y < SIZE {
            Some(Self { x, y })
        } else {
            None
        }
    }

    pub const fn from_i8(x: i8, y: i8) -> Option<Self> {
        if x >= 0 && y >= 0 && (x as u8) < SIZE && (y as u8) < SIZE {
            Some(Self {
                x: x as u8,
                y: y as u8,
            })
        } else {
            None
        }
    }

    pub const fn x(self) -> u8 {
        self.x
    }

    pub const fn y(self) -> u8 {
        self.y
    }

    pub const fn index(self) -> usize {
        self.y as usize * SIZE as usize + self.x as usize
    }

    pub const fn from_index(i: usize) -> Option<Self> {
        if i < (SIZE as usize) * (SIZE as usize) {
            Some(Self {
                x: (i % SIZE as usize) as u8,
                y: (i / SIZE as usize) as u8,
            })
        } else {
            None
        }
    }

    pub const fn column_letter(self) -> char {
        (b'A' + self.x) as char
    }

    pub const fn row_number(self) -> u8 {
        self.y + 1
    }

    /// Grammar: `^\s*([A-Oa-o])\s*(1[0-5]|[1-9])\s*$`; `h8`, `H8`, `h 8` all accepted
    pub fn from_notation(s: &str) -> Result<Self, PosParseError> {
        let s = s.trim();
        let mut chars = s.chars();
        let col = chars.next().ok_or(PosParseError::Format)?;
        if !col.is_ascii_alphabetic() {
            return Err(PosParseError::Format);
        }
        let rest = chars.as_str().trim_start();
        if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
            return Err(PosParseError::Format);
        }
        let x = col.to_ascii_uppercase() as u8 - b'A';
        if x >= SIZE {
            return Err(PosParseError::Column);
        }
        let row: u32 = rest.parse().map_err(|_| PosParseError::Row)?;
        if !(1..=u32::from(SIZE)).contains(&row) {
            return Err(PosParseError::Row);
        }
        Ok(Self {
            x,
            y: (row - 1) as u8,
        })
    }

    /// Move-vs-chat classifier: letter + 1-2 digits, bounds not checked
    pub fn looks_like_notation(s: &str) -> bool {
        let s = s.trim();
        let mut chars = s.chars();
        let Some(first) = chars.next() else {
            return false;
        };
        let rest = chars.as_str().trim_start();
        first.is_ascii_alphabetic()
            && !rest.is_empty()
            && rest.len() <= 2
            && rest.bytes().all(|b| b.is_ascii_digit())
    }

    pub fn step(self, dx: i8, dy: i8, n: i8) -> Option<Self> {
        Self::from_i8(self.x as i8 + dx * n, self.y as i8 + dy * n)
    }
}

impl fmt::Display for Pos {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.column_letter(), self.row_number())
    }
}

impl core::str::FromStr for Pos {
    type Err = PosParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_notation(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_forms() {
        for s in ["h8", "H8", "h 8", "  H8  ", "h  8"] {
            assert_eq!(Pos::from_notation(s), Ok(Pos::new(7, 7).unwrap()), "{s:?}");
        }
        assert_eq!(Pos::from_notation("a1"), Ok(Pos::new(0, 0).unwrap()));
        assert_eq!(Pos::from_notation("O15"), Ok(Pos::new(14, 14).unwrap()));
    }

    #[test]
    fn rejects_bad_forms() {
        assert_eq!(Pos::from_notation("p8"), Err(PosParseError::Column));
        assert_eq!(Pos::from_notation("h16"), Err(PosParseError::Row));
        assert_eq!(Pos::from_notation("h0"), Err(PosParseError::Row));
        assert_eq!(Pos::from_notation("hello"), Err(PosParseError::Format));
        assert_eq!(Pos::from_notation("8h"), Err(PosParseError::Format));
        assert_eq!(Pos::from_notation(""), Err(PosParseError::Format));
        assert_eq!(Pos::from_notation("h"), Err(PosParseError::Format));
    }

    #[test]
    fn display_roundtrip() {
        for i in 0..crate::CELLS {
            let p = Pos::from_index(i).unwrap();
            assert_eq!(Pos::from_notation(&p.to_string()), Ok(p));
            assert_eq!(p.index(), i);
        }
    }

    #[test]
    fn looks_like_notation_distinguishes_chat() {
        assert!(Pos::looks_like_notation("h8"));
        assert!(Pos::looks_like_notation("Z99"));
        assert!(!Pos::looks_like_notation("hi there"));
        assert!(!Pos::looks_like_notation("gg"));
        assert!(!Pos::looks_like_notation("h123"));
    }
}
