//! Arbitrary bytes into the parser and, if they parse, into a session.

#![no_main]

use chess_core::{Color, Position};
use chess_verify::{GameId, Msg, Session, SigningKey, TimeControl};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|bytes: &[u8]| {
    if let Ok(msg) = Msg::decode(bytes) {
        assert_eq!(msg.encode(), bytes);
    }
    let opponent = SigningKey::from_bytes(&[1; 32]).verifying_key();
    let mut session = Session::new(GameId([0; 16]), Position::starting(), Color::Black, SigningKey::from_bytes(&[2; 32]), opponent, TimeControl { base_ms: 60_000, increment_ms: 0 });
    let _ = session.receive(bytes);
});
