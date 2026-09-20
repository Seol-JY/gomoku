use crate::{Board, Forbidden, Pos, Stone, check_win, forbidden_points, forbidden_reason};

/// Board-derived only, resignation = server concern
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum GameStatus {
    Playing,
    Won { winner: Stone, line: Vec<Pos> },
    Draw,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MoveError {
    Occupied,
    GameOver,
    Forbidden(Forbidden),
}

impl core::fmt::Display for MoveError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Occupied => f.write_str("that point is occupied"),
            Self::GameOver => f.write_str("the game is over"),
            Self::Forbidden(kind) => write!(f, "forbidden for Black ({kind})"),
        }
    }
}

impl core::error::Error for MoveError {}

/// Move list = source of truth, board = cache
#[derive(Clone, Debug)]
pub struct Game {
    board: Board,
    moves: Vec<Pos>,
    status: GameStatus,
}

impl Default for Game {
    fn default() -> Self {
        Self::new()
    }
}

impl Game {
    pub const fn new() -> Self {
        Self {
            board: Board::new(),
            moves: Vec::new(),
            status: GameStatus::Playing,
        }
    }

    /// Err = index of the offending move
    pub fn from_moves<I: IntoIterator<Item = Pos>>(moves: I) -> Result<Self, (usize, MoveError)> {
        let mut game = Self::new();
        for (i, m) in moves.into_iter().enumerate() {
            game.play(m).map_err(|e| (i, e))?;
        }
        Ok(game)
    }

    /// Derived from move count => no desync on undo
    pub fn turn(&self) -> Stone {
        if self.moves.len().is_multiple_of(2) {
            Stone::Black
        } else {
            Stone::White
        }
    }

    pub const fn board(&self) -> &Board {
        &self.board
    }

    pub fn moves(&self) -> &[Pos] {
        &self.moves
    }

    pub const fn status(&self) -> &GameStatus {
        &self.status
    }

    pub fn is_over(&self) -> bool {
        self.status != GameStatus::Playing
    }

    pub fn last_move(&self) -> Option<Pos> {
        self.moves.last().copied()
    }

    pub fn validate(&self, pos: Pos) -> Result<(), MoveError> {
        if self.is_over() {
            return Err(MoveError::GameOver);
        }
        if !self.board.is_empty(pos) {
            return Err(MoveError::Occupied);
        }
        if self.turn() == Stone::Black
            && let Some(kind) = forbidden_reason(&self.board, pos)
        {
            return Err(MoveError::Forbidden(kind));
        }
        Ok(())
    }

    pub fn play(&mut self, pos: Pos) -> Result<&GameStatus, MoveError> {
        self.validate(pos)?;
        let stone = self.turn();
        self.board.set(pos, stone);
        self.moves.push(pos);
        if let Some(line) = check_win(&self.board, pos) {
            self.status = GameStatus::Won {
                winner: stone,
                line,
            };
        } else if self.board.is_full() {
            self.status = GameStatus::Draw;
        }
        Ok(&self.status)
    }

    pub fn undo(&mut self) -> Option<Pos> {
        let pos = self.moves.pop()?;
        self.board.clear(pos);
        self.status = GameStatus::Playing;
        Some(pos)
    }

    /// Empty unless Black to move in a live game
    pub fn forbidden_points(&self) -> Vec<(Pos, Forbidden)> {
        if self.is_over() || self.turn() != Stone::Black {
            return Vec::new();
        }
        forbidden_points(&self.board)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Pos {
        Pos::from_notation(s).unwrap()
    }

    #[test]
    fn alternates_and_detects_win() {
        let mut g = Game::new();
        assert_eq!(g.turn(), Stone::Black);
        for (i, m) in ["H8", "A1", "I8", "A2", "J8", "A3", "K8", "A4"]
            .iter()
            .enumerate()
        {
            let expected = if i % 2 == 0 {
                Stone::Black
            } else {
                Stone::White
            };
            assert_eq!(g.turn(), expected);
            assert_eq!(g.play(p(m)).unwrap(), &GameStatus::Playing);
        }
        assert_eq!(g.play(p("H8")), Err(MoveError::Occupied));
        match g.play(p("L8")).unwrap() {
            GameStatus::Won {
                winner: Stone::Black,
                line,
            } => assert_eq!(line.len(), 5),
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(g.play(p("B1")), Err(MoveError::GameOver));
        assert!(g.forbidden_points().is_empty());
    }

    #[test]
    fn rejects_forbidden_for_black_only() {
        // two open twos for Black; White far away
        let mut g = Game::new();
        for m in ["F8", "A1", "G8", "A2", "H6", "A3", "H7", "A4"] {
            g.play(p(m)).unwrap();
        }
        assert_eq!(g.turn(), Stone::Black);
        assert_eq!(
            g.play(p("H8")),
            Err(MoveError::Forbidden(Forbidden::DoubleThree))
        );
        assert!(g.forbidden_points().iter().any(|(q, _)| *q == p("H8")));

        // same shape legal for White
        let mut g = Game::new();
        for m in ["A1", "F8", "B1", "G8", "C1", "H6", "D1", "H7", "F1"] {
            g.play(p(m)).unwrap();
        }
        assert_eq!(g.turn(), Stone::White);
        assert!(g.play(p("H8")).is_ok());
    }

    #[test]
    fn undo_restores_turn_and_status() {
        let mut g = Game::new();
        for m in ["H8", "A1", "I8", "A2", "J8", "A3", "K8", "A4", "L8"] {
            g.play(p(m)).unwrap();
        }
        assert!(g.is_over());
        assert_eq!(g.undo(), Some(p("L8")));
        assert!(!g.is_over());
        assert_eq!(g.turn(), Stone::Black);
        assert!(g.board().is_empty(p("L8")));
        assert_eq!(g.moves().len(), 8);
    }

    #[test]
    fn from_moves_matches_sequential_play() {
        let seq: Vec<Pos> = ["H8", "I9", "G7", "J10", "F6"]
            .iter()
            .map(|s| p(s))
            .collect();
        let mut a = Game::new();
        for m in &seq {
            a.play(*m).unwrap();
        }
        let b = Game::from_moves(seq.iter().copied()).unwrap();
        assert_eq!(a.board(), b.board());
        assert_eq!(a.moves(), b.moves());
        assert_eq!(a.status(), b.status());

        let bad = Game::from_moves([p("H8"), p("H8")]);
        assert_eq!(bad.unwrap_err(), (1, MoveError::Occupied));
    }
}
