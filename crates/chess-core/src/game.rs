//! A game: the positions so far, for repetition, and how it ended.

use crate::{Color, Move, Position};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    Checkmate { winner: Color },
    Stalemate,
    /// The same position (pieces, side to move, castling rights, en passant)
    /// for the third time.
    Repetition,
    /// A hundred half-moves without a capture or pawn move.
    FiftyMoves,
    InsufficientMaterial,
}

impl Outcome {
    pub fn winner(self) -> Option<Color> {
        match self {
            Outcome::Checkmate { winner } => Some(winner),
            _ => None,
        }
    }

    pub fn reason(self) -> &'static str {
        match self {
            Outcome::Checkmate { .. } => "checkmate",
            Outcome::Stalemate => "stalemate",
            Outcome::Repetition => "threefold repetition",
            Outcome::FiftyMoves => "50-move rule",
            Outcome::InsufficientMaterial => "insufficient material",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Game {
    /// Every position from the start, the current one last.
    positions: Vec<Position>,
    moves: Vec<Move>,
}

impl Default for Game {
    fn default() -> Self {
        Game::new()
    }
}

impl Game {
    pub fn new() -> Self {
        Game::from_position(Position::starting())
    }

    pub fn from_position(start: Position) -> Self {
        Game { positions: vec![start], moves: Vec::new() }
    }

    pub fn position(&self) -> &Position {
        self.positions.last().expect("a game always has a position")
    }

    pub fn moves(&self) -> &[Move] {
        &self.moves
    }

    /// Plays `mv` if it is legal and the game is not over.
    pub fn play(&mut self, mv: Move) -> bool {
        if self.outcome().is_some() {
            return false;
        }
        let Some(next) = self.position().play(mv) else { return false };
        self.positions.push(next);
        self.moves.push(mv);
        true
    }

    /// How many times the current position has occurred, itself included.
    pub fn repetitions(&self) -> usize {
        let now = self.position();
        // Positions before the last capture or pawn move cannot recur.
        let window = (now.halfmove_clock() as usize + 1).min(self.positions.len());
        self.positions[self.positions.len() - window..].iter().filter(|p| p.same_position(now)).count()
    }

    /// Draws by repetition and the 50-move rule are applied automatically,
    /// with no claim needed.
    pub fn outcome(&self) -> Option<Outcome> {
        let pos = self.position();
        if pos.legal_moves().is_empty() {
            return Some(if pos.in_check() { Outcome::Checkmate { winner: pos.turn().opponent() } } else { Outcome::Stalemate });
        }
        if pos.insufficient_material() {
            Some(Outcome::InsufficientMaterial)
        } else if self.repetitions() >= 3 {
            Some(Outcome::Repetition)
        } else if pos.halfmove_clock() >= 100 {
            Some(Outcome::FiftyMoves)
        } else {
            None
        }
    }
}
