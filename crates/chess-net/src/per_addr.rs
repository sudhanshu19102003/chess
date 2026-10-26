//! A count per client address, for capping how much of a server one address
//! can hold at once. [`PerAddr::take`] hands out a guard that gives the slot
//! back when it is dropped.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

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
}
