//! Games on the local network, with no server at all: two machines on the
//! same Wi-Fi, or on one phone's hotspot, without internet.
//!
//! The host listens on a TCP port and announces it by UDP broadcast every
//! second (`CHESS-LAN 1 HERE <nameplate> <port>`). The joiner broadcasts
//! `CHESS-LAN 1 WHERE <nameplate>`, which the host answers directly. Either
//! one is enough: Android's Wi-Fi often drops incoming broadcasts for a
//! program like this, so a phone may hear neither, but the other side hears
//! the phone. Broadcasts carry only the nameplate. The words stay the
//! handshake's secret, as on the internet, so anyone on the network who
//! answers in the host's place fails the handshake. The connection then
//! passes the same hello, round trip measurement and handshake as a direct
//! one.

use std::future::pending;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use socket2::{Domain, Protocol, Socket, Type};
use tokio::io::BufReader;
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::time::{Instant, interval, sleep_until, timeout};

use crate::client::{HELLO_WAIT, hello};
use crate::code::{new_lan_code, parse_lan_code};
use crate::connect::{Error, Linked, Stage};
use crate::handshake::{self, Offer};
use crate::{Path, Role, Via, line, measure_rtt};

/// The UDP port announcements and questions go to.
pub const LAN_PORT: u16 = 7779;
/// How long the joiner looks for the host before giving up.
pub const SEARCH_LIMIT: Duration = Duration::from_secs(30);
const EVERY: Duration = Duration::from_secs(1);
const CONNECT_WAIT: Duration = Duration::from_secs(3);
const MAGIC: &str = "CHESS-LAN 1";

/// Hosts a game on the local network, announcing it on UDP `port`. Waits
/// until someone joins; cancel by dropping the future.
pub async fn host(offer: Offer, port: u16, mut progress: impl FnMut(Stage)) -> Result<Linked, Error> {
    let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)).await.map_err(Error::NoConnection)?;
    let tcp_port = listener.local_addr().map_err(Error::NoConnection)?.port();
    let udp = udp_socket(port).map_err(Error::NoConnection)?;
    let mut bytes = [0u8; 2];
    getrandom::fill(&mut bytes).expect("no system randomness");
    let nameplate = u16::from_le_bytes(bytes) % 100;
    let code = new_lan_code(nameplate);
    let token = format!("lan-{nameplate}");
    progress(Stage::Code(code.clone()));

    let here = format!("{MAGIC} HERE {nameplate} {tcp_port}");
    let question = format!("{MAGIC} WHERE {nameplate}");
    let mut tick = interval(EVERY);
    let mut buf = [0u8; 64];
    let mut stream = loop {
        tokio::select! {
            _ = tick.tick() => announce(&udp, &here, port).await,
            Ok((n, from)) = udp.recv_from(&mut buf) => {
                if &buf[..n] == question.as_bytes() {
                    let _ = udp.send_to(here.as_bytes(), from).await;
                }
            }
            Ok((conn, _)) = listener.accept() => {
                if let Ok(Ok(conn)) = timeout(HELLO_WAIT, hello(BufReader::new(conn), &token, Role::Host)).await {
                    break conn;
                }
            }
        }
    };
    line::write(stream.get_mut(), "PICK").await.map_err(Error::NoConnection)?;
    let rtt = measure_rtt(&mut stream, Role::Host).await.map_err(Error::NoConnection)?;
    progress(Stage::Handshake);
    let established = handshake::host(stream, &code, offer).await.map_err(Error::Handshake)?;
    Ok(Linked { established, path: Path { via: Via::Lan, outgoing: false }, rtt })
}

/// Joins the game with a `lan-` code, looking for its host on UDP `port`
/// for up to [`SEARCH_LIMIT`].
pub async fn join(code: &str, port: u16, mut progress: impl FnMut(Stage)) -> Result<Linked, Error> {
    let (nameplate, code) = parse_lan_code(code).ok_or(Error::BadCode)?;
    let token = format!("lan-{nameplate}");
    progress(Stage::Searching);
    // Answers come back to the asking socket. Announcements go to `port`,
    // which another program on this machine may hold without sharing it.
    let ask = udp_socket(0).map_err(Error::NoConnection)?;
    let listen = udp_socket(port).ok();

    let question = format!("{MAGIC} WHERE {nameplate}");
    let here = format!("{MAGIC} HERE {nameplate} ");
    let deadline = Instant::now() + SEARCH_LIMIT;
    let mut tick = interval(EVERY);
    let (mut a, mut b) = ([0u8; 64], [0u8; 64]);
    let mut stream = loop {
        let found = tokio::select! {
            _ = sleep_until(deadline) => {
                let e = io::Error::new(io::ErrorKind::TimedOut, "no game with that code on this network");
                return Err(Error::NoConnection(e));
            }
            _ = tick.tick() => {
                announce(&ask, &question, port).await;
                None
            }
            Ok((n, from)) = ask.recv_from(&mut a) => host_addr(&a[..n], &here, from),
            Ok((n, from)) = recv_maybe(listen.as_ref(), &mut b) => host_addr(&b[..n], &here, from),
        };
        let Some(addr) = found else { continue };
        let attempt = async { hello(BufReader::new(TcpStream::connect(addr).await?), &token, Role::Join).await };
        if let Ok(Ok(stream)) = timeout(CONNECT_WAIT + HELLO_WAIT, attempt).await {
            break stream;
        }
    };
    let rtt = measure_rtt(&mut stream, Role::Join).await.map_err(Error::NoConnection)?;
    progress(Stage::Handshake);
    let established = handshake::join(stream, &code).await.map_err(Error::Handshake)?;
    Ok(Linked { established, path: Path { via: Via::Lan, outgoing: true }, rtt })
}

/// The host's TCP address, if `packet` is its `HERE` (`here` is the
/// announcement up to the port).
fn host_addr(packet: &[u8], here: &str, from: SocketAddr) -> Option<SocketAddr> {
    let port = std::str::from_utf8(packet).ok()?.strip_prefix(here)?.parse().ok()?;
    Some(SocketAddr::new(from.ip(), port))
}

async fn recv_maybe(socket: Option<&UdpSocket>, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
    match socket {
        Some(socket) => socket.recv_from(buf).await,
        None => pending().await,
    }
}

/// Sends `message` to `port` on every network this machine is on, and on
/// this machine itself. A network that drops it just misses one.
async fn announce(socket: &UdpSocket, message: &str, port: u16) {
    for to in targets(port) {
        let _ = socket.send_to(message.as_bytes(), to).await;
    }
}

/// The broadcast address of each IPv4 network, plus the limited broadcast
/// address (which goes out on only one network) and this machine.
fn targets(port: u16) -> Vec<SocketAddr> {
    let mut ips = vec![Ipv4Addr::LOCALHOST, Ipv4Addr::BROADCAST];
    for interface in if_addrs::get_if_addrs().unwrap_or_default() {
        if let if_addrs::IfAddr::V4(v4) = interface.addr
            && !v4.ip.is_loopback()
        {
            let broadcast = v4.broadcast.unwrap_or_else(|| Ipv4Addr::from(u32::from(v4.ip) | !u32::from(v4.netmask)));
            if !ips.contains(&broadcast) {
                ips.push(broadcast);
            }
        }
    }
    ips.into_iter().map(|ip| SocketAddr::from((ip, port))).collect()
}

/// A UDP socket on `port` (0 for any) that may broadcast, and shares the
/// port with other programs doing the same.
fn udp_socket(port: u16) -> io::Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    socket.set_broadcast(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)).into())?;
    UdpSocket::from_std(socket.into())
}
