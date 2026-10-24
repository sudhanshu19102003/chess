//! The move protocol both peers run: message format, hash chain,
//! signatures, the ten checks, the agreed clock, resignation and draws by
//! agreement. No networking; the bytes
//! come and go through whatever transport the caller has.
//!
//! Each client runs the same deterministic state machine ([`Chain`]) and
//! applies an opponent's move only if every check passes. The first failure
//! rejects the message and leaves the state unchanged. The accepted messages
//! form a signed, hash-chained log that anyone holding the two public keys
//! can check offline with [`replay`].

mod chain;
mod hash;
mod wire;

pub use chain::{Chain, Received, Reject, Session, replay};
pub use ed25519_dalek::{SigningKey, VerifyingKey};
pub use hash::{Hash, genesis, link, position_hash, signal_link};
pub use wire::{GameId, MAX_MESSAGE_LEN, Msg, MoveMsg, ParseError, Signal, SignalMsg};

/// Bumped whenever the wire format or anything hashed changes. Peers with
/// different versions must refuse to play.
pub const PROTOCOL_VERSION: u8 = 1;

/// Base time and increment, agreed in the handshake and bound into the
/// genesis hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeControl {
    pub base_ms: u32,
    pub increment_ms: u32,
}

impl TimeControl {
    /// At most a day's base and an hour's increment, and some base.
    pub fn is_sane(self) -> bool {
        (1..=24 * 3600 * 1000).contains(&self.base_ms) && self.increment_ms <= 3600 * 1000
    }
}
