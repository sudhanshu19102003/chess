//! Stockfish, the opponent in VS COMPUTER games. It is not shipped with the
//! game: the first time the mode is opened, the official release build for
//! this system is downloaded from GitHub (with the platform's own `curl` and
//! `tar`, as `clipboard` uses the platform's tools), checked against the
//! SHA-256 pinned below and kept in the user's data folder. It then runs as
//! a child process spoken to in UCI.
//!
//! Like `net`, this runs in the background: the UI sends [`Request`]s and
//! gets [`Event`]s back. A worker thread owns the process; a second thread
//! reads its output.

use std::collections::VecDeque;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

/// The Stockfish release downloaded. Changing it means updating `ASSETS`
/// from the release's published digests.
pub const RELEASE: &str = "sf_19";
pub const NAME: &str = "Stockfish 19";
const DOWNLOAD: &str = "https://github.com/official-stockfish/Stockfish/releases/download/sf_19/";

/// One build in the release: its file name, size and SHA-256.
#[derive(Clone, Copy, Debug)]
pub struct Asset {
    pub name: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

/// The "universal" builds, which pick the fastest code for the CPU they run
/// on. Digests as GitHub publishes them for the release (checked 2026-10-25).
const ASSETS: [(&str, &str, Asset); 6] = [
    ("macos", "any", Asset {
        name: "stockfish-macos-universal.tar.gz",
        size: 82_323_876,
        sha256: "a1f0e3bcc5a6927a11fe6fc8e54a779754645f3c2bae2cf13420fd1957adaa77",
    }),
    ("linux", "x86_64", Asset {
        name: "stockfish-linux-x86-64-universal.tar.gz",
        size: 81_388_977,
        sha256: "9defc0d4e55d49c65a6d042f3e571a39fcea499ade6dbe741b53b8c65e03611f",
    }),
    ("linux", "aarch64", Asset {
        name: "stockfish-linux-arm64-universal.tar.gz",
        size: 80_181_250,
        sha256: "fe26cfd1d9db4c8af3d21e24d9ff34cacb31c1f940085a7583da11796f2bac01",
    }),
    ("android", "aarch64", Asset {
        name: "stockfish-android-arm64-universal.tar.gz",
        size: 80_233_187,
        sha256: "ebb24051aa4a222b4daaf049b882ecf1163d370c128fe02316602643f4d5e426",
    }),
    ("windows", "x86_64", Asset {
        name: "stockfish-windows-x86-64-universal.zip",
        size: 81_431_614,
        sha256: "3c8bf1f9ea66a09350a40df4f632288285ac206d99f33ab5842c408fc30b48a7",
    }),
    ("windows", "aarch64", Asset {
        name: "stockfish-windows-arm64-universal.zip",
        size: 80_190_536,
        sha256: "8372ad3f0d7276deb2c70f801f541ec7db463219fc6d9c7592864e542aa4f401",
    }),
];

/// Whether this is Android. In Termux the game is a Linux (musl) binary,
/// but Stockfish's Linux build is a plain static one, which Android's
/// loader refuses, so it needs the Android build.
fn android() -> bool {
    cfg!(target_os = "android")
        || (cfg!(target_os = "linux")
            && (std::env::var_os("TERMUX_VERSION").is_some() || Path::new("/system/bin/linker64").exists()))
}

/// The build for this system, if the release has one.
pub fn asset() -> Option<Asset> {
    let os = if android() { "android" } else { std::env::consts::OS };
    let arch = std::env::consts::ARCH;
    ASSETS.iter().find(|(o, a, _)| *o == os && (*a == "any" || *a == arch)).map(|&(_, _, asset)| asset)
}

/// The game's own data folder: `%LOCALAPPDATA%\chess` on Windows,
/// `~/Library/Application Support/chess` on macOS, and
/// `$XDG_DATA_HOME/chess` (or `~/.local/share/chess`) elsewhere.
pub fn data_dir() -> Option<PathBuf> {
    let var = |name| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    let base = if cfg!(windows) {
        var("LOCALAPPDATA")?
    } else if cfg!(target_os = "macos") {
        var("HOME")?.join("Library/Application Support")
    } else {
        var("XDG_DATA_HOME").or_else(|| Some(var("HOME")?.join(".local/share")))?
    };
    Some(base.join("chess"))
}

/// Where the downloaded engine lives.
pub fn install_dir() -> Option<PathBuf> {
    Some(data_dir()?.join("stockfish").join(RELEASE))
}

const EXE: &str = if cfg!(windows) { "stockfish.exe" } else { "stockfish" };

/// The engine to run: `$CHESS_STOCKFISH` if set (any UCI engine), else the
/// downloaded one if it is there.
pub fn installed() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("CHESS_STOCKFISH").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(path));
    }
    install_dir().map(|d| d.join(EXE)).filter(|p| p.is_file())
}

/// How strongly the computer plays. Below full strength Stockfish's own
/// Skill Level weakens it (it picks among its top moves with deliberate
/// mistakes), and a depth and time cap keep it from seeing far; the figures
/// follow Lichess's eight levels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Level {
    pub skill: u8,
    /// None: full strength, managing its own clock.
    pub depth: Option<u8>,
    pub movetime_ms: u64,
    pub name: &'static str,
}

pub const LEVELS: [Level; 8] = [
    Level { skill: 0, depth: Some(1), movetime_ms: 50, name: "beginner" },
    Level { skill: 3, depth: Some(2), movetime_ms: 100, name: "novice" },
    Level { skill: 6, depth: Some(3), movetime_ms: 150, name: "casual" },
    Level { skill: 9, depth: Some(5), movetime_ms: 200, name: "club player" },
    Level { skill: 12, depth: Some(8), movetime_ms: 300, name: "strong club player" },
    Level { skill: 15, depth: Some(11), movetime_ms: 400, name: "expert" },
    Level { skill: 18, depth: Some(15), movetime_ms: 700, name: "master" },
    Level { skill: 20, depth: None, movetime_ms: 0, name: "full strength" },
];

/// 4, a club player.
pub const DEFAULT_LEVEL: usize = 3;

/// A search for the computer's next move.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Think {
    /// Comes back with the move, so a late answer is not taken for a later
    /// question.
    pub id: u64,
    /// The first search of a game, which clears the engine's memory.
    pub new_game: bool,
    /// The game so far from the start position, in UCI.
    pub moves: Vec<String>,
    /// Index into `LEVELS`.
    pub level: usize,
    /// White's and Black's time left.
    pub left_ms: [u64; 2],
    pub increment_ms: u64,
}

impl Think {
    /// The UCI `go` line. Full strength plays on the clock; the other levels
    /// stop at their depth or time cap, and never spend more than a thirtieth
    /// of their remaining time.
    pub fn go(&self) -> String {
        let level = LEVELS[self.level.min(LEVELS.len() - 1)];
        let [w, b] = self.left_ms;
        let inc = self.increment_ms;
        match level.depth {
            None => format!("go wtime {w} btime {b} winc {inc} binc {inc}"),
            Some(depth) => {
                let own = if self.moves.len().is_multiple_of(2) { w } else { b };
                let ms = level.movetime_ms.min(own / 30 + inc / 2).max(10);
                format!("go depth {depth} movetime {ms}")
            }
        }
    }

    /// The UCI `position` line.
    pub fn position(&self) -> String {
        match self.moves.as_slice() {
            [] => "position startpos".into(),
            moves => format!("position startpos moves {}", moves.join(" ")),
        }
    }
}

/// What the UI asks of the engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Download Stockfish if needed, and start it.
    Prepare,
    Think(Think),
    /// Drop the search under way: its game is over.
    Stop,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// Downloading: bytes so far, of `total`.
    Progress { got: u64, total: u64 },
    /// Running and answering; `name` as it introduces itself.
    Ready { name: String },
    /// Could not download or start, or stopped.
    Failed(String),
    /// The move for search `id`, in UCI.
    Move { id: u64, uci: String },
}

enum Input {
    Request(Request),
    /// A line of the engine's output, from the process of that generation.
    Line(u64, String),
    /// That process closed its output.
    Exited(u64),
    Quit,
}

/// The UI's handle on the engine. The worker thread starts with the first
/// request.
pub struct Engine {
    worker: Option<(Sender<Input>, Receiver<()>)>,
    events: Receiver<Event>,
    events_tx: Sender<Event>,
    quit: Arc<AtomicBool>,
}

impl Default for Engine {
    fn default() -> Self {
        Engine::new()
    }
}

impl Engine {
    pub fn new() -> Engine {
        let (events_tx, events) = mpsc::channel();
        Engine { worker: None, events, events_tx, quit: Arc::new(AtomicBool::new(false)) }
    }

    pub fn request(&mut self, request: Request) {
        let (inputs, _) = self.worker.get_or_insert_with(|| {
            let (inputs_tx, inputs) = mpsc::channel();
            let (done_tx, done) = mpsc::channel();
            let mut worker = Worker {
                inputs,
                inputs_tx: inputs_tx.clone(),
                events: self.events_tx.clone(),
                quit: self.quit.clone(),
                process: None,
                name: String::new(),
                generation: 0,
                deferred: VecDeque::new(),
                search: None,
                queued: None,
            };
            thread::spawn(move || {
                worker.run();
                let _ = done_tx.send(());
            });
            (inputs_tx, done)
        });
        let _ = inputs.send(Input::Request(request));
    }

    /// Everything that has arrived since the last call.
    pub fn poll(&mut self) -> Vec<Event> {
        self.events.try_iter().collect()
    }
}

impl Drop for Engine {
    /// Stops a download and the engine, so neither outlives the game.
    fn drop(&mut self) {
        self.quit.store(true, Ordering::Relaxed);
        if let Some((inputs, done)) = self.worker.take() {
            let _ = inputs.send(Input::Quit);
            let _ = done.recv_timeout(Duration::from_secs(2));
        }
    }
}

struct Process {
    child: Child,
    stdin: ChildStdin,
}

/// A search under way; `id` is None once it was abandoned, so its move is
/// dropped.
struct Search {
    id: Option<u64>,
}

struct Worker {
    inputs: Receiver<Input>,
    inputs_tx: Sender<Input>,
    events: Sender<Event>,
    quit: Arc<AtomicBool>,
    process: Option<Process>,
    /// The engine's name, as it gave it.
    name: String,
    /// Counts processes started, to tell an old one's output from the
    /// current one's.
    generation: u64,
    /// Requests that came while waiting for the engine's answer.
    deferred: VecDeque<Input>,
    search: Option<Search>,
    /// A search asked for while an abandoned one was still stopping.
    queued: Option<Think>,
}

/// How long the engine gets to answer `uci` and `isready`.
const ANSWER_TIME: Duration = Duration::from_secs(15);

impl Worker {
    fn run(&mut self) {
        loop {
            let input = match self.deferred.pop_front() {
                Some(input) => input,
                None => match self.inputs.recv() {
                    Ok(input) => input,
                    Err(_) => break,
                },
            };
            match input {
                Input::Quit => break,
                // Already running: say so again for the screen that asks.
                Input::Request(Request::Prepare) if self.process.is_some() => {
                    self.emit(Event::Ready { name: self.name.clone() })
                }
                Input::Request(Request::Prepare) => _ = self.start(),
                Input::Request(Request::Think(think)) => self.think(think),
                Input::Request(Request::Stop) => self.stop(),
                Input::Line(generation, line) if generation == self.generation => self.line(&line),
                Input::Exited(generation) if generation == self.generation => {
                    self.process = None;
                    self.search = None;
                    self.queued = None;
                    self.emit(Event::Failed("Stockfish stopped unexpectedly".into()));
                }
                Input::Line(..) | Input::Exited(_) => {}
            }
        }
        self.shut_down();
    }

    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
    }

    /// Downloads the engine if needed and starts it; on failure says why.
    fn start(&mut self) -> Result<(), ()> {
        let result = (|| {
            let path = match installed() {
                Some(path) => path,
                None => install(&self.events, &self.quit)?,
            };
            self.launch(&path)
        })();
        result.map_err(|why| {
            self.kill();
            self.emit(Event::Failed(why));
        })
    }

    /// Runs the engine at `path` and waits for it to say it is ready.
    fn launch(&mut self, path: &Path) -> Result<(), String> {
        let mut child = Command::new(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("could not start {}: {e}", path.display()))?;
        let stdin = child.stdin.take().ok_or("no input to the engine")?;
        let stdout = child.stdout.take().ok_or("no output from the engine")?;
        self.generation += 1;
        let generation = self.generation;
        let lines = self.inputs_tx.clone();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if lines.send(Input::Line(generation, line)).is_err() {
                    return;
                }
            }
            let _ = lines.send(Input::Exited(generation));
        });
        self.process = Some(Process { child, stdin });
        self.send("uci")?;
        let said = self.expect("uciok")?;
        self.name = said.iter().find_map(|l| l.strip_prefix("id name ")).unwrap_or("the engine").trim().to_string();
        self.send("isready")?;
        self.expect("readyok")?;
        self.emit(Event::Ready { name: self.name.clone() });
        Ok(())
    }

    /// The engine's lines up to one starting with `want`, putting requests
    /// that come meanwhile aside.
    fn expect(&mut self, want: &str) -> Result<Vec<String>, String> {
        let deadline = Instant::now() + ANSWER_TIME;
        let mut said = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.inputs.recv_timeout(left) {
                Ok(Input::Line(g, line)) if g == self.generation => {
                    if line.starts_with(want) {
                        return Ok(said);
                    }
                    said.push(line);
                }
                Ok(Input::Exited(g)) if g == self.generation => return Err("the engine quit as it started".into()),
                Ok(Input::Line(..) | Input::Exited(_)) => {}
                Ok(Input::Quit) => {
                    self.deferred.push_back(Input::Quit);
                    return Err("quitting".into());
                }
                Ok(input) => self.deferred.push_back(input),
                Err(RecvTimeoutError::Timeout) => return Err(format!("the engine did not answer `{want}`")),
                Err(RecvTimeoutError::Disconnected) => return Err("quitting".into()),
            }
        }
    }

    fn send(&mut self, line: &str) -> Result<(), String> {
        let process = self.process.as_mut().ok_or("the engine is not running")?;
        writeln!(process.stdin, "{line}").and_then(|_| process.stdin.flush()).map_err(|e| format!("the engine stopped: {e}"))
    }

    fn think(&mut self, think: Think) {
        if self.process.is_none() && self.start().is_err() {
            return;
        }
        match &mut self.search {
            // One at a time: stop the old search and start this after its
            // (dropped) answer.
            Some(search) => {
                if search.id.take().is_some() {
                    let _ = self.send("stop");
                }
                self.queued = Some(think);
            }
            None => self.begin(think),
        }
    }

    fn begin(&mut self, think: Think) {
        let level = LEVELS[think.level.min(LEVELS.len() - 1)];
        let mut lines = Vec::new();
        if think.new_game {
            lines.push("ucinewgame".to_string());
        }
        lines.push(format!("setoption name Skill Level value {}", level.skill));
        lines.push(think.position());
        lines.push(think.go());
        let sent = lines.iter().try_for_each(|l| self.send(l));
        match sent {
            Ok(()) => self.search = Some(Search { id: Some(think.id) }),
            Err(why) => {
                self.kill();
                self.emit(Event::Failed(why));
            }
        }
    }

    fn stop(&mut self) {
        self.queued = None;
        if let Some(search) = &mut self.search
            && search.id.take().is_some()
        {
            let _ = self.send("stop");
        }
    }

    fn line(&mut self, line: &str) {
        let mut words = line.split_whitespace();
        if words.next() != Some("bestmove") {
            return;
        }
        if let Some(Search { id: Some(id) }) = self.search.take() {
            let uci = words.next().unwrap_or_default().to_string();
            self.emit(Event::Move { id, uci });
        }
        if let Some(think) = self.queued.take() {
            self.begin(think);
        }
    }

    fn kill(&mut self) {
        if let Some(mut p) = self.process.take() {
            let _ = p.child.kill();
            let _ = p.child.wait();
        }
        self.search = None;
        self.queued = None;
    }

    /// Asks the engine to quit, and makes sure it has.
    fn shut_down(&mut self) {
        if let Some(p) = &mut self.process {
            let _ = writeln!(p.stdin, "quit").and_then(|_| p.stdin.flush());
            let deadline = Instant::now() + Duration::from_millis(500);
            while Instant::now() < deadline && matches!(p.child.try_wait(), Ok(None)) {
                thread::sleep(Duration::from_millis(20));
            }
        }
        self.kill();
    }
}

/// A Windows tool from System32 rather than whatever is first on the PATH
/// (Git's GNU tar, say, which cannot open a zip).
fn tool(name: &str) -> PathBuf {
    if cfg!(windows)
        && let Some(root) = std::env::var_os("SystemRoot")
    {
        let path = Path::new(&root).join("System32").join(format!("{name}.exe"));
        if path.is_file() {
            return path;
        }
    }
    PathBuf::from(name)
}

/// Downloads, checks and unpacks this system's build into `install_dir`.
/// Reports progress; stops early if `quit` is set. The path of the engine.
fn install(events: &Sender<Event>, quit: &AtomicBool) -> Result<PathBuf, String> {
    let asset = asset().ok_or_else(|| {
        format!(
            "{NAME} has no build for {} {}. Install a UCI engine and set CHESS_STOCKFISH to its path.",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })?;
    let dir = install_dir().ok_or("no home folder to keep Stockfish in")?;
    fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    // Named per process, so two games downloading at once don't collide.
    let pid = std::process::id();
    let archive = dir.join(format!("download-{pid}-{}", asset.name));
    let unpack = dir.join(format!("unpack-{pid}"));
    let result = (|| {
        download(&asset, &archive, events, quit)?;
        check(&asset, &archive)?;
        unpack_engine(&archive, &unpack, &dir)
    })();
    let _ = fs::remove_file(&archive);
    let _ = fs::remove_dir_all(&unpack);
    result
}

fn download(asset: &Asset, to: &Path, events: &Sender<Event>, quit: &AtomicBool) -> Result<(), String> {
    let url = format!("{DOWNLOAD}{}", asset.name);
    let curl = Command::new(tool("curl"))
        .args(["-fsSL", "--connect-timeout", "8", "--retry", "3", "-o"])
        .arg(to)
        .arg(&url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn();
    // Some Linux systems have wget but no curl.
    let mut child = match curl {
        Ok(child) => child,
        Err(_) => Command::new("wget")
            .args(["-q", "-O"])
            .arg(to)
            .arg(&url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|_| "downloading Stockfish needs curl (or wget); please install it".to_string())?,
    };
    let status = loop {
        if quit.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("cancelled".into());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => return Err(format!("download failed: {e}")),
        }
        let got = fs::metadata(to).map_or(0, |m| m.len());
        let _ = events.send(Event::Progress { got, total: asset.size });
        thread::sleep(Duration::from_millis(150));
    };
    if !status.success() {
        let mut said = String::new();
        if let Some(mut err) = child.stderr.take() {
            let _ = err.read_to_string(&mut said);
        }
        let said = said.trim();
        return Err(if said.is_empty() { format!("download failed ({status})") } else { format!("download failed: {said}") });
    }
    let _ = events.send(Event::Progress { got: asset.size, total: asset.size });
    Ok(())
}

/// Refuses a download whose SHA-256 is not the release's.
fn check(asset: &Asset, archive: &Path) -> Result<(), String> {
    let mut file = fs::File::open(archive).map_err(|e| format!("could not read the download: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0; 1 << 16];
    loop {
        let n = file.read(&mut buf).map_err(|e| format!("could not read the download: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if digest != asset.sha256 {
        return Err(format!("the download did not match {NAME}'s published checksum, so it was not used"));
    }
    Ok(())
}

/// Unpacks `archive` into `scratch` and moves the engine (and its licence)
/// into `dir`. The engine is put in place last, by rename, so it is there
/// only once complete.
fn unpack_engine(archive: &Path, scratch: &Path, dir: &Path) -> Result<PathBuf, String> {
    let _ = fs::remove_dir_all(scratch);
    fs::create_dir_all(scratch).map_err(|e| format!("could not unpack: {e}"))?;
    let flags = if archive.extension().is_some_and(|e| e == "zip") { "-xf" } else { "-xzf" };
    let out = Command::new(tool("tar"))
        .arg(flags)
        .arg(archive)
        .arg("-C")
        .arg(scratch)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("unpacking Stockfish needs tar: {e}"))?;
    if !out.status.success() {
        return Err(format!("could not unpack: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    // The archive holds stockfish/stockfish-<build>[.exe] and its sources.
    let folder = scratch.join("stockfish");
    let engine = fs::read_dir(&folder)
        .map_err(|e| format!("unexpected archive: {e}"))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .find(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            p.is_file() && name.starts_with("stockfish") && (p.extension().is_none() || name.ends_with(".exe"))
        })
        .ok_or("the archive had no engine in it")?;
    let _ = fs::copy(folder.join("Copying.txt"), dir.join("COPYING.txt"));
    let target = dir.join(EXE);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&engine, fs::Permissions::from_mode(0o755)).map_err(|e| format!("could not install: {e}"))?;
    }
    fs::rename(&engine, &target).map_err(|e| format!("could not install: {e}"))?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn think(moves: usize, level: usize) -> Think {
        Think {
            id: 1,
            new_game: false,
            moves: vec!["e2e4".into(); moves],
            level,
            left_ms: [60_000, 30_000],
            increment_ms: 0,
        }
    }

    #[test]
    fn go_lines() {
        assert_eq!(think(0, 0).go(), "go depth 1 movetime 50");
        // Black, with 30 s left: a thirtieth is 1 s, above the cap.
        assert_eq!(think(1, 6).go(), "go depth 15 movetime 700");
        assert_eq!(think(1, 7).go(), "go wtime 60000 btime 30000 winc 0 binc 0");
        // Low on time, the cap gives way.
        let short = Think { left_ms: [3_000, 3_000], ..think(0, 6) };
        assert_eq!(short.go(), "go depth 15 movetime 100");
    }

    #[test]
    fn position_lines() {
        assert_eq!(think(0, 0).position(), "position startpos");
        assert_eq!(think(2, 0).position(), "position startpos moves e2e4 e2e4");
    }

    #[test]
    fn every_target_has_a_build() {
        for (os, arch) in [("macos", "x86_64"), ("macos", "aarch64"), ("linux", "x86_64"), ("linux", "aarch64"), ("windows", "x86_64"), ("windows", "aarch64"), ("android", "aarch64")] {
            assert!(ASSETS.iter().any(|(o, a, _)| *o == os && (*a == "any" || *a == arch)), "{os} {arch}");
        }
        assert!(asset().is_some());
    }
}
