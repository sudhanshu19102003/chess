//! Per-address limits for the servers: [`PerAddr`] caps how much one address
//! can hold at once ([`PerAddr::take`] hands out a guard that gives the slot
//! back when it is dropped), and [`RateLimit`] caps how often it may ask.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub(crate) struct PerAddr {
    counts: Mutex<HashMap<IpAddr, usize>>,
    max: usize,
}

/// One slot held by an address.
pub(crate) struct Slot {
    owner: Arc<PerAddr>,
    ip: IpAddr,
}

impl PerAddr {
    pub(crate) fn new(max: usize) -> Arc<PerAddr> {
        Arc::new(PerAddr { counts: Mutex::new(HashMap::new()), max })
    }

    /// A slot for `ip`, or `None` if it already holds `max`.
    pub(crate) fn take(self: &Arc<Self>, ip: IpAddr) -> Option<Slot> {
        let mut counts = self.counts.lock().unwrap();
        let count = counts.entry(ip).or_insert(0);
        if *count >= self.max {
            return None;
        }
        *count += 1;
        Some(Slot { owner: self.clone(), ip })
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut counts = self.owner.counts.lock().unwrap();
        if let Some(count) = counts.get_mut(&self.ip) {
            *count -= 1;
            if *count == 0 {
                counts.remove(&self.ip);
            }
        }
    }
}

/// At most `max` requests per address in each `window`. The count starts
/// again when an address's window is over, so this slows an address down and
/// never bans it.
pub(crate) struct RateLimit {
    seen: Mutex<HashMap<IpAddr, (Instant, u32)>>,
    max: u32,
    window: Duration,
}

impl RateLimit {
    pub(crate) fn new(max: u32, window: Duration) -> RateLimit {
        RateLimit { seen: Mutex::new(HashMap::new()), max, window }
    }

    /// Counts a request from `ip`; false once it has made too many lately.
    pub(crate) fn allow(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let mut seen = self.seen.lock().unwrap();
        if seen.len() > 10_000 {
            seen.retain(|_, (began, _)| now.duration_since(*began) < self.window);
        }
        let entry = seen.entry(ip).or_insert((now, 0));
        if now.duration_since(entry.0) >= self.window {
            *entry = (now, 0);
        }
        entry.1 += 1;
        entry.1 <= self.max
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_are_per_address_and_come_back() {
        let caps = PerAddr::new(2);
        let (a, b): (IpAddr, IpAddr) = ("10.0.0.1".parse().unwrap(), "10.0.0.2".parse().unwrap());
        let first = caps.take(a).unwrap();
        let _second = caps.take(a).unwrap();
        assert!(caps.take(a).is_none());
        assert!(caps.take(b).is_some());
        drop(first);
        assert!(caps.take(a).is_some());
        assert!(caps.counts.lock().unwrap().len() <= 2);
    }

    #[test]
    fn the_rate_limit_counts_per_address_and_starts_again() {
        let limit = RateLimit::new(2, Duration::from_millis(100));
        let (a, b): (IpAddr, IpAddr) = ("10.0.0.1".parse().unwrap(), "10.0.0.2".parse().unwrap());
        assert!(limit.allow(a) && limit.allow(a));
        assert!(!limit.allow(a));
        assert!(limit.allow(b));
        std::thread::sleep(Duration::from_millis(120));
        assert!(limit.allow(a));
    }
}
