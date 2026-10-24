//! A position: pieces, side to move, castling rights, en passant square and
//! the two move counters. Moves are generated pseudo-legally and then any
//! that leaves the mover's king attacked is dropped.

use std::fmt;

use crate::{Color, Kind, Move, Piece, Square, file_of, name, parse_square, rank_of, square};

pub const START_FEN: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

const WHITE_KINGSIDE: u8 = 1;
const WHITE_QUEENSIDE: u8 = 2;
const BLACK_KINGSIDE: u8 = 4;
const BLACK_QUEENSIDE: u8 = 8;

const KNIGHT: [(i32, i32); 8] = [(1, 2), (2, 1), (2, -1), (1, -2), (-1, -2), (-2, -1), (-2, 1), (-1, 2)];
const KING: [(i32, i32); 8] = [(1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1)];
const ORTHOGONAL: [(i32, i32); 4] = [(1, 0), (0, 1), (-1, 0), (0, -1)];
const DIAGONAL: [(i32, i32); 4] = [(1, 1), (-1, 1), (-1, -1), (1, -1)];

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Position {
    board: [Option<Piece>; 64],
    turn: Color,
    /// Bit set of the four rights above.
    castling: u8,
    /// Set only when the side to move has a legal en passant capture, so two
    /// positions that play the same compare (and later hash) the same.
    ep: Option<Square>,
    halfmove: u32,
    fullmove: u32,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FenError(pub &'static str);

impl fmt::Display for FenError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "invalid FEN: {}", self.0)
    }
}

impl std::error::Error for FenError {}

/// The square `df` files and `dr` ranks from `sq`, if still on the board.
fn offset(sq: Square, df: i32, dr: i32) -> Option<Square> {
    let f = file_of(sq) as i32 + df;
    let r = rank_of(sq) as i32 + dr;
    ((0..8).contains(&f) && (0..8).contains(&r)).then(|| square(f as usize, r as usize))
}

/// The castling rights lost when a piece moves from or to `sq`.
fn rights_touched(sq: Square) -> u8 {
    match sq {
        0 => WHITE_QUEENSIDE,
        4 => WHITE_KINGSIDE | WHITE_QUEENSIDE,
        7 => WHITE_KINGSIDE,
        56 => BLACK_QUEENSIDE,
        60 => BLACK_KINGSIDE | BLACK_QUEENSIDE,
        63 => BLACK_KINGSIDE,
        _ => 0,
    }
}

fn castle_right(color: Color, kingside: bool) -> u8 {
    match (color, kingside) {
        (Color::White, true) => WHITE_KINGSIDE,
        (Color::White, false) => WHITE_QUEENSIDE,
        (Color::Black, true) => BLACK_KINGSIDE,
        (Color::Black, false) => BLACK_QUEENSIDE,
    }
}

impl Default for Position {
    fn default() -> Self {
        Position::starting()
    }
}

impl Position {
    pub fn starting() -> Self {
        Position::from_fen(START_FEN).expect("start FEN is valid")
    }

    pub fn piece_at(&self, sq: Square) -> Option<Piece> {
        self.board[sq]
    }

    pub fn turn(&self) -> Color {
        self.turn
    }

    pub fn can_castle(&self, color: Color, kingside: bool) -> bool {
        self.castling & castle_right(color, kingside) != 0
    }

    /// The en passant square, only when a legal en passant capture exists.
    pub fn en_passant(&self) -> Option<Square> {
        self.ep
    }

    /// Half-moves since the last capture or pawn move, for the 50-move rule.
    pub fn halfmove_clock(&self) -> u32 {
        self.halfmove
    }

    pub fn fullmove_number(&self) -> u32 {
        self.fullmove
    }

    pub fn king(&self, color: Color) -> Option<Square> {
        (0..64).find(|&sq| self.board[sq] == Some(Piece { color, kind: Kind::King }))
    }

    pub fn in_check(&self) -> bool {
        self.king(self.turn).is_some_and(|k| self.is_attacked(k, self.turn.opponent()))
    }

    /// Whether any piece of `by` attacks `sq`, whatever stands on it.
    pub fn is_attacked(&self, sq: Square, by: Color) -> bool {
        let is = |s: Option<Square>, kinds: &[Kind]| {
            s.and_then(|s| self.board[s]).is_some_and(|p| p.color == by && kinds.contains(&p.kind))
        };
        // A pawn of `by` attacks from one rank behind, as `by` sees it.
        let back = -by.forward();
        if is(offset(sq, -1, back), &[Kind::Pawn]) || is(offset(sq, 1, back), &[Kind::Pawn]) {
            return true;
        }
        if KNIGHT.iter().any(|&(df, dr)| is(offset(sq, df, dr), &[Kind::Knight])) {
            return true;
        }
        if KING.iter().any(|&(df, dr)| is(offset(sq, df, dr), &[Kind::King])) {
            return true;
        }
        let ray_hits = |dirs: &[(i32, i32)], kinds: &[Kind]| {
            dirs.iter().any(|&(df, dr)| {
                let mut s = sq;
                while let Some(next) = offset(s, df, dr) {
                    if let Some(p) = self.board[next] {
                        return p.color == by && kinds.contains(&p.kind);
                    }
                    s = next;
                }
                false
            })
        };
        ray_hits(&ORTHOGONAL, &[Kind::Rook, Kind::Queen]) || ray_hits(&DIAGONAL, &[Kind::Bishop, Kind::Queen])
    }

    pub fn legal_moves(&self) -> Vec<Move> {
        let mut moves = Vec::with_capacity(64);
        for sq in 0..64 {
            self.pseudo_moves_from(sq, &mut moves);
        }
        moves.retain(|&m| self.keeps_king_safe(m));
        moves
    }

    /// The legal moves of the piece on `sq`; empty if it is not the side
    /// to move's.
    pub fn legal_moves_from(&self, sq: Square) -> Vec<Move> {
        let mut moves = Vec::new();
        self.pseudo_moves_from(sq, &mut moves);
        moves.retain(|&m| self.keeps_king_safe(m));
        moves
    }

    pub fn is_legal(&self, mv: Move) -> bool {
        mv.from < 64 && self.legal_moves_from(mv.from).contains(&mv)
    }

    /// The position after `mv`, or `None` if `mv` is not legal here.
    pub fn play(&self, mv: Move) -> Option<Position> {
        if !self.is_legal(mv) {
            return None;
        }
        let mut next = self.play_unchecked(mv);
        if next.ep.is_some() && !next.ep_capture_exists() {
            next.ep = None;
        }
        Some(next)
    }

    /// The piece `mv` captures and the square it stands on, which differs
    /// from `mv.to` for en passant.
    pub fn captured(&self, mv: Move) -> Option<(Piece, Square)> {
        if let Some(p) = self.board[mv.to] {
            return Some((p, mv.to));
        }
        if self.is_en_passant(mv) {
            let sq = square(file_of(mv.to), rank_of(mv.from));
            return self.board[sq].map(|p| (p, sq));
        }
        None
    }

    /// For castling, the rook's move as (from, to).
    pub fn castling_rook(&self, mv: Move) -> Option<(Square, Square)> {
        let piece = self.board[mv.from]?;
        if piece.kind != Kind::King || file_of(mv.from).abs_diff(file_of(mv.to)) != 2 {
            return None;
        }
        let rank = rank_of(mv.from);
        Some(if file_of(mv.to) == 6 { (square(7, rank), square(5, rank)) } else { (square(0, rank), square(3, rank)) })
    }

    /// Standard algebraic notation, with "+" or "#". `mv` must be legal.
    pub fn san(&self, mv: Move) -> String {
        let Some(piece) = self.board[mv.from] else { return mv.uci() };
        let mut s = String::new();
        if let Some((_, to)) = self.castling_rook(mv) {
            s.push_str(if file_of(to) == 5 { "O-O" } else { "O-O-O" });
        } else {
            let capture = self.captured(mv).is_some();
            if piece.kind == Kind::Pawn {
                if capture {
                    s.push((b'a' + file_of(mv.from) as u8) as char);
                }
            } else {
                s.push(piece.kind.letter());
                // Name the file, the rank or both when another piece of the
                // same kind could also reach `to`.
                let rivals: Vec<Square> = self
                    .legal_moves()
                    .into_iter()
                    .filter(|m| m.to == mv.to && m.from != mv.from && self.board[m.from] == Some(piece))
                    .map(|m| m.from)
                    .collect();
                if !rivals.is_empty() {
                    let file_clash = rivals.iter().any(|&r| file_of(r) == file_of(mv.from));
                    let rank_clash = rivals.iter().any(|&r| rank_of(r) == rank_of(mv.from));
                    let from = name(mv.from);
                    if !file_clash {
                        s.push_str(&from[..1]);
                    } else if !rank_clash {
                        s.push_str(&from[1..]);
                    } else {
                        s.push_str(&from);
                    }
                }
            }
            if capture {
                s.push('x');
            }
            s.push_str(&name(mv.to));
            if let Some(k) = mv.promotion {
                s.push('=');
                s.push(k.letter());
            }
        }
        if let Some(next) = self.play(mv)
            && next.in_check()
        {
            s.push(if next.legal_moves().is_empty() { '#' } else { '+' });
        }
        s
    }

    /// Neither side can possibly checkmate: bare kings, a single minor
    /// piece, or only bishops that all stand on one colour of square.
    pub fn insufficient_material(&self) -> bool {
        let mut minors = 0;
        let mut bishop_colors = [false; 2];
        let mut knights = false;
        for sq in 0..64 {
            let Some(p) = self.board[sq] else { continue };
            match p.kind {
                Kind::King => {}
                Kind::Pawn | Kind::Rook | Kind::Queen => return false,
                Kind::Knight => {
                    minors += 1;
                    knights = true;
                }
                Kind::Bishop => {
                    minors += 1;
                    bishop_colors[(file_of(sq) + rank_of(sq)) % 2] = true;
                }
            }
        }
        minors <= 1 || (!knights && !(bishop_colors[0] && bishop_colors[1]))
    }

    /// Whether `color` has enough to mate at all, for deciding a flag fall:
    /// running out of time against a lone king or king and minor is a draw.
    pub fn has_mating_material(&self, color: Color) -> bool {
        let mut minors = 0;
        for p in self.board.iter().flatten().filter(|p| p.color == color) {
            match p.kind {
                Kind::Pawn | Kind::Rook | Kind::Queen => return true,
                Kind::Knight | Kind::Bishop => minors += 1,
                Kind::King => {}
            }
        }
        minors >= 2
    }

    fn is_en_passant(&self, mv: Move) -> bool {
        self.board[mv.from].is_some_and(|p| p.kind == Kind::Pawn)
            && Some(mv.to) == self.ep
            && file_of(mv.from) != file_of(mv.to)
            && self.board[mv.to].is_none()
    }

    fn ep_capture_exists(&self) -> bool {
        let Some(ep) = self.ep else { return false };
        let back = -self.turn.forward();
        [-1, 1].into_iter().filter_map(|df| offset(ep, df, back)).any(|from| {
            self.board[from] == Some(Piece { color: self.turn, kind: Kind::Pawn })
                && self.keeps_king_safe(Move { from, to: ep, promotion: None })
        })
    }

    fn keeps_king_safe(&self, mv: Move) -> bool {
        let next = self.play_unchecked(mv);
        next.king(self.turn).is_some_and(|k| !next.is_attacked(k, self.turn.opponent()))
    }

    /// Plays a pseudo-legal move without checking it. The en passant square
    /// is set after every double push; `play` clears it if unusable.
    fn play_unchecked(&self, mv: Move) -> Position {
        let mut p = self.clone();
        let piece = p.board[mv.from].take().expect("a piece on the from square");
        let mut reset = piece.kind == Kind::Pawn || self.board[mv.to].is_some();

        if self.is_en_passant(mv) {
            p.board[square(file_of(mv.to), rank_of(mv.from))] = None;
            reset = true;
        }
        if let Some((rook_from, rook_to)) = self.castling_rook(mv) {
            p.board[rook_to] = p.board[rook_from].take();
        }
        let placed = match mv.promotion {
            Some(kind) => Piece { color: piece.color, kind },
            None => piece,
        };
        p.board[mv.to] = Some(placed);

        p.castling &= !(rights_touched(mv.from) | rights_touched(mv.to));
        p.ep = None;
        if piece.kind == Kind::Pawn && rank_of(mv.from).abs_diff(rank_of(mv.to)) == 2 {
            p.ep = Some(square(file_of(mv.from), (rank_of(mv.from) + rank_of(mv.to)) / 2));
        }
        p.halfmove = if reset { 0 } else { self.halfmove + 1 };
        if self.turn == Color::Black {
            p.fullmove += 1;
        }
        p.turn = self.turn.opponent();
        p
    }

    fn pseudo_moves_from(&self, from: Square, out: &mut Vec<Move>) {
        let Some(piece) = self.board.get(from).copied().flatten() else { return };
        if piece.color != self.turn {
            return;
        }
        match piece.kind {
            Kind::Pawn => self.pawn_moves(from, out),
            Kind::Knight | Kind::King => {
                let steps = if piece.kind == Kind::Knight { &KNIGHT } else { &KING };
                for to in steps.iter().filter_map(|&(df, dr)| offset(from, df, dr)) {
                    if self.board[to].is_none_or(|p| p.color != piece.color) {
                        out.push(Move { from, to, promotion: None });
                    }
                }
                if piece.kind == Kind::King {
                    self.castling_moves(from, out);
                }
            }
            Kind::Bishop => self.slides(from, &DIAGONAL, out),
            Kind::Rook => self.slides(from, &ORTHOGONAL, out),
            Kind::Queen => {
                self.slides(from, &DIAGONAL, out);
                self.slides(from, &ORTHOGONAL, out);
            }
        }
    }

    fn slides(&self, from: Square, dirs: &[(i32, i32)], out: &mut Vec<Move>) {
        for &(df, dr) in dirs {
            let mut s = from;
            while let Some(to) = offset(s, df, dr) {
                match self.board[to] {
                    None => out.push(Move { from, to, promotion: None }),
                    Some(p) => {
                        if p.color != self.turn {
                            out.push(Move { from, to, promotion: None });
                        }
                        break;
                    }
                }
                s = to;
            }
        }
    }

    fn pawn_moves(&self, from: Square, out: &mut Vec<Move>) {
        let us = self.turn;
        let dir = us.forward();
        let last_rank = us.opponent().back_rank();
        let mut add = |to: Square| {
            if rank_of(to) == last_rank {
                out.extend(Kind::PROMOTIONS.iter().map(|&k| Move { from, to, promotion: Some(k) }));
            } else {
                out.push(Move { from, to, promotion: None });
            }
        };
        if let Some(one) = offset(from, 0, dir)
            && self.board[one].is_none()
        {
            add(one);
            let start_rank = (us.back_rank() as i32 + dir) as usize;
            if rank_of(from) == start_rank
                && let Some(two) = offset(from, 0, 2 * dir)
                && self.board[two].is_none()
            {
                add(two);
            }
        }
        for df in [-1, 1] {
            let Some(to) = offset(from, df, dir) else { continue };
            let enemy = self.board[to].is_some_and(|p| p.color != us);
            if enemy || Some(to) == self.ep {
                add(to);
            }
        }
    }

    /// Castling: the right is held, the squares between king and rook are
    /// empty, and the king is not in check and does not pass through an
    /// attacked square. Landing in check is caught by the general filter.
    fn castling_moves(&self, from: Square, out: &mut Vec<Move>) {
        let us = self.turn;
        let rank = us.back_rank();
        if from != square(4, rank) {
            return;
        }
        let them = us.opponent();
        let rook = Some(Piece { color: us, kind: Kind::Rook });
        for kingside in [true, false] {
            if !self.can_castle(us, kingside) {
                continue;
            }
            let (rook_file, between, passed): (usize, &[usize], usize) =
                if kingside { (7, &[5, 6], 5) } else { (0, &[1, 2, 3], 3) };
            if self.board[square(rook_file, rank)] != rook
                || between.iter().any(|&f| self.board[square(f, rank)].is_some())
                || self.is_attacked(from, them)
                || self.is_attacked(square(passed, rank), them)
            {
                continue;
            }
            let to = square(if kingside { 6 } else { 2 }, rank);
            out.push(Move { from, to, promotion: None });
        }
    }

    /// Parses all six FEN fields strictly. The en passant square is kept
    /// only if a legal en passant capture exists.
    pub fn from_fen(fen: &str) -> Result<Position, FenError> {
        let fields: Vec<&str> = fen.split(' ').collect();
        let &[placement, turn, castling, ep, halfmove, fullmove] = fields.as_slice() else {
            return Err(FenError("expected six fields"));
        };

        let mut board = [None; 64];
        let ranks: Vec<&str> = placement.split('/').collect();
        if ranks.len() != 8 {
            return Err(FenError("expected eight ranks"));
        }
        for (i, row) in ranks.iter().enumerate() {
            let rank = 7 - i;
            let mut file = 0;
            for c in row.chars() {
                if let Some(d) = c.to_digit(10) {
                    if !(1..=8).contains(&d) {
                        return Err(FenError("bad empty-square count"));
                    }
                    file += d as usize;
                } else {
                    let kind = Kind::from_letter(c).ok_or(FenError("bad piece letter"))?;
                    let color = if c.is_ascii_uppercase() { Color::White } else { Color::Black };
                    if file >= 8 {
                        return Err(FenError("rank too long"));
                    }
                    board[square(file, rank)] = Some(Piece { color, kind });
                    file += 1;
                }
                if file > 8 {
                    return Err(FenError("rank too long"));
                }
            }
            if file != 8 {
                return Err(FenError("rank too short"));
            }
        }

        let turn = match turn {
            "w" => Color::White,
            "b" => Color::Black,
            _ => return Err(FenError("bad side to move")),
        };

        let mut rights = 0;
        if castling != "-" {
            for c in castling.chars() {
                let bit = match c {
                    'K' => WHITE_KINGSIDE,
                    'Q' => WHITE_QUEENSIDE,
                    'k' => BLACK_KINGSIDE,
                    'q' => BLACK_QUEENSIDE,
                    _ => return Err(FenError("bad castling rights")),
                };
                if rights & bit != 0 {
                    return Err(FenError("repeated castling right"));
                }
                rights |= bit;
            }
        }

        let halfmove = halfmove.parse().map_err(|_| FenError("bad halfmove clock"))?;
        let fullmove: u32 = fullmove.parse().map_err(|_| FenError("bad fullmove number"))?;
        if fullmove == 0 {
            return Err(FenError("fullmove number starts at 1"));
        }

        let mut p = Position { board, turn, castling: rights, ep: None, halfmove, fullmove };

        for color in [Color::White, Color::Black] {
            let kings = (0..64).filter(|&s| board[s] == Some(Piece { color, kind: Kind::King })).count();
            if kings != 1 {
                return Err(FenError("each side needs exactly one king"));
            }
        }
        if (0..8).any(|f| [0, 7].iter().any(|&r| board[square(f, r)].is_some_and(|p| p.kind == Kind::Pawn))) {
            return Err(FenError("pawn on the first or last rank"));
        }
        let them = turn.opponent();
        if p.king(them).is_some_and(|k| p.is_attacked(k, turn)) {
            return Err(FenError("side not to move is in check"));
        }
        for (color, kingside) in [(Color::White, true), (Color::White, false), (Color::Black, true), (Color::Black, false)] {
            if !p.can_castle(color, kingside) {
                continue;
            }
            let rank = color.back_rank();
            let rook_file = if kingside { 7 } else { 0 };
            if board[square(4, rank)] != Some(Piece { color, kind: Kind::King })
                || board[square(rook_file, rank)] != Some(Piece { color, kind: Kind::Rook })
            {
                return Err(FenError("castling right without king and rook at home"));
            }
        }

        if ep != "-" {
            let sq = parse_square(ep).ok_or(FenError("bad en passant square"))?;
            // The square a pawn of `them` just skipped over.
            let expected_rank = (them.back_rank() as i32 + 2 * them.forward()) as usize;
            let pushed = offset(sq, 0, -turn.forward());
            if rank_of(sq) != expected_rank
                || board[sq].is_some()
                || pushed.and_then(|s| board[s]) != Some(Piece { color: them, kind: Kind::Pawn })
            {
                return Err(FenError("impossible en passant square"));
            }
            p.ep = Some(sq);
            if !p.ep_capture_exists() {
                p.ep = None;
            }
        }
        Ok(p)
    }

    pub fn fen(&self) -> String {
        let mut s = String::new();
        for rank in (0..8).rev() {
            let mut empty = 0;
            for file in 0..8 {
                match self.board[square(file, rank)] {
                    None => empty += 1,
                    Some(p) => {
                        if empty > 0 {
                            s.push_str(&empty.to_string());
                            empty = 0;
                        }
                        let c = p.kind.letter();
                        s.push(if p.color == Color::White { c } else { c.to_ascii_lowercase() });
                    }
                }
            }
            if empty > 0 {
                s.push_str(&empty.to_string());
            }
            if rank > 0 {
                s.push('/');
            }
        }
        s.push_str(if self.turn == Color::White { " w " } else { " b " });
        let rights: String = [(WHITE_KINGSIDE, 'K'), (WHITE_QUEENSIDE, 'Q'), (BLACK_KINGSIDE, 'k'), (BLACK_QUEENSIDE, 'q')]
            .iter()
            .filter(|(bit, _)| self.castling & bit != 0)
            .map(|&(_, c)| c)
            .collect();
        s.push_str(if rights.is_empty() { "-" } else { &rights });
        s.push(' ');
        s.push_str(&self.ep.map(name).unwrap_or_else(|| "-".into()));
        s.push_str(&format!(" {} {}", self.halfmove, self.fullmove));
        s
    }

    /// Board, side to move, castling rights and en passant square: what
    /// must match for two positions to count as a repetition.
    pub(crate) fn same_position(&self, other: &Position) -> bool {
        self.board == other.board && self.turn == other.turn && self.castling == other.castling && self.ep == other.ep
    }
}
