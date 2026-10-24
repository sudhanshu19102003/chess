//! A two-sided chess clock with increment. It starts when the game's
//! countdown ends and from then on runs for whoever is to move.

use std::time::{Duration, Instant};

use chess_core::Color as Side;

#[derive(Clone, Copy, Debug)]
pub struct TimeControl {
    pub minutes: u64,
    pub increment: u64,
    pub label: &'static str,
}

impl TimeControl {
    /// Written the way chess.com writes it: "10 min", or "15 | 10" with an
    /// increment.
    pub fn name(&self) -> String {
        match self.increment {
            0 => format!("{} min", self.minutes),
            inc => format!("{} | {inc}", self.minutes),
        }
    }
}

/// chess.com's new-game choices, three for each speed.
pub const PRESETS: [TimeControl; 9] = [
    TimeControl { minutes: 1, increment: 0, label: "BULLET" },
    TimeControl { minutes: 1, increment: 1, label: "BULLET" },
    TimeControl { minutes: 2, increment: 1, label: "BULLET" },
    TimeControl { minutes: 3, increment: 0, label: "BLITZ" },
    TimeControl { minutes: 3, increment: 2, label: "BLITZ" },
    TimeControl { minutes: 5, increment: 0, label: "BLITZ" },
    TimeControl { minutes: 10, increment: 0, label: "RAPID" },
    TimeControl { minutes: 15, increment: 10, label: "RAPID" },
    TimeControl { minutes: 30, increment: 0, label: "RAPID" },
];

/// 10 min, chess.com's default.
pub const DEFAULT_PRESET: usize = 6;

pub struct Clock {
    pub preset: usize,
    control: TimeControl,
    left: [Duration; 2],
    running: Option<(Side, Instant)>,
    paused: Option<Side>,
    pub flagged: Option<Side>,
}

fn slot(side: Side) -> usize {
    match side {
        Side::White => 0,
        Side::Black => 1,
    }
}

impl Clock {
    pub fn new(preset: usize) -> Self {
        let base = Duration::from_secs(PRESETS[preset].minutes * 60);
        Clock { preset, control: PRESETS[preset], left: [base; 2], running: None, paused: None, flagged: None }
    }

    /// A clock for a time control agreed over the network: the matching
    /// preset, or one labelled CUSTOM.
    pub fn agreed(tc: chess_verify::TimeControl) -> Self {
        let matches = |p: &TimeControl| p.minutes * 60_000 == tc.base_ms as u64 && p.increment * 1000 == tc.increment_ms as u64;
        match PRESETS.iter().position(matches) {
            Some(preset) => Clock::new(preset),
            None => Clock {
                control: TimeControl { minutes: tc.base_ms as u64 / 60_000, increment: tc.increment_ms as u64 / 1000, label: "CUSTOM" },
                ..Clock::new(DEFAULT_PRESET)
            },
        }
    }

    pub fn control(&self) -> TimeControl {
        self.control
    }

    /// The network time control for this clock's preset.
    pub fn offer(&self) -> chess_verify::TimeControl {
        let c = self.control;
        chess_verify::TimeControl { base_ms: (c.minutes * 60_000) as u32, increment_ms: (c.increment * 1000) as u32 }
    }

    /// Shows figures worked out elsewhere (the agreed clock of a network
    /// game): `left` for each side, and `running` counting down from when
    /// its turn began.
    pub fn set(&mut self, left: [Duration; 2], running: Option<(Side, Instant)>) {
        self.left = left;
        self.running = running;
    }

    pub fn remaining(&self, side: Side, now: Instant) -> Duration {
        let left = self.left[slot(side)];
        match self.running {
            Some((s, since)) if s == side => left.saturating_sub(now.saturating_duration_since(since)),
            _ => left,
        }
    }

    pub fn running(&self) -> Option<Side> {
        self.running.map(|(s, _)| s)
    }

    pub fn paused(&self) -> bool {
        self.paused.is_some()
    }

    fn stop(&mut self, now: Instant) -> Option<Side> {
        let (side, since) = self.running.take()?;
        let used = now.saturating_duration_since(since);
        self.left[slot(side)] = self.left[slot(side)].saturating_sub(used);
        Some(side)
    }

    /// The game begins: `side`'s time starts running.
    pub fn start(&mut self, side: Side, now: Instant) {
        if self.running.is_none() && self.flagged.is_none() {
            self.running = Some((side, now));
        }
    }

    /// `mover` just moved: bank their time, add increment, start the opponent.
    pub fn on_move(&mut self, mover: Side, now: Instant) {
        if self.flagged.is_some() {
            return;
        }
        if self.stop(now) == Some(mover) {
            self.left[slot(mover)] += Duration::from_secs(self.control.increment);
        }
        self.paused = None;
        self.running = Some((mover.opponent(), now));
    }

    /// The game is over: stop both clocks for good.
    pub fn halt(&mut self, now: Instant) {
        self.stop(now);
        self.paused = None;
    }

    pub fn toggle_pause(&mut self, now: Instant) {
        if let Some(side) = self.stop(now) {
            self.paused = Some(side);
        } else if let Some(side) = self.paused.take() {
            self.running = Some((side, now));
        }
    }

    pub fn tick(&mut self, now: Instant) {
        if let Some(side) = self.running()
            && self.remaining(side, now).is_zero()
        {
            self.stop(now);
            self.flagged = Some(side);
        }
    }
}

/// "M:SS" normally, "SS.t" in the last twenty seconds.
pub fn format(d: Duration) -> String {
    let ms = d.as_millis() as u64;
    if ms < 20_000 {
        format!("{:02}.{}", ms / 1000, (ms / 100) % 10)
    } else {
        let secs = ms / 1000;
        format!("{}:{:02}", secs / 60, secs % 60)
    }
}
