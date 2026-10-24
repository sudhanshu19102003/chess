//! A game's traffic over the secure channel. The signed messages
//! `chess-verify` makes (tags 0x01 and 0x02) pass through untouched, both
//! ways. Ping and Pong (tags 0x10 and 0x11) keep a round-trip estimate for
//! the lag check and keep a relayed connection from going idle. They travel
//! inside the Noise session, so they are authenticated, but they are not
//! signed and not logged.
//!
//! ```text
//! Ping  0x10, sequence number (u32, big-endian)
//! Pong  0x11, the same sequence number
//! ```

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio::time::{MissedTickBehavior, interval};

use crate::handshake::Channel;

const PING: u8 = 0x10;
const PONG: u8 = 0x11;
pub const PING_EVERY: Duration = Duration::from_secs(2);
/// With nothing at all heard for this long, not even a Pong, the
/// connection counts as lost.
pub const SILENCE_LIMIT: Duration = Duration::from_secs(20);
/// The estimate is the median of the last few round trips.
const RTT_SAMPLES: usize = 5;

#[derive(Debug)]
pub enum Event {
    /// A message for `chess_verify::Session::receive`, and when it arrived.
    Message { bytes: Vec<u8>, at: Instant },
    /// A new round-trip estimate.
    Rtt(Duration),
    /// The connection is gone; nothing more will arrive.
    Closed(String),
}

/// Runs until the connection fails or `outgoing` closes (the game hung
/// up). Sends each message from `outgoing`, and reports what arrives.
pub async fn run<S: AsyncRead + AsyncWrite + Unpin>(
    mut channel: Channel<S>,
    mut outgoing: mpsc::UnboundedReceiver<Vec<u8>>,
    events: mpsc::UnboundedSender<Event>,
) {
    let mut ticks = interval(PING_EVERY);
    ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut seq: u32 = 0;
    let mut pending: VecDeque<(u32, Instant)> = VecDeque::new();
    let mut samples: VecDeque<Duration> = VecDeque::new();
    let mut heard = Instant::now();
    let closed = loop {
        tokio::select! {
            out = outgoing.recv() => {
                let Some(bytes) = out else { return };
                if let Err(e) = channel.send(&bytes).await {
                    break e.to_string();
                }
            }
            _ = ticks.tick() => {
                if heard.elapsed() > SILENCE_LIMIT {
                    break format!("nothing heard for {} s", SILENCE_LIMIT.as_secs());
                }
                seq = seq.wrapping_add(1);
                pending.push_back((seq, Instant::now()));
                if pending.len() > 2 * RTT_SAMPLES {
                    pending.pop_front();
                }
                if let Err(e) = channel.send(&[&[PING][..], &seq.to_be_bytes()].concat()).await {
                    break e.to_string();
                }
            }
            got = channel.recv() => {
                let bytes = match got {
                    Ok(bytes) => bytes,
                    Err(e) => break e.to_string(),
                };
                let at = Instant::now();
                heard = at;
                match (bytes.first(), bytes.len()) {
                    (Some(&PING), 5) => {
                        let pong = [&[PONG][..], &bytes[1..]].concat();
                        if let Err(e) = channel.send(&pong).await {
                            break e.to_string();
                        }
                    }
                    (Some(&PONG), 5) => {
                        let n = u32::from_be_bytes(bytes[1..].try_into().unwrap());
                        if let Some(i) = pending.iter().position(|&(s, _)| s == n) {
                            let (_, sent) = pending.remove(i).unwrap();
                            samples.push_back(at - sent);
                            if samples.len() > RTT_SAMPLES {
                                samples.pop_front();
                            }
                            let mut sorted: Vec<Duration> = samples.iter().copied().collect();
                            sorted.sort();
                            let _ = events.send(Event::Rtt(sorted[sorted.len() / 2]));
                        }
                    }
                    // Anything else is the game's to judge, a stray Ping or
                    // Pong of the wrong length included.
                    _ => {
                        let _ = events.send(Event::Message { bytes, at });
                    }
                }
            }
        }
    };
    let _ = events.send(Event::Closed(closed));
}
