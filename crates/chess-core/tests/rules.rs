//! One test per rule that is easy to get wrong.

use chess_core::{Color, Game, Kind, Move, Outcome, Position, parse_square};

fn pos(fen: &str) -> Position {
    Position::from_fen(fen).unwrap()
}

fn mv(uci: &str) -> Move {
    Move::parse_uci(uci).unwrap()
}

fn legal(p: &Position, uci: &str) -> bool {
    p.is_legal(mv(uci))
}

fn play(p: &Position, uci: &str) -> Position {
    p.play(mv(uci)).unwrap_or_else(|| panic!("{uci} should be legal in {}", p.fen()))
}

#[test]
fn en_passant_that_exposes_the_king_along_a_rank_is_illegal() {
    // White king a5 and pawn e5, Black has just played d7d5, black rook h5.
    // Taking en passant would leave nothing between the rook and the king.
    let p = pos("4k3/8/8/K2pP2r/8/8/8/8 w - d6 0 1");
    assert!(!legal(&p, "e5d6"));
    // So the en passant square does not count, and is not in the FEN.
    assert_eq!(p.en_passant(), None);
    assert_eq!(p.fen(), "4k3/8/8/K2pP2r/8/8/8/8 w - - 0 1");
}

#[test]
fn en_passant_capture_removes_the_pawn() {
    let p = play(&pos("4k3/3p4/8/4P3/8/8/8/4K3 b - - 0 1"), "d7d5");
    assert_eq!(p.en_passant(), parse_square("d6"));
    let after = play(&p, "e5d6");
    assert_eq!(after.piece_at(parse_square("d5").unwrap()), None);
    assert_eq!(p.captured(mv("e5d6")).unwrap().1, parse_square("d5").unwrap());
}

#[test]
fn en_passant_square_only_set_when_a_capture_exists() {
    let p = play(&Position::starting(), "e2e4");
    assert_eq!(p.en_passant(), None);
    assert!(p.fen().contains(" b KQkq - "));
}

#[test]
fn en_passant_is_only_available_immediately() {
    let p = play(&pos("4k3/3p4/8/4P3/8/8/8/4K3 b - - 0 1"), "d7d5");
    let p = play(&play(&p, "e1e2"), "e8e7");
    assert!(!legal(&p, "e5d6"));
}

#[test]
fn castling_through_check_is_illegal() {
    // Black rook on f8 covers f1.
    let p = pos("4kr2/8/8/8/8/8/8/R3K2R w KQ - 0 1");
    assert!(!legal(&p, "e1g1"));
    assert!(legal(&p, "e1c1"));
}

#[test]
fn castling_out_of_or_into_check_is_illegal() {
    let out = pos("4r1k1/8/8/8/8/8/8/R3K2R w KQ - 0 1");
    assert!(!legal(&out, "e1g1") && !legal(&out, "e1c1"));
    let into = pos("6r1/4k3/8/8/8/8/8/R3K2R w KQ - 0 1");
    assert!(!legal(&into, "e1g1"));
    assert!(legal(&into, "e1c1"));
}

#[test]
fn queenside_castling_may_pass_an_attacked_b_file() {
    // b1 is attacked but the king never crosses it.
    let p = pos("1r2k3/8/8/8/8/8/8/R3K3 w Q - 0 1");
    assert!(legal(&p, "e1c1"));
    // But b1 must be empty.
    let blocked = pos("4k3/8/8/8/8/8/8/RN2K3 w Q - 0 1");
    assert!(!legal(&blocked, "e1c1"));
}

#[test]
fn castling_moves_the_rook() {
    let p = play(&pos("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1"), "e1g1");
    assert_eq!(p.fen(), "r3k2r/8/8/8/8/8/8/R4RK1 b kq - 1 1");
    let p = play(&p, "e8c8");
    assert_eq!(p.fen(), "2kr3r/8/8/8/8/8/8/R4RK1 w - - 2 2");
}

#[test]
fn capturing_a_rook_on_its_home_square_removes_that_right() {
    let p = pos("r3k2r/8/8/8/8/8/6B1/R3K2R w KQkq - 0 1");
    let p = play(&p, "g2a8");
    assert!(!p.can_castle(Color::Black, false));
    assert!(p.can_castle(Color::Black, true));
    assert!(!legal(&p, "e8c8"));
}

#[test]
fn moving_king_or_rook_removes_rights() {
    let p = pos("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1");
    let rook = play(&p, "h1h2");
    assert!(!rook.can_castle(Color::White, true) && rook.can_castle(Color::White, false));
    let king = play(&p, "e1e2");
    assert!(!king.can_castle(Color::White, true) && !king.can_castle(Color::White, false));
}

#[test]
fn promotion_offers_four_pieces_including_capture_promotions() {
    let p = pos("1n2k3/P7/8/8/8/8/8/4K3 w - - 0 1");
    let moves = p.legal_moves_from(parse_square("a7").unwrap());
    assert_eq!(moves.len(), 8, "four pushes and four captures: {moves:?}");
    for k in Kind::PROMOTIONS {
        assert!(moves.iter().any(|m| m.to == parse_square("b8").unwrap() && m.promotion == Some(k)));
    }
    assert!(!legal(&p, "a7a8"), "a promotion must name its piece");
    let after = play(&p, "a7b8n");
    assert_eq!(after.fen(), "1N2k3/8/8/8/8/8/8/4K3 b - - 0 1");
}

#[test]
fn checkmate_and_stalemate() {
    let mut g = Game::new();
    for m in ["f2f3", "e7e5", "g2g4", "d8h4"] {
        assert!(g.play(mv(m)));
    }
    assert_eq!(g.outcome(), Some(Outcome::Checkmate { winner: Color::Black }));
    assert!(!g.play(mv("a2a3")), "no moves after the game ends");

    let stale = Game::from_position(pos("7k/5Q2/6K1/8/8/8/8/8 b - - 0 1"));
    assert_eq!(stale.outcome(), Some(Outcome::Stalemate));
}

#[test]
fn threefold_repetition() {
    let mut g = Game::new();
    for m in ["g1f3", "g8f6", "f3g1", "f6g8", "g1f3", "g8f6", "f3g1"] {
        assert!(g.play(mv(m)));
        assert_eq!(g.outcome(), None);
    }
    assert!(g.play(mv("f6g8")));
    assert_eq!(g.repetitions(), 3);
    assert_eq!(g.outcome(), Some(Outcome::Repetition));
}

#[test]
fn repetition_needs_the_same_castling_rights() {
    // The start position recurs, but White has lost castling rights.
    let mut g = Game::from_position(pos("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1"));
    for m in ["e1e2", "e8e7", "e2e1", "e7e8", "e1e2", "e8e7", "e2e1", "e7e8"] {
        assert!(g.play(mv(m)));
    }
    // Rights went with the first king move, so the first position never recurs;
    // the one after "e2e1 e7e8" has occurred twice.
    assert_eq!(g.repetitions(), 2);
    assert_eq!(g.outcome(), None);
}

#[test]
fn fifty_move_rule_and_its_resets() {
    let g = Game::from_position(pos("4k3/8/8/8/8/8/4P3/R3K3 w - - 99 80"));
    assert_eq!(g.outcome(), None);
    let mut g2 = g.clone();
    assert!(g2.play(mv("a1a2")));
    assert_eq!(g2.outcome(), Some(Outcome::FiftyMoves));
    let mut g3 = g.clone();
    assert!(g3.play(mv("e2e3")));
    assert_eq!(g3.position().halfmove_clock(), 0, "pawn moves reset the count");
    let capture = pos("4k3/8/8/8/8/8/r7/R3K3 w - - 99 80");
    assert_eq!(play(&capture, "a1a2").halfmove_clock(), 0, "captures reset the count");
}

#[test]
fn checkmate_on_the_hundredth_half_move_is_checkmate() {
    let mut g = Game::from_position(pos("7k/R7/6K1/8/8/8/8/8 w - - 99 80"));
    assert!(g.play(mv("a7a8")));
    assert_eq!(g.outcome(), Some(Outcome::Checkmate { winner: Color::White }));
}

#[test]
fn insufficient_material() {
    for fen in [
        "4k3/8/8/8/8/8/8/4K3 w - - 0 1",
        "4k3/8/8/8/8/8/8/4KN2 w - - 0 1",
        "4k3/8/8/8/8/8/8/4KB2 w - - 0 1",
        // Bishops all on light squares, either side.
        "4kb2/8/8/8/8/8/8/2B1K3 w - - 0 1",
        "4k3/8/8/8/8/8/8/B1B1K3 w - - 0 1",
    ] {
        assert!(pos(fen).insufficient_material(), "{fen}");
    }
    for fen in [
        "4k3/8/8/8/8/8/4P3/4K3 w - - 0 1",
        "4k3/8/8/8/8/8/8/3RK3 w - - 0 1",
        "4kb2/8/8/8/8/8/8/3BK3 w - - 0 1",   // opposite-coloured bishops
        "4kn2/8/8/8/8/8/8/4KN2 w - - 0 1",   // mate is possible
        "4k3/8/8/8/8/8/8/3NKN2 w - - 0 1",
        "4k3/8/8/8/8/8/8/3NKB2 w - - 0 1",
    ] {
        assert!(!pos(fen).insufficient_material(), "{fen}");
    }
}

#[test]
fn san() {
    let p = Position::starting();
    assert_eq!(p.san(mv("e2e4")), "e4");
    assert_eq!(p.san(mv("g1f3")), "Nf3");
    // Two knights reach d2: name the file.
    let p = pos("4k3/8/8/8/8/8/8/1N2KN2 w - - 0 1");
    assert_eq!(p.san(mv("b1d2")), "Nbd2");
    // Two rooks on one file: name the rank.
    let p = pos("4k3/R7/8/8/8/8/8/R3K3 w - - 0 1");
    assert_eq!(p.san(mv("a1a4")), "R1a4");
    assert_eq!(pos("r3k3/8/8/8/8/8/8/4K2R w K - 0 1").san(mv("e1g1")), "O-O");
    assert_eq!(pos("1n2k3/P7/8/8/8/8/8/4K3 w - - 0 1").san(mv("a7b8q")), "axb8=Q+");
    assert_eq!(pos("6k1/5ppp/8/8/8/8/8/R5K1 w - - 0 1").san(mv("a1a8")), "Ra8#");
}

#[test]
fn fen_round_trips_and_rejects_nonsense() {
    for fen in [
        "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1",
        "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
        "rnbqkbnr/ppp1p1pp/8/3pPp2/8/8/PPPP1PPP/RNBQKBNR w KQkq f6 0 3",
    ] {
        assert_eq!(pos(fen).fen(), fen);
    }
    for bad in [
        "",
        "8/8/8/8/8/8/8/8 w - - 0 1",                                   // no kings
        "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0",      // five fields
        "rnbqkbnr/pppppppp/9/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1",    // nine squares
        "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBN w KQkq - 0 1",     // seven squares
        "4k3/8/8/8/8/8/8/4K3 x - - 0 1",
        "4k3/8/8/8/8/8/8/4K3 w K - 0 1",                               // right without rook
        "4k3/8/8/8/8/8/8/4K3 w - e3 0 1",                              // no pawn pushed
        "4k3/8/8/8/8/8/8/4K3 w - - 0 0",
        "P3k3/8/8/8/8/8/8/4K3 w - - 0 1",                              // pawn on last rank
        "4k3/4Q3/8/8/8/8/8/4K3 w - - 0 1",                             // opponent in check
        "4k3/8/8/8/8/8/8/4K3  w - - 0 1",                              // double space
    ] {
        assert!(Position::from_fen(bad).is_err(), "{bad:?} should be rejected");
    }
}

#[test]
fn uci_parsing_is_strict() {
    assert_eq!(mv("e7e8q").promotion, Some(Kind::Queen));
    for bad in ["", "e2", "e2e9", "i2i4", "e7e8k", "e2e4 ", "E2E4", "e2e4qq", "é2e4"] {
        assert_eq!(Move::parse_uci(bad), None, "{bad:?}");
    }
}

#[test]
fn has_mating_material() {
    let p = pos("4k3/8/8/8/8/8/8/3NKB2 w - - 0 1");
    assert!(p.has_mating_material(Color::White));
    assert!(!p.has_mating_material(Color::Black));
    assert!(!pos("4k3/8/8/8/8/8/8/4KB2 w - - 0 1").has_mating_material(Color::White));
}
