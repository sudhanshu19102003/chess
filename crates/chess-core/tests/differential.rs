//! Plays random legal games and checks, at every ply, that our engine and
//! shakmaty agree on the legal moves, the FEN (en passant counted only when
//! capturable), SAN, check, checkmate, stalemate and insufficient material.

use chess_core::{Move, Position};
use shakmaty::{CastlingMode, Chess, EnPassantMode, Position as _, fen::Fen, san::SanPlus};

/// xorshift64*, so failures reproduce from the seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

fn oracle_fen(pos: &Chess) -> String {
    Fen::from_position(pos, EnPassantMode::Legal).to_string()
}

fn compare(ours: &Position, theirs: &Chess, context: &str) {
    assert_eq!(ours.fen(), oracle_fen(theirs), "{context}");
    let mut a: Vec<String> = ours.legal_moves().iter().map(Move::uci).collect();
    let mut b: Vec<String> = theirs.legal_moves().iter().map(|m| m.to_uci(CastlingMode::Standard).to_string()).collect();
    a.sort();
    b.sort();
    assert_eq!(a, b, "legal moves differ at {}", ours.fen());
    assert_eq!(ours.in_check(), theirs.is_check(), "check at {}", ours.fen());
    assert_eq!(ours.insufficient_material(), theirs.is_insufficient_material(), "material at {}", ours.fen());
}

fn random_games(start_fen: &str, games: u32, max_plies: u32, seed: u64) {
    let mut rng = Rng(seed);
    for game in 0..games {
        let mut ours = Position::from_fen(start_fen).unwrap();
        let mut theirs: Chess = start_fen.parse::<Fen>().unwrap().into_position(CastlingMode::Standard).unwrap();
        for ply in 0..max_plies {
            let context = format!("seed {seed} game {game} ply {ply}");
            compare(&ours, &theirs, &context);
            let moves = theirs.legal_moves();
            if moves.is_empty() || ours.insufficient_material() {
                break;
            }
            let m = moves[(rng.next() % moves.len() as u64) as usize];
            let uci = m.to_uci(CastlingMode::Standard).to_string();
            let mv = Move::parse_uci(&uci).unwrap();
            assert_eq!(ours.san(mv), SanPlus::from_move(theirs.clone(), m).to_string(), "SAN of {uci} at {}", ours.fen());
            ours = ours.play(mv).unwrap_or_else(|| panic!("{context}: {uci} rejected at {}", ours.fen()));
            theirs.play_unchecked(m);
        }
    }
}

#[test]
fn from_the_start() {
    random_games("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1", 300, 300, 1);
}

#[test]
fn from_kiwipete() {
    random_games("r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1", 200, 200, 2);
}

#[test]
fn from_promotion_and_en_passant_positions() {
    random_games("rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8", 200, 200, 3);
    random_games("8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1", 200, 200, 4);
    random_games("r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1", 200, 200, 5);
}
