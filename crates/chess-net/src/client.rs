//! The player's side: meet through the rendezvous server, then open a
//! direct TCP connection to the peer by simultaneous open.
//!
//! Every socket (the listener, the NAT probe, the server connection and the
//! connection attempts to the peer) is bound to the same local port with
//! `SO_REUSEADDR` and `SO_REUSEPORT`. A NAT that maps ports independently of
//! the destination then shows the peer the same public port the server saw,
//! and each side's outgoing SYN opens a hole for the other's.
//!
//! When that fails, both sides meet at the relay instead ([`via_relay`]), which
//! pairs them by the token the rendezvous server gave them.

use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::BufReader;
use tokio::net::{TcpListener, TcpSocket, TcpStream};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio::time::{sleep, timeout};

use crate::line;

/// How long the client gives the punch before reporting failure.
pub const PUNCH_LIMIT: Duration = Duration::from_secs(20);
/// How long a client waits at the relay for the other side.
pub const RELAY_LIMIT: Duration = Duration::from_secs(20);
/// Extra time the joiner punches for. The host decides which connection
/// wins, so a joiner that gave up at the same instant could miss a pick
/// made just before the host's deadline and go to the relay alone.
const PICK_GRACE: Duration = Duration::from_secs(2);
const SERVER_WAIT: Duration = Duration::from_secs(10);
const PROBE_WAIT: Duration = Duration::from_secs(3);
/// One connection attempt. The kernel retransmits a SYN after about a
/// second, so this covers two or three SYNs before starting over.
const ATTEMPT_WAIT: Duration = Duration::from_secs(3);
const RETRY_DELAY: Duration = Duration::from_millis(200);
pub(crate) const HELLO_WAIT: Duration = Duration::from_secs(5);
const PINGS: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Host,
    Join,
}

impl Role {
    fn other(self) -> Role {
        match self {
            Role::Host => Role::Join,
            Role::Join => Role::Host,
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(match self {
            Role::Host => "host",
            Role::Join => "join",
        })
    }
}

/// The other player, as the server introduced them.
#[derive(Clone, Debug)]
pub struct Peer {
    /// Shared by both sides of this pairing only; proves a connection
    /// reached the right peer.
    pub token: String,
    /// This client's own address as the server saw it.
    pub you: SocketAddr,
    pub public: SocketAddr,
    pub private: SocketAddr,
}

/// How this client's NAT maps its port, from the probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nat {
    /// The server saw the client's own address.
    None,
    /// Same public address towards both server ports: endpoint-independent
    /// mapping, which hole punching needs.
    Eim,
    /// A different public address per destination (a "symmetric" NAT):
    /// punching usually fails unless the other side has no NAT.
    Edm,
    /// The probe got no answer.
    Unknown,
}

impl fmt::Display for Nat {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(match self {
            Nat::None => "none",
            Nat::Eim => "eim",
            Nat::Edm => "edm",
            Nat::Unknown => "unknown",
        })
    }
}

/// Which peer address the winning connection used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    Public,
    Private,
    /// Accepted from an address the server did not report.
    Other,
    /// Through the relay, not direct.
    Relay,
    /// Found on the local network, with no server.
    Lan,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Path {
    pub via: Via,
    /// This client's connect succeeded, rather than its listener accepting.
    /// In a true simultaneous open both sides see an outgoing connection.
    pub outgoing: bool,
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let via = match self.via {
            Via::Public => "public",
            Via::Private => "private",
            Via::Other => "other",
            Via::Relay => return f.write_str("relay"),
            Via::Lan => return f.write_str("lan"),
        };
        write!(f, "{via}-{}", if self.outgoing { "out" } else { "in" })
    }
}

/// A connection to the peer, direct or relayed, past the hello.
pub struct Connection {
    pub stream: BufReader<TcpStream>,
    pub path: Path,
    /// From the start of the attempt to the chosen connection.
    pub elapsed: Duration,
}

/// What a client tells the server once it has a connection or has given up.
pub enum Report {
    Connected {
        nat: Nat,
        path: Path,
        /// Time to connect, counting the failed punch for a relayed one.
        elapsed: Duration,
        rtt: Duration,
    },
    Failed {
        nat: Nat,
        waited: Duration,
    },
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Report::Connected {
                nat,
                path,
                elapsed,
                rtt,
            } => write!(
                f,
                "RESULT link={} nat={nat} path={path} ms={} rtt={}",
                if path.via == Via::Relay {
                    "relay"
                } else {
                    "direct"
                },
                elapsed.as_millis(),
                rtt.as_millis()
            ),
            Report::Failed { nat, waited } => {
                write!(f, "RESULT link=none nat={nat} ms={}", waited.as_millis())
            }
        }
    }
}

/// A connection to the rendezvous server, and the local port everything
/// shares.
pub struct Rendezvous {
    conn: BufReader<TcpStream>,
    listener: Arc<TcpListener>,
    /// The unspecified address with the shared port.
    bind: SocketAddr,
    private: SocketAddr,
    probe_seen: Option<SocketAddr>,
}

impl Rendezvous {
    /// Takes a local port, listens on it, probes the NAT through `probe` and
    /// connects to the server at `server`, all from that port.
    pub async fn connect(server: SocketAddr, probe: SocketAddr) -> io::Result<Rendezvous> {
        let unspecified: IpAddr = match server {
            SocketAddr::V4(_) => Ipv4Addr::UNSPECIFIED.into(),
            SocketAddr::V6(_) => Ipv6Addr::UNSPECIFIED.into(),
        };
        let socket = reusable(server)?;
        socket.bind(SocketAddr::new(unspecified, 0))?;
        let bind = socket.local_addr()?;
        let listener = Arc::new(socket.listen(16)?);
        let probe_seen = timeout(PROBE_WAIT, probe_nat(bind, probe))
            .await
            .ok()
            .and_then(Result::ok);
        let stream = timeout(SERVER_WAIT, connect_from(bind, server))
            .await
            .map_err(|_| timed_out("no answer from the rendezvous server"))??;
        let private = stream.local_addr()?;
        Ok(Rendezvous {
            conn: BufReader::new(stream),
            listener,
            bind,
            private,
            probe_seen,
        })
    }

    /// This client's address on its own network.
    pub fn private_addr(&self) -> SocketAddr {
        self.private
    }

    /// Asks the server for a nameplate to wait on.
    pub async fn host(&mut self) -> io::Result<u16> {
        line::write(self.conn.get_mut(), &format!("HOST {}", self.private)).await?;
        let reply = self.reply().await?;
        reply
            .strip_prefix("CODE ")
            .and_then(|n| n.parse().ok())
            .ok_or_else(|| line::invalid(format!("unexpected reply: {reply}")))
    }

    /// Waits for someone to join the nameplate from [`host`](Self::host).
    /// The server gives up after a few minutes.
    pub async fn wait_peer(&mut self) -> io::Result<Peer> {
        let reply = self.reply().await?;
        parse_peer(&reply)
    }

    pub async fn join(&mut self, nameplate: u16) -> io::Result<Peer> {
        line::write(
            self.conn.get_mut(),
            &format!("JOIN {nameplate} {}", self.private),
        )
        .await?;
        let reply = timeout(SERVER_WAIT, self.reply())
            .await
            .map_err(|_| timed_out("no answer"))??;
        parse_peer(&reply)
    }

    pub fn nat(&self, peer: &Peer) -> Nat {
        match self.probe_seen {
            _ if peer.you == self.private => Nat::None,
            None => Nat::Unknown,
            Some(seen) if seen == peer.you => Nat::Eim,
            Some(_) => Nat::Edm,
        }
    }

    /// Opens a direct connection to `peer`: listens and connects to each of
    /// its addresses at once, retrying, until one connection passes the
    /// hello or `limit` runs out. Several may pass; the host picks the
    /// first and says so on it, and the joiner takes the one picked. The
    /// joiner waits a little longer than `limit` for a late pick.
    pub async fn punch(&self, peer: &Peer, role: Role, limit: Duration) -> io::Result<Connection> {
        let start = Instant::now();
        let limit = match role {
            Role::Host => limit,
            Role::Join => limit + PICK_GRACE,
        };
        let mut targets = vec![(peer.public, Via::Public)];
        if peer.private != peer.public {
            targets.push((peer.private, Via::Private));
        }
        let (found_tx, mut found) = mpsc::channel(4);
        // Dropping the set when this returns aborts every attempt.
        let mut tasks = JoinSet::new();

        let (listener, tx, token, targets_in) = (
            self.listener.clone(),
            found_tx.clone(),
            peer.token.clone(),
            targets.clone(),
        );
        tasks.spawn(async move {
            let mut hellos = JoinSet::new();
            loop {
                let Ok((stream, from)) = listener.accept().await else {
                    sleep(RETRY_DELAY).await;
                    continue;
                };
                let via = targets_in
                    .iter()
                    .find(|t| t.0 == from)
                    .map_or(Via::Other, |t| t.1);
                let (tx, token) = (tx.clone(), token.clone());
                hellos.spawn(async move {
                    let conn = BufReader::new(stream);
                    if let Ok(Ok(conn)) = timeout(HELLO_WAIT, hello(conn, &token, role)).await {
                        let _ = tx
                            .send((
                                conn,
                                Path {
                                    via,
                                    outgoing: false,
                                },
                            ))
                            .await;
                    }
                });
                while hellos.try_join_next().is_some() {}
            }
        });

        for (addr, via) in targets {
            let (tx, token, bind) = (found_tx.clone(), peer.token.clone(), self.bind);
            tasks.spawn(async move {
                loop {
                    if let Ok(Ok(stream)) = timeout(ATTEMPT_WAIT, connect_from(bind, addr)).await
                        && let Ok(Ok(conn)) =
                            timeout(HELLO_WAIT, hello(BufReader::new(stream), &token, role)).await
                    {
                        let _ = tx
                            .send((
                                conn,
                                Path {
                                    via,
                                    outgoing: true,
                                },
                            ))
                            .await;
                        return;
                    }
                    sleep(RETRY_DELAY).await;
                }
            });
        }
        drop(found_tx);

        let (mut stream, path) = timeout(limit, found.recv())
            .await
            .ok()
            .flatten()
            .ok_or_else(|| timed_out("no direct connection"))?;
        if role == Role::Host {
            line::write(stream.get_mut(), "PICK").await?;
        }
        Ok(Connection {
            stream,
            path,
            elapsed: start.elapsed(),
        })
    }

    /// Sends the outcome to the server, which logs it.
    pub async fn report(&mut self, report: &Report) -> io::Result<()> {
        line::write(self.conn.get_mut(), &report.to_string()).await?;
        // The server answers once it has both reports.
        let reply = timeout(Duration::from_secs(90), self.reply())
            .await
            .map_err(|_| timed_out("no answer"))??;
        if reply == "BYE" {
            Ok(())
        } else {
            Err(line::invalid(format!("unexpected reply: {reply}")))
        }
    }

    async fn reply(&mut self) -> io::Result<String> {
        let reply = line::expect(&mut self.conn).await?;
        match reply.strip_prefix("ERR ") {
            Some(reason) => Err(io::Error::other(format!("rendezvous server: {reason}"))),
            None => Ok(reply),
        }
    }
}

/// Meets `peer` at the relay, which pairs the two by the pairing's token,
/// and runs the same hello as a direct connection through it.
pub async fn via_relay(
    relay: SocketAddr,
    peer: &Peer,
    role: Role,
    limit: Duration,
) -> io::Result<Connection> {
    let start = Instant::now();
    let attempt = async {
        let mut conn = BufReader::new(TcpStream::connect(relay).await?);
        line::write(conn.get_mut(), &format!("RELAY {}", peer.token)).await?;
        match line::expect(&mut conn).await?.as_str() {
            "PAIRED" => hello(conn, &peer.token, role).await,
            reply => Err(io::Error::other(format!("relay: {reply}"))),
        }
    };
    let mut stream = timeout(limit, attempt)
        .await
        .map_err(|_| timed_out("the peer did not reach the relay"))??;
    if role == Role::Host {
        line::write(stream.get_mut(), "PICK").await?;
    }
    Ok(Connection {
        stream,
        path: Path {
            via: Via::Relay,
            outgoing: true,
        },
        elapsed: start.elapsed(),
    })
}

/// Measures the round trip over a connection: the host pings, the
/// joiner echoes, and the host sends the median so both sides know it.
pub async fn measure_rtt(stream: &mut BufReader<TcpStream>, role: Role) -> io::Result<Duration> {
    let exchange = async {
        match role {
            Role::Host => {
                let mut samples = Vec::with_capacity(PINGS);
                for i in 0..PINGS {
                    let sent = Instant::now();
                    line::write(stream.get_mut(), &format!("PING {i}")).await?;
                    if line::expect(stream).await? != format!("PONG {i}") {
                        return Err(line::invalid("expected PONG"));
                    }
                    samples.push(sent.elapsed());
                }
                samples.sort();
                let median = samples[PINGS / 2];
                line::write(stream.get_mut(), &format!("RTT {}", median.as_micros())).await?;
                Ok(median)
            }
            Role::Join => loop {
                let msg = line::expect(stream).await?;
                if let Some(i) = msg.strip_prefix("PING ") {
                    line::write(stream.get_mut(), &format!("PONG {i}")).await?;
                } else if let Some(us) = msg.strip_prefix("RTT ").and_then(|us| us.parse().ok()) {
                    break Ok(Duration::from_micros(us));
                } else {
                    break Err(line::invalid("expected PING or RTT"));
                }
            },
        }
    };
    timeout(Duration::from_secs(10), exchange)
        .await
        .map_err(|_| timed_out("no answer"))?
}

/// Checks the connection reached this pairing's peer in the other role. The
/// joiner then waits for the host to pick this connection.
pub(crate) async fn hello(
    mut conn: BufReader<TcpStream>,
    token: &str,
    role: Role,
) -> io::Result<BufReader<TcpStream>> {
    line::write(conn.get_mut(), &format!("CHESS-PUNCH 1 {token} {role}")).await?;
    if line::expect(&mut conn).await? != format!("CHESS-PUNCH 1 {token} {}", role.other()) {
        return Err(line::invalid("not our peer"));
    }
    if role == Role::Join && line::expect(&mut conn).await? != "PICK" {
        return Err(line::invalid("expected PICK"));
    }
    Ok(conn)
}

async fn probe_nat(bind: SocketAddr, probe: SocketAddr) -> io::Result<SocketAddr> {
    let mut conn = BufReader::new(connect_from(bind, probe).await?);
    let reply = line::expect(&mut conn).await?;
    let seen = reply
        .strip_prefix("SEEN ")
        .and_then(|a| a.parse().ok())
        .ok_or_else(|| line::invalid("bad probe reply"))?;
    // Let the server close first, so the TIME_WAIT is on its side and not
    // on the shared port.
    let _ = line::read(&mut conn).await;
    Ok(seen)
}

fn parse_peer(reply: &str) -> io::Result<Peer> {
    let words: Vec<&str> = reply.split(' ').collect();
    let addr = |s: &str| s.parse().map_err(|_| line::invalid("bad address"));
    match words[..] {
        ["PEER", token, you, public, private] => Ok(Peer {
            token: token.to_string(),
            you: addr(you)?,
            public: addr(public)?,
            private: addr(private)?,
        }),
        _ => Err(line::invalid(format!("unexpected reply: {reply}"))),
    }
}

async fn connect_from(bind: SocketAddr, to: SocketAddr) -> io::Result<TcpStream> {
    let socket = reusable(to)?;
    socket.bind(bind)?;
    socket.connect(to).await
}

fn reusable(addr: SocketAddr) -> io::Result<TcpSocket> {
    let socket = match addr {
        SocketAddr::V4(_) => TcpSocket::new_v4()?,
        SocketAddr::V6(_) => TcpSocket::new_v6()?,
    };
    socket.set_reuseaddr(true)?;
    #[cfg(unix)]
    socket.set_reuseport(true)?;
    Ok(socket)
}

fn timed_out(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, msg)
}
