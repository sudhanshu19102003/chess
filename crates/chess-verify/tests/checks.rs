//! One test per check: each bad message is refused with the right reason
//! and leaves the receiver's state as it was.

use chess_core::{Color, Move, Outcome, Position};
use chess_verify::{
    GameId, Hash, Msg, MoveMsg, ParseError, Received, Reject, Session, Signal, SignalMsg, SigningKey, TimeControl, position_hash,
    replay,
};

const GAME: GameId = GameId([9; 16]);
/// Five minutes plus two seconds a move.
const TC: TimeControl = TimeControl { base_ms: 300_000, increment_ms: 2_000 };

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn mv(uci: &str) -> Move {
    Move::parse_uci(uci).unwrap()
}

/// White's and Black's sessions of the same game.
fn pair() -> (Session, Session) {
    let start = Position::starting();
    let white = Session::new(GAME, start.clone(), Color::White, key(1), key(2).verifying_key(), TC);
    let black = Session::new(GAME, start, Color::Black, key(2), key(1).verifying_key(), TC);
    (white, black)
}

/// Plays `moves` alternately from whichever side is to move.
fn play(white: &mut Session, black: &mut Session, moves: &[&str]) {
    for uci in moves {
        let (mover, other) = if white.chain().game().position().turn() == Color::White {
            (&mut *white, &mut *black)
        } else {
            (&mut *black, &mut *white)
        };
        let msg = mover.play(mv(uci), 0).unwrap();
        assert_eq!(other.receive(&msg.encode()), Ok(Received::Move { mv: mv(uci), think_ms: 0 }));
    }
}

fn state(s: &Session) -> (Hash, u32, String, bool) {
    let c = s.chain();
    (c.head(), c.ply(), c.game().position().fen(), c.is_halted())
}

/// A message as an honest White would send it next, for `black` to receive.
fn honest(black: &Session, uci: &str) -> MoveMsg {
    let c = black.chain();
    let next = c.game().position().play(mv(uci)).unwrap();
    MoveMsg::signed(&key(1), GAME, c.ply() + 1, c.head(), mv(uci), 0, position_hash(&next))
}

/// Asserts `black` refuses `bytes` with `why` and nothing changes.
fn refused(black: &mut Session, bytes: &[u8], why: Reject) {
    let before = state(black);
    assert_eq!(black.receive(bytes), Err(why));
    let after = state(black);
    assert_eq!((after.0, after.1, after.2), (before.0, before.1, before.2));
}

#[test]
fn honest_game_keeps_chains_in_step() {
    let (mut w, mut b) = pair();
    play(&mut w, &mut b, &["e2e4", "e7e5", "g1f3", "b8c6", "f1c4", "g8f6", "e1g1"]);
    assert_eq!(w.chain().head(), b.chain().head());
    assert_eq!(w.chain().ply(), 7);
    assert_eq!(w.chain().log(), b.chain().log());
}

#[test]
fn honest_message_is_accepted() {
    let (_, mut b) = pair();
    let msg = honest(&b, "e2e4");
    assert_eq!(b.receive(&msg.encode()), Ok(Received::Move { mv: mv("e2e4"), think_ms: 0 }));
}

#[test]
fn check_1_malformed() {
    let (_, mut b) = pair();
    let bytes = honest(&b, "e2e4").encode();
    refused(&mut b, &bytes[..bytes.len() - 1], Reject::Malformed(ParseError::WrongLength));
    refused(&mut b, &[0; 1000], Reject::Malformed(ParseError::TooLong));
}

#[test]
fn check_2_wrong_game() {
    let (_, mut b) = pair();
    let mut msg = honest(&b, "e2e4");
    msg.game_id = GameId([8; 16]);
    let msg = MoveMsg::signed(&key(1), msg.game_id, msg.ply, msg.prev_hash, msg.mv, 0, msg.post_hash);
    refused(&mut b, &msg.encode(), Reject::WrongGame);
}

#[test]
fn check_3_bad_signature() {
    let (_, mut b) = pair();
    let msg = honest(&b, "e2e4");
    // Signed by someone else.
    let forged = MoveMsg::signed(&key(3), GAME, msg.ply, msg.prev_hash, msg.mv, 0, msg.post_hash);
    refused(&mut b, &forged.encode(), Reject::BadSignature);
    // Signed by Black, the receiver itself.
    let own = MoveMsg::signed(&key(2), GAME, msg.ply, msg.prev_hash, msg.mv, 0, msg.post_hash);
    refused(&mut b, &own.encode(), Reject::BadSignature);
    // A field changed after signing.
    let mut altered = msg.clone();
    altered.mv = mv("d2d4");
    refused(&mut b, &altered.encode(), Reject::BadSignature);
    let mut bytes = msg.encode();
    *bytes.last_mut().unwrap() ^= 1;
    refused(&mut b, &bytes, Reject::BadSignature);
}

#[test]
fn check_4_wrong_ply() {
    let (mut w, mut b) = pair();
    let first = w.play(mv("e2e4"), 0).unwrap();
    b.receive(&first.encode()).unwrap();
    // A replay of an accepted message.
    let before = state(&w);
    assert_eq!(w.receive(&first.encode()), Err(Reject::BadSignature));
    assert_eq!(state(&w), before);
    let reply = b.play(mv("e7e5"), 0).unwrap();
    w.receive(&reply.encode()).unwrap();
    refused(&mut b, &first.encode(), Reject::WrongPly { expected: 3, got: 1 });
    // Skipping ahead.
    let c = b.chain();
    let next = c.game().position().play(mv("d2d4")).unwrap();
    let skip = MoveMsg::signed(&key(1), GAME, 4, c.head(), mv("d2d4"), 0, position_hash(&next));
    refused(&mut b, &skip.encode(), Reject::WrongPly { expected: 3, got: 4 });
}

#[test]
fn check_5_not_senders_turn() {
    let (mut w, b) = pair();
    // Black sends first, with everything else right.
    let c = w.chain();
    let next = c.game().position().play(mv("e2e4")).unwrap();
    let msg = MoveMsg::signed(&key(2), GAME, 1, c.head(), mv("e2e4"), 0, position_hash(&next));
    let before = state(&w);
    assert_eq!(w.receive(&msg.encode()), Err(Reject::NotSendersTurn));
    assert_eq!(state(&w), before);
    // And our own side cannot move out of turn either.
    let mut b = b;
    assert_eq!(b.play(mv("e7e5"), 0), Err(Reject::NotSendersTurn));
}

#[test]
fn check_6_wrong_prev_hash() {
    let (_, mut b) = pair();
    let msg = honest(&b, "e2e4");
    let forged = MoveMsg::signed(&key(1), GAME, msg.ply, Hash([0; 32]), msg.mv, 0, msg.post_hash);
    refused(&mut b, &forged.encode(), Reject::WrongPrevHash);
}

#[test]
fn check_7_illegal_move() {
    let (mut w, mut b) = pair();
    for uci in ["e2e5", "e1e2", "e7e5", "a1a3", "e2e4q"] {
        let c = b.chain();
        let msg = MoveMsg::signed(&key(1), GAME, 1, c.head(), mv(uci), 0, Hash([0; 32]));
        refused(&mut b, &msg.encode(), Reject::IllegalMove);
    }
    assert_eq!(w.play(mv("e2e5"), 0), Err(Reject::IllegalMove));
}

#[test]
fn check_7_no_moves_after_the_game_ends() {
    let (mut w, mut b) = pair();
    // Threefold repetition: the game ends although moves are still legal.
    let shuffle = ["g1f3", "g8f6", "f3g1", "f6g8"];
    play(&mut w, &mut b, &shuffle);
    play(&mut w, &mut b, &shuffle);
    assert_eq!(b.chain().game().outcome(), Some(Outcome::Repetition));
    let msg = honest(&b, "g1f3");
    refused(&mut b, &msg.encode(), Reject::GameOver);
    assert_eq!(w.play(mv("g1f3"), 0), Err(Reject::GameOver));
}

#[test]
fn check_9_hash_mismatch_halts() {
    let (_, mut b) = pair();
    let msg = honest(&b, "e2e4");
    let wrong = MoveMsg::signed(&key(1), GAME, msg.ply, msg.prev_hash, msg.mv, 0, Hash([5; 32]));
    let before = state(&b);
    let ours = position_hash(&Position::starting().play(mv("e2e4")).unwrap());
    assert_eq!(b.receive(&wrong.encode()), Err(Reject::HashMismatch { ours, theirs: Hash([5; 32]) }));
    let after = state(&b);
    assert_eq!((after.0, after.1, &after.2), (before.0, before.1, &before.2));
    assert!(b.chain().is_halted());
    // Even the honest message is refused now.
    assert_eq!(b.receive(&msg.encode()), Err(Reject::Halted));
}

#[test]
fn log_replays_offline() {
    let (mut w, mut b) = pair();
    play(&mut w, &mut b, &["f2f3", "e7e5", "g2g4", "d8h4"]);
    let log = w.chain().log().to_vec();
    let (wk, bk) = (key(1).verifying_key(), key(2).verifying_key());
    let chain = replay(GAME, Position::starting(), wk, bk, TC, &log).unwrap();
    assert_eq!(chain.head(), w.chain().head());
    assert_eq!(chain.game().outcome(), Some(Outcome::Checkmate { winner: Color::Black }));

    // Tampering anywhere is caught at the first bad message.
    let mut swapped = log.clone();
    swapped.swap(1, 2);
    assert_eq!(replay(GAME, Position::starting(), wk, bk, TC, &swapped).unwrap_err().0, 1);
    assert_eq!(replay(GAME, Position::starting(), wk, bk, TC, &log[1..]).unwrap_err().0, 0);
    // Under swapped keys, or another game, nothing verifies.
    assert_eq!(replay(GAME, Position::starting(), bk, wk, TC, &log).unwrap_err(), (0, Reject::BadSignature));
    assert_eq!(replay(GameId([0; 16]), Position::starting(), wk, bk, TC, &log).unwrap_err(), (0, Reject::WrongGame));
}

#[test]
fn starts_from_any_position() {
    // Black to move with an en passant capture available.
    let start = Position::from_fen("4k3/8/8/8/3pP3/8/8/4K3 b - e3 0 1").unwrap();
    let mut b = Session::new(GAME, start.clone(), Color::Black, key(2), key(1).verifying_key(), TC);
    let mut w = Session::new(GAME, start, Color::White, key(1), key(2).verifying_key(), TC);
    let msg = b.play(mv("d4e3"), 0).unwrap();
    assert_eq!(w.receive(&msg.encode()), Ok(Received::Move { mv: mv("d4e3"), think_ms: 0 }));
    assert_eq!(w.chain().head(), b.chain().head());
}

/// Plays `moves` alternately, each with its thinking time in ms.
fn play_timed(white: &mut Session, black: &mut Session, moves: &[(&str, u32)]) {
    for &(uci, think_ms) in moves {
        let (mover, other) = if white.chain().game().position().turn() == Color::White {
            (&mut *white, &mut *black)
        } else {
            (&mut *black, &mut *white)
        };
        let msg = mover.play(mv(uci), think_ms).unwrap();
        assert_eq!(other.receive(&msg.encode()), Ok(Received::Move { mv: mv(uci), think_ms }));
    }
}

#[test]
fn the_agreed_clock_comes_from_the_log() {
    let (mut w, mut b) = pair();
    play_timed(&mut w, &mut b, &[("e2e4", 1_500), ("e7e5", 10_000), ("g1f3", 500)]);
    for s in [&w, &b] {
        // Base, minus thinking, plus an increment per move made.
        assert_eq!(s.chain().remaining_ms(Color::White), 300_000 - 2_000 + 2 * 2_000);
        assert_eq!(s.chain().remaining_ms(Color::Black), 300_000 - 10_000 + 2_000);
    }
}

#[test]
fn a_move_after_the_time_ran_out_ends_the_game_on_time() {
    let (mut w, mut b) = pair();
    play_timed(&mut w, &mut b, &[("e2e4", 1_000), ("e7e5", 1_000)]);
    let left = b.chain().remaining_ms(Color::White) as u32;
    // Exactly all the time left is still in time.
    let (mut w2, mut b2) = (w.clone(), b.clone());
    play_timed(&mut w2, &mut b2, &[("g1f3", left)]);
    assert_eq!(b2.chain().remaining_ms(Color::White), 2_000);
    // One millisecond more is not: the message is logged, the move is not played.
    let msg = w.play(mv("g1f3"), left + 1).unwrap();
    assert_eq!(w.chain().timed_out(), Some(Color::White));
    assert_eq!(b.receive(&msg.encode()), Ok(Received::OutOfTime));
    for s in [&w, &b] {
        assert_eq!(s.chain().timed_out(), Some(Color::White));
        assert_eq!(s.chain().ply(), 3);
        assert_eq!(s.chain().game().moves().len(), 2);
        assert!(s.chain().is_over());
    }
    assert_eq!(w.chain().head(), b.chain().head());
    // Nothing more is accepted.
    assert_eq!(b.play(mv("g8f6"), 0), Err(Reject::NotSendersTurn));
    assert_eq!(w.flag().unwrap_err(), Reject::GameOver);
    let log = w.chain().log().to_vec();
    let chain = replay(GAME, Position::starting(), key(1).verifying_key(), key(2).verifying_key(), TC, &log).unwrap();
    assert_eq!(chain.timed_out(), Some(Color::White));
}

#[test]
fn a_flag_concedes_on_time() {
    let (mut w, mut b) = pair();
    play_timed(&mut w, &mut b, &[("e2e4", 1_000)]);
    // Only on the sender's own turn.
    assert_eq!(w.flag(), Err(Reject::NotSendersTurn));
    let c = w.chain();
    let early = SignalMsg::signed(Signal::Flag, &key(1), GAME, c.ply() + 1, c.head());
    refused(&mut b, &early.encode(), Reject::NotSendersTurn);
    // Signed by someone else, or for another place in the chain.
    let (ply, head) = (w.chain().ply() + 1, w.chain().head());
    let before = state(&w);
    let forged = SignalMsg::signed(Signal::Flag, &key(3), GAME, ply, head);
    assert_eq!(w.receive(&forged.encode()), Err(Reject::BadSignature));
    let stale = SignalMsg::signed(Signal::Flag, &key(2), GAME, ply, Hash([0; 32]));
    assert_eq!(w.receive(&stale.encode()), Err(Reject::WrongPrevHash));
    assert_eq!(state(&w), before);

    let flag = b.flag().unwrap();
    assert_eq!(w.receive(&flag.encode()), Ok(Received::OutOfTime));
    for s in [&w, &b] {
        assert_eq!(s.chain().timed_out(), Some(Color::Black));
        assert_eq!(s.chain().log().last(), Some(&Msg::Signal(flag.clone())));
    }
    assert_eq!(w.chain().head(), b.chain().head());
    assert_eq!(w.receive(&flag.encode()), Err(Reject::WrongPly { expected: 3, got: 2 }));
    let log = w.chain().log().to_vec();
    let chain = replay(GAME, Position::starting(), key(1).verifying_key(), key(2).verifying_key(), TC, &log).unwrap();
    assert_eq!(chain.timed_out(), Some(Color::Black));
    assert_eq!(chain.head(), w.chain().head());
}

#[test]
fn the_time_control_is_bound_into_the_chain() {
    let (mut w, _) = pair();
    let other = TimeControl { base_ms: 60_000, ..TC };
    let mut b = Session::new(GAME, Position::starting(), Color::Black, key(2), key(1).verifying_key(), other);
    let msg = w.play(mv("e2e4"), 0).unwrap();
    assert_eq!(b.receive(&msg.encode()), Err(Reject::WrongPrevHash));
}

/// Replays `s`'s log offline and checks it reaches the same head.
fn replays(s: &Session) -> chess_verify::Chain {
    let log = s.chain().log().to_vec();
    let chain = replay(GAME, Position::starting(), key(1).verifying_key(), key(2).verifying_key(), TC, &log).unwrap();
    assert_eq!(chain.head(), s.chain().head());
    chain
}

#[test]
fn a_resignation_ends_the_game() {
    let (mut w, mut b) = pair();
    play(&mut w, &mut b, &["e2e4"]);
    // Only on the sender's own turn, and signed by the sender.
    assert_eq!(w.signal(Signal::Resign), Err(Reject::NotSendersTurn));
    let (ply, head) = (w.chain().ply() + 1, w.chain().head());
    let forged = SignalMsg::signed(Signal::Resign, &key(3), GAME, ply, head);
    assert_eq!(w.receive(&forged.encode()), Err(Reject::BadSignature));

    let resign = b.signal(Signal::Resign).unwrap();
    assert_eq!(w.receive(&resign.encode()), Ok(Received::Resigned));
    for s in [&w, &b] {
        assert_eq!(s.chain().resigned(), Some(Color::Black));
        assert!(s.chain().is_over());
    }
    assert_eq!(w.chain().head(), b.chain().head());
    assert_eq!(w.play(mv("d2d4"), 0).unwrap_err(), Reject::NotSendersTurn);
    assert_eq!(b.play(mv("e7e5"), 0).unwrap_err(), Reject::GameOver);
    assert_eq!(replays(&w).resigned(), Some(Color::Black));
}

#[test]
fn a_draw_offer_is_accepted_on_the_next_turn() {
    let (mut w, mut b) = pair();
    // White offers, then moves; the offer stands.
    let offer = w.signal(Signal::OfferDraw).unwrap();
    assert_eq!(b.receive(&offer.encode()), Ok(Received::DrawOffered));
    // One offer at a time, and the offer does not pass the turn.
    assert_eq!(w.signal(Signal::OfferDraw), Err(Reject::OfferStands));
    assert_eq!(b.signal(Signal::AcceptDraw), Err(Reject::NotSendersTurn));
    play(&mut w, &mut b, &["e2e4"]);
    assert_eq!(b.chain().draw_offer(), Some(Color::White));
    // Black takes it instead of moving.
    let accept = b.signal(Signal::AcceptDraw).unwrap();
    assert_eq!(w.receive(&accept.encode()), Ok(Received::DrawAgreed));
    for s in [&w, &b] {
        assert!(s.chain().agreed_draw());
        assert!(s.chain().is_over());
    }
    assert_eq!(w.chain().head(), b.chain().head());
    assert!(replays(&w).agreed_draw());
}

#[test]
fn moving_declines_a_draw_offer() {
    let (mut w, mut b) = pair();
    let offer = w.signal(Signal::OfferDraw).unwrap();
    b.receive(&offer.encode()).unwrap();
    play(&mut w, &mut b, &["e2e4", "e7e5"]);
    assert_eq!(w.chain().draw_offer(), None);
    // Too late to accept now, and an accept that was never offered is refused.
    let (ply, head) = (b.chain().ply() + 1, b.chain().head());
    let late = SignalMsg::signed(Signal::AcceptDraw, &key(1), GAME, ply, head);
    refused(&mut b, &late.encode(), Reject::NoDrawOffer);
    assert_eq!(w.signal(Signal::AcceptDraw), Err(Reject::NoDrawOffer));
    // Black's own offer, made on its turn, Black cannot take.
    play(&mut w, &mut b, &["g1f3"]);
    let offer = b.signal(Signal::OfferDraw).unwrap();
    w.receive(&offer.encode()).unwrap();
    assert_eq!(b.signal(Signal::AcceptDraw), Err(Reject::NoDrawOffer));
    assert!(!replays(&w).agreed_draw());
}
