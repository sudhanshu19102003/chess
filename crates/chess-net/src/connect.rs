//! From a game code to a secure channel, the way the game uses it: meet
//! through the rendezvous server, connect directly or through the relay,
//! report how it went, and run the handshake. Progress is reported as it
//! goes, so a UI can show each stage. Hosting on the local network, or a
//! `lan-` code, goes to [`lan`] instead, with no server.

use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::BufReader;
use tokio::net::TcpStream;

use crate::client::{PUNCH_LIMIT, RELAY_LIMIT};
use crate::code::{new_code, parse_code, parse_lan_code};
use crate::handshake::{self, Established, Offer};
use crate::lan::{self, LAN_PORT};
use crate::relay::DEFAULT_RELAY_PORT;
use crate::{DEFAULT_PORT, DEFAULT_SERVER, Path, Rendezvous, Report, Role, measure_rtt, via_relay};

/// Where the servers are, as typed: `HOST` or `HOST:PORT`.
#[derive(Clone, Debug)]
pub struct Servers {
    pub rendezvous: String,
    /// The relay; by default the rendezvous server's host on port 443.
    pub relay: Option<String>,
    /// Skip the direct attempt (both sides must, or they miss each other).
    pub force_relay: bool,
}

impl Servers {
    /// `$CHESS_RENDEZVOUS` (or [`DEFAULT_SERVER`]) and `$CHESS_RELAY`.
    pub fn from_env() -> Servers {
        Servers {
            rendezvous: std::env::var("CHESS_RENDEZVOUS").unwrap_or_else(|_| DEFAULT_SERVER.into()),
            relay: std::env::var("CHESS_RELAY").ok(),
            force_relay: false,
        }
    }
}

/// Which side of the pairing this client takes.
#[derive(Clone, Debug)]
pub enum Start {
    /// Get a code and offer this time control and colour choice.
    Host(Offer),
    /// Host on the local network, with no server (see [`lan`]).
    HostLan(Offer),
    /// Join with a code the player typed. A `lan-` code is looked for on
    /// the local network.
    Join(String),
}

/// How far connecting has got.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Talking to the rendezvous server.
    Server,
    /// The host's code: tell it to the other player. Waiting for them.
    Code(String),
    /// Introduced; trying a direct connection.
    Punching,
    /// No direct connection; meeting at the relay.
    Relay,
    /// Looking for the host on the local network.
    Searching,
    /// Connected; checking the code and agreeing keys and colours.
    Handshake,
}

/// A secure connection to the other player, ready for a game.
pub struct Linked {
    pub established: Established<BufReader<TcpStream>>,
    pub path: Path,
    /// Median of a few round trips measured right after connecting.
    pub rtt: Duration,
}

#[derive(Debug)]
pub enum Error {
    /// The typed code is not one this program would make.
    BadCode,
    /// The rendezvous server (or the relay's name) could not be reached at
    /// all. `resolved` says whether its name resolved: if it did, the
    /// player's own connection is probably fine and the server is down.
    NoServer { resolved: bool, error: io::Error },
    /// The rendezvous server answered but pairing failed.
    Server(io::Error),
    /// Neither a direct connection nor the relay worked.
    NoConnection(io::Error),
    Handshake(handshake::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::BadCode => f.write_str("that is not a game code (they look like 7-comet-pilot)"),
            Error::NoServer { error, .. } => write!(f, "cannot reach the rendezvous server: {error}"),
            Error::Server(e) => write!(f, "rendezvous server: {e}"),
            Error::NoConnection(e) => write!(f, "could not reach the other player: {e}"),
            Error::Handshake(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for Error {}

/// Connects to the other player. Cancel by dropping the future.
pub async fn connect(
    servers: &Servers,
    start: Start,
    mut progress: impl FnMut(Stage),
) -> Result<Linked, Error> {
    // Check a typed code before contacting anyone.
    let join = match &start {
        Start::HostLan(offer) => return lan::host(*offer, LAN_PORT, progress).await,
        Start::Join(code) if parse_lan_code(code).is_some() => return lan::join(code, LAN_PORT, progress).await,
        Start::Host(_) => None,
        Start::Join(code) => {
            let (nameplate, words) = parse_code(code).ok_or(Error::BadCode)?;
            Some((nameplate, format!("{nameplate}-{words}")))
        }
    };
    progress(Stage::Server);
    let unresolved = |error| Error::NoServer { resolved: false, error };
    let server = resolve(&servers.rendezvous, DEFAULT_PORT).await.map_err(unresolved)?;
    let relay = match &servers.relay {
        Some(relay) => resolve(relay, DEFAULT_RELAY_PORT).await.map_err(unresolved)?,
        None => SocketAddr::new(server.ip(), DEFAULT_RELAY_PORT),
    };
    let probe = SocketAddr::new(server.ip(), server.port() + 1);
    let mut rendezvous = Rendezvous::connect(server, probe)
        .await
        .map_err(|error| Error::NoServer { resolved: true, error })?;

    let (role, peer, code) = match join {
        None => {
            let code = new_code(rendezvous.host().await.map_err(Error::Server)?);
            progress(Stage::Code(code.clone()));
            let peer = rendezvous.wait_peer(&code).await.map_err(introduction_error)?;
            (Role::Host, peer, code)
        }
        Some((nameplate, code)) => {
            let peer = rendezvous.join(nameplate, &code).await.map_err(introduction_error)?;
            (Role::Join, peer, code)
        }
    };
    let nat = rendezvous.nat(&peer);

    let started = std::time::Instant::now();
    let direct = if servers.force_relay {
        None
    } else {
        progress(Stage::Punching);
        rendezvous.punch(&peer, role, PUNCH_LIMIT).await.ok()
    };
    let connection = match direct {
        Some(connection) => Ok(connection),
        None => {
            progress(Stage::Relay);
            via_relay(relay, &peer, role, RELAY_LIMIT).await
        }
    };
    let mut connection = match connection {
        Ok(connection) => connection,
        Err(e) => {
            let report = Report::Failed { nat, waited: started.elapsed() };
            tokio::spawn(async move { rendezvous.report(&report).await });
            return Err(Error::NoConnection(e));
        }
    };
    let rtt = measure_rtt(&mut connection.stream, role).await.map_err(Error::NoConnection)?;
    // The server logs the pairing once both have reported, which can take a
    // while; the game does not wait for it.
    let report = Report::Connected { nat, path: connection.path, elapsed: started.elapsed(), rtt };
    tokio::spawn(async move { rendezvous.report(&report).await });

    progress(Stage::Handshake);
    let established = match start {
        Start::Host(offer) | Start::HostLan(offer) => handshake::host(connection.stream, &code, offer).await,
        Start::Join(_) => handshake::join(connection.stream, &code).await,
    }
    .map_err(Error::Handshake)?;
    Ok(Linked { established, path: connection.path, rtt })
}

/// A failed pairing: the other side not knowing the code's words is a wrong
/// code, anything else the server's or the network's doing.
fn introduction_error(e: io::Error) -> Error {
    match e.kind() {
        io::ErrorKind::PermissionDenied => Error::Handshake(handshake::Error::WrongCode),
        _ => Error::Server(e),
    }
}

/// `name` with `default_port` unless it has a port, resolved to an IPv4
/// address (the servers are IPv4 only for now).
pub async fn resolve(name: &str, default_port: u16) -> io::Result<SocketAddr> {
    let with_port = if name.rsplit_once(':').is_some_and(|(_, p)| p.parse::<u16>().is_ok()) {
        name.to_string()
    } else {
        format!("{name}:{default_port}")
    };
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host(&with_port).await?.collect();
    addrs
        .iter()
        .find(|a| a.is_ipv4())
        .copied()
        .ok_or_else(|| io::Error::other(format!("{name} has no IPv4 address")))
}
