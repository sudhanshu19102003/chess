//! Property tests over random games and random bytes.

use chess_core::{Color, Position};
use chess_verify::{GameId, MAX_MESSAGE_LEN, Msg, MoveMsg, Received, Reject, Session, SigningKey, TimeControl, replay};
use proptest::prelude::*;

const GAME: GameId = GameId([4; 16]);
const TC: TimeControl = TimeControl { base_ms: 600_000, increment_ms: 1_000 };

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

/// Plays a random game between two sessions, choosing each move and its
/// thinking time by the next number in `choices`, and returns the sessions
/// and every message sent.
fn random_game(choices: &[usize]) -> (Session, Session, Vec<MoveMsg>) {
    let start = Position::starting();
    let mut white = Session::new(GAME, start.clone(), Color::White, key(1), key(2).verifying_key(), TC);
    let mut black = Session::new(GAME, start, Color::Black, key(2), key(1).verifying_key(), TC);
    let mut sent = Vec::new();
    for &c in choices {
        if white.chain().game().outcome().is_some() {
            break;
        }
        let moves = white.chain().game().position().legal_moves();
        let mv = moves[c % moves.len()];
        let (mover, other) = match white.chain().game().position().turn() {
            Color::White => (&mut white, &mut black),
            Color::Black => (&mut black, &mut white),
        };
        let think_ms = (c % 5_000) as u32;
        let msg = mover.play(mv, think_ms).unwrap();
        assert_eq!(other.receive(&msg.encode()), Ok(Received::Move { mv, think_ms }));
        sent.push(msg);
    }
    (white, black, sent)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn honest_peers_agree(choices in prop::collection::vec(any::<usize>(), 0..120)) {
        let (white, black, sent) = random_game(&choices);
        prop_assert_eq!(white.chain().head(), black.chain().head());
        prop_assert_eq!(white.chain().game().position(), black.chain().game().position());
        let log: Vec<Msg> = sent.into_iter().map(Msg::Move).collect();
        prop_assert_eq!(white.chain().log(), &log[..]);
        prop_assert_eq!(white.chain().remaining_ms(Color::Black), black.chain().remaining_ms(Color::Black));
        let chain = replay(GAME, Position::starting(), key(1).verifying_key(), key(2).verifying_key(), TC, &log).unwrap();
        prop_assert_eq!(chain.head(), white.chain().head());
    }

    /// Changing any one byte of an honest message gets it refused, with the
    /// receiver's state untouched.
    #[test]
    fn any_altered_byte_is_refused(
        choices in prop::collection::vec(any::<usize>(), 1..60),
        at in any::<prop::sample::Index>(),
        flip in 1..=255u8,
    ) {
        let (_, _, mut sent) = random_game(&choices);
        let last = sent.pop().unwrap();
        // Rebuild the receiver as it was before the last message.
        let receiver_is_white = last.ply % 2 == 0;
        let start = Position::starting();
        let mut fresh = if receiver_is_white {
            Session::new(GAME, start, Color::White, key(1), key(2).verifying_key(), TC)
        } else {
            Session::new(GAME, start, Color::Black, key(2), key(1).verifying_key(), TC)
        };
        for msg in &sent {
            if (msg.ply % 2 == 1) == receiver_is_white {
                fresh.play(msg.mv, msg.think_ms).unwrap();
            } else {
                fresh.receive(&msg.encode()).unwrap();
            }
        }
        let mut bytes = last.encode();
        let i = at.index(bytes.len());
        bytes[i] ^= flip;
        let before = (fresh.chain().head(), fresh.chain().ply());
        let result = fresh.receive(&bytes);
        prop_assert!(result.is_err(), "altered byte {} accepted", i);
        let mismatch = matches!(result, Err(Reject::HashMismatch { .. }));
        prop_assert!(!mismatch, "altered byte {} reached the hash check", i);
        prop_assert_eq!((fresh.chain().head(), fresh.chain().ply()), before);
        // And the original still goes through.
        prop_assert_eq!(fresh.receive(&last.encode()), Ok(Received::Move { mv: last.mv, think_ms: last.think_ms }));
    }

    /// The parser never panics, and anything it accepts is the one
    /// canonical encoding of what it parsed.
    #[test]
    fn parser_is_strict(bytes in prop::collection::vec(any::<u8>(), 0..MAX_MESSAGE_LEN + 8)) {
        if let Ok(msg) = Msg::decode(&bytes) {
            prop_assert_eq!(msg.encode(), bytes);
        }
    }

    /// Random bytes that keep a valid header and move field, so the parser
    /// gets past the first few checks.
    #[test]
    fn parser_accepts_well_formed(
        id in any::<[u8; 16]>(), ply in 1..u32::MAX, prev in any::<[u8; 32]>(),
        post in any::<[u8; 32]>(), sig in prop::collection::vec(any::<u8>(), 64),
        uci in "[a-h][1-8][a-h][1-8][qrbn]?", think in any::<u32>(),
    ) {
        let bytes = [&[1u8][..], &id, &ply.to_be_bytes(), &prev, &[uci.len() as u8], uci.as_bytes(), &think.to_be_bytes(), &post, &sig].concat();
        let msg = Msg::decode(&bytes).unwrap();
        prop_assert_eq!(msg.encode(), bytes);
    }
}
