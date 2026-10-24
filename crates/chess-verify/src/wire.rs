//! The signed messages and their byte format. Parsing is strict: one valid
//! encoding per message, exact lengths, nothing trailing.
//!
//! ```text
//! Move                               Signal (flag, resign, draw offer, accept)
//! offset  size  field                offset  size  field
//!      0     1  tag = 0x01                0     1  tag = 0x02 to 0x05
//!      1    16  game_id                   1    16  game_id
//!     17     4  ply, big-endian, >= 1    17     4  ply, >= 1
//!     21    32  prev_hash                21    32  prev_hash
//!     53     1  move length, 4 or 5      53    64  sig
//!     54   4-5  move in UCI, lower case
//!      .     4  think_ms, big-endian
//!      .    32  post_hash
//!      .    64  sig
//! ```
//!
//! `ply` is the message's place in the log (the first move is ply 1); a
//! signal takes a place like a move does. A flag (0x02) concedes on time, a
//! resignation (0x03) concedes, a draw offer (0x04) proposes a draw and an
//! accept (0x05) takes the opponent's standing offer. `prev_hash`
//! is the chain head before it, `post_hash` the position hash after the
//! move, and `think_ms` the sender's own measure of how long it took. The
//! signature is ed25519 over a per-kind domain tag followed by every byte
//! before it.

use std::fmt;

use chess_core::Move;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};

use crate::Hash;

const MOVE_TAG: u8 = 0x01;
const MOVE_SIG_TAG: &[u8] = b"chess-p2p/v1/move-sig";

/// The longest message there is (a promotion); anything longer is refused
/// before it is looked at.
pub const MAX_MESSAGE_LEN: usize = 1 + 16 + 4 + 32 + 1 + 5 + 4 + 32 + 64;

/// Random, chosen in the handshake.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct GameId(pub [u8; 16]);

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MoveMsg {
    pub game_id: GameId,
    pub ply: u32,
    pub prev_hash: Hash,
    pub mv: Move,
    /// From receiving the opponent's move (or the start, for White's first
    /// move) to sending this one, by the sender's own clock.
    pub think_ms: u32,
    pub post_hash: Hash,
    pub sig: [u8; 64],
}

/// What a signal says. Every kind is sent on the sender's own turn.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Signal {
    /// The sender ran out of time and concedes.
    Flag,
    /// The sender concedes.
    Resign,
    /// The sender proposes a draw. It stands until the opponent moves.
    OfferDraw,
    /// The sender takes the opponent's standing draw offer.
    AcceptDraw,
}

impl Signal {
    pub const ALL: [Signal; 4] = [Signal::Flag, Signal::Resign, Signal::OfferDraw, Signal::AcceptDraw];

    fn tag(self) -> u8 {
        match self {
            Signal::Flag => 0x02,
            Signal::Resign => 0x03,
            Signal::OfferDraw => 0x04,
            Signal::AcceptDraw => 0x05,
        }
    }

    fn sig_tag(self) -> &'static [u8] {
        match self {
            Signal::Flag => b"chess-p2p/v1/flag-sig",
            Signal::Resign => b"chess-p2p/v1/resign-sig",
            Signal::OfferDraw => b"chess-p2p/v1/draw-offer-sig",
            Signal::AcceptDraw => b"chess-p2p/v1/draw-accept-sig",
        }
    }
}

/// A signal: no move, just its kind, its place in the chain and the
/// sender's signature.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SignalMsg {
    pub kind: Signal,
    pub game_id: GameId,
    pub ply: u32,
    pub prev_hash: Hash,
    pub sig: [u8; 64],
}

/// Any signed message: what goes into the log.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Msg {
    Move(MoveMsg),
    Signal(SignalMsg),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ParseError {
    TooLong,
    /// The length does not match what the header says.
    WrongLength,
    UnknownTag,
    ZeroPly,
    BadMoveLength,
    /// Not UCI, or not its one canonical spelling.
    BadMove,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(match self {
            ParseError::TooLong => "message too long",
            ParseError::WrongLength => "message length does not match its contents",
            ParseError::UnknownTag => "unknown message type",
            ParseError::ZeroPly => "ply 0",
            ParseError::BadMoveLength => "move field is not 4 or 5 bytes",
            ParseError::BadMove => "move is not canonical UCI",
        })
    }
}

impl std::error::Error for ParseError {}

/// The header all kinds share.
fn header(tag: u8, game_id: GameId, ply: u32, prev_hash: Hash) -> Vec<u8> {
    let mut out = Vec::with_capacity(MAX_MESSAGE_LEN);
    out.push(tag);
    out.extend_from_slice(&game_id.0);
    out.extend_from_slice(&ply.to_be_bytes());
    out.extend_from_slice(&prev_hash.0);
    out
}

/// Strict verification: rejects weak keys and non-canonical signatures, so a
/// message has exactly one valid signature.
fn verify(key: &VerifyingKey, tag: &[u8], body: &[u8], sig: &[u8; 64]) -> bool {
    key.verify_strict(&[tag, body].concat(), &Signature::from_bytes(sig)).is_ok()
}

impl MoveMsg {
    /// Builds and signs a message.
    pub fn signed(
        key: &SigningKey,
        game_id: GameId,
        ply: u32,
        prev_hash: Hash,
        mv: Move,
        think_ms: u32,
        post_hash: Hash,
    ) -> MoveMsg {
        let mut msg = MoveMsg { game_id, ply, prev_hash, mv, think_ms, post_hash, sig: [0; 64] };
        msg.sig = key.sign(&[MOVE_SIG_TAG, &msg.body()].concat()).to_bytes();
        msg
    }

    /// Every byte of the encoding except the signature.
    fn body(&self) -> Vec<u8> {
        let uci = self.mv.uci();
        let mut out = header(MOVE_TAG, self.game_id, self.ply, self.prev_hash);
        out.push(uci.len() as u8);
        out.extend_from_slice(uci.as_bytes());
        out.extend_from_slice(&self.think_ms.to_be_bytes());
        out.extend_from_slice(&self.post_hash.0);
        out
    }

    /// Whether `sig` is `key`'s signature over this message.
    pub fn verify(&self, key: &VerifyingKey) -> bool {
        verify(key, MOVE_SIG_TAG, &self.body(), &self.sig)
    }

    pub fn encode(&self) -> Vec<u8> {
        [&self.body()[..], &self.sig].concat()
    }
}

impl SignalMsg {
    pub fn signed(kind: Signal, key: &SigningKey, game_id: GameId, ply: u32, prev_hash: Hash) -> SignalMsg {
        let mut msg = SignalMsg { kind, game_id, ply, prev_hash, sig: [0; 64] };
        msg.sig = key.sign(&[kind.sig_tag(), &msg.body()].concat()).to_bytes();
        msg
    }

    fn body(&self) -> Vec<u8> {
        header(self.kind.tag(), self.game_id, self.ply, self.prev_hash)
    }

    pub fn verify(&self, key: &VerifyingKey) -> bool {
        verify(key, self.kind.sig_tag(), &self.body(), &self.sig)
    }

    pub fn encode(&self) -> Vec<u8> {
        [&self.body()[..], &self.sig].concat()
    }
}

impl Msg {
    pub fn game_id(&self) -> GameId {
        match self {
            Msg::Move(m) => m.game_id,
            Msg::Signal(f) => f.game_id,
        }
    }

    pub fn ply(&self) -> u32 {
        match self {
            Msg::Move(m) => m.ply,
            Msg::Signal(f) => f.ply,
        }
    }

    pub fn prev_hash(&self) -> Hash {
        match self {
            Msg::Move(m) => m.prev_hash,
            Msg::Signal(f) => f.prev_hash,
        }
    }

    pub fn verify(&self, key: &VerifyingKey) -> bool {
        match self {
            Msg::Move(m) => m.verify(key),
            Msg::Signal(f) => f.verify(key),
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        match self {
            Msg::Move(m) => m.encode(),
            Msg::Signal(f) => f.encode(),
        }
    }

    pub fn decode(bytes: &[u8]) -> Result<Msg, ParseError> {
        if bytes.len() > MAX_MESSAGE_LEN {
            return Err(ParseError::TooLong);
        }
        let mut r = Reader(bytes);
        let [tag] = r.take()?;
        let signal = Signal::ALL.into_iter().find(|s| s.tag() == tag);
        if tag != MOVE_TAG && signal.is_none() {
            return Err(ParseError::UnknownTag);
        }
        let game_id = GameId(r.take()?);
        let ply = u32::from_be_bytes(r.take()?);
        if ply == 0 {
            return Err(ParseError::ZeroPly);
        }
        let prev_hash = Hash(r.take()?);
        let msg = if let Some(kind) = signal {
            Msg::Signal(SignalMsg { kind, game_id, ply, prev_hash, sig: r.take()? })
        } else {
            let [len] = r.take()?;
            if !(4..=5).contains(&len) {
                return Err(ParseError::BadMoveLength);
            }
            let uci = r.slice(len as usize)?;
            let mv = std::str::from_utf8(uci).ok().and_then(Move::parse_uci).ok_or(ParseError::BadMove)?;
            if mv.uci().as_bytes() != uci {
                return Err(ParseError::BadMove);
            }
            let think_ms = u32::from_be_bytes(r.take()?);
            let post_hash = Hash(r.take()?);
            Msg::Move(MoveMsg { game_id, ply, prev_hash, mv, think_ms, post_hash, sig: r.take()? })
        };
        if !r.0.is_empty() {
            return Err(ParseError::WrongLength);
        }
        Ok(msg)
    }
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn slice(&mut self, n: usize) -> Result<&'a [u8], ParseError> {
        if self.0.len() < n {
            return Err(ParseError::WrongLength);
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N], ParseError> {
        Ok(self.slice(N)?.try_into().expect("slice has length N"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> SigningKey {
        SigningKey::from_bytes(&[7; 32])
    }

    fn sample(uci: &str) -> MoveMsg {
        MoveMsg::signed(&key(), GameId([1; 16]), 3, Hash([2; 32]), Move::parse_uci(uci).unwrap(), 4321, Hash([3; 32]))
    }

    fn flag() -> SignalMsg {
        signal(Signal::Flag)
    }

    fn signal(kind: Signal) -> SignalMsg {
        SignalMsg::signed(kind, &key(), GameId([1; 16]), 3, Hash([2; 32]))
    }

    #[test]
    fn round_trips() {
        for uci in ["e2e4", "e7e8q", "a2a1n"] {
            let msg = sample(uci);
            let bytes = msg.encode();
            assert_eq!(Msg::decode(&bytes), Ok(Msg::Move(msg)));
        }
        assert_eq!(sample("e7e8q").encode().len(), MAX_MESSAGE_LEN);
        let bytes = flag().encode();
        assert_eq!(bytes.len(), 1 + 16 + 4 + 32 + 64);
        assert_eq!(Msg::decode(&bytes), Ok(Msg::Signal(flag())));
        for kind in Signal::ALL {
            assert_eq!(Msg::decode(&signal(kind).encode()), Ok(Msg::Signal(signal(kind))));
        }
    }

    #[test]
    fn signature_checks() {
        let other = SigningKey::from_bytes(&[8; 32]).verifying_key();
        let msg = sample("e2e4");
        assert!(msg.verify(&key().verifying_key()));
        assert!(!msg.verify(&other));
        assert!(flag().verify(&key().verifying_key()));
        assert!(!flag().verify(&other));
        // The kinds sign under different tags, so neither signature fits the other.
        let mut as_move = flag();
        as_move.sig = sample("e2e4").sig;
        assert!(!as_move.verify(&key().verifying_key()));
        // Nor does one signal's fit another: a draw offer is not a resignation.
        for a in Signal::ALL {
            for b in Signal::ALL.into_iter().filter(|&b| b != a) {
                let mut other = signal(b);
                other.sig = signal(a).sig;
                assert!(!other.verify(&key().verifying_key()));
            }
        }
    }

    #[test]
    fn rejects_malformed() {
        let good = sample("e2e4").encode();
        let with = |i: usize, b: u8| {
            let mut v = good.clone();
            v[i] = b;
            Msg::decode(&v)
        };
        assert_eq!(Msg::decode(&[]), Err(ParseError::WrongLength));
        assert_eq!(Msg::decode(&good[..good.len() - 1]), Err(ParseError::WrongLength));
        assert_eq!(Msg::decode(&[&good[..], &[0]].concat()), Err(ParseError::WrongLength));
        assert_eq!(Msg::decode(&[0; MAX_MESSAGE_LEN + 1]), Err(ParseError::TooLong));
        assert_eq!(with(0, 0), Err(ParseError::UnknownTag));
        assert_eq!(with(0, 6), Err(ParseError::UnknownTag));
        let mut zero_ply = good.clone();
        zero_ply[17..21].copy_from_slice(&[0; 4]);
        assert_eq!(Msg::decode(&zero_ply), Err(ParseError::ZeroPly));
        assert_eq!(with(53, 3), Err(ParseError::BadMoveLength));
        assert_eq!(with(54, b'E'), Err(ParseError::BadMove));
        assert_eq!(with(55, b'9'), Err(ParseError::BadMove));
        // A promotion letter is part of UCI only in lower case.
        let promo = sample("e7e8q").encode();
        let mut upper = promo.clone();
        upper[58] = b'Q';
        assert_eq!(Msg::decode(&upper), Err(ParseError::BadMove));
        // A flag is exactly its header and signature.
        let f = flag().encode();
        assert_eq!(Msg::decode(&f[..f.len() - 1]), Err(ParseError::WrongLength));
        assert_eq!(Msg::decode(&[&f[..], &[0]].concat()), Err(ParseError::WrongLength));
    }
}
