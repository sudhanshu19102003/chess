//! The state machine: a game, its chain head and the log of accepted
//! messages, the ten checks every message goes through, the agreed clock,
//! which is worked out from the log alone, and the endings by signal:
//! flags, resignations and draws by agreement.

use std::fmt;

use chess_core::{Color, Game, Move, Position};
use ed25519_dalek::{SigningKey, VerifyingKey};

use crate::{GameId, Hash, Msg, MoveMsg, ParseError, Signal, SignalMsg, TimeControl, genesis, link, position_hash, signal_link};

/// Why a message was refused. Every variant leaves the state unchanged;
/// `HashMismatch` also halts the chain for good.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Reject {
    Malformed(ParseError),
    WrongGame,
    BadSignature,
    WrongPly { expected: u32, got: u32 },
    NotSendersTurn,
    WrongPrevHash,
    GameOver,
    IllegalMove,
    /// A draw offer while one already stands (the sender's own, or the
    /// opponent's, which the sender should accept instead).
    OfferStands,
    /// Accepting a draw the opponent has not offered.
    NoDrawOffer,
    /// The move was legal but the sender's position after it differs from
    /// ours: the two engines disagree. Nothing more can be trusted.
    HashMismatch { ours: Hash, theirs: Hash },
    /// An earlier hash mismatch stopped the chain.
    Halted,
}

impl fmt::Display for Reject {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Reject::Malformed(e) => write!(f, "malformed message: {e}"),
            Reject::WrongGame => f.write_str("message is for another game"),
            Reject::BadSignature => f.write_str("bad signature"),
            Reject::WrongPly { expected, got } => write!(f, "expected ply {expected}, got {got}"),
            Reject::NotSendersTurn => f.write_str("sender is not the side to move"),
            Reject::WrongPrevHash => f.write_str("message does not follow the current chain head"),
            Reject::GameOver => f.write_str("the game is over"),
            Reject::IllegalMove => f.write_str("illegal move"),
            Reject::OfferStands => f.write_str("a draw offer already stands"),
            Reject::NoDrawOffer => f.write_str("accepting a draw that was not offered"),
            Reject::HashMismatch { ours, theirs } => {
                write!(f, "position hashes differ after a legal move: ours {ours}, theirs {theirs}")
            }
            Reject::Halted => f.write_str("halted after a hash mismatch"),
        }
    }
}

impl std::error::Error for Reject {}

/// One game's verified history. Knows both players' public keys, so it
/// serves a live session and offline checking of a finished log alike.
#[derive(Clone, Debug)]
pub struct Chain {
    game_id: GameId,
    /// White's key, then Black's.
    keys: [VerifyingKey; 2],
    time_control: TimeControl,
    game: Game,
    head: Hash,
    log: Vec<Msg>,
    /// Each side's thinking time so far, the sum of its `think_ms`.
    used_ms: [u64; 2],
    /// Moves each side has made, for the increments.
    moves: [u64; 2],
    /// The side that lost on time: it flagged, or sent a move after its
    /// time ran out. Either message is logged and ends the game.
    timed_out: Option<Color>,
    /// The side that resigned.
    resigned: Option<Color>,
    /// The side whose draw offer stands: made on its turn, until the
    /// opponent moves.
    draw_offer: Option<Color>,
    /// A draw offer was accepted.
    agreed_draw: bool,
    halted: bool,
}

fn slot(color: Color) -> usize {
    match color {
        Color::White => 0,
        Color::Black => 1,
    }
}

impl Chain {
    pub fn new(
        game_id: GameId,
        start: Position,
        white: VerifyingKey,
        black: VerifyingKey,
        time_control: TimeControl,
    ) -> Chain {
        let head = genesis(&game_id, &start, &white, &black, time_control);
        Chain {
            game_id,
            keys: [white, black],
            time_control,
            game: Game::from_position(start),
            head,
            log: Vec::new(),
            used_ms: [0; 2],
            moves: [0; 2],
            timed_out: None,
            resigned: None,
            draw_offer: None,
            agreed_draw: false,
            halted: false,
        }
    }

    pub fn game_id(&self) -> GameId {
        self.game_id
    }

    pub fn time_control(&self) -> TimeControl {
        self.time_control
    }

    pub fn game(&self) -> &Game {
        &self.game
    }

    pub fn head(&self) -> Hash {
        self.head
    }

    /// Messages accepted so far.
    pub fn ply(&self) -> u32 {
        self.log.len() as u32
    }

    /// Every accepted message, in order: the evidence for this game.
    pub fn log(&self) -> &[Msg] {
        &self.log
    }

    pub fn is_halted(&self) -> bool {
        self.halted
    }

    /// The side that lost on time, if one did.
    pub fn timed_out(&self) -> Option<Color> {
        self.timed_out
    }

    /// The side that resigned, if one did.
    pub fn resigned(&self) -> Option<Color> {
        self.resigned
    }

    /// The side whose draw offer stands, if any.
    pub fn draw_offer(&self) -> Option<Color> {
        self.draw_offer
    }

    /// Whether the game was drawn by agreement.
    pub fn agreed_draw(&self) -> bool {
        self.agreed_draw
    }

    /// Whether no more messages can be accepted: the board says the game is
    /// over, a side lost on time or resigned, or a draw was agreed.
    pub fn is_over(&self) -> bool {
        self.timed_out.is_some() || self.resigned.is_some() || self.agreed_draw || self.game.outcome().is_some()
    }

    /// The agreed clock: `color`'s base time, minus the sum of its
    /// `think_ms`, plus one increment per move it has made. Computed from
    /// the log alone, so both players and an offline verifier get the same
    /// figure, and network delay never counts against anyone.
    pub fn remaining_ms(&self, color: Color) -> u64 {
        let tc = self.time_control;
        let i = slot(color);
        (tc.base_ms as u64 + tc.increment_ms as u64 * self.moves[i]).saturating_sub(self.used_ms[i])
    }

    fn key(&self, color: Color) -> &VerifyingKey {
        &self.keys[slot(color)]
    }

    /// Runs the checks, in order, on a parsed message from `sender`, and
    /// commits it if all pass.
    pub fn apply(&mut self, msg: &Msg, sender: Color) -> Result<(), Reject> {
        if self.halted {
            return Err(Reject::Halted);
        }
        // 1. (Parsing happened in `Msg::decode`.)
        // 2.
        if msg.game_id() != self.game_id {
            return Err(Reject::WrongGame);
        }
        // 3.
        if !msg.verify(self.key(sender)) {
            return Err(Reject::BadSignature);
        }
        // 4.
        let expected = self.ply() + 1;
        if msg.ply() != expected {
            return Err(Reject::WrongPly { expected, got: msg.ply() });
        }
        // 5. Signals too are sent on the sender's own turn.
        let pos = self.game.position();
        if pos.turn() != sender {
            return Err(Reject::NotSendersTurn);
        }
        // 6.
        if msg.prev_hash() != self.head {
            return Err(Reject::WrongPrevHash);
        }
        // 7. Nothing is accepted once the game has ended.
        if self.is_over() {
            return Err(Reject::GameOver);
        }
        let m = match msg {
            Msg::Signal(signal) => {
                match signal.kind {
                    // A player may always concede.
                    Signal::Flag => self.timed_out = Some(sender),
                    Signal::Resign => self.resigned = Some(sender),
                    Signal::OfferDraw if self.draw_offer.is_some() => return Err(Reject::OfferStands),
                    Signal::OfferDraw => self.draw_offer = Some(sender),
                    Signal::AcceptDraw if self.draw_offer != Some(sender.opponent()) => return Err(Reject::NoDrawOffer),
                    Signal::AcceptDraw => self.agreed_draw = true,
                }
                self.head = signal_link(signal.kind, &self.head);
                self.log.push(msg.clone());
                return Ok(());
            }
            Msg::Move(m) => m,
        };
        // 7, 8.
        let next = pos.play(m.mv).ok_or(Reject::IllegalMove)?;
        // 9.
        let ours = position_hash(&next);
        if m.post_hash != ours {
            self.halted = true;
            return Err(Reject::HashMismatch { ours, theirs: m.post_hash });
        }
        // 10. A move that took longer than the time its sender had left is
        // logged but not played: the game ends on time instead.
        if m.think_ms as u64 > self.remaining_ms(sender) {
            self.timed_out = Some(sender);
        } else {
            let played = self.game.play(m.mv);
            assert!(played, "a legal move in an unfinished game is playable");
            self.used_ms[slot(sender)] += m.think_ms as u64;
            self.moves[slot(sender)] += 1;
            // Moving instead of accepting declines the opponent's offer.
            if self.draw_offer == Some(sender.opponent()) {
                self.draw_offer = None;
            }
        }
        self.head = link(&self.head, m.mv, m.think_ms, &ours);
        self.log.push(msg.clone());
        Ok(())
    }
}

/// Checks a finished log offline: every message in order, each signed by
/// the side that was to move. On failure, gives the index of the first bad
/// message and why.
pub fn replay(
    game_id: GameId,
    start: Position,
    white: VerifyingKey,
    black: VerifyingKey,
    time_control: TimeControl,
    log: &[Msg],
) -> Result<Chain, (usize, Reject)> {
    let mut chain = Chain::new(game_id, start, white, black, time_control);
    for (i, msg) in log.iter().enumerate() {
        let sender = chain.game.position().turn();
        chain.apply(msg, sender).map_err(|e| (i, e))?;
    }
    Ok(chain)
}

/// What an accepted message from the opponent did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Received {
    /// They played `mv`, taking `think_ms` by their own account.
    Move { mv: Move, think_ms: u32 },
    /// They lost on time: they flagged, or their move came after their time
    /// ran out and was not played.
    OutOfTime,
    Resigned,
    /// They offered a draw; it stands until we move.
    DrawOffered,
    /// They accepted our draw offer.
    DrawAgreed,
}

/// One player's side of a live game: signs our moves and checks the
/// opponent's. Our own messages go through the same checks as theirs, so
/// both sides' chains advance identically.
#[derive(Clone, Debug)]
pub struct Session {
    chain: Chain,
    me: Color,
    key: SigningKey,
}

impl Session {
    pub fn new(
        game_id: GameId,
        start: Position,
        me: Color,
        key: SigningKey,
        opponent: VerifyingKey,
        time_control: TimeControl,
    ) -> Session {
        let mine = key.verifying_key();
        let (white, black) = match me {
            Color::White => (mine, opponent),
            Color::Black => (opponent, mine),
        };
        Session { chain: Chain::new(game_id, start, white, black, time_control), me, key }
    }

    pub fn chain(&self) -> &Chain {
        &self.chain
    }

    pub fn me(&self) -> Color {
        self.me
    }

    /// Plays our move, `think_ms` after the opponent's arrived (or after the
    /// start), and returns the message to send. If that is more than our
    /// time left, the move is not played and we have lost on time; the
    /// message still goes to the opponent, who reaches the same verdict.
    pub fn play(&mut self, mv: Move, think_ms: u32) -> Result<MoveMsg, Reject> {
        let pos = self.chain.game.position();
        if pos.turn() != self.me {
            return Err(Reject::NotSendersTurn);
        }
        let next = pos.play(mv).ok_or(Reject::IllegalMove)?;
        let msg = MoveMsg::signed(
            &self.key,
            self.chain.game_id,
            self.chain.ply() + 1,
            self.chain.head,
            mv,
            think_ms,
            position_hash(&next),
        );
        self.chain.apply(&Msg::Move(msg.clone()), self.me)?;
        Ok(msg)
    }

    /// Concedes on time, on our own turn, and returns the message to send.
    pub fn flag(&mut self) -> Result<SignalMsg, Reject> {
        self.signal(Signal::Flag)
    }

    /// Sends a signal on our own turn: concedes on time, resigns, offers a
    /// draw or accepts the opponent's offer. Returns the message to send.
    pub fn signal(&mut self, kind: Signal) -> Result<SignalMsg, Reject> {
        let msg = SignalMsg::signed(kind, &self.key, self.chain.game_id, self.chain.ply() + 1, self.chain.head);
        self.chain.apply(&Msg::Signal(msg.clone()), self.me)?;
        Ok(msg)
    }

    /// Checks a message from the opponent and, if it passes, commits it.
    pub fn receive(&mut self, bytes: &[u8]) -> Result<Received, Reject> {
        if self.chain.halted {
            return Err(Reject::Halted);
        }
        let msg = Msg::decode(bytes).map_err(Reject::Malformed)?;
        self.chain.apply(&msg, self.me.opponent())?;
        Ok(match msg {
            Msg::Move(m) if self.chain.timed_out.is_none() => Received::Move { mv: m.mv, think_ms: m.think_ms },
            Msg::Move(_) => Received::OutOfTime,
            Msg::Signal(s) => match s.kind {
                Signal::Flag => Received::OutOfTime,
                Signal::Resign => Received::Resigned,
                Signal::OfferDraw => Received::DrawOffered,
                Signal::AcceptDraw => Received::DrawAgreed,
            },
        })
    }
}

