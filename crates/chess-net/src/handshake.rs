//! The handshake two players run over a fresh connection, direct or
//! relayed. It ends with an encrypted channel and everything a
//! [`chess_verify::Session`] needs.
//!
//! ```text
//! 1. versions   both send "chess-p2p handshake=.. verify=.. engine=..";
//!               any difference ends it with Error::Version
//! 2. SPAKE2     host is side A, joiner side B, password = the whole game
//!               code; both get the same key only if they typed the same code
//! 3. Noise      NNpsk0_25519_ChaChaPoly_BLAKE2s, psk from the SPAKE2 key,
//!               prologue = the version line; the host initiates
//!      -> psk, e      payload: host hello (signing key, commitment,
//!                     time control, colour choice)
//!      <- e, ee       payload: joiner hello (signing key, commitment)
//! 4. reveal     both send their coin nonce over the channel and check the
//!               other's against its commitment
//! ```
//!
//! The server only ever saw the nameplate, so it cannot run SPAKE2 itself;
//! anyone in the middle gets one guess at the two words per attempt. A
//! wrong code fails the first Noise message, and the joiner sends an abort
//! frame so both sides can say so.
//!
//! Colours: the host may choose its colour. Otherwise the host plays White
//! if the first bit of `H(coin tag, host nonce, joiner nonce)` is 0. Each
//! side commits before seeing the other's nonce, so neither can choose the
//! result, only walk away from it. The game id is hashed from the Noise
//! transcript and both nonces, so it is fresh for every game and covers the
//! time control and the colour choice.

use std::fmt;
use std::io;
use std::time::Duration;

use chess_core::Color;
use chess_verify::{GameId, SigningKey, VerifyingKey};
pub use chess_verify::TimeControl;
use sha2::{Digest, Sha256};
use spake2::{Ed25519Group, Identity, Password, Spake2};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::timeout;

use crate::Role;
use crate::frame::{Frame, Framed, MAX_FRAME};

/// Bumped whenever this handshake changes.
pub const HANDSHAKE_VERSION: u32 = 1;
const NOISE_PARAMS: &str = "Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s";
/// The whole handshake. Nobody types during it, so it is quick or broken.
const HANDSHAKE_WAIT: Duration = Duration::from_secs(30);
const TAG_LEN: usize = 16;

const HOST_HELLO: u8 = 1;
const JOIN_HELLO: u8 = 2;
const REVEAL: u8 = 3;
/// kind, signing key, commitment, base ms, increment ms, colour choice.
const HOST_HELLO_LEN: usize = 1 + 32 + 32 + 4 + 4 + 1;
/// The colour choice byte: a coin flip, or the host's colour.
const FLIP: u8 = 0;
const HOST_WHITE: u8 = 1;
const HOST_BLACK: u8 = 2;
/// kind, signing key, commitment.
const JOIN_HELLO_LEN: usize = 1 + 32 + 32;
/// kind, nonce.
const REVEAL_LEN: usize = 1 + 32;

/// A finished handshake.
pub struct Established<S> {
    pub channel: Channel<S>,
    pub game_id: GameId,
    pub me: Color,
    /// Made fresh for this game; signs our moves.
    pub key: SigningKey,
    pub opponent: VerifyingKey,
    pub time_control: TimeControl,
}

impl<S> fmt::Debug for Established<S> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Established")
            .field("me", &self.me)
            .field("time_control", &self.time_control)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    /// The peer runs a different release.
    Version {
        ours: String,
        theirs: String,
    },
    /// The two codes differ (a typo), or someone in the middle guessed.
    WrongCode,
    /// The peer broke the protocol, or a message was altered on the way.
    Protocol(&'static str),
    TimedOut,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "connection failed during the handshake: {e}"),
            Error::Version { ours, theirs } => write!(
                f,
                "the other player runs a different version ({theirs}; this is {ours}); \
                 both need the same release"
            ),
            Error::WrongCode => f.write_str(
                "the game codes did not match: check the code, or someone tried to listen in",
            ),
            Error::Protocol(what) => write!(f, "handshake failed: {what}"),
            Error::TimedOut => {
                f.write_str("the other player stopped answering during the handshake")
            }
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Error {
        Error::Io(e)
    }
}

/// The version line both sides must agree on.
pub fn version() -> String {
    format!(
        "chess-p2p handshake={HANDSHAKE_VERSION} verify={} engine={}",
        chess_verify::PROTOCOL_VERSION,
        chess_core::ENGINE_VERSION
    )
}

/// What the host offers: the time control, and its own colour or `None`
/// for a coin flip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Offer {
    pub time_control: TimeControl,
    pub colour: Option<Color>,
}

/// Runs the host's side with the code it showed and its offer.
pub async fn host<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    code: &str,
    offer: Offer,
) -> Result<Established<S>, Error> {
    assert!(offer.time_control.is_sane(), "{offer:?}");
    run(stream, code, Some(offer)).await
}

/// Runs the joiner's side with the code the player typed.
pub async fn join<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    code: &str,
) -> Result<Established<S>, Error> {
    run(stream, code, None).await
}

/// `offer` is the host's; `None` for the joiner.
async fn run<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    code: &str,
    offer: Option<Offer>,
) -> Result<Established<S>, Error> {
    timeout(HANDSHAKE_WAIT, steps(stream, code, offer))
        .await
        .map_err(|_| Error::TimedOut)?
}

async fn steps<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    code: &str,
    offer: Option<Offer>,
) -> Result<Established<S>, Error> {
    let role = if offer.is_some() {
        Role::Host
    } else {
        Role::Join
    };
    let mut framed = Framed::new(stream);

    // 1. Versions, in the clear, so a mismatch gets a plain message.
    let ours = version();
    framed.send(ours.as_bytes()).await?;
    let theirs = recv(&mut framed, 256).await?;
    if theirs != ours.as_bytes() {
        let theirs = String::from_utf8_lossy(&theirs).into_owned();
        return Err(Error::Version { ours, theirs });
    }

    // 2. SPAKE2 on the code.
    let password = Password::new(code.as_bytes());
    let (id_a, id_b) = (
        Identity::new(b"chess-p2p host"),
        Identity::new(b"chess-p2p join"),
    );
    let (spake, message) = match role {
        Role::Host => Spake2::<Ed25519Group>::start_a(&password, &id_a, &id_b),
        Role::Join => Spake2::<Ed25519Group>::start_b(&password, &id_a, &id_b),
    };
    framed.send(&message).await?;
    let their_message = recv(&mut framed, 64).await?;
    let shared = spake
        .finish(&their_message)
        .map_err(|_| Error::Protocol("bad SPAKE2 message"))?;
    let psk = sha256(b"chess-p2p/v1/noise-psk", &[&shared]);

    // 3. Noise, carrying the hellos.
    let builder = snow::Builder::new(NOISE_PARAMS.parse().expect("valid Noise parameters"))
        .psk(0, &psk)
        .and_then(|b| b.prologue(ours.as_bytes()))
        .map_err(|_| Error::Protocol("Noise setup"))?;
    let mut noise = match role {
        Role::Host => builder.build_initiator(),
        Role::Join => builder.build_responder(),
    }
    .map_err(|_| Error::Protocol("Noise setup"))?;

    let key = SigningKey::from_bytes(&random32());
    let nonce = random32();
    let commit = commitment(role, &nonce);
    let mut buf = vec![0u8; MAX_FRAME];

    let (Offer { time_control, colour }, opponent, their_commit) = match role {
        Role::Host => {
            let offer = offer.unwrap();
            let tc = offer.time_control;
            let mut hello = vec![HOST_HELLO];
            hello.extend_from_slice(key.verifying_key().as_bytes());
            hello.extend_from_slice(&commit);
            hello.extend_from_slice(&tc.base_ms.to_be_bytes());
            hello.extend_from_slice(&tc.increment_ms.to_be_bytes());
            hello.push(match offer.colour {
                None => FLIP,
                Some(Color::White) => HOST_WHITE,
                Some(Color::Black) => HOST_BLACK,
            });
            let n = noise
                .write_message(&hello, &mut buf)
                .map_err(|_| Error::Protocol("Noise write"))?;
            framed.send(&buf[..n]).await?;

            let reply = recv(&mut framed, 256).await?;
            let n = noise
                .read_message(&reply, &mut buf)
                .map_err(|_| Error::Protocol("a handshake message was altered"))?;
            let hello = &buf[..n];
            if hello.len() != JOIN_HELLO_LEN || hello[0] != JOIN_HELLO {
                return Err(Error::Protocol("bad joiner hello"));
            }
            (offer, signing_key(&hello[1..33])?, to32(&hello[33..65]))
        }
        Role::Join => {
            let first = recv(&mut framed, 256).await?;
            let Ok(n) = noise.read_message(&first, &mut buf) else {
                // The psk is wrong, so the codes differ. Tell the host.
                let _ = framed.send_abort().await;
                return Err(Error::WrongCode);
            };
            let hello = &buf[..n];
            if hello.len() != HOST_HELLO_LEN || hello[0] != HOST_HELLO {
                return Err(Error::Protocol("bad host hello"));
            }
            let tc = TimeControl {
                base_ms: u32::from_be_bytes(hello[65..69].try_into().unwrap()),
                increment_ms: u32::from_be_bytes(hello[69..73].try_into().unwrap()),
            };
            if !tc.is_sane() {
                return Err(Error::Protocol("unreasonable time control"));
            }
            let colour = match hello[73] {
                FLIP => None,
                HOST_WHITE => Some(Color::White),
                HOST_BLACK => Some(Color::Black),
                _ => return Err(Error::Protocol("bad colour choice")),
            };
            let offer = Offer {
                time_control: tc,
                colour,
            };
            let found = (offer, signing_key(&hello[1..33])?, to32(&hello[33..65]));

            let mut hello = vec![JOIN_HELLO];
            hello.extend_from_slice(key.verifying_key().as_bytes());
            hello.extend_from_slice(&commit);
            let n = noise
                .write_message(&hello, &mut buf)
                .map_err(|_| Error::Protocol("Noise write"))?;
            framed.send(&buf[..n]).await?;
            found
        }
    };
    if opponent == key.verifying_key() || their_commit == commit {
        return Err(Error::Protocol("the peer echoed our own hello"));
    }
    let transcript = noise.get_handshake_hash().to_vec();
    let transport = noise
        .into_transport_mode()
        .map_err(|_| Error::Protocol("Noise handshake incomplete"))?;
    let mut channel = Channel {
        framed,
        noise: transport,
    };

    // 4. Reveal and flip.
    let mut reveal = vec![REVEAL];
    reveal.extend_from_slice(&nonce);
    channel.send(&reveal).await?;
    let theirs = channel.recv().await.map_err(|e| match e.kind() {
        io::ErrorKind::InvalidData => Error::Protocol("a message was altered"),
        _ => Error::Io(e),
    })?;
    if theirs.len() != REVEAL_LEN || theirs[0] != REVEAL {
        return Err(Error::Protocol("bad reveal"));
    }
    let their_nonce = to32(&theirs[1..]);
    let other_role = match role {
        Role::Host => Role::Join,
        Role::Join => Role::Host,
    };
    if commitment(other_role, &their_nonce) != their_commit {
        return Err(Error::Protocol("the reveal does not match the commitment"));
    }
    let (host_nonce, join_nonce) = match role {
        Role::Host => (nonce, their_nonce),
        Role::Join => (their_nonce, nonce),
    };
    let coin = sha256(b"chess-p2p/v1/coin", &[&host_nonce, &join_nonce])[0] & 1;
    let host_color = colour.unwrap_or(if coin == 0 {
        Color::White
    } else {
        Color::Black
    });
    let me = match role {
        Role::Host => host_color,
        Role::Join => host_color.opponent(),
    };
    let id = sha256(
        b"chess-p2p/v1/game-id",
        &[&transcript, &host_nonce, &join_nonce],
    );
    Ok(Established {
        channel,
        game_id: GameId(id[..16].try_into().unwrap()),
        me,
        key,
        opponent,
        time_control,
    })
}

/// An encrypted, authenticated channel to the opponent. Messages arrive in
/// order and unaltered, or `recv` fails.
pub struct Channel<S> {
    framed: Framed<S>,
    noise: snow::TransportState,
}

/// Longest message [`Channel::send`] takes.
pub const MAX_MESSAGE: usize = MAX_FRAME - TAG_LEN;

impl<S: AsyncRead + AsyncWrite + Unpin> Channel<S> {
    pub async fn send(&mut self, message: &[u8]) -> io::Result<()> {
        assert!(message.len() <= MAX_MESSAGE);
        let mut buf = vec![0u8; message.len() + TAG_LEN];
        let n = self
            .noise
            .write_message(message, &mut buf)
            .map_err(|_| io::Error::other("Noise write"))?;
        self.framed.send(&buf[..n]).await
    }

    /// The next message. Cancel-safe. A message that fails to decrypt is an
    /// `InvalidData` error, and the channel is no use after it.
    pub async fn recv(&mut self) -> io::Result<Vec<u8>> {
        let Frame::Data(frame) = self.framed.recv(MAX_FRAME).await? else {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "abort frame"));
        };
        let mut buf = vec![0u8; frame.len()];
        let n = self
            .noise
            .read_message(&frame, &mut buf)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "message failed to decrypt"))?;
        buf.truncate(n);
        Ok(buf)
    }
}

/// A handshake frame; an abort means the other side found the codes differ.
async fn recv<S: AsyncRead + AsyncWrite + Unpin>(
    framed: &mut Framed<S>,
    max: usize,
) -> Result<Vec<u8>, Error> {
    match framed.recv(max).await? {
        Frame::Data(bytes) => Ok(bytes),
        Frame::Abort => Err(Error::WrongCode),
    }
}

fn commitment(role: Role, nonce: &[u8; 32]) -> [u8; 32] {
    let role = [match role {
        Role::Host => 0,
        Role::Join => 1,
    }];
    sha256(b"chess-p2p/v1/coin-commit", &[&role, nonce])
}

/// A peer's public signing key; refuses invalid and small-order points.
fn signing_key(bytes: &[u8]) -> Result<VerifyingKey, Error> {
    VerifyingKey::from_bytes(&to32(bytes))
        .ok()
        .filter(|k| !k.is_weak())
        .ok_or(Error::Protocol("bad signing key"))
}

fn sha256(tag: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(tag);
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

fn random32() -> [u8; 32] {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("no system randomness");
    bytes
}

fn to32(bytes: &[u8]) -> [u8; 32] {
    bytes.try_into().expect("32 bytes")
}
