//! The rules of chess. Both clients run this to decide which moves are
//! legal, so a bug here is also a security bug: favour plain, checkable code
//! over speed.

mod game;
mod position;

pub use game::{Game, Outcome};
pub use position::{FenError, Position};

/// Bumped with any change to which moves are legal, how games end, FEN or
/// SAN. Peers pin it in the handshake: two engines that disagree on a rule
/// would disagree on a move's legality mid-game.
pub const ENGINE_VERSION: u32 = 1;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Color {
    White,
    Black,
}

impl Color {
    pub fn opponent(self) -> Color {
        match self {
            Color::White => Color::Black,
            Color::Black => Color::White,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Color::White => "WHITE",
            Color::Black => "BLACK",
        }
    }

    /// The rank this side's pieces start on (0 or 7).
    pub fn back_rank(self) -> usize {
        match self {
            Color::White => 0,
            Color::Black => 7,
        }
    }

    /// The direction this side's pawns advance, in ranks.
    pub fn forward(self) -> i32 {
        match self {
            Color::White => 1,
            Color::Black => -1,
        }
    }
}

/// Piece kinds, in display order for captured pieces: pawns first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Kind {
    Pawn,
    Knight,
    Bishop,
    Rook,
    Queen,
    King,
}

impl Kind {
    /// The pieces a pawn may promote to.
    pub const PROMOTIONS: [Kind; 4] = [Kind::Queen, Kind::Rook, Kind::Bishop, Kind::Knight];

    /// Conventional material value; the king counts for nothing.
    pub fn value(self) -> i32 {
        match self {
            Kind::Pawn => 1,
            Kind::Knight | Kind::Bishop => 3,
            Kind::Rook => 5,
            Kind::Queen => 9,
            Kind::King => 0,
        }
    }

    /// Upper-case letter as in SAN and FEN; pawns get "P" here.
    pub fn letter(self) -> char {
        match self {
            Kind::Pawn => 'P',
            Kind::Knight => 'N',
            Kind::Bishop => 'B',
            Kind::Rook => 'R',
            Kind::Queen => 'Q',
            Kind::King => 'K',
        }
    }

    fn from_letter(c: char) -> Option<Kind> {
        Some(match c.to_ascii_uppercase() {
            'P' => Kind::Pawn,
            'N' => Kind::Knight,
            'B' => Kind::Bishop,
            'R' => Kind::Rook,
            'Q' => Kind::Queen,
            'K' => Kind::King,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Piece {
    pub color: Color,
    pub kind: Kind,
}

/// Square index 0..64, a1 = 0, b1 = 1, ..., h8 = 63.
pub type Square = usize;

pub fn square(file: usize, rank: usize) -> Square {
    rank * 8 + file
}

pub fn file_of(sq: Square) -> usize {
    sq % 8
}

pub fn rank_of(sq: Square) -> usize {
    sq / 8
}

/// "e4" and so on.
pub fn name(sq: Square) -> String {
    format!("{}{}", (b'a' + file_of(sq) as u8) as char, rank_of(sq) + 1)
}

/// Parses "e4"; anything else is `None`.
pub fn parse_square(s: &str) -> Option<Square> {
    match s.as_bytes() {
        &[f @ b'a'..=b'h', r @ b'1'..=b'8'] => Some(square((f - b'a') as usize, (r - b'1') as usize)),
        _ => None,
    }
}

/// A move as the wire protocol sends it (UCI): from, to and a promotion
/// piece. Castling is the king moving two squares; en passant is a pawn
/// moving diagonally to the en passant square. Only moves from
/// `Position::legal_moves` may be played.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Move {
    pub from: Square,
    pub to: Square,
    pub promotion: Option<Kind>,
}

impl Move {
    pub fn uci(&self) -> String {
        let promo = self.promotion.map(|k| k.letter().to_ascii_lowercase().to_string()).unwrap_or_default();
        format!("{}{}{promo}", name(self.from), name(self.to))
    }

    /// Parses UCI syntax only; whether the move is legal is a separate
    /// question for `Position::legal_moves`.
    pub fn parse_uci(s: &str) -> Option<Move> {
        if !s.is_ascii() || !(4..=5).contains(&s.len()) {
            return None;
        }
        let from = parse_square(&s[0..2])?;
        let to = parse_square(&s[2..4])?;
        let promotion = match s.as_bytes().get(4) {
            None => None,
            Some(b'q') => Some(Kind::Queen),
            Some(b'r') => Some(Kind::Rook),
            Some(b'b') => Some(Kind::Bishop),
            Some(b'n') => Some(Kind::Knight),
            Some(_) => return None,
        };
        Some(Move { from, to, promotion })
    }
}
