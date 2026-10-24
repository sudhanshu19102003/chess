//! Milestones 3 and 4: a rendezvous server that introduces two players, a
//! client that opens a direct TCP connection between them by simultaneous
//! open (hole punching), and a relay for the pairs where that fails. Each
//! pairing ends with both clients reporting how it went, and the server logs
//! one line per pairing so the direct connection rate can be counted
//! (`chess-rendezvous stats`).
//!
//! Getting connected is plain text lines, unauthenticated: the token only
//! picks the right connection. Milestone 5's [`handshake`] then runs over
//! that connection and authenticates and encrypts everything after it.
//! [`connect`] does all of that in one call for the game, and [`link`]
//! carries a game's messages afterwards.

pub mod client;
pub mod code;
pub mod connect;
mod frame;
pub mod handshake;
mod line;
pub mod link;
pub mod relay;
pub mod server;

pub use client::{
    Connection, Nat, Path, Peer, Rendezvous, Report, Role, Via, measure_rtt, via_relay,
};
pub use handshake::{Channel, Established, TimeControl};
pub use relay::{Limits, serve_relay};
pub use server::{Stats, serve};

/// Default port of the rendezvous server. The NAT probe listens on the next
/// port up.
pub const DEFAULT_PORT: u16 = 7777;
