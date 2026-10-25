use std::time::{Duration, Instant};

use chess_core::{Color as Side, Game, Kind, Move, Piece, Square};
use chess_net::connect::Stage;
use chess_net::link;
use chess_verify::{Received, Signal};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::clock::{self, Clock};
use crate::engine::{self, Think};
use crate::layout::{self, Fit};
use crate::net::{Failure, NetEvent, Request};
use crate::online::Online;
use crate::panel::{self, HomeHit};

const SLIDE_TIME: Duration = Duration::from_millis(180);
/// With auto flip, how long the moved piece stays in view, after it lands,
/// before the board turns.
const FLIP_PAUSE: Duration = Duration::from_millis(500);
/// From choosing a mode to White's clock starting.
pub const COUNTDOWN: Duration = Duration::from_secs(3);
/// From the end of a game to its pop-up, so the last move is seen landing.
pub const END_PAUSE: Duration = Duration::from_millis(900);
/// How long a first `r` mid-game waits for the second.
const LEAVE_TIME: Duration = Duration::from_secs(3);
/// How long the copy button says COPIED.
const COPIED_TIME: Duration = Duration::from_secs(2);
/// Suggestions shown for a typed move.
pub const SUGGESTIONS: usize = 5;
/// Keys that start typing a move: a piece letter, or O for castling.
const MOVE_KEYS: &str = "KQRBNPO";

/// Who the game is against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Two players at one keyboard.
    Offline,
    PeerToPeer,
    Computer,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Offline, Mode::PeerToPeer, Mode::Computer];

    pub fn name(self) -> &'static str {
        match self {
            Mode::Offline => "OFFLINE",
            Mode::PeerToPeer => "PEER TO PEER",
            Mode::Computer => "VS COMPUTER",
        }
    }
}

/// What the window shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    /// The menu: choose a mode and a time control. No board.
    Home,
    /// Setting up a peer-to-peer game: see `App::lobby`. No board.
    Lobby,
    /// Setting up a game against the computer: see `App::setup`. No board.
    Setup,
    /// The board is set up and White's clock starts at `go`.
    Countdown { go: Instant },
    Playing,
}

/// The steps of setting up a peer-to-peer game.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lobby {
    /// Host or join. The host's time control is the one played.
    Choose { pick: Pick },
    /// Typing the code to join; `error` once Enter found it is not a code.
    Code { input: String, error: bool },
    /// Waiting on the network; `code` is the host's, once the server gave it.
    Connecting { request: Request, stage: Stage, code: Option<String> },
    Failed { request: Request, failure: Failure },
}

/// The lobby's buttons, top to bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    /// Through the rendezvous server.
    Host,
    /// On the local network, with no server.
    HostLan,
    Join,
}

impl Pick {
    pub const ALL: [Pick; 3] = [Pick::Host, Pick::HostLan, Pick::Join];

    pub fn name(self) -> &'static str {
        match self {
            Pick::Host => "HOST ONLINE",
            Pick::HostLan => "HOST ON THIS NETWORK",
            Pick::Join => "JOIN WITH A CODE",
        }
    }

    /// The button `by` places further down, wrapping round.
    fn step(self, by: usize) -> Pick {
        let at = Pick::ALL.iter().position(|&p| p == self).unwrap_or(0);
        Pick::ALL[(at + by) % Pick::ALL.len()]
    }
}

impl Lobby {
    /// The button that led here.
    pub fn pick(&self) -> Pick {
        match self {
            Lobby::Choose { pick } => *pick,
            Lobby::Code { .. } => Pick::Join,
            Lobby::Connecting { request, .. } | Lobby::Failed { request, .. } => match request {
                Request::Host(_) => Pick::Host,
                Request::HostLan(_) => Pick::HostLan,
                _ => Pick::Join,
            },
        }
    }

    /// Whether the game is being looked for on the local network.
    pub fn lan(&self) -> bool {
        match self {
            Lobby::Connecting { request, .. } | Lobby::Failed { request, .. } => on_lan(request),
            _ => false,
        }
    }
}

/// Whether `request` looks for the game on the local network.
fn on_lan(request: &Request) -> bool {
    match request {
        Request::HostLan(_) => true,
        Request::Join(code) => chess_net::code::parse_lan_code(code).is_some(),
        _ => false,
    }
}

/// The colour the player takes against the computer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Colour {
    White,
    Black,
    Random,
}

impl Colour {
    pub const ALL: [Colour; 3] = [Colour::White, Colour::Black, Colour::Random];

    pub fn name(self) -> &'static str {
        match self {
            Colour::White => "WHITE",
            Colour::Black => "BLACK",
            Colour::Random => "RANDOM",
        }
    }

    /// The side played, a coin flip for `Random`.
    fn side(self) -> Side {
        use std::hash::{BuildHasher, RandomState};
        match self {
            Colour::White => Side::White,
            Colour::Black => Side::Black,
            Colour::Random if RandomState::new().hash_one(0u8) & 1 == 0 => Side::White,
            Colour::Random => Side::Black,
        }
    }
}

/// The rows of the VS COMPUTER setup screen, top to bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    Level,
    Colour,
    Time,
    Play,
}

impl Setting {
    pub const ALL: [Setting; 4] = [Setting::Level, Setting::Colour, Setting::Time, Setting::Play];
}

/// The choices for a game against the computer, kept between games.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Setup {
    /// The row that left and right change.
    pub focus: Setting,
    /// Index into `engine::LEVELS`.
    pub level: usize,
    pub colour: Colour,
}

/// Where Stockfish stands, as the setup screen shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineStatus {
    /// Not asked for yet.
    Unknown,
    /// Asked to start; it is looking for, or starting, the engine.
    Starting,
    Downloading { got: u64, total: u64 },
    Ready { name: String },
    Failed(String),
}

/// A game against the computer in progress.
#[derive(Clone, Debug)]
pub struct Versus {
    /// The player's side; the engine plays the other.
    pub human: Side,
    /// Index into `engine::LEVELS`.
    pub level: usize,
    /// The search waiting for an answer.
    pub asked: Option<u64>,
    /// Searches asked for so far this game.
    pub asks: u64,
    /// The engine failed mid-game, which ends it with no result.
    pub error: Option<String>,
}

/// A move as shown in the history panel.
#[derive(Clone, Debug)]
pub struct MoveRecord {
    pub piece: Piece,
    pub san: String,
    pub captured: Option<Piece>,
}

/// Pieces gliding to their new squares after a move: one, or king and
/// rook when castling.
pub struct Slide {
    pub movers: Vec<(Piece, Square, Square)>,
    /// The captured piece stays on its square until the mover arrives.
    pub captured: Option<(Piece, Square)>,
    pub start: Instant,
}

impl Slide {
    /// 0..1, eased so the piece decelerates into its square.
    pub fn progress(&self, now: Instant) -> f32 {
        let t = now.saturating_duration_since(self.start).as_secs_f32() / SLIDE_TIME.as_secs_f32();
        1.0 - (1.0 - t.min(1.0)).powi(3)
    }
}

/// A move being typed, like "Nf3": a piece letter, then a square.
#[derive(Clone, Debug, Default)]
pub struct MoveInput {
    pub text: String,
    /// The suggestion Enter would play, an index into `App::suggestions`.
    pub choice: usize,
}

/// A legal move matching what was typed.
#[derive(Clone, Debug)]
pub struct Suggestion {
    pub mv: Move,
    pub san: String,
    /// What was typed spells this move in full, not just its start.
    pub exact: bool,
}

/// A piece held under the mouse button.
#[derive(Clone, Copy, Debug)]
pub struct Drag {
    pub from: Square,
    /// The cell under the mouse.
    pub at: (i32, i32),
    /// The mouse has left the piece's square, so the piece follows it.
    pub moved: bool,
    /// The piece was already picked up, so a click without a drag puts it down.
    pub was_selected: bool,
}

/// The buttons of the pop-up at the end of a game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndChoice {
    /// The same mode again: a fresh countdown offline, the lobby online.
    NewGame,
    Home,
}

/// A pawn has reached the last rank and the player is choosing its piece.
#[derive(Clone, Copy, Debug)]
pub struct Promotion {
    pub from: Square,
    pub to: Square,
    /// Index into `Kind::PROMOTIONS`.
    pub choice: usize,
}

pub struct App {
    pub screen: Screen,
    /// The mode chosen, or highlighted on the home screen.
    pub mode: Mode,
    /// Why the last network game was drawn, shown on the home screen until
    /// a key is pressed.
    pub drawn: Option<String>,
    pub game: Game,
    pub cursor: Square,
    pub selected: Option<Square>,
    pub promotion: Option<Promotion>,
    pub last_move: Option<(Square, Square)>,
    pub history: Vec<MoveRecord>,
    /// Draw the board from Black's side.
    pub flipped: bool,
    /// Offline, turn the board after each move so the side to move is at
    /// the bottom.
    pub auto_flip: bool,
    /// When the board turns next, with auto flip.
    pub flip_at: Option<Instant>,
    pub quit: bool,
    pub clock: Clock,
    pub slide: Option<Slide>,
    /// For animation timing.
    pub started: Instant,
    /// Draw pieces at twice the board's resolution with octant characters.
    pub hires: bool,
    /// The terminal's size in cells, which decides whether the board or a
    /// zoom message shows.
    pub size: (u16, u16),
    /// The user chose to play in a window bigger than the layout needs
    /// instead of zooming in.
    pub play_anyway: bool,
    pub lobby: Lobby,
    /// A peer-to-peer game in progress.
    pub online: Option<Online>,
    /// For the network, taken by the main loop.
    pub requests: Vec<Request>,
    /// The rendezvous server, as shown in the lobby.
    pub server: String,
    /// A move being typed.
    pub input: Option<MoveInput>,
    pub drag: Option<Drag>,
    /// Text for the clipboard, taken by the main loop.
    pub clipboard: Option<String>,
    /// When the game code was last copied, and whether that worked.
    pub copied: Option<(Instant, bool)>,
    /// The side that resigned. Online, mirrors the session's chain.
    pub resigned: Option<Side>,
    /// The side whose draw offer stands, until the other side moves.
    pub draw_offer: Option<Side>,
    /// A draw offer was accepted.
    pub agreed_draw: bool,
    /// When resign was first pressed; a second press resigns.
    pub resigning: Option<Instant>,
    /// The VS COMPUTER choices.
    pub setup: Setup,
    pub engine: EngineStatus,
    /// A game against the computer.
    pub versus: Option<Versus>,
    /// For the engine, taken by the main loop.
    pub engine_requests: Vec<engine::Request>,
    /// When this client saw the game end, for the pop-up and its confetti.
    pub ended_at: Option<Instant>,
    /// The pop-up's chosen button.
    pub end_choice: EndChoice,
    /// Esc put the pop-up away to look at the final position.
    pub end_hidden: bool,
}

impl App {
    pub fn new(now: Instant) -> Self {
        App {
            screen: Screen::Home,
            mode: Mode::Offline,
            drawn: None,
            game: Game::new(),
            cursor: chess_core::square(4, 1),
            selected: None,
            promotion: None,
            last_move: None,
            history: Vec::new(),
            flipped: false,
            auto_flip: true,
            flip_at: None,
            quit: false,
            clock: Clock::new(clock::DEFAULT_PRESET),
            slide: None,
            started: now,
            hires: false,
            size: layout::needed(),
            play_anyway: false,
            lobby: Lobby::Choose { pick: Pick::Host },
            online: None,
            requests: Vec::new(),
            server: String::new(),
            input: None,
            drag: None,
            clipboard: None,
            copied: None,
            resigned: None,
            draw_offer: None,
            agreed_draw: false,
            resigning: None,
            setup: Setup { focus: Setting::Level, level: engine::DEFAULT_LEVEL, colour: Colour::White },
            engine: EngineStatus::Unknown,
            versus: None,
            engine_requests: Vec::new(),
            ended_at: None,
            end_choice: EndChoice::NewGame,
            end_hidden: false,
        }
    }

    /// What the window shows: the board, or a message to zoom out or in.
    pub fn fit(&self) -> Fit {
        match layout::fit(self.size.0 as i32, self.size.1 as i32) {
            Fit::TooBig if self.play_anyway => Fit::Good,
            fit => fit,
        }
    }

    pub fn piece_at(&self, sq: Square) -> Option<Piece> {
        self.game.position().piece_at(sq)
    }

    pub fn to_move(&self) -> Side {
        self.game.position().turn()
    }

    /// How the game ended, if it has: the winner (`None` for a draw) and why.
    pub fn result(&self) -> Option<(Option<Side>, &'static str)> {
        if let Some(outcome) = self.game.outcome() {
            return Some((outcome.winner(), outcome.reason()));
        }
        // Running out of time loses, unless the opponent could never mate.
        if let Some(loser) = self.clock.flagged {
            let winner = loser.opponent();
            return Some((self.game.position().has_mating_material(winner).then_some(winner), "time"));
        }
        if let Some(loser) = self.resigned {
            return Some((Some(loser.opponent()), "resignation"));
        }
        self.agreed_draw.then_some((None, "agreement"))
    }

    /// No more moves: the game has a result, or a network game was dropped.
    pub fn over(&self) -> bool {
        self.result().is_some()
            || self.online.as_ref().is_some_and(|o| o.dropped.is_some())
            || self.versus.as_ref().is_some_and(|v| v.error.is_some())
    }

    /// Whether the game-over pop-up shows: a moment after the game ended,
    /// unless Esc put it away.
    pub fn end_popup(&self, now: Instant) -> bool {
        self.screen == Screen::Playing && !self.end_hidden && self.ended_at.is_some_and(|at| now >= at + END_PAUSE)
    }

    /// The pop-up's buttons: another game of the same kind, or home.
    fn end_go(&mut self, choice: EndChoice, now: Instant) {
        match choice {
            // A network game needs a new code, so it starts again in the lobby.
            EndChoice::NewGame if self.online.is_some() => {
                self.go_home(now);
                self.screen = Screen::Lobby;
            }
            EndChoice::NewGame => self.begin(now),
            EndChoice::Home => self.go_home(now),
        }
    }

    fn on_end_key(&mut self, code: KeyCode, now: Instant) {
        match code {
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down | KeyCode::Tab | KeyCode::Char('h' | 'l' | 'k' | 'j') => {
                self.end_choice = match self.end_choice {
                    EndChoice::NewGame => EndChoice::Home,
                    EndChoice::Home => EndChoice::NewGame,
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') => self.end_go(self.end_choice, now),
            KeyCode::Char('n') => self.end_go(EndChoice::NewGame, now),
            KeyCode::Char('r') => self.end_go(EndChoice::Home, now),
            KeyCode::Esc => self.end_hidden = true,
            KeyCode::Char('f') => self.flipped = !self.flipped,
            KeyCode::Char('g') => self.hires = !self.hires,
            _ => {}
        }
    }

    /// The side this player plays: online or against the computer. Offline
    /// both sides are theirs.
    pub fn me(&self) -> Option<Side> {
        match (&self.online, &self.versus) {
            (Some(o), _) => Some(o.me()),
            (None, Some(v)) => Some(v.human),
            _ => None,
        }
    }

    /// Two players at one keyboard: no network, no computer.
    fn local(&self) -> bool {
        self.me().is_none()
    }

    /// Whether this player may move now: always offline, only on their own
    /// turn online and against the computer.
    fn my_turn(&self) -> bool {
        self.me().is_none_or(|me| me == self.to_move())
    }

    /// Who the draw and resign buttons act for: this player online or
    /// against the computer, the side to move offline.
    pub fn actor(&self) -> Side {
        self.me().unwrap_or(self.to_move())
    }

    /// Whether a second resign press now resigns.
    pub fn resign_armed(&self, now: Instant) -> bool {
        self.resigning.is_some_and(|at| now.saturating_duration_since(at) < LEAVE_TIME)
    }

    /// The signal waiting for this player's turn, online.
    pub fn pending(&self) -> Option<Signal> {
        self.online.as_ref().and_then(|o| o.pending)
    }

    /// What the draw button would do now, if anything.
    pub fn draw_action(&self) -> Option<Signal> {
        // The computer plays on.
        if self.over() || self.screen != Screen::Playing || self.pending().is_some() || self.versus.is_some() {
            return None;
        }
        let actor = self.actor();
        match self.draw_offer {
            Some(by) if by != actor => Some(Signal::AcceptDraw),
            // Offers go on your own turn, one at a time.
            None if self.my_turn() => Some(Signal::OfferDraw),
            _ => None,
        }
    }

    /// The draw button or `d`: offers a draw, or accepts the opponent's.
    /// Online, an acceptance on the opponent's turn waits for their move.
    fn draw(&mut self, now: Instant) {
        if let Some(kind) = self.draw_action() {
            self.signal(kind, now);
        }
    }

    /// The resign button or `x`: the first press arms it, a second within
    /// a few seconds resigns. Online, on the opponent's turn the
    /// resignation waits for their move.
    fn resign(&mut self, now: Instant) {
        if self.over() || self.screen != Screen::Playing || self.pending().is_some() {
            return;
        }
        if !self.resign_armed(now) {
            self.resigning = Some(now);
            return;
        }
        self.resigning = None;
        self.signal(Signal::Resign, now);
    }

    /// Resigns, offers or accepts a draw. Online it is signed and sent, or
    /// queued if it is not this player's turn.
    fn signal(&mut self, kind: Signal, now: Instant) {
        let (actor, my_turn) = (self.actor(), self.my_turn());
        if let Some(o) = &mut self.online {
            if !my_turn {
                o.pending = Some(kind);
                return;
            }
            let Ok(msg) = o.session.signal(kind) else { return };
            self.requests.push(Request::Send(msg.encode()));
            self.sync_online(now);
            return;
        }
        match kind {
            Signal::Resign => self.resigned = Some(actor),
            Signal::OfferDraw => self.draw_offer = Some(actor),
            Signal::AcceptDraw => self.agreed_draw = true,
            Signal::Flag => {}
        }
        if self.over() {
            self.clock.halt(now);
        }
    }

    /// Sends what was queued for our turn, once it has come.
    fn send_pending(&mut self, now: Instant) {
        if self.over() || !self.my_turn() {
            return;
        }
        if let Some(kind) = self.online.as_mut().and_then(|o| o.pending.take()) {
            self.signal(kind, now);
        }
    }

    /// The legal moves of the selected piece.
    pub fn targets(&self) -> Vec<Move> {
        self.selected.map(|sq| self.game.position().legal_moves_from(sq)).unwrap_or_default()
    }

    /// The four squares of the promotion picker, from the promotion square
    /// toward the middle of the board, with the piece each offers.
    pub fn promotion_squares(&self) -> Option<[(Square, Kind); 4]> {
        let p = self.promotion?;
        let back = -self.to_move().forward();
        let (file, rank) = (chess_core::file_of(p.to), chess_core::rank_of(p.to) as i32);
        Some(std::array::from_fn(|i| (chess_core::square(file, (rank + back * i as i32) as usize), Kind::PROMOTIONS[i])))
    }

    /// Seconds left before the clock starts, counting 3, 2, 1, while the
    /// countdown runs.
    pub fn countdown(&self, now: Instant) -> Option<u64> {
        match self.screen {
            Screen::Countdown { go } => Some(go.saturating_duration_since(now).as_secs_f32().ceil().max(1.0) as u64),
            _ => None,
        }
    }

    /// A fresh board for the chosen mode, keeping the player's settings.
    fn fresh(&self, now: Instant) -> App {
        App {
            mode: self.mode,
            // With auto flip a new game starts from White's side.
            flipped: self.flipped && !self.auto_flip,
            auto_flip: self.auto_flip,
            hires: self.hires,
            size: self.size,
            play_anyway: self.play_anyway,
            clock: Clock::new(self.clock.preset),
            started: self.started,
            server: self.server.clone(),
            setup: self.setup,
            engine: self.engine.clone(),
            ..App::new(now)
        }
    }

    /// Back to the home screen, leaving any network game or stopping the
    /// computer.
    fn go_home(&mut self, now: Instant) {
        let (online, versus) = (self.online.is_some(), self.versus.is_some());
        *self = self.fresh(now);
        if online {
            self.requests.push(Request::Hangup);
        }
        if versus {
            self.engine_requests.push(engine::Request::Stop);
        }
    }

    /// A network game is drawn: the opponent sent a refused message, or
    /// their timekeeping failed the lag check. Hang up and go home.
    fn draw_and_leave(&mut self, why: String, now: Instant) {
        self.go_home(now);
        self.drawn = Some(why);
    }

    /// `r` in a game. Leaving a network game that is still on takes a
    /// second press, since it ends the game without a result.
    fn leave(&mut self, now: Instant) {
        let unfinished = !self.over();
        match &mut self.online {
            Some(o) if unfinished && o.leaving.is_none() => o.leaving = Some(now),
            _ => self.go_home(now),
        }
    }

    /// Sets up a new game and counts down to its start. Against the
    /// computer the board turns to the player's side.
    pub fn begin(&mut self, now: Instant) {
        if self.versus.is_some() {
            self.engine_requests.push(engine::Request::Stop);
        }
        let requests = std::mem::take(&mut self.engine_requests);
        *self = App { screen: Screen::Countdown { go: now + COUNTDOWN }, engine_requests: requests, ..self.fresh(now) };
        if self.mode == Mode::Computer {
            let human = self.setup.colour.side();
            self.versus = Some(Versus { human, level: self.setup.level, asked: None, asks: 0, error: None });
            self.flipped = human == Side::Black;
            self.cursor = chess_core::square(4, if human == Side::White { 1 } else { 6 });
        }
    }

    /// On the computer's turn, asks the engine for its move, once the last
    /// move has finished sliding in.
    fn ask_engine(&mut self, now: Instant) {
        if self.screen != Screen::Playing || self.over() || self.slide.is_some() || self.my_turn() {
            return;
        }
        let Some(v) = &mut self.versus else { return };
        if v.asked.is_some() {
            return;
        }
        v.asks += 1;
        v.asked = Some(v.asks);
        let left = [Side::White, Side::Black].map(|s| self.clock.remaining(s, now).as_millis() as u64);
        let think = Think {
            id: v.asks,
            new_game: v.asks == 1,
            moves: self.game.moves().iter().map(|m| m.uci()).collect(),
            level: v.level,
            left_ms: left,
            increment_ms: self.clock.control().increment * 1000,
        };
        self.engine_requests.push(engine::Request::Think(think));
    }

    /// Something from the engine.
    pub fn on_engine(&mut self, event: engine::Event, now: Instant) {
        match event {
            engine::Event::Progress { got, total } => self.engine = EngineStatus::Downloading { got, total },
            engine::Event::Ready { name } => self.engine = EngineStatus::Ready { name },
            engine::Event::Failed(why) => {
                let over = self.over();
                if let Some(v) = &mut self.versus
                    && !over
                {
                    v.error = Some(why.clone());
                    self.clock.halt(now);
                }
                self.engine = EngineStatus::Failed(why);
            }
            engine::Event::Move { id, uci } => {
                let over = self.over();
                let Some(v) = &mut self.versus else { return };
                if v.asked != Some(id) || over {
                    return;
                }
                v.asked = None;
                let legal = Move::parse_uci(&uci).filter(|mv| self.game.position().legal_moves().contains(mv));
                match legal {
                    Some(mv) => _ = self.show_move(mv, now),
                    None => {
                        v.error = Some(format!("the engine sent a move that is not legal here ({uci})"));
                        self.clock.halt(now);
                    }
                }
            }
        }
    }

    /// Opens the VS COMPUTER setup, starting Stockfish (and downloading it
    /// the first time) unless it is already up or on its way.
    fn open_setup(&mut self) {
        self.screen = Screen::Setup;
        if matches!(self.engine, EngineStatus::Unknown | EngineStatus::Failed(_)) {
            self.engine = EngineStatus::Starting;
            self.engine_requests.push(engine::Request::Prepare);
        }
    }

    /// The setup screen: up and down choose a row, left and right change
    /// it, 1 to 8 pick a level, Enter plays once the engine is ready (or
    /// tries again after a failure), Esc goes back.
    fn on_setup_key(&mut self, code: KeyCode, now: Instant) {
        let rows = Setting::ALL.len();
        let at = Setting::ALL.iter().position(|&s| s == self.setup.focus).unwrap_or(0);
        let change = |app: &mut App, by: i32| match app.setup.focus {
            Setting::Level => {
                app.setup.level = (app.setup.level as i32 + by).clamp(0, engine::LEVELS.len() as i32 - 1) as usize;
            }
            Setting::Colour => {
                let i = Colour::ALL.iter().position(|&c| c == app.setup.colour).unwrap_or(0) as i32;
                app.setup.colour = Colour::ALL[(i + by).rem_euclid(Colour::ALL.len() as i32) as usize];
            }
            Setting::Time => {
                let presets = clock::PRESETS.len() as i32;
                app.clock = Clock::new((app.clock.preset as i32 + by).rem_euclid(presets) as usize);
            }
            Setting::Play => {}
        };
        match code {
            KeyCode::Up | KeyCode::Char('k') => self.setup.focus = Setting::ALL[(at + rows - 1) % rows],
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => self.setup.focus = Setting::ALL[(at + 1) % rows],
            KeyCode::Right | KeyCode::Char('l') => change(self, 1),
            KeyCode::Left | KeyCode::Char('h') => change(self, -1),
            KeyCode::Char('t') => {
                self.setup.focus = Setting::Time;
                change(self, 1);
            }
            KeyCode::Char(c @ '1'..='8') => {
                self.setup.focus = Setting::Level;
                self.setup.level = c as usize - '1' as usize;
            }
            KeyCode::Enter | KeyCode::Char(' ') => match self.engine {
                EngineStatus::Ready { .. } => self.begin(now),
                EngineStatus::Failed(_) => self.open_setup(),
                _ => {}
            },
            KeyCode::Esc => self.screen = Screen::Home,
            _ => {}
        }
    }

    pub fn tick(&mut self, now: Instant) {
        if let Screen::Countdown { go } = self.screen
            && now >= go
        {
            self.screen = Screen::Playing;
            match &mut self.online {
                // A move may already have come in during our countdown.
                Some(o) => _ = o.turn_since.get_or_insert(go),
                None => self.clock.start(self.to_move(), go),
            }
        }
        if let Some(o) = &mut self.online {
            if o.leaving.is_some_and(|at| now.saturating_duration_since(at) >= LEAVE_TIME) {
                o.leaving = None;
            }
            if self.screen == Screen::Playing {
                if o.own_time_up(now) {
                    if let Ok(flag) = o.session.flag() {
                        self.requests.push(Request::Send(flag.encode()));
                    }
                } else if o.opponent_silent(now) {
                    o.silent = true;
                }
            }
            self.sync_online(now);
        } else {
            self.clock.tick(now);
        }
        if self.slide.as_ref().is_some_and(|s| now.saturating_duration_since(s.start) >= SLIDE_TIME) {
            self.slide = None;
        }
        if self.screen == Screen::Playing && self.ended_at.is_none() && self.result().is_some() {
            self.ended_at = Some(now);
        }
        // Not while a piece is being dragged: the board would turn under it.
        if self.flip_at.is_some_and(|at| now >= at) && self.drag.is_none() {
            self.flip_at = None;
            if self.auto_flip && self.local() {
                self.flipped = self.to_move() == Side::Black;
            }
        }
        self.ask_engine(now);
    }

    /// `a`: auto flip on or off. Turning it on turns the board at once.
    fn toggle_auto_flip(&mut self, now: Instant) {
        self.auto_flip = !self.auto_flip;
        self.flip_at = self.auto_flip.then_some(now);
    }

    pub fn on_key(&mut self, key: KeyEvent, now: Instant) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        // Game codes have q in them (quartz, quill), and a typed promotion
        // may end in q, so typing one does not quit.
        let typing = (self.screen == Screen::Lobby && matches!(self.lobby, Lobby::Code { .. })) || self.input.is_some();
        let quit = (key.code == KeyCode::Char('q') && !typing)
            || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL));
        if quit {
            self.quit = true;
            return;
        }
        // While a zoom message shows, only quitting works, plus Enter to play
        // anyway in a too-big window.
        if self.fit() != Fit::Good {
            if key.code == KeyCode::Enter && self.fit() == Fit::TooBig {
                self.play_anyway = true;
            }
            return;
        }
        match self.screen {
            Screen::Home => return self.on_home_key(key.code, now),
            Screen::Lobby => return self.on_lobby_key(key.code),
            Screen::Setup => return self.on_setup_key(key.code, now),
            // The board shows but waits for the clock; Esc goes back.
            Screen::Countdown { .. } => {
                match key.code {
                    KeyCode::Esc => self.go_home(now),
                    KeyCode::Char('f') => self.flipped = !self.flipped,
                    KeyCode::Char('a') if self.local() => self.toggle_auto_flip(now),
                    KeyCode::Char('g') => self.hires = !self.hires,
                    _ => {}
                }
                return;
            }
            Screen::Playing => {}
        }
        if self.end_popup(now) {
            return self.on_end_key(key.code, now);
        }
        if let Some(p) = self.promotion {
            self.on_promotion_key(key.code, p, now);
            return;
        }
        if self.input.is_some() {
            return self.on_input_key(key.code, now);
        }
        match key.code {
            KeyCode::Char(c) if MOVE_KEYS.contains(c) && self.can_move() => {
                self.selected = None;
                self.input = Some(MoveInput::default());
                self.on_input_key(key.code, now);
            }
            KeyCode::Char('f') => self.flipped = !self.flipped,
            KeyCode::Char('a') if self.local() => self.toggle_auto_flip(now),
            KeyCode::Char('g') => self.hires = !self.hires,
            KeyCode::Char('r') => self.leave(now),
            KeyCode::Char('d') => self.draw(now),
            KeyCode::Char('x') => self.resign(now),
            // Nobody can pause the other player's clock, and the computer's
            // runs while it thinks.
            KeyCode::Char('p') if self.online.is_none() && self.my_turn() => self.clock.toggle_pause(now),
            // Esc brings back a game-over pop-up put away to see the board.
            KeyCode::Esc if self.end_hidden => self.end_hidden = false,
            KeyCode::Esc => self.selected = None,
            KeyCode::Left | KeyCode::Char('h') => self.step(-1, 0),
            KeyCode::Right | KeyCode::Char('l') => self.step(1, 0),
            KeyCode::Up | KeyCode::Char('k') => self.step(0, 1),
            KeyCode::Down | KeyCode::Char('j') => self.step(0, -1),
            KeyCode::Enter | KeyCode::Char(' ') => self.activate(now),
            _ => {}
        }
    }

    /// Up and down choose the mode, left and right (or T) the time control,
    /// Enter starts the countdown.
    fn on_home_key(&mut self, code: KeyCode, now: Instant) {
        let i = Mode::ALL.iter().position(|&m| m == self.mode).unwrap_or(0) as i32;
        let n = Mode::ALL.len() as i32;
        let presets = clock::PRESETS.len();
        match code {
            KeyCode::Up | KeyCode::Char('k') => self.mode = Mode::ALL[((i - 1).rem_euclid(n)) as usize],
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => self.mode = Mode::ALL[((i + 1) % n) as usize],
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Char('t') => {
                self.clock = Clock::new((self.clock.preset + 1) % presets);
            }
            KeyCode::Left | KeyCode::Char('h') => self.clock = Clock::new((self.clock.preset + presets - 1) % presets),
            KeyCode::Char('a') if self.mode == Mode::Offline => self.auto_flip = !self.auto_flip,
            KeyCode::Enter | KeyCode::Char(' ') if self.mode == Mode::PeerToPeer => {
                self.screen = Screen::Lobby;
                self.lobby = Lobby::Choose { pick: Pick::Host };
            }
            KeyCode::Enter | KeyCode::Char(' ') if self.mode == Mode::Computer => self.open_setup(),
            KeyCode::Enter | KeyCode::Char(' ') => self.begin(now),
            _ => {}
        }
        self.drawn = None;
    }

    /// Choosing host or join, typing a code, waiting for the connection.
    fn on_lobby_key(&mut self, code: KeyCode) {
        let presets = clock::PRESETS.len();
        match (&mut self.lobby, code) {
            (Lobby::Choose { pick }, KeyCode::Up | KeyCode::Char('k')) => *pick = pick.step(Pick::ALL.len() - 1),
            (Lobby::Choose { pick }, KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab) => *pick = pick.step(1),
            (Lobby::Choose { .. }, KeyCode::Right | KeyCode::Char('l' | 't')) => {
                self.clock = Clock::new((self.clock.preset + 1) % presets);
            }
            (Lobby::Choose { .. }, KeyCode::Left | KeyCode::Char('h')) => {
                self.clock = Clock::new((self.clock.preset + presets - 1) % presets);
            }
            (Lobby::Choose { pick: Pick::Join }, KeyCode::Enter) => self.lobby = Lobby::Code { input: String::new(), error: false },
            (Lobby::Choose { pick: Pick::Host }, KeyCode::Enter) => self.connect(Request::Host(self.clock.offer())),
            (Lobby::Choose { pick: Pick::HostLan }, KeyCode::Enter) => self.connect(Request::HostLan(self.clock.offer())),
            (Lobby::Choose { .. }, KeyCode::Esc) => self.screen = Screen::Home,
            (Lobby::Code { input, error }, KeyCode::Char(c)) if input.len() < 40 => {
                if c.is_ascii_alphanumeric() || c == '-' {
                    input.push(c.to_ascii_lowercase());
                } else if c == ' ' {
                    input.push('-');
                }
                *error = false;
            }
            (Lobby::Code { input, error }, KeyCode::Backspace) => {
                input.pop();
                *error = false;
            }
            (Lobby::Code { input, error }, KeyCode::Enter) => {
                if chess_net::code::parse_code(input).is_some() || chess_net::code::parse_lan_code(input).is_some() {
                    let request = Request::Join(input.clone());
                    self.connect(request);
                } else {
                    *error = true;
                }
            }
            (Lobby::Code { .. }, KeyCode::Esc) => self.lobby = Lobby::Choose { pick: Pick::Join },
            (Lobby::Connecting { code: Some(_), .. }, KeyCode::Char('c')) => self.copy_code(),
            (Lobby::Connecting { .. }, KeyCode::Esc) => {
                self.requests.push(Request::Hangup);
                self.lobby = Lobby::Choose { pick: self.lobby.pick() };
            }
            (Lobby::Failed { request, .. }, KeyCode::Enter) => {
                let request = request.clone();
                self.connect(request);
            }
            (Lobby::Failed { .. }, KeyCode::Esc) => self.lobby = Lobby::Choose { pick: self.lobby.pick() },
            _ => {}
        }
    }

    /// Asks the network to host or join.
    fn connect(&mut self, request: Request) {
        self.requests.push(request.clone());
        let stage = if on_lan(&request) { Stage::Searching } else { Stage::Server };
        self.lobby = Lobby::Connecting { request, stage, code: None };
    }

    /// Something from the network.
    pub fn on_net(&mut self, event: NetEvent, now: Instant) {
        match event {
            NetEvent::Stage(next) => {
                if let Lobby::Connecting { stage, code, .. } = &mut self.lobby {
                    if let Stage::Code(c) = &next {
                        *code = Some(c.clone());
                        // Ready to paste to the other player.
                        self.clipboard = Some(c.clone());
                    }
                    *stage = next;
                }
            }
            NetEvent::Failed(failure) => {
                if let Lobby::Connecting { request, .. } = &self.lobby {
                    self.lobby = Lobby::Failed { request: request.clone(), failure };
                }
            }
            NetEvent::Ready { session, path, rtt } => {
                if self.screen != Screen::Lobby {
                    return;
                }
                let me = session.me();
                *self = App {
                    mode: Mode::PeerToPeer,
                    screen: Screen::Countdown { go: now + COUNTDOWN },
                    flipped: me == Side::Black,
                    cursor: chess_core::square(4, if me == Side::White { 1 } else { 6 }),
                    clock: Clock::agreed(session.chain().time_control()),
                    online: Some(Online::new(*session, path, rtt)),
                    ..self.fresh(now)
                };
                self.sync_online(now);
            }
            NetEvent::Link(link::Event::Message { bytes, at }) => self.receive(&bytes, at),
            NetEvent::Link(link::Event::Rtt(rtt)) => {
                if let Some(o) = &mut self.online {
                    o.rtt = rtt;
                }
            }
            NetEvent::Link(link::Event::Closed(reason)) => {
                if let Some(o) = &mut self.online {
                    if o.over() {
                        o.peer_left = true;
                    } else {
                        o.dropped = Some(format!("connection lost: {reason}"));
                    }
                }
                self.sync_online(now);
            }
        }
    }

    /// A message from the opponent, received `at`. Whatever the session
    /// refuses is not played, and draws the game, as does a move whose lag
    /// goes past the opponent's quota.
    fn receive(&mut self, bytes: &[u8], at: Instant) {
        let Some(o) = &mut self.online else { return };
        if o.over() {
            return;
        }
        let drawn = match o.session.receive(bytes) {
            Ok(Received::Move { mv, think_ms }) => {
                let laggy = o.observe(think_ms, at);
                o.turn_since = Some(at);
                self.show_move(mv, at);
                // A move that ended the game on the board keeps its result.
                laggy.filter(|_| self.game.outcome().is_none())
            }
            Ok(Received::OutOfTime | Received::Resigned | Received::DrawOffered | Received::DrawAgreed) => None,
            Err(why) => o.refuse(why),
        };
        match drawn {
            Some(why) => self.draw_and_leave(why, at),
            None => {
                self.sync_online(at);
                self.send_pending(at);
            }
        }
    }

    /// Shows a network game's agreed clock and any loss on time. A dropped
    /// game's clock stops where it stood.
    fn sync_online(&mut self, now: Instant) {
        let Some(o) = &self.online else { return };
        if o.dropped.is_some() {
            self.clock.halt(now);
            return;
        }
        let mut left = [Side::White, Side::Black].map(|s| Duration::from_millis(o.session.chain().remaining_ms(s)));
        let loser = o.loser_on_time();
        if let Some(side) = loser {
            left[side as usize] = Duration::ZERO;
        }
        let running = match (self.screen, o.turn_since) {
            (Screen::Playing, Some(since)) if !o.over() => Some((self.to_move(), since)),
            _ => None,
        };
        self.clock.set(left, running);
        self.clock.flagged = loser;
        let chain = o.session.chain();
        (self.resigned, self.draw_offer, self.agreed_draw) = (chain.resigned(), chain.draw_offer(), chain.agreed_draw());
    }

    /// The picker is a column on screen: up and down walk it, as do left and
    /// right; Enter takes the piece, Esc puts the pawn back.
    fn on_promotion_key(&mut self, code: KeyCode, mut p: Promotion, now: Instant) {
        // The column runs down the screen when the promotion square is on the top row.
        let runs_down = (self.to_move() == Side::White) != self.flipped;
        let step = |down: bool| if down == runs_down { 1 } else { -1 };
        let delta = match code {
            KeyCode::Down | KeyCode::Char('j') => step(true),
            KeyCode::Up | KeyCode::Char('k') => step(false),
            KeyCode::Right | KeyCode::Char('l') => 1,
            KeyCode::Left | KeyCode::Char('h') => -1,
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.promotion = None;
                let mv = Move { from: p.from, to: p.to, promotion: Some(Kind::PROMOTIONS[p.choice]) };
                self.make_move(mv, now);
                self.selected = None;
                return;
            }
            KeyCode::Esc => {
                self.promotion = None;
                return;
            }
            _ => 0,
        };
        p.choice = (p.choice as i32 + delta).clamp(0, 3) as usize;
        self.promotion = Some(p);
    }

    /// Move the cursor by screen direction, so arrows feel right when flipped.
    fn step(&mut self, dfile: i32, drank: i32) {
        let (df, dr) = if self.flipped { (-dfile, -drank) } else { (dfile, drank) };
        let file = (chess_core::file_of(self.cursor) as i32 + df).clamp(0, 7) as usize;
        let rank = (chess_core::rank_of(self.cursor) as i32 + dr).clamp(0, 7) as usize;
        self.cursor = chess_core::square(file, rank);
    }

    fn own_piece_at(&self, sq: Square) -> bool {
        self.my_turn() && self.piece_at(sq).is_some_and(|p| p.color == self.to_move())
    }

    fn activate(&mut self, now: Instant) {
        if self.over() {
            self.selected = None;
            return;
        }
        let Some(from) = self.selected else {
            if self.own_piece_at(self.cursor) {
                self.selected = Some(self.cursor);
            }
            return;
        };
        if from == self.cursor {
            self.selected = None;
            return;
        }
        let moves: Vec<Move> = self.targets().into_iter().filter(|m| m.to == self.cursor).collect();
        match moves.as_slice() {
            // Not a legal move: pick up another of your pieces, or let go.
            [] => self.selected = self.own_piece_at(self.cursor).then_some(self.cursor),
            [mv] => {
                self.make_move(*mv, now);
                self.selected = None;
            }
            // Several moves to one square are the four promotions.
            _ => self.promotion = Some(Promotion { from, to: self.cursor, choice: 0 }),
        }
    }

    /// The pieces `side` has captured, pawns first.
    pub fn captured_by(&self, side: Side) -> Vec<Piece> {
        let mut taken: Vec<Piece> = self.history.iter().filter(|m| m.piece.color == side).filter_map(|m| m.captured).collect();
        taken.sort_by_key(|p| p.kind);
        taken
    }

    /// How far ahead in captured material `side` is.
    pub fn material_lead(&self, side: Side) -> i32 {
        let worth = |s: Side| self.captured_by(s).iter().map(|p| p.kind.value()).sum::<i32>();
        worth(side) - worth(side.opponent())
    }

    /// Plays `mv` if it is legal, the game is still on and, online, it is
    /// this player's turn. Online, the move is signed and sent.
    pub fn make_move(&mut self, mv: Move, now: Instant) -> bool {
        if self.over() || !self.my_turn() {
            return false;
        }
        if let Some(o) = &mut self.online {
            let Ok(msg) = o.session.play(mv, o.think_ms(now)) else { return false };
            self.requests.push(Request::Send(msg.encode()));
            o.turn_since = Some(now);
            // Too late: the message says so, and the game ends on time.
            if o.session.chain().timed_out().is_some() {
                self.sync_online(now);
                return false;
            }
        }
        let shown = self.show_move(mv, now);
        self.sync_online(now);
        shown
    }

    /// Plays `mv` on the board, with its slide and history entry.
    fn show_move(&mut self, mv: Move, now: Instant) -> bool {
        let before = self.game.position().clone();
        let Some(piece) = before.piece_at(mv.from) else { return false };
        if !self.game.play(mv) {
            return false;
        }
        let captured = before.captured(mv);
        let mut movers = vec![(piece, mv.from, mv.to)];
        if let Some((from, to)) = before.castling_rook(mv) {
            movers.extend(before.piece_at(from).map(|rook| (rook, from, to)));
        }
        self.clock.on_move(piece.color, now);
        if self.draw_offer == Some(piece.color.opponent()) {
            self.draw_offer = None;
        }
        if self.game.outcome().is_some() {
            self.clock.halt(now);
        }
        self.last_move = Some((mv.from, mv.to));
        self.slide = Some(Slide { movers, captured, start: now });
        if self.auto_flip && self.local() && !self.over() {
            self.flip_at = Some(now + SLIDE_TIME + FLIP_PAUSE);
        }
        self.history.push(MoveRecord { piece, san: before.san(mv), captured: captured.map(|(p, _)| p) });
        true
    }

    /// Whether this player can make a move now.
    fn can_move(&self) -> bool {
        self.screen == Screen::Playing && self.promotion.is_none() && !self.over() && self.my_turn()
    }

    /// Typing a move: characters narrow the suggestions, up and down (or
    /// Tab) choose one, Enter plays it, Esc gives up. A move spelled in full
    /// that only one move matches plays at once.
    fn on_input_key(&mut self, code: KeyCode, now: Instant) {
        let n = self.suggestions().len().clamp(1, SUGGESTIONS);
        let Some(input) = self.input.as_mut() else { return };
        match code {
            KeyCode::Esc => self.input = None,
            KeyCode::Backspace => {
                input.text.pop();
                input.choice = 0;
                if input.text.is_empty() {
                    self.input = None;
                }
            }
            KeyCode::Down | KeyCode::Tab => input.choice = (input.choice + 1) % n,
            KeyCode::Up | KeyCode::BackTab => input.choice = (input.choice + n - 1) % n,
            KeyCode::Enter => {
                if let Some(s) = self.chosen_suggestion() {
                    self.input = None;
                    self.make_move(s.mv, now);
                }
            }
            KeyCode::Char(c) if c.is_ascii_alphanumeric() || "-=x+#".contains(c) => {
                if input.text.len() < 8 {
                    input.text.push(c);
                    input.choice = 0;
                }
                if let [only] = self.suggestions().as_slice()
                    && only.exact
                {
                    let mv = only.mv;
                    self.input = None;
                    self.make_move(mv, now);
                }
            }
            _ => {}
        }
    }

    /// The legal moves matching the move being typed, best first: moves
    /// spelled in full, then captures (the biggest first), checks, the rest
    /// by SAN.
    pub fn suggestions(&self) -> Vec<Suggestion> {
        let Some(input) = &self.input else { return Vec::new() };
        let typed = normalise(&input.text);
        if typed.is_empty() {
            return Vec::new();
        }
        let pos = self.game.position();
        let mut found: Vec<(Suggestion, i32)> = pos
            .legal_moves()
            .into_iter()
            .filter_map(|mv| {
                let spellings = spellings(pos, mv);
                let exact = spellings.contains(&typed);
                if !exact && !spellings.iter().any(|s| s.starts_with(&typed)) {
                    return None;
                }
                let san = pos.san(mv);
                let worth = pos.captured(mv).map_or(0, |(p, _)| 10 * p.kind.value() + 1)
                    + if san.ends_with(['+', '#']) { 5 } else { 0 };
                Some((Suggestion { mv, san, exact }, worth))
            })
            .collect();
        found.sort_by(|(a, wa), (b, wb)| b.exact.cmp(&a.exact).then(wb.cmp(wa)).then(a.san.cmp(&b.san)));
        found.into_iter().map(|(s, _)| s).collect()
    }

    /// The suggestion Enter would play.
    pub fn chosen_suggestion(&self) -> Option<Suggestion> {
        let choice = self.input.as_ref()?.choice;
        self.suggestions().into_iter().take(SUGGESTIONS).nth(choice)
    }

    /// The legal moves to mark on the board: the selected piece's, or the
    /// suggestion chosen while typing a move.
    pub fn hints(&self) -> Vec<Move> {
        if self.input.is_some() {
            return self.chosen_suggestion().map(|s| s.mv).into_iter().collect();
        }
        self.targets()
    }

    /// The square to highlight as picked up: the selected piece, or the
    /// chosen suggestion's piece while typing a move.
    pub fn lifted(&self) -> Option<Square> {
        if self.input.is_some() {
            return self.chosen_suggestion().map(|s| s.mv.from);
        }
        self.selected
    }

    /// Asks for the game code to go on the clipboard.
    fn copy_code(&mut self) {
        if let Lobby::Connecting { code: Some(code), .. } = &self.lobby {
            self.clipboard = Some(code.clone());
        }
    }

    /// Whether the copy button says COPIED (or FAILED) right now.
    pub fn copied_showing(&self, now: Instant) -> Option<bool> {
        self.copied.filter(|&(at, _)| now.saturating_duration_since(at) < COPIED_TIME).map(|(_, ok)| ok)
    }

    /// The mouse: buttons on the home screen and in the lobby; on the
    /// board, click a piece and then a square, or drag the piece there.
    /// Right click lets go.
    pub fn on_mouse(&mut self, ev: MouseEvent, now: Instant) {
        if self.fit() != Fit::Good {
            return;
        }
        let (col, row) = (ev.column as i32, ev.row as i32);
        match self.screen {
            Screen::Home | Screen::Lobby | Screen::Setup => {
                if ev.kind == MouseEventKind::Down(MouseButton::Left) {
                    self.on_home_click(col, row, now);
                }
            }
            Screen::Countdown { .. } => {}
            // The pop-up takes the clicks; the board under it does not.
            Screen::Playing if self.end_popup(now) => {
                if ev.kind == MouseEventKind::Down(MouseButton::Left)
                    && let Some(layout) = layout::compute(self.size.0 as i32, self.size.1 as i32)
                    && let Some(choice) = panel::end_hit(&layout.g, col, row)
                {
                    self.end_go(choice, now);
                }
            }
            Screen::Playing => self.on_board_mouse(ev.kind, col, row, now),
        }
    }

    fn on_home_click(&mut self, col: i32, row: i32, now: Instant) {
        let Some(hit) = panel::home_hit(self, self.size.0 as i32, self.size.1 as i32, col, row) else { return };
        match hit {
            HomeHit::Mode(mode) => {
                self.mode = mode;
                self.on_home_key(KeyCode::Enter, now);
            }
            HomeHit::Time(next) => {
                let code = if next { KeyCode::Right } else { KeyCode::Left };
                match self.screen {
                    Screen::Lobby => self.on_lobby_key(code),
                    _ => self.on_home_key(code, now),
                }
            }
            HomeHit::Pick(pick) => {
                self.lobby = Lobby::Choose { pick };
                self.on_lobby_key(KeyCode::Enter);
            }
            HomeHit::Copy => self.copy_code(),
            HomeHit::Setting(setting, next) => {
                self.setup.focus = setting;
                self.on_setup_key(if next { KeyCode::Right } else { KeyCode::Left }, now);
            }
            HomeHit::Play => {
                self.setup.focus = Setting::Play;
                self.on_setup_key(KeyCode::Enter, now);
            }
        }
    }

    /// The square under a cell, if the board is there.
    fn square_at(&self, col: i32, row: i32) -> Option<Square> {
        let layout = layout::compute(self.size.0 as i32, self.size.1 as i32)?;
        layout.g.square_at_cell(col, row, self.flipped)
    }

    fn on_board_mouse(&mut self, kind: MouseEventKind, col: i32, row: i32, now: Instant) {
        let sq = self.square_at(col, row);
        match kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.input = None;
                self.drag = None;
                if let Some(layout) = layout::compute(self.size.0 as i32, self.size.1 as i32)
                    && let Some(hit) = panel::action_hit(self, layout.panel, col, row)
                {
                    match hit {
                        Signal::Resign => self.resign(now),
                        _ => self.draw(now),
                    }
                    return;
                }
                if let Some(p) = self.promotion {
                    // A tile of the picker takes that piece; anywhere else
                    // puts the pawn back.
                    let squares = self.promotion_squares().unwrap_or([(p.to, Kind::Queen); 4]);
                    match squares.iter().position(|&(s, _)| Some(s) == sq) {
                        Some(choice) => self.on_promotion_key(KeyCode::Enter, Promotion { choice, ..p }, now),
                        None => self.promotion = None,
                    }
                    return;
                }
                let Some(sq) = sq else {
                    self.selected = None;
                    return;
                };
                self.cursor = sq;
                let target = self.targets().iter().any(|m| m.to == sq);
                if self.selected.is_some() && target {
                    self.activate(now);
                } else if self.own_piece_at(sq) && !self.over() {
                    let was_selected = self.selected == Some(sq);
                    self.selected = Some(sq);
                    self.drag = Some(Drag { from: sq, at: (col, row), moved: false, was_selected });
                } else {
                    self.selected = None;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(d) = &mut self.drag {
                    d.at = (col, row);
                    d.moved |= sq != Some(d.from);
                    if let Some(sq) = sq {
                        self.cursor = sq;
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let Some(d) = self.drag.take() else { return };
                match sq {
                    Some(sq) if sq != d.from => {
                        self.cursor = sq;
                        let before = self.history.len();
                        self.activate(now);
                        // The piece is already there: no slide.
                        if self.history.len() > before {
                            self.slide = None;
                        }
                    }
                    Some(_) if d.was_selected && !d.moved => self.selected = None,
                    Some(_) => {}
                    // Dropped off the board: it goes back.
                    None => self.selected = None,
                }
            }
            MouseEventKind::Down(MouseButton::Right) => {
                self.selected = None;
                self.drag = None;
                self.input = None;
            }
            _ => {}
        }
    }

    /// Plays a move given in UCI notation, like "e2e4" or "e7e8q".
    pub fn play_uci(&mut self, uci: &str, now: Instant) -> bool {
        Move::parse_uci(uci).is_some_and(|mv| self.make_move(mv, now))
    }
}

/// A move as typed, for matching: no capture, check, promotion or castling
/// punctuation, and lower case after the first letter (a piece, or O for
/// castling), so "Pe8=Q", "Pe8q" and "Pe8Q" agree. The first letter keeps
/// its case, so a b-pawn's "bxc3" never reads as a bishop's "Bc3".
fn normalise(s: &str) -> String {
    let mut chars = s.chars().filter(|c| !"x-=+#!? ".contains(*c));
    let first = chars.next().map(String::from).unwrap_or_default();
    first + &chars.collect::<String>().to_ascii_lowercase()
}

/// The ways a move may be typed, normalised: the piece letter and the
/// square it goes to ("Nf3", "Pe4", "Kg1" for castling), the piece letter
/// and both squares ("Ng1f3"), its SAN ("Nbd2", "O-O"), and for pawns the
/// letter before the SAN ("Pexd5"). Promotions add the new piece's letter.
fn spellings(pos: &chess_core::Position, mv: Move) -> Vec<String> {
    let Some(piece) = pos.piece_at(mv.from) else { return Vec::new() };
    let letter = piece.kind.letter();
    let promo = mv.promotion.map(|k| k.letter().to_string()).unwrap_or_default();
    let (from, to) = (chess_core::name(mv.from), chess_core::name(mv.to));
    let san = pos.san(mv);
    let mut out = vec![format!("{letter}{to}{promo}"), format!("{letter}{from}{to}{promo}"), san.clone()];
    if piece.kind == Kind::Pawn {
        out.push(format!("P{san}"));
    }
    out.iter().map(|s| normalise(s)).collect()
}
