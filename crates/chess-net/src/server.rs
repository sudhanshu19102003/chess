//! The rendezvous server. It pairs a host and a joiner by nameplate, passes
//! messages between them, collects both reports and logs one line per
//! pairing. It holds no game logic, and nothing outlives a pairing.
//!
//! The server never tells one player the other's address. Anyone who knows
//! a nameplate can join it, so the addresses go between the players inside
//! messages the server cannot read, and only once each has shown it knows
//! the code's words (see [`crate::introduce`]). To the server those messages
//! are opaque hex.
//!
//! One text line at a time, client first unless noted:
//!
//! ```text
//! probe port   server: SEEN <your addr>, then closes
//! main port    HOST <private addr>
//!              server: CODE <nameplate>, later PAIRED <your public addr>
//!          or  JOIN <nameplate> <private addr>
//!              server: PAIRED <your public addr> or ERR <reason>
//!   then       MSG <hex>   (a few each way; the server forwards each to the other side)
//!   then       RESULT key=value ...
//!              server: BYE
//! ```
//!
//! The probe port tells the client how its NAT maps ports: if the server
//! sees the same public address on both ports, the mapping does not depend
//! on the destination and hole punching usually works.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::io::{self, Write as _};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::io::BufReader;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{sleep, timeout};

use crate::line;
use crate::per_addr::PerAddr;

/// Time for a new connection to say what it wants.
const FIRST_LINE_WAIT: Duration = Duration::from_secs(10);
/// How long a code waits for the joiner.
const CODE_TTL: Duration = Duration::from_secs(5 * 60);
/// How long a pairing may take, from `PAIRED` to a player's report: the
/// introduction, the punch and the relay fallback.
const PAIRED_WAIT: Duration = Duration::from_secs(90);
/// How long the joiner's side waits for the host's report after its own.
const REPORT_WAIT: Duration = Duration::from_secs(60);
/// Most codes waiting at once.
const MAX_WAITING: usize = 100;
/// What one address may hold: open connections, and codes waiting. A host
/// needs one of each, so a few at once is plenty for one person or one
/// household behind a shared address.
const MAX_CONNECTIONS_PER_ADDR: usize = 6;
const MAX_WAITING_PER_ADDR: usize = 3;
/// Messages one player may have forwarded, and the longest one (a `MSG `
/// line with its hex).
const MAX_MESSAGES: usize = 4;
/// `JOIN`s one address may make in a window. Guessing a nameplate costs the
/// host its code, so scanning is made slow.
const MAX_JOINS: u32 = 10;
const JOIN_WINDOW: Duration = Duration::from_secs(60);

#[derive(Clone, Copy)]
struct Addrs {
    /// As the server sees it, after any NAT.
    public: SocketAddr,
    /// As the client sees it.
    private: SocketAddr,
}

type Report = Vec<(String, String)>;

struct Waiting {
    id: u64,
    host: Addrs,
    pair: oneshot::Sender<Pairing>,
}

/// What a player has to talk to the other one through the server.
struct Mailbox {
    to_peer: mpsc::Sender<String>,
    from_peer: mpsc::Receiver<String>,
}

/// What the joiner hands the waiting host.
struct Pairing {
    /// The host's end of the two mailboxes.
    mailbox: Mailbox,
    host_report: oneshot::Sender<Report>,
}

struct State {
    waiting: Mutex<HashMap<u16, Waiting>>,
    /// Recent `JOIN`s by address: when the window began and how many.
    joins: Mutex<HashMap<IpAddr, (Instant, u32)>>,
    connections: Arc<PerAddr>,
    hosts: Arc<PerAddr>,
    next_id: Mutex<u64>,
    log: Option<PathBuf>,
}

/// Runs the server on `main` (pairing) and `probe` (NAT probe) until an
/// unrecoverable error. Each pairing's line goes to stdout and, if given,
/// is appended to `log`.
pub async fn serve(main: TcpListener, probe: TcpListener, log: Option<PathBuf>) -> io::Result<()> {
    let state = Arc::new(State {
        waiting: Mutex::new(HashMap::new()),
        joins: Mutex::new(HashMap::new()),
        connections: PerAddr::new(MAX_CONNECTIONS_PER_ADDR),
        hosts: PerAddr::new(MAX_WAITING_PER_ADDR),
        next_id: Mutex::new(0),
        log,
    });
    tokio::spawn(async move {
        loop {
            match probe.accept().await {
                Ok((mut stream, addr)) => {
                    tokio::spawn(async move {
                        let seen = format!("SEEN {addr}");
                        let _ = timeout(FIRST_LINE_WAIT, line::write(&mut stream, &seen)).await;
                    });
                }
                Err(_) => sleep(Duration::from_millis(100)).await,
            }
        }
    });
    loop {
        let (stream, addr) = match main.accept().await {
            Ok(accepted) => accepted,
            Err(e) => {
                eprintln!("accept: {e}");
                sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let state = state.clone();
        tokio::spawn(async move {
            let Some(_slot) = state.connections.take(addr.ip()) else {
                let mut stream = stream;
                let _ = timeout(FIRST_LINE_WAIT, line::write(&mut stream, "ERR too many connections")).await;
                return;
            };
            if let Err(e) = handle(&state, stream, addr).await {
                eprintln!("{addr}: {e}");
            }
        });
    }
}

async fn handle(state: &State, stream: TcpStream, public: SocketAddr) -> io::Result<()> {
    let mut conn = BufReader::new(stream);
    let first = timeout(FIRST_LINE_WAIT, line::expect(&mut conn))
        .await
        .map_err(|_| timed_out())??;
    let words: Vec<&str> = first.split(' ').collect();
    match words[..] {
        ["HOST", private] => {
            let me = Addrs {
                public,
                private: parse_addr(private)?,
            };
            let Some(_slot) = state.hosts.take(public.ip()) else {
                return line::write(conn.get_mut(), "ERR too many codes").await;
            };
            host(state, conn, me).await
        }
        ["JOIN", nameplate, private] => {
            let nameplate = nameplate
                .parse()
                .map_err(|_| line::invalid("bad nameplate"))?;
            let me = Addrs {
                public,
                private: parse_addr(private)?,
            };
            if !state.allow_join(public.ip()) {
                return line::write(conn.get_mut(), "ERR too many tries").await;
            }
            join(state, conn, nameplate, me).await
        }
        _ => {
            line::write(conn.get_mut(), "ERR unknown request").await?;
            Err(line::invalid("unknown request"))
        }
    }
}

async fn host(state: &State, mut conn: BufReader<TcpStream>, me: Addrs) -> io::Result<()> {
    let (pair, paired) = oneshot::channel();
    let id = {
        let mut next = state.next_id.lock().unwrap();
        *next += 1;
        *next
    };
    let nameplate = {
        let mut waiting = state.waiting.lock().unwrap();
        if waiting.len() >= MAX_WAITING {
            None
        } else {
            let n = (1..=u16::MAX).find(|n| !waiting.contains_key(n)).unwrap();
            waiting.insert(n, Waiting { id, host: me, pair });
            Some(n)
        }
    };
    let Some(nameplate) = nameplate else {
        return line::write(conn.get_mut(), "ERR server busy").await;
    };
    line::write(conn.get_mut(), &format!("CODE {nameplate}")).await?;

    // The host sends nothing while it waits, so any line or the end of the
    // stream means it gave up.
    let pairing = tokio::select! {
        pairing = paired => pairing.ok(),
        _ = sleep(CODE_TTL) => None,
        _ = line::read(&mut conn) => None,
    };
    let Some(pairing) = pairing else {
        {
            let mut waiting = state.waiting.lock().unwrap();
            if waiting.get(&nameplate).is_some_and(|w| w.id == id) {
                waiting.remove(&nameplate);
            }
        }
        return line::write(conn.get_mut(), "ERR code expired").await;
    };
    let (read, mut write) = split(conn);
    if let Ok(report) = bridge(read, &mut write, me.public, pairing.mailbox).await {
        let _ = pairing.host_report.send(report);
        line::write(&mut write, "BYE").await?;
    }
    Ok(())
}

async fn join(
    state: &State,
    mut conn: BufReader<TcpStream>,
    nameplate: u16,
    me: Addrs,
) -> io::Result<()> {
    let waiting = state.waiting.lock().unwrap().remove(&nameplate);
    let Some(waiting) = waiting else {
        return line::write(conn.get_mut(), "ERR unknown code").await;
    };
    let host = waiting.host;
    let (host_report, host_reported) = oneshot::channel();
    let (to_host, from_joiner) = mpsc::channel(MAX_MESSAGES);
    let (to_joiner, from_host) = mpsc::channel(MAX_MESSAGES);
    let pairing = Pairing {
        mailbox: Mailbox { to_peer: to_joiner, from_peer: from_joiner },
        host_report,
    };
    if waiting.pair.send(pairing).is_err() {
        return line::write(conn.get_mut(), "ERR host left").await;
    }
    let (read, mut write) = split(conn);
    let mailbox = Mailbox { to_peer: to_host, from_peer: from_host };
    let mine = bridge(read, &mut write, me.public, mailbox).await.ok();
    let theirs = timeout(REPORT_WAIT, host_reported).await.ok().and_then(Result::ok);
    state.record(host, me, theirs.as_ref(), mine.as_ref());
    line::write(&mut write, "BYE").await
}

/// The two halves of a connection. Nothing may be buffered yet: a client
/// sends nothing until it has been told `PAIRED`.
fn split(conn: BufReader<TcpStream>) -> (OwnedReadHalf, OwnedWriteHalf) {
    debug_assert!(conn.buffer().is_empty());
    conn.into_inner().into_split()
}

/// Tells a player `PAIRED` with its own public address, then passes their
/// `MSG` lines to the other player and the other's to them until the
/// player's `RESULT` arrives, which it returns.
async fn bridge(
    read: OwnedReadHalf,
    write: &mut OwnedWriteHalf,
    public: SocketAddr,
    mailbox: Mailbox,
) -> io::Result<Report> {
    line::write(write, &format!("PAIRED {public}")).await?;
    // Reading in its own task keeps a half-read line from being lost when
    // the other branch wins.
    let (lines_tx, mut lines) = mpsc::channel(4);
    let reader = tokio::spawn(async move {
        let mut read = BufReader::new(read);
        loop {
            let next = line::expect(&mut read).await;
            let stop = next.is_err();
            if lines_tx.send(next).await.is_err() || stop {
                break;
            }
        }
    });
    let Mailbox { to_peer, mut from_peer } = mailbox;
    let work = async {
        let (mut sent, mut peer_open) = (0, true);
        loop {
            tokio::select! {
                next = lines.recv() => {
                    let text = next.ok_or_else(|| line::invalid("connection closed"))??;
                    let Some(hex) = text.strip_prefix("MSG ") else {
                        return parse_report(&text);
                    };
                    if sent == MAX_MESSAGES || !is_hex(hex) {
                        return Err(line::invalid("bad MSG"));
                    }
                    sent += 1;
                    // If the other side is gone, there is nobody to tell.
                    let _ = to_peer.send(text).await;
                }
                message = from_peer.recv(), if peer_open => match message {
                    Some(message) => line::write(write, &message).await?,
                    None => peer_open = false,
                },
            }
        }
    };
    let result = timeout(PAIRED_WAIT, work).await.map_err(|_| timed_out())?;
    reader.abort();
    result
}

fn is_hex(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `RESULT key=value ...`: at most eight pairs of short lower-case words.
fn parse_report(text: &str) -> io::Result<Report> {
    let pairs = text
        .strip_prefix("RESULT ")
        .ok_or_else(|| line::invalid("expected RESULT"))?;
    let word = |s: &str| {
        (1..=32).contains(&s.len())
            && s.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    };
    let mut report = Vec::new();
    for pair in pairs.split(' ') {
        match pair.split_once('=') {
            Some((k, v)) if word(k) && word(v) && report.len() < 8 => {
                report.push((k.into(), v.into()))
            }
            _ => return Err(line::invalid("bad RESULT")),
        }
    }
    Ok(report)
}

impl State {
    /// Counts a `JOIN` from `ip`; false once it has made too many lately.
    fn allow_join(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let mut joins = self.joins.lock().unwrap();
        if joins.len() > 10_000 {
            joins.retain(|_, (began, _)| now.duration_since(*began) < JOIN_WINDOW);
        }
        let entry = joins.entry(ip).or_insert((now, 0));
        if now.duration_since(entry.0) >= JOIN_WINDOW {
            *entry = (now, 0);
        }
        entry.1 += 1;
        entry.1 <= MAX_JOINS
    }

    fn record(
        &self,
        host: Addrs,
        joiner: Addrs,
        host_report: Option<&Report>,
        join_report: Option<&Report>,
    ) {
        fn link(r: Option<&Report>) -> Option<&str> {
            r.map(|r| {
                r.iter()
                    .find(|(k, _)| k == "link")
                    .map_or("?", |(_, v)| v.as_str())
            })
        }
        let outcome = match (link(host_report), link(join_report)) {
            (Some("direct"), Some("direct")) => "direct",
            (Some("relay"), Some("relay")) => "relay",
            (Some("none"), Some("none")) => "failed",
            (Some(_), Some(_)) => "split",
            _ => "incomplete",
        };
        let yes_no = |b: bool| if b { "yes" } else { "no" };
        let t = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let mut line = format!(
            "t={t} outcome={outcome} same_ip={}",
            yes_no(host.public.ip() == joiner.public.ip())
        );
        for (side, addrs, report) in [("host", host, host_report), ("join", joiner, join_report)] {
            line += &format!(
                " {side}.kept={}",
                yes_no(addrs.public.port() == addrs.private.port())
            );
            for (k, v) in report.into_iter().flatten() {
                line += &format!(" {side}.{k}={v}");
            }
        }
        println!("{line}");
        if let Some(path) = &self.log {
            let written = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .and_then(|mut f| writeln!(f, "{line}"));
            if let Err(e) = written {
                eprintln!("log {}: {e}", path.display());
            }
        }
    }
}

/// Totals over a log written by [`serve`].
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub direct: usize,
    /// Punching failed and the relay worked.
    pub relay: usize,
    /// Neither worked.
    pub failed: usize,
    pub split: usize,
    /// A client left before reporting; not counted in the rate.
    pub incomplete: usize,
    /// Keyed by both sides' NAT kinds, sorted: (direct, reported).
    pub by_nat: BTreeMap<String, (usize, usize)>,
    /// Keyed by the host's path for direct pairings.
    pub by_path: BTreeMap<String, usize>,
}

impl Stats {
    pub fn from_log(log: &str) -> Stats {
        let mut stats = Stats::default();
        for entry in log.lines().filter(|l| !l.trim().is_empty()) {
            let get = |key: &str| {
                entry
                    .split(' ')
                    .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))
            };
            let direct = match get("outcome") {
                Some("direct") => {
                    stats.direct += 1;
                    let path = get("host.path").unwrap_or("?");
                    *stats.by_path.entry(path.into()).or_default() += 1;
                    true
                }
                Some("relay") => {
                    stats.relay += 1;
                    false
                }
                Some("failed") => {
                    stats.failed += 1;
                    false
                }
                Some("split") => {
                    stats.split += 1;
                    false
                }
                _ => {
                    stats.incomplete += 1;
                    continue;
                }
            };
            let mut nats = [
                get("host.nat").unwrap_or("?"),
                get("join.nat").unwrap_or("?"),
            ];
            nats.sort();
            let entry = stats.by_nat.entry(nats.join("+")).or_default();
            entry.0 += usize::from(direct);
            entry.1 += 1;
        }
        stats
    }

    pub fn reported(&self) -> usize {
        self.direct + self.relay + self.failed + self.split
    }
}

impl fmt::Display for Stats {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let reported = self.reported();
        let percent = |n: usize, of: usize| {
            if of == 0 {
                0.0
            } else {
                100.0 * n as f64 / of as f64
            }
        };
        writeln!(
            f,
            "direct: {} of {reported} pairings ({:.0}%)",
            self.direct,
            percent(self.direct, reported)
        )?;
        writeln!(
            f,
            "relay: {}, no connection: {}, split (sides disagree): {}",
            self.relay, self.failed, self.split
        )?;
        writeln!(
            f,
            "incomplete (a client left, not counted): {}",
            self.incomplete
        )?;
        writeln!(
            f,
            "direct by NAT (none, eim = endpoint-independent, edm = dependent, unknown):"
        )?;
        for (nats, (direct, of)) in &self.by_nat {
            writeln!(f, "  {nats:<16} {direct}/{of}")?;
        }
        writeln!(f, "direct pairings by the host's path:")?;
        for (path, n) in &self.by_path {
            writeln!(f, "  {path:<16} {n}")?;
        }
        Ok(())
    }
}

fn parse_addr(s: &str) -> io::Result<SocketAddr> {
    s.parse().map_err(|_| line::invalid("bad address"))
}

fn timed_out() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "timed out")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_count_outcomes() {
        let log = "\
t=1 outcome=direct same_ip=no host.kept=yes host.link=direct host.nat=eim host.path=public-out join.kept=yes join.link=direct join.nat=eim
t=2 outcome=relay same_ip=no host.kept=no host.link=relay host.nat=edm host.path=relay join.kept=yes join.link=relay join.nat=eim
t=3 outcome=incomplete same_ip=no host.kept=yes join.kept=yes
t=4 outcome=direct same_ip=yes host.kept=yes host.link=direct host.nat=none host.path=private-in join.kept=yes join.link=direct join.nat=none
t=5 outcome=failed same_ip=no host.kept=no host.link=none host.nat=edm join.kept=no join.link=none join.nat=edm
";
        let stats = Stats::from_log(log);
        assert_eq!(
            (
                stats.direct,
                stats.relay,
                stats.failed,
                stats.split,
                stats.incomplete
            ),
            (2, 1, 1, 0, 1)
        );
        assert_eq!(stats.by_nat["edm+eim"], (0, 1));
        assert_eq!(stats.by_nat["edm+edm"], (0, 1));
        assert_eq!(stats.by_nat["eim+eim"], (1, 1));
        assert_eq!(stats.by_path["private-in"], 1);
        assert!(
            stats
                .to_string()
                .starts_with("direct: 2 of 4 pairings (50%)")
        );
    }
}
