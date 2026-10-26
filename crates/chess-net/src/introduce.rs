//! Learning the other player's address without the server's help.
//!
//! Anyone who knows a nameplate can join it, and the numbers are small, so
//! the server must never hand a joiner the host's address. Instead the two
//! players talk through the server, which forwards opaque `MSG <hex>` lines:
//!
//! ```text
//! 1. SPAKE2    host is side A, joiner side B, password = the whole code
//!              (the words never reach the server)
//! 2. addresses each sends "<own public addr> <own private addr>", sealed
//!              with ChaCha20-Poly1305 under a key from the SPAKE2 secret
//! ```
//!
//! A player who does not know the words ends up with a different secret and
//! cannot open the other's message, so they learn nothing, and an attempt
//! costs them one guess at the two words. The pairing token the hello and
//! the relay use comes from the same secret, so the server does not know it
//! either. The game's own handshake later runs SPAKE2 again, over the
//! connection itself.

use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use sha2::{Digest, Sha256};
use spake2::{Ed25519Group, Identity, Password, Spake2};
use tokio::io::BufReader;
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::line;
use crate::{Peer, Role};

/// The whole introduction, once the server has said `PAIRED`.
const WAIT: Duration = Duration::from_secs(30);

/// What a failed introduction says when the other side could not be told
/// from a stranger: a typo in the code, or someone guessing. It comes back
/// as an [`io::ErrorKind::PermissionDenied`] error.
pub const WRONG_CODE: &str = "the game codes did not match: check the code, or someone tried to listen in";

/// Runs the introduction over the server connection `conn`. `you` is this
/// client's address as the server saw it, `private` its own.
pub(crate) async fn introduce(
    conn: &mut BufReader<TcpStream>,
    role: Role,
    code: &str,
    you: SocketAddr,
    private: SocketAddr,
) -> io::Result<Peer> {
    timeout(WAIT, steps(conn, role, code, you, private))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "the other player stopped answering"))?
}

async fn steps(
    conn: &mut BufReader<TcpStream>,
    role: Role,
    code: &str,
    you: SocketAddr,
    private: SocketAddr,
) -> io::Result<Peer> {
    let password = Password::new(code.as_bytes());
    let (id_a, id_b) = (
        Identity::new(b"chess-p2p rendezvous host"),
        Identity::new(b"chess-p2p rendezvous join"),
    );
    let (spake, message) = match role {
        Role::Host => Spake2::<Ed25519Group>::start_a(&password, &id_a, &id_b),
        Role::Join => Spake2::<Ed25519Group>::start_b(&password, &id_a, &id_b),
    };
    send(conn, &message).await?;
    let theirs = recv(conn).await?;
    let shared = spake
        .finish(&theirs)
        .map_err(|_| line::invalid("bad SPAKE2 message"))?;
    let key = sha256(b"chess-p2p/v1/rendezvous-key", &shared);
    let token = hex(&sha256(b"chess-p2p/v1/rendezvous-token", &shared)[..16]);
    let cipher = ChaCha20Poly1305::new((&key).into());

    let mine = format!("{you} {private}");
    let sealed = cipher
        .encrypt(&nonce(role), Payload { msg: mine.as_bytes(), aad: &[role_byte(role)] })
        .map_err(|_| line::invalid("could not seal the address"))?;
    send(conn, &sealed).await?;

    let sealed = recv(conn).await?;
    let other = other(role);
    let opened = cipher
        .decrypt(&nonce(other), Payload { msg: &sealed, aad: &[role_byte(other)] })
        .map_err(|_| io::Error::new(io::ErrorKind::PermissionDenied, WRONG_CODE))?;
    let text = String::from_utf8(opened).map_err(|_| line::invalid("address is not UTF-8"))?;
    let addr = |s: &str| s.parse().map_err(|_| line::invalid("bad address"));
    match text.split(' ').collect::<Vec<_>>()[..] {
        [public, private] => Ok(Peer { token, you, public: addr(public)?, private: addr(private)? }),
        _ => Err(line::invalid("bad address message")),
    }
}

fn other(role: Role) -> Role {
    match role {
        Role::Host => Role::Join,
        Role::Join => Role::Host,
    }
}

fn role_byte(role: Role) -> u8 {
    match role {
        Role::Host => 1,
        Role::Join => 2,
    }
}

/// One key is used once in each direction, so the role makes the nonce.
fn nonce(role: Role) -> Nonce {
    let mut n = [0u8; 12];
    n[11] = role_byte(role);
    n.into()
}

async fn send(conn: &mut BufReader<TcpStream>, bytes: &[u8]) -> io::Result<()> {
    line::write(conn.get_mut(), &format!("MSG {}", hex(bytes))).await
}

async fn recv(conn: &mut BufReader<TcpStream>) -> io::Result<Vec<u8>> {
    let text = line::expect(conn).await?;
    if let Some(reason) = text.strip_prefix("ERR ") {
        return Err(io::Error::other(format!("rendezvous server: {reason}")));
    }
    text.strip_prefix("MSG ")
        .and_then(unhex)
        .ok_or_else(|| line::invalid(format!("unexpected message: {text}")))
}

fn sha256(tag: &[u8], data: &[u8]) -> [u8; 32] {
    Sha256::new().chain_update(tag).chain_update(data).finalize().into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || !s.is_ascii() {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}
