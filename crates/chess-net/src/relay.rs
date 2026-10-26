//! The relay: the fallback when the two players cannot connect directly.
//! It pairs two connections that name the same session (the token the
//! rendezvous server gave both players) and forwards bytes between them.
//!
//! ```text
//! client: RELAY <token>
//! relay:  PAIRED once the other side arrives (or ERR <reason>), then
//!         every byte either side sends goes to the other unchanged
//! ```
//!
//! It never reads what it forwards. Once the handshake is built the traffic
//! is end-to-end encrypted, so the relay cannot read or alter moves either.
//! Limits on waiting, idling and bytes keep it from serving as a general
//! proxy.

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::time::{sleep, timeout};

use crate::line;
use crate::per_addr::{PerAddr, Slot};

/// Default relay port: 443 gets through most firewalls that allow web
/// traffic.
pub const DEFAULT_RELAY_PORT: u16 = 443;

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// How long the first side waits for the second.
    pub wait: Duration,
    /// A session with no bytes either way for this long is closed.
    pub idle: Duration,
    /// Longest a session may last.
    pub lifetime: Duration,
    /// Most bytes forwarded in each direction. A whole chess game with
    /// clocks is a few tens of kilobytes.
    pub max_bytes: u64,
    /// Most sessions waiting or running at once.
    pub max_sessions: usize,
    /// Most connections one address may have open at once, waiting or
    /// forwarding.
    pub max_per_addr: usize,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            wait: Duration::from_secs(60),
            idle: Duration::from_secs(30 * 60),
            lifetime: Duration::from_secs(6 * 60 * 60),
            max_bytes: 1 << 20,
            max_sessions: 100,
            max_per_addr: 6,
        }
    }
}

const FIRST_LINE_WAIT: Duration = Duration::from_secs(10);

struct Waiting {
    id: u64,
    partner: oneshot::Sender<(TcpStream, Slot)>,
}

struct State {
    waiting: Mutex<HashMap<String, Waiting>>,
    next_id: Mutex<u64>,
    /// Sessions waiting for their second side or forwarding.
    sessions: Mutex<usize>,
    connections: Arc<PerAddr>,
    limits: Limits,
}

/// Runs the relay on `listener` until an unrecoverable error.
pub async fn serve_relay(listener: TcpListener, limits: Limits) -> io::Result<()> {
    let state = Arc::new(State {
        waiting: Mutex::new(HashMap::new()),
        next_id: Mutex::new(0),
        sessions: Mutex::new(0),
        connections: PerAddr::new(limits.max_per_addr),
        limits,
    });
    loop {
        let (stream, addr) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(e) => {
                eprintln!("accept: {e}");
                sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let state = state.clone();
        tokio::spawn(async move {
            let Some(slot) = state.connections.take(addr.ip()) else {
                let mut stream = stream;
                let _ = timeout(FIRST_LINE_WAIT, line::write(&mut stream, "ERR too many connections")).await;
                return;
            };
            if let Err(e) = handle(&state, stream, slot).await {
                eprintln!("{addr}: {e}");
            }
        });
    }
}

/// `slot` is this connection's place in the per-address count; the second
/// side of a session hands it to the first side's task, which keeps both for
/// as long as the session runs.
async fn handle(state: &State, stream: TcpStream, slot: Slot) -> io::Result<()> {
    let mut conn = BufReader::new(stream);
    let first = timeout(FIRST_LINE_WAIT, line::expect(&mut conn))
        .await
        .map_err(|_| timed_out())??;
    let Some(token) = first.strip_prefix("RELAY ").filter(|t| is_token(t)) else {
        line::write(conn.get_mut(), "ERR bad request").await?;
        return Err(line::invalid("bad request"));
    };
    // The client waits for PAIRED before sending more, so nothing may be
    // buffered past the first line.
    if !conn.buffer().is_empty() {
        return Err(line::invalid("data before PAIRED"));
    }
    let stream = conn.into_inner();

    let waiting = state.waiting.lock().unwrap().remove(token);
    match waiting {
        Some(first) => {
            // The first side's task does the forwarding.
            if let Err((stream, _)) = first.partner.send((stream, slot)) {
                let mut stream = stream;
                line::write(&mut stream, "ERR peer left").await?;
            }
            Ok(())
        }
        None => wait_and_forward(state, token.to_string(), stream, slot).await,
    }
}

async fn wait_and_forward(state: &State, token: String, mut stream: TcpStream, _slot: Slot) -> io::Result<()> {
    let Some(_session) = Session::open(state) else {
        return line::write(&mut stream, "ERR relay busy").await;
    };
    let (partner, arrived) = oneshot::channel();
    let id = {
        let mut next = state.next_id.lock().unwrap();
        *next += 1;
        *next
    };
    state
        .waiting
        .lock()
        .unwrap()
        .insert(token.clone(), Waiting { id, partner });

    // The client sends nothing while it waits, so a read that returns means
    // it left.
    let mut probe = [0u8; 1];
    let second = tokio::select! {
        second = arrived => second.ok(),
        _ = sleep(state.limits.wait) => None,
        _ = stream.read(&mut probe) => None,
    };
    let Some((mut second, _second_slot)) = second else {
        {
            let mut waiting = state.waiting.lock().unwrap();
            if waiting.get(&token).is_some_and(|w| w.id == id) {
                waiting.remove(&token);
            }
        }
        return line::write(&mut stream, "ERR no peer").await;
    };

    line::write(&mut stream, "PAIRED").await?;
    line::write(&mut second, "PAIRED").await?;
    let limits = state.limits;
    let last = Mutex::new(Instant::now());
    let (a_read, a_write) = stream.into_split();
    let (b_read, b_write) = second.into_split();
    let forward = async {
        tokio::try_join!(
            pump(a_read, b_write, &last, limits),
            pump(b_read, a_write, &last, limits),
        )
    };
    match timeout(limits.lifetime, forward).await {
        Ok(result) => result.map(|_| ()),
        Err(_) => Err(timed_out()),
    }
}

/// Copies one direction until end of stream, then passes the end on.
async fn pump(
    mut from: OwnedReadHalf,
    mut to: OwnedWriteHalf,
    last: &Mutex<Instant>,
    limits: Limits,
) -> io::Result<()> {
    // Check for idling at least this often.
    let tick = limits.idle.min(Duration::from_secs(60));
    let mut buf = [0u8; 4096];
    let mut total = 0;
    loop {
        let n = match timeout(tick, from.read(&mut buf)).await {
            Ok(n) => n?,
            Err(_) if last.lock().unwrap().elapsed() >= limits.idle => {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "idle"));
            }
            Err(_) => continue,
        };
        if n == 0 {
            return to.shutdown().await;
        }
        total += n as u64;
        if total > limits.max_bytes {
            return Err(io::Error::other("byte limit"));
        }
        *last.lock().unwrap() = Instant::now();
        to.write_all(&buf[..n]).await?;
    }
}

/// Counts a session against `max_sessions` while it lives. Only the first
/// side's task holds one: a second side can only complete a waiting session.
struct Session<'a>(&'a State);

impl<'a> Session<'a> {
    fn open(state: &'a State) -> Option<Session<'a>> {
        let mut sessions = state.sessions.lock().unwrap();
        (*sessions < state.limits.max_sessions).then(|| {
            *sessions += 1;
            Session(state)
        })
    }
}

impl Drop for Session<'_> {
    fn drop(&mut self) {
        *self.0.sessions.lock().unwrap() -= 1;
    }
}

/// The rendezvous server's tokens: 32 lower-case hex digits.
fn is_token(s: &str) -> bool {
    s.len() == 32
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn timed_out() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "timed out")
}
