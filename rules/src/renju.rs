//! Black's forbidden moves. Per-direction 11-cell window, so directions never
//! interfere; fours/threes keyed by stone-set mask (`_XXXX_` = one four,
//! `XXX_X_XXX` centred = two). Precedence: five (legal) > overline > 4-4 > 3-3
//! Three = a *legal* straight-four move exists: recursive, bounded by `MAX_DEPTH`

use crate::{Board, Pos, Stone};

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub enum Forbidden {
    Overline,
    DoubleFour,
    DoubleThree,
}

impl Forbidden {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Overline => "overline",
            Self::DoubleFour => "4-4",
            Self::DoubleThree => "3-3",
        }
    }
}

impl core::fmt::Display for Forbidden {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.label())
    }
}

/// Opposite directions covered by scanning both ways
const DIRS: [(i8, i8); 4] = [(1, 0), (0, 1), (1, 1), (1, -1)];

/// Centre +-5: enough to tell 5 from 6+
const WIN: usize = 11;
const CENTER: usize = 5;
/// Bound for the recursive three rule
const MAX_DEPTH: u8 = 6;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Cell {
    Wall,
    Empty,
    Black,
    White,
}

type Line = [Cell; WIN];

/// Centre = hypothetical black stone
fn extract_line(board: &Board, pos: Pos, dir: (i8, i8)) -> Line {
    let mut line = [Cell::Wall; WIN];
    for (i, cell) in line.iter_mut().enumerate() {
        let k = i as i8 - CENTER as i8;
        let x = pos.x() as i8 + dir.0 * k;
        let y = pos.y() as i8 + dir.1 * k;
        *cell = match board.get_i8(x, y) {
            None => Cell::Wall,
            Some(None) => Cell::Empty,
            Some(Some(Stone::Black)) => Cell::Black,
            Some(Some(Stone::White)) => Cell::White,
        };
    }
    line[CENTER] = Cell::Black;
    line
}

/// Inclusive; `i` black by precondition
fn black_run(line: &Line, i: usize) -> (usize, usize) {
    let mut s = i;
    while s > 0 && line[s - 1] == Cell::Black {
        s -= 1;
    }
    let mut e = i;
    while e + 1 < WIN && line[e + 1] == Cell::Black {
        e += 1;
    }
    (s, e)
}

/// `completions == 2` means a straight four
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Four {
    stones: u16,
    completions: u8,
}

/// Four = four black stones + one empty cell giving an *exact* five
/// Completion cells 1..=9 only; a five through the centre reaches no further
fn fours_in_line(line: &Line, required: u16) -> Vec<Four> {
    let mut fours: Vec<Four> = Vec::new();
    for i in 1..WIN - 1 {
        if line[i] != Cell::Empty {
            continue;
        }
        let mut l = *line;
        l[i] = Cell::Black;
        let (s, e) = black_run(&l, i);
        if e - s + 1 != 5 {
            continue;
        }
        let mut stones: u16 = 0;
        for k in s..=e {
            if k != i {
                stones |= 1 << k;
            }
        }
        if stones & required != required {
            continue;
        }
        if let Some(f) = fours.iter_mut().find(|f| f.stones == stones) {
            f.completions += 1;
        } else {
            fours.push(Four {
                stones,
                completions: 1,
            });
        }
    }
    fours
}

/// `with_pos` includes the candidate stone, needed for the legality recursion
fn count_threes(line: &Line, with_pos: &Board, pos: Pos, dir: (i8, i8), depth: u8) -> usize {
    let mut counted: Vec<u16> = Vec::new();
    for i in 1..WIN - 1 {
        if line[i] != Cell::Empty {
            continue;
        }
        let mut l = *line;
        l[i] = Cell::Black;
        let required = (1u16 << CENTER) | (1u16 << i);
        for four in fours_in_line(&l, required) {
            if four.completions != 2 {
                continue; // not a straight four
            }
            let three = four.stones & !(1u16 << i);
            if counted.contains(&three) {
                continue;
            }
            let step = i as i8 - CENTER as i8;
            let p = pos
                .step(dir.0, dir.1, step)
                .expect("an Empty cell is on the board");
            if forbidden_inner(with_pos, p, depth + 1).is_none() {
                counted.push(three);
            }
        }
    }
    counted.len()
}

fn forbidden_inner(board: &Board, pos: Pos, depth: u8) -> Option<Forbidden> {
    let lines: [Line; 4] = DIRS.map(|d| extract_line(board, pos, d));

    // 1. five: legal even when it also forms 4-4 / 3-3
    let mut overline = false;
    for line in &lines {
        let (s, e) = black_run(line, CENTER);
        match e - s + 1 {
            5 => return None,
            n if n >= 6 => overline = true,
            _ => {}
        }
    }
    if overline {
        return Some(Forbidden::Overline);
    }

    // 2. fours; a straight four counts once
    let fours: usize = lines
        .iter()
        .map(|l| fours_in_line(l, 1 << CENTER).len())
        .sum();
    if fours >= 2 {
        return Some(Forbidden::DoubleFour);
    }

    // 3. threes; past MAX_DEPTH assume legal
    if depth >= MAX_DEPTH {
        return None;
    }
    let mut with_pos = *board;
    with_pos.set(pos, Stone::Black);
    let mut threes = 0usize;
    for (dir, line) in DIRS.iter().zip(&lines) {
        threes += count_threes(line, &with_pos, pos, *dir, depth);
        if threes >= 2 {
            return Some(Forbidden::DoubleThree);
        }
    }
    None
}

/// Occupied points never reported; occupancy = caller's check
pub fn forbidden_reason(board: &Board, pos: Pos) -> Option<Forbidden> {
    if !board.is_empty(pos) {
        return None;
    }
    forbidden_inner(board, pos, 0)
}

pub fn forbidden_points(board: &Board) -> Vec<(Pos, Forbidden)> {
    board
        .iter()
        .filter(|(_, c)| c.is_none())
        .filter_map(|(p, _)| forbidden_inner(board, p, 0).map(|f| (p, f)))
        .collect()
}

/// Black: exactly five, White: five or more; returns winning stones sorted
pub fn check_win(board: &Board, pos: Pos) -> Option<Vec<Pos>> {
    let stone = board.get(pos)?;
    for (dx, dy) in DIRS {
        let mut cells = vec![pos];
        for sign in [-1i8, 1] {
            let mut k = 1i8;
            while let Some(p) = pos.step(dx, dy, sign * k) {
                if board.get(p) == Some(stone) {
                    cells.push(p);
                    k += 1;
                } else {
                    break;
                }
            }
        }
        let n = cells.len();
        let won = match stone {
            Stone::Black => n == 5,
            Stone::White => n >= 5,
        };
        if won {
            cells.sort();
            return Some(cells);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Pos {
        Pos::from_notation(s).unwrap()
    }

    fn board(black: &[&str], white: &[&str]) -> Board {
        let mut b = Board::new();
        for s in black {
            b.set(p(s), Stone::Black);
        }
        for s in white {
            b.set(p(s), Stone::White);
        }
        b
    }

    #[test]
    fn empty_board_has_no_forbidden_points() {
        assert!(forbidden_points(&Board::new()).is_empty());
    }

    #[test]
    fn exact_five_wins_for_black() {
        let mut b = board(&["D8", "E8", "F8", "G8"], &[]);
        assert_eq!(forbidden_reason(&b, p("H8")), None);
        b.set(p("H8"), Stone::Black);
        let line = check_win(&b, p("H8")).unwrap();
        assert_eq!(line, vec![p("D8"), p("E8"), p("F8"), p("G8"), p("H8")]);
    }

    #[test]
    fn overline_is_forbidden_and_not_a_win_for_black() {
        let mut b = board(&["D8", "E8", "F8", "G8", "I8"], &[]);
        assert_eq!(forbidden_reason(&b, p("H8")), Some(Forbidden::Overline));
        b.set(p("H8"), Stone::Black);
        assert_eq!(check_win(&b, p("H8")), None);
    }

    #[test]
    fn overline_wins_for_white() {
        let mut b = board(&[], &["D8", "E8", "F8", "G8", "I8"]);
        b.set(p("H8"), Stone::White);
        assert_eq!(check_win(&b, p("H8")).unwrap().len(), 6);
    }

    #[test]
    fn vertical_and_diagonal_fives() {
        let mut b = board(&["H4", "H5", "H6", "H7"], &[]);
        b.set(p("H8"), Stone::Black);
        assert!(check_win(&b, p("H8")).is_some());

        let mut b = board(&["D4", "E5", "F6", "G7"], &[]);
        b.set(p("H8"), Stone::Black);
        assert!(check_win(&b, p("H8")).is_some());

        let mut b = board(&["L4", "K5", "J6", "I7"], &[]);
        b.set(p("H8"), Stone::Black);
        assert!(check_win(&b, p("H8")).is_some());
    }

    #[test]
    fn double_four_crossing() {
        // E8 F8 G8 [H8] + H5 H6 H7 [H8]
        let b = board(&["E8", "F8", "G8", "H5", "H6", "H7"], &[]);
        assert_eq!(forbidden_reason(&b, p("H8")), Some(Forbidden::DoubleFour));
    }

    #[test]
    fn double_four_in_one_line() {
        // B8 C8 D8 _ [F8] _ H8 I8 J8
        let b = board(&["B8", "C8", "D8", "H8", "I8", "J8"], &[]);
        assert_eq!(forbidden_reason(&b, p("F8")), Some(Forbidden::DoubleFour));
    }

    #[test]
    fn pre_existing_four_does_not_count() {
        // D8..G8 pre-existing; L8 = one new four only
        let b = board(&["D8", "E8", "F8", "G8", "I8", "J8", "K8"], &[]);
        assert_eq!(forbidden_reason(&b, p("L8")), None);
    }

    #[test]
    fn straight_four_counts_once() {
        // _ E8 F8 G8 [H8] _
        let b = board(&["E8", "F8", "G8"], &[]);
        assert_eq!(forbidden_reason(&b, p("H8")), None);
    }

    #[test]
    fn double_three_crossing() {
        let b = board(&["F8", "G8", "H6", "H7"], &[]);
        assert_eq!(forbidden_reason(&b, p("H8")), Some(Forbidden::DoubleThree));
    }

    #[test]
    fn double_three_with_gaps() {
        // E8 _ G8 [H8] and H5 _ H7 [H8]: broken threes, both open
        let b = board(&["E8", "G8", "H5", "H7"], &[]);
        assert_eq!(forbidden_reason(&b, p("H8")), Some(Forbidden::DoubleThree));
    }

    #[test]
    fn four_three_is_legal() {
        let b = board(&["E8", "F8", "G8", "H6", "H7"], &[]);
        assert_eq!(forbidden_reason(&b, p("H8")), None);
    }

    #[test]
    fn five_beats_double_four() {
        let b = board(&["D8", "E8", "F8", "G8", "H5", "H6", "H7"], &[]);
        assert_eq!(forbidden_reason(&b, p("H8")), None);
    }

    #[test]
    fn five_beats_double_three() {
        let b = board(&["D8", "E8", "F8", "G8", "H6", "H7", "G7", "F6"], &[]);
        assert_eq!(forbidden_reason(&b, p("H8")), None);
    }

    #[test]
    fn three_blocked_by_white_is_not_open() {
        // D8 white blocks the horizontal three
        let b = board(&["F8", "G8", "E6", "E7"], &["D8"]);
        assert_eq!(forbidden_reason(&b, p("E8")), None);
    }

    #[test]
    fn three_blocked_by_edge_is_not_open() {
        // A8 B8 [C8]: wall on the left, never a straight four
        let b = board(&["A8", "B8", "C6", "C7"], &[]);
        assert_eq!(forbidden_reason(&b, p("C8")), None);
    }

    #[test]
    fn three_whose_straight_four_points_are_forbidden_does_not_count() {
        // F8 G8 [H8]: both straight-four points E8/I8 = 4-4 for Black -> not a three
        // H6 H7 [H8] = real three -> one three, legal
        let b = board(
            &["F8", "G8", "E5", "E6", "E7", "I5", "I6", "I7", "H6", "H7"],
            &[],
        );
        assert_eq!(forbidden_reason(&b, p("H8")), None);

        // control: without E5, E8 = legal 4-3 -> horizontal three counts -> 3-3
        let b = board(&["F8", "G8", "E6", "E7", "I5", "I6", "I7", "H6", "H7"], &[]);
        assert_eq!(forbidden_reason(&b, p("H8")), Some(Forbidden::DoubleThree));
    }

    #[test]
    fn forbidden_points_matches_pointwise_check() {
        let b = board(
            &[
                "F8", "G8", "H6", "H7", "B2", "C2", "D2", "J12", "K12", "L12", "C3", "D3", "E3",
                "F3",
            ],
            &["A1", "M8"],
        );
        let pts = forbidden_points(&b);
        assert!(pts.contains(&(p("H8"), Forbidden::DoubleThree)));
        for (pos, reason) in &pts {
            assert_eq!(forbidden_reason(&b, *pos), Some(*reason), "{pos}");
        }
        for (pos, c) in b.iter() {
            if c.is_none() && !pts.iter().any(|(q, _)| *q == pos) {
                assert_eq!(forbidden_reason(&b, pos), None, "{pos}");
            }
        }
    }

    /// Judgement must be invariant under the 8 board symmetries: catches any
    /// direction- or edge-specific bug the hand-written cases miss
    #[test]
    fn forbidden_points_are_symmetric_on_random_boards() {
        type Sym = fn(usize, usize, usize) -> (usize, usize);
        let n = crate::SIZE as usize;
        let m = n - 1;
        let syms: [Sym; 8] = [
            |x, y, _| (x, y),
            |x, y, m| (m - x, y),
            |x, y, m| (x, m - y),
            |x, y, m| (m - x, m - y),
            |x, y, _| (y, x),
            |x, y, m| (m - y, x),
            |x, y, m| (y, m - x),
            |x, y, m| (m - y, m - x),
        ];
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move || {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (seed >> 33) as usize
        };
        for _ in 0..150 {
            let mut base = Board::new();
            let stones = 6 + next() % 40;
            for k in 0..stones {
                let q = Pos::from_index(next() % (n * n)).unwrap();
                if base.is_empty(q) {
                    let c = if k % 3 == 2 {
                        Stone::White
                    } else {
                        Stone::Black
                    };
                    base.set(q, c);
                }
            }
            let mut expected: Vec<(Pos, Forbidden)> = forbidden_points(&base);
            expected.sort();
            for sym in &syms {
                let map = |q: Pos| {
                    let (x, y) = sym(q.x() as usize, q.y() as usize, m);
                    Pos::new(x as u8, y as u8).unwrap()
                };
                let inverse = |q: Pos| {
                    (0..n * n)
                        .map(|i| Pos::from_index(i).unwrap())
                        .find(|c| map(*c) == q)
                        .unwrap()
                };
                let mut b = Board::new();
                for (q, c) in base.iter() {
                    if let Some(c) = c {
                        b.set(map(q), c);
                    }
                }
                let mut got: Vec<(Pos, Forbidden)> = forbidden_points(&b)
                    .into_iter()
                    .map(|(q, f)| (inverse(q), f))
                    .collect();
                got.sort();
                assert_eq!(got, expected, "symmetry mismatch\n{base:?}");
            }
        }
    }

    #[test]
    fn occupied_point_is_never_reported() {
        let b = board(&["H8"], &[]);
        assert_eq!(forbidden_reason(&b, p("H8")), None);
    }
}
