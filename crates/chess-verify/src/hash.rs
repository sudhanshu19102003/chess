//! Position hashes and the hash chain. Everything is SHA-256 over a domain
//! tag followed by fixed-size fields, with variable-length fields prefixed
//! by their length, so no two different inputs encode the same.

use std::fmt;

use chess_core::{Move, Position};
use ed25519_dalek::VerifyingKey;
use sha2::{Digest, Sha256};

use crate::{GameId, Signal, TimeControl};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Hash(pub [u8; 32]);

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.0.iter().try_for_each(|b| write!(f, "{b:02x}"))
    }
}

impl fmt::Debug for Hash {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Hash({self})")
    }
}

fn sha256(tag: &[u8], parts: &[&[u8]]) -> Hash {
    let mut h = Sha256::new();
    h.update(tag);
    for p in parts {
        h.update(p);
    }
    Hash(h.finalize().into())
}

/// `bytes` with a two-byte big-endian length in front.
fn prefixed(bytes: &[u8]) -> Vec<u8> {
    let len = u16::try_from(bytes.len()).expect("hashed field fits in 64 KiB");
    [&len.to_be_bytes()[..], bytes].concat()
}

/// Covers pieces, side to move, castling rights, en passant square and both
/// move counters, by hashing the FEN. `Position::fen` is canonical (one
/// string per position, the en passant square only when a capture exists),
/// so equal positions hash equal and different ones differ.
pub fn position_hash(pos: &Position) -> Hash {
    sha256(b"chess-p2p/v1/position", &[&prefixed(pos.fen().as_bytes())])
}

/// The chain head before any move: binds the game, the starting position,
/// both players' keys and the time control, so the log cannot be replayed
/// under other keys or another clock.
pub fn genesis(
    game_id: &GameId,
    start: &Position,
    white: &VerifyingKey,
    black: &VerifyingKey,
    time_control: TimeControl,
) -> Hash {
    sha256(
        b"chess-p2p/v1/genesis",
        &[
            &game_id.0,
            &position_hash(start).0,
            white.as_bytes(),
            black.as_bytes(),
            &time_control.base_ms.to_be_bytes(),
            &time_control.increment_ms.to_be_bytes(),
        ],
    )
}

/// The next chain head after a move: `H(head, move, think_ms, post_hash)`.
pub fn link(head: &Hash, mv: Move, think_ms: u32, post_hash: &Hash) -> Hash {
    sha256(
        b"chess-p2p/v1/link",
        &[&head.0, &prefixed(mv.uci().as_bytes()), &think_ms.to_be_bytes(), &post_hash.0],
    )
}

/// The chain head after a signal: `H(head)` under a tag for its kind.
pub fn signal_link(kind: Signal, head: &Hash) -> Hash {
    let tag: &[u8] = match kind {
        Signal::Flag => b"chess-p2p/v1/flag",
        Signal::Resign => b"chess-p2p/v1/resign",
        Signal::OfferDraw => b"chess-p2p/v1/draw-offer",
        Signal::AcceptDraw => b"chess-p2p/v1/draw-accept",
    };
    sha256(tag, &[&head.0])
}
