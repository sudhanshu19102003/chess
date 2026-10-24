//! The rendezvous server. It pairs a host and a joiner by nameplate, tells
//! each the other's addresses, collects both reports and logs one line per
//! pairing. It holds no game logic, and nothing outlives a pairing.
//!
//! One text line at a time, client first unless noted:
//!
//! ```text
//! probe port   server: SEEN <your addr>, then closes
//! main port    HOST <private addr>
//!              server: CODE <nameplate>, later PEER <token> <you> <peer public> <peer private>
//!          or  JOIN <nameplate> <private addr>
//!              server: PEER ... or ERR <reason>
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
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::io::BufReader;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::time::{sleep, timeout};

use crate::line;

/// Time for a new connection to say what it wants.
const FIRST_LINE_WAIT: Duration = Duration::from_secs(10);
/// How long a code waits for the joiner.
const CODE_TTL: Duration = Duration::from_secs(5 * 60);
/// How long after the introduction the server waits for each report.
const REPORT_WAIT: Duration = Duration::from_secs(60);
/// Most codes waiting at once.
const MAX_WAITING: usize = 1000;

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

/// What the joiner hands the waiting host.
struct Pairing {
    token: String,
    joiner: Addrs,
    host_report: oneshot::Sender<Report>,
}

struct State {
    waiting: Mutex<HashMap<u16, Waiting>>,
    next_id: Mutex<u64>,
    log: Option<PathBuf>,
}

/// Runs the server on `main` (pairing) and `probe` (NAT probe) until an
/// unrecoverable error. Each pairing's line goes to stdout and, if given,
/// is appended to `log`.
pub async fn serve(main: TcpListener, probe: TcpListener, log: Option<PathBuf>) -> io::Result<()> {
    let state = Arc::new(State {
        waiting: Mutex::new(HashMap::new()),
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
    let joiner = pairing.joiner;
    let peer = format!(
        "PEER {} {} {} {}",
        pairing.token, me.public, joiner.public, joiner.private
    );
    line::write(conn.get_mut(), &peer).await?;
    if let Ok(Ok(report)) = timeout(REPORT_WAIT, read_report(&mut conn)).await {
        let _ = pairing.host_report.send(report);
        line::write(conn.get_mut(), "BYE").await?;
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
    let token = random_token();
    let (host_report, host_reported) = oneshot::channel();
    let pairing = Pairing {
        token: token.clone(),
        joiner: me,
        host_report,
    };
    if waiting.pair.send(pairing).is_err() {
        return line::write(conn.get_mut(), "ERR host left").await;
    }
    let peer = format!(
        "PEER {token} {} {} {}",
        me.public, host.public, host.private
    );
    line::write(conn.get_mut(), &peer).await?;

    let (mine, theirs) = tokio::join!(
        timeout(REPORT_WAIT, read_report(&mut conn)),
        timeout(REPORT_WAIT, host_reported),
    );
    let mine = mine.ok().and_then(Result::ok);
    let theirs = theirs.ok().and_then(Result::ok);
    state.record(host, me, theirs.as_ref(), mine.as_ref());
    line::write(conn.get_mut(), "BYE").await
}

/// `RESULT key=value ...`: at most eight pairs of short lower-case words.
async fn read_report(conn: &mut BufReader<TcpStream>) -> io::Result<Report> {
    let text = line::expect(conn).await?;
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

fn random_token() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("no system randomness");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
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
