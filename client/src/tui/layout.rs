use ratatui::layout::{Constraint, Direction, Layout, Rect};

/// 2 (row label) + 1 + 15*2-1
pub const BOARD_W: u16 = 32;
/// 2 + 1 + 15
pub const BOARD_COMPACT_W: u16 = 18;
pub const BOARD_H: u16 = 16;
pub const CHAT_MIN_W: u16 = 30;
const GAP: u16 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Split { compact: bool },
    Stacked { compact: bool },
    TooSmall,
}

impl Mode {
    /// Width 64+ split, 34..63 stacked, 20..33 compact stacked, under 20 too small
    /// Under 22 rows a stacked layout leaves no chat, so split when width allows
    pub fn choose(width: u16, height: u16) -> Self {
        if width < 20 {
            return Self::TooSmall;
        }
        if width >= BOARD_W + GAP + CHAT_MIN_W {
            return Self::Split { compact: false };
        }
        if height < BOARD_H + 6 && width >= BOARD_COMPACT_W + GAP + CHAT_MIN_W {
            return Self::Split { compact: true };
        }
        if width >= 34 {
            Self::Stacked { compact: false }
        } else {
            Self::Stacked { compact: true }
        }
    }

    pub const fn compact(self) -> bool {
        match self {
            Self::Split { compact } | Self::Stacked { compact } => compact,
            Self::TooSmall => true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Areas {
    pub board: Rect,
    pub chat: Rect,
}

pub fn split(area: Rect, mode: Mode) -> Option<Areas> {
    match mode {
        Mode::TooSmall => None,
        Mode::Split { compact } => {
            let w = if compact { BOARD_COMPACT_W } else { BOARD_W };
            let [board, _, chat] = Layout::new(
                Direction::Horizontal,
                [
                    Constraint::Length(w),
                    Constraint::Length(GAP),
                    Constraint::Min(1),
                ],
            )
            .areas(area);
            Some(Areas { board, chat })
        }
        Mode::Stacked { .. } => {
            let [board, chat] = Layout::new(
                Direction::Vertical,
                [Constraint::Length(BOARD_H), Constraint::Min(1)],
            )
            .areas(area);
            Some(Areas { board, chat })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_thresholds() {
        assert_eq!(Mode::choose(120, 40), Mode::Split { compact: false });
        assert_eq!(Mode::choose(64, 40), Mode::Split { compact: false });
        assert_eq!(Mode::choose(63, 40), Mode::Stacked { compact: false });
        assert_eq!(Mode::choose(34, 40), Mode::Stacked { compact: false });
        assert_eq!(Mode::choose(33, 40), Mode::Stacked { compact: true });
        assert_eq!(Mode::choose(20, 40), Mode::Stacked { compact: true });
        assert_eq!(Mode::choose(19, 40), Mode::TooSmall);
        assert_eq!(Mode::choose(55, 20), Mode::Split { compact: true });
        assert_eq!(Mode::choose(40, 20), Mode::Stacked { compact: false });
    }

    #[test]
    fn split_sizes() {
        let area = Rect::new(0, 0, 100, 30);
        let a = split(area, Mode::Split { compact: false }).unwrap();
        assert_eq!(a.board.width, BOARD_W);
        assert_eq!(a.chat.width, 100 - BOARD_W - GAP);
        let a = split(Rect::new(0, 0, 40, 30), Mode::Stacked { compact: false }).unwrap();
        assert_eq!(a.board.height, BOARD_H);
        assert_eq!(a.chat.height, 30 - BOARD_H);
        assert!(split(area, Mode::TooSmall).is_none());
    }
}
