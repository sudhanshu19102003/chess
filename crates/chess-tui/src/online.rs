//! A peer-to-peer game's state beside the board: the verified session, the
//! agreed clock as this machine runs it, the lag check, and what happens to
//! messages the session refuses. No I/O: the app feeds it messages and times.

use std::time::{Duration, Instant};

use chess_core::Color as Side;
use chess_net::Path;
use chess_verify::{Reject, Session, Signal, TimeControl};

/// How much lag the opponent's moves may show, Lichess's way (scalachess
/// `LagTracker`): each move may arrive later than its `think_ms` says by
/// up to the quota left, which pays for it; the quota grows by `gain` with
/// every move, up to `max`. Lichess's figures, doubled, since the gap we
/// see spans two network legs (our move out, theirs back) where a server
/// sees one. The round-trip estimate plays no part, so a peer who slows
/// its Pongs gains nothing.
#[derive(Clone, Copy, Debug)]
pub struct LagQuota {
    gain: Duration,
    left: Duration,
    max: Duration,
}

impl LagQuota {
    pub fn new(tc: TimeControl) -> LagQuota {
        // Lichess's estimate of a game's length: base plus 40 increments.
        let estimate_s = (tc.base_ms as u64 + 40 * tc.increment_ms as u64) / 1000;
        let gain = Duration::from_millis(2 * (estimate_s * 4 + 150).min(1000));
        LagQuota { gain, left: 3 * gain, max: 7 * gain }
    }

    /// The lag the next move may show.
    pub fn left(&self) -> Duration {
        self.left
    }

    /// Pays for one move's `lag`, and refills. False if the quota left was
    /// not enough.
    pub fn spend(&mut self, lag: Duration) -> bool {
        let paid = lag <= self.left;
        self.left = (self.left.saturating_sub(lag) + self.gain).min(self.max);
        paid
    }
}

pub struct Online {
    pub session: Session,
    pub path: Path,
    /// Round-trip estimate, kept fresh by the link's pings. Shown only.
    pub rtt: Duration,
    /// When the side to move's turn began on this machine: the end of the
    /// countdown, or when the last move was sent or received.
    pub turn_since: Option<Instant>,
    /// The game stopped without a result, and why.
    pub dropped: Option<String>,
    /// The opponent's lag quota.
    pub lag: LagQuota,
    /// The opponent went silent past their time, by our lag-checked view,
    /// and the game ended here on time. Not in the log.
    pub silent: bool,
    /// `r` was pressed once mid-game; a second press leaves.
    pub leaving: Option<Instant>,
    /// The connection closed after the game ended.
    pub peer_left: bool,
    /// A resignation or a draw acceptance chosen on the opponent's turn,
    /// sent as soon as it is ours (signals go on the sender's own turn).
    pub pending: Option<Signal>,
}

impl Online {
    pub fn new(session: Session, path: Path, rtt: Duration) -> Online {
        let lag = LagQuota::new(session.chain().time_control());
        Online {
            session,
            path,
            rtt,
            turn_since: None,
            dropped: None,
            lag,
            silent: false,
            leaving: None,
            peer_left: false,
            pending: None,
        }
    }

    pub fn me(&self) -> Side {
        self.session.me()
    }

    fn to_move(&self) -> Side {
        self.session.chain().game().position().turn()
    }

    /// No more moves: the game has a result, or it was dropped.
    pub fn over(&self) -> bool {
        self.session.chain().is_over() || self.silent || self.dropped.is_some()
    }

    /// Who lost on time: by a flag or a late move in the log, or by going
    /// silent past their time.
    pub fn loser_on_time(&self) -> Option<Side> {
        self.session.chain().timed_out().or(self.silent.then(|| self.me().opponent()))
    }

    /// Time the side to move has used on this turn so far.
    fn elapsed(&self, now: Instant) -> Duration {
        self.turn_since.map_or(Duration::ZERO, |t| now.saturating_duration_since(t))
    }

    /// The agreed time left for `side`, counted down through the current
    /// turn. For the opponent this runs from when our move left, so it
    /// snaps back a little when their move arrives.
    pub fn remaining(&self, side: Side, now: Instant) -> Duration {
        let agreed = Duration::from_millis(self.session.chain().remaining_ms(side));
        if side == self.to_move() && !self.over() { agreed.saturating_sub(self.elapsed(now)) } else { agreed }
    }

    /// What to put in our next message's `think_ms`.
    pub fn think_ms(&self, now: Instant) -> u32 {
        self.elapsed(now).as_millis().min(u32::MAX as u128) as u32
    }

    /// Our own clock ran out on our turn: time to flag.
    pub fn own_time_up(&self, now: Instant) -> bool {
        self.to_move() == self.me() && !self.over() && self.turn_since.is_some() && self.remaining(self.me(), now).is_zero()
    }

    /// The opponent has gone silent past their time and their lag quota:
    /// any move they sent now would be either too late or too laggy.
    pub fn opponent_silent(&self, now: Instant) -> bool {
        let them = self.me().opponent();
        if self.to_move() != them || self.over() || self.turn_since.is_none() {
            return false;
        }
        let agreed = Duration::from_millis(self.session.chain().remaining_ms(them));
        self.elapsed(now) > agreed + self.lag.left()
    }

    /// The lag check for an opponent move that took `think_ms` by their
    /// account and arrived `at`: the gap we saw beyond what they reported
    /// is lag, paid from their quota. If the quota could not pay, the
    /// timekeeping has failed and the game is drawn; gives the reason.
    pub fn observe(&mut self, think_ms: u32, at: Instant) -> Option<String> {
        let gap = self.turn_since.map_or(Duration::ZERO, |t| at.saturating_duration_since(t));
        let lag = gap.saturating_sub(Duration::from_millis(think_ms as u64));
        let left = self.lag.left();
        (!self.lag.spend(lag)).then(|| {
            format!("their move came {:.1} s later than their clock said; {:.1} s of lag was allowed", lag.as_secs_f32(), left.as_secs_f32())
        })
    }

    /// A message from the opponent was refused. A hash mismatch drops the
    /// game with no result (the two engines disagree); anything else draws
    /// it, and gives the reason.
    pub fn refuse(&mut self, why: Reject) -> Option<String> {
        match why {
            Reject::Halted => None,
            Reject::HashMismatch { ours, theirs } => {
                self.dropped = Some(format!("the positions differ after a legal move. ours {ours} theirs {theirs}"));
                None
            }
            why => Some(format!("they sent a message that was refused: {why}")),
        }
    }
}
