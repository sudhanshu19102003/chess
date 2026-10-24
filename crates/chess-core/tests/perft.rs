//! Move counts against published perft totals (Chess Programming Wiki,
//! "Perft Results"). Deeper runs are ignored by default; run them with
//! `cargo test --release -p chess-core -- --ignored`.

use chess_core::Position;

fn perft(pos: &Position, depth: u32) -> u64 {
    let moves = pos.legal_moves();
    if depth == 1 {
        return moves.len() as u64;
    }
    moves.into_iter().map(|m| perft(&pos.play(m).expect("generated moves are legal"), depth - 1)).sum()
}

fn check(fen: &str, expected: &[u64]) {
    let pos = Position::from_fen(fen).unwrap();
    for (i, &n) in expected.iter().enumerate() {
        assert_eq!(perft(&pos, i as u32 + 1), n, "{fen} at depth {}", i + 1);
    }
}

const START: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";
const KIWIPETE: &str = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";
const POSITION_3: &str = "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1";
const POSITION_4: &str = "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1";
const POSITION_5: &str = "rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8";

#[test]
fn start_position() {
    check(START, &[20, 400, 8_902, 197_281]);
}

#[test]
fn kiwipete() {
    check(KIWIPETE, &[48, 2_039, 97_862]);
}

#[test]
fn position_3() {
    check(POSITION_3, &[14, 191, 2_812, 43_238]);
}

#[test]
fn position_4() {
    check(POSITION_4, &[6, 264, 9_467]);
}

#[test]
fn position_5() {
    check(POSITION_5, &[44, 1_486, 62_379]);
}

#[test]
#[ignore]
fn deep() {
    check(START, &[20, 400, 8_902, 197_281, 4_865_609]);
    check(KIWIPETE, &[48, 2_039, 97_862, 4_085_603]);
    check(POSITION_3, &[14, 191, 2_812, 43_238, 674_624]);
    check(POSITION_4, &[6, 264, 9_467, 422_333]);
    check(POSITION_5, &[44, 1_486, 62_379, 2_103_487]);
}
