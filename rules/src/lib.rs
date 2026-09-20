//! Renju rules shared by server and client. No dependencies: both sides embed
//! identical logic; `x` = column A..O, `y` = row 1..15, both 0-based, `H8` = (7, 7)

#![forbid(unsafe_code)]

mod board;
mod game;
mod pos;
mod renju;

pub use board::{Board, Stone};
pub use game::{Game, GameStatus, MoveError};
pub use pos::{Pos, PosParseError};
pub use renju::{Forbidden, check_win, forbidden_points, forbidden_reason};

pub const SIZE: u8 = 15;
pub const CELLS: usize = (SIZE as usize) * (SIZE as usize);
