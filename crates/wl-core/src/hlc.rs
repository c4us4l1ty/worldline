//! Hybrid Logical Clock (Kulkarni et al., 2014).
//!
//! Provides timestamps that are (a) causally ordered, (b) monotonic,
//! and (c) bounded-drift from physical time. Worldline stores HLC
//! timestamps on every domain row and CRDT operation so that
//! multi-device merges converge deterministically (PRD §2.1, US-4).

use std::cmp::Ordering;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

/// Wall-clock offset applied to `SystemTime::now()` (nanoseconds).
/// Used in tests to simulate clock drift; always zero in production.
pub struct Hlc {
    /// Physical nanos since UNIX epoch (ms precision + counter).
    inner: std::sync::Mutex<HlcInner>,
    drift_nanos: AtomicU64,
}

struct HlcInner {
    last_wall_nanos: u64,
    counter: u16,
}

/// The comparable HLC timestamp stored on domain rows and CRDT ops.
///
/// Layout: 64-bit physical component (nanoseconds since UNIX epoch)
/// plus a 16-bit logical counter. Encoded as `pt.ctr.device` text with
/// an optional device id for total-order tie-breaking across devices
/// that generated identical `(pt, ctr)` pairs.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct HlcTimestamp {
    /// Physical component: nanoseconds since UNIX epoch.
    pub physical: u64,
    /// Logical counter component.
    pub counter: u16,
    /// Device discriminator (small integer or hash of device id).
    pub device: u16,
}

#[derive(Debug, thiserror::Error)]
pub enum HlcError {
    #[error("invalid HLC string: {0}")]
    Parse(String),
}

impl Hlc {
    pub fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(HlcInner {
                last_wall_nanos: 0,
                counter: 0,
            }),
            drift_nanos: AtomicU64::new(0),
        }
    }

    /// Test hook: simulate clock drift (nanoseconds added to wall time).
    #[cfg(test)]
    pub fn set_drift_nanos(&self, drift: u64) {
        self.drift_nanos.store(drift, AtomicOrdering::Relaxed);
    }

    fn wall_nanos(&self) -> u64 {
        let drift = self.drift_nanos.load(AtomicOrdering::Relaxed);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock before 1970");
        now.as_nanos() as u64 + drift
    }

    /// Issues a new local timestamp (tick).
    ///
    /// Counter exhaustion (65 535 same-instant ticks) advances the
    /// physical component by one tick instead of wrapping: timestamps
    /// stay strictly monotonic even on coarse wall clocks during bulk
    /// enqueues (E6 — `wrapping_add` used to regress mid-burst).
    pub fn now(&self, device: u16) -> HlcTimestamp {
        let mut inner = self.inner.lock().expect("HLC mutex poisoned");
        let wall = self.wall_nanos();
        if wall > inner.last_wall_nanos {
            inner.last_wall_nanos = wall;
            inner.counter = 0;
        } else if inner.counter == u16::MAX {
            // Same-instant counter exhausted: step physical forward so
            // the new timestamp still exceeds every predecessor.
            inner.last_wall_nanos = inner.last_wall_nanos.saturating_add(1).max(wall);
            inner.counter = 0;
        } else {
            inner.counter += 1;
        }
        HlcTimestamp {
            physical: inner.last_wall_nanos,
            counter: inner.counter,
            device,
        }
    }

    /// Merges a remote timestamp into the local clock (receive event).
    /// Returns the timestamp to stamp on any resulting local update.
    /// Same overflow rule as [`Hlc::now`]: never wraps, steps physical
    /// forward instead.
    pub fn observe(&self, remote: &HlcTimestamp, device: u16) -> HlcTimestamp {
        let mut inner = self.inner.lock().expect("HLC mutex poisoned");
        let wall = self.wall_nanos();
        let pt = wall.max(remote.physical);
        if pt > inner.last_wall_nanos {
            inner.last_wall_nanos = pt;
            inner.counter = 0;
        } else if inner.counter == u16::MAX {
            inner.last_wall_nanos = inner.last_wall_nanos.saturating_add(1).max(pt);
            inner.counter = 0;
        } else {
            inner.counter += 1;
        }
        HlcTimestamp {
            physical: inner.last_wall_nanos,
            counter: inner.counter,
            device,
        }
    }

    /// Current clock head (for persistence across restarts — E5).
    pub fn head(&self) -> (u64, u16) {
        let inner = self.inner.lock().expect("HLC mutex poisoned");
        (inner.last_wall_nanos, inner.counter)
    }

    /// Restores a persisted clock head at boot (E5). A fresh process
    /// seeds `last_wall_nanos` from the `hlc_clock` table so a regressed
    /// wall clock after restart cannot issue timestamps older than
    /// pre-restart ops.
    pub fn restore(&self, last_wall_nanos: u64, counter: u16) {
        let mut inner = self.inner.lock().expect("HLC mutex poisoned");
        if last_wall_nanos > inner.last_wall_nanos
            || (last_wall_nanos == inner.last_wall_nanos && counter > inner.counter)
        {
            inner.last_wall_nanos = last_wall_nanos;
            inner.counter = counter;
        }
    }
}

impl Default for Hlc {
    fn default() -> Self {
        Self::new()
    }
}

impl HlcTimestamp {
    /// Parses the canonical text encoding `physical.counter.device`.
    pub fn parse(s: &str) -> Result<Self, HlcError> {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() != 3 {
            return Err(HlcError::Parse(format!(
                "expected pt.ctr.device, got {s:?}"
            )));
        }
        let physical: u64 = parts[0]
            .parse()
            .map_err(|_| HlcError::Parse(format!("bad physical component in {s:?}")))?;
        let counter: u16 = parts[1]
            .parse()
            .map_err(|_| HlcError::Parse(format!("bad counter component in {s:?}")))?;
        let device: u16 = parts[2]
            .parse()
            .map_err(|_| HlcError::Parse(format!("bad device component in {s:?}")))?;
        Ok(Self {
            physical,
            counter,
            device,
        })
    }

    /// UNIX-epoch milliseconds truncated from the physical component
    /// (for display and coarse queries).
    pub fn epoch_ms(&self) -> u64 {
        self.physical / 1_000_000
    }
}

impl PartialOrd for HlcTimestamp {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HlcTimestamp {
    fn cmp(&self, other: &Self) -> Ordering {
        self.physical
            .cmp(&other.physical)
            .then(self.counter.cmp(&other.counter))
            .then(self.device.cmp(&other.device))
    }
}

/// Canonical text encoding with FIXED component widths:
/// `physical(20 digits).counter(5).device(5)`, zero-padded.
///
/// Fixed width is load-bearing: every persisted TEXT comparison
/// (relay `ORDER BY hlc`, `WHERE hlc > ?`, client row guards) must
/// sort identically to the numeric [`Ord`] — unpadded decimal breaks
/// that at every digit boundary (`"10" < "2"` lexicographically).
impl fmt::Display for HlcTimestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:020}.{:05}.{:05}",
            self.physical, self.counter, self.device
        )
    }
}

impl fmt::Debug for HlcTimestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hlc({})", self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_is_strictly_monotonic_same_device() {
        let hlc = Hlc::new();
        let mut last = hlc.now(1);
        for _ in 0..1000 {
            let t = hlc.now(1);
            assert!(t > last, "HLC went backwards: {last} -> {t}");
            last = t;
        }
    }

    #[test]
    fn observe_guarantees_causality() {
        let a = Hlc::new();
        let b = Hlc::new();
        // a sends t1; b observes it; b's next local tick must be > t1.
        let t1 = a.now(1);
        let merged = b.observe(&t1, 2);
        let t2 = b.now(2);
        assert!(merged >= t1);
        assert!(t2 > t1, "receive event must advance past remote ts");
    }

    #[test]
    fn drift_is_absorbed() {
        let fast = Hlc::new();
        let slow = Hlc::new();
        // Simulate `slow` running 10 seconds behind.
        slow.set_drift_nanos(10_000_000_000);
        let t_fast = fast.now(1);
        let _t_slow = slow.now(2);
        // slow (behind) observing fast's timestamp jumps forward.
        let merged = slow.observe(&t_fast, 2);
        assert!(merged >= t_fast);
    }

    #[test]
    fn counter_advances_within_same_millisecond() {
        let hlc = Hlc::new();
        let t1 = hlc.now(1);
        let t2 = hlc.now(1);
        if t1.physical == t2.physical {
            assert_eq!(t2.counter, t1.counter + 1);
        }
        assert!(t2 > t1);
    }

    #[test]
    fn text_roundtrip_preserves_order() {
        let hlc = Hlc::new();
        let t1 = hlc.now(1);
        let t2 = hlc.now(1);
        let s1 = t1.to_string();
        let s2 = t2.to_string();
        let back1 = HlcTimestamp::parse(&s1).unwrap();
        let back2 = HlcTimestamp::parse(&s2).unwrap();
        assert_eq!(back1, t1);
        assert_eq!(back2, t2);
        assert_eq!(back1.cmp(&back2), t1.cmp(&t2));
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(HlcTimestamp::parse("not a timestamp").is_err());
        assert!(HlcTimestamp::parse("1.2").is_err());
        assert!(HlcTimestamp::parse("a.b.c").is_err());
        assert!(HlcTimestamp::parse("-1.0.0").is_err());
    }

    #[test]
    fn total_order_via_device_tiebreak() {
        // Identical pt+ctr from two devices: device id breaks the tie.
        let a = HlcTimestamp {
            physical: 100,
            counter: 5,
            device: 1,
        };
        let b = HlcTimestamp {
            physical: 100,
            counter: 5,
            device: 2,
        };
        assert!(a < b);
        assert_eq!(a.max(b), b);
    }

    #[test]
    fn fixed_width_text_sorts_like_numeric_order() {
        // Digit-boundary pairs that unpadded decimal inverted.
        let pairs = [
            ("9000000000000001.10.1", "9000000000000001.2.2"),
            ("100000000000000000.100.1", "100000000000000000.99.1"),
            ("99999999999999999.65535.1", "99999999999999999.9999.2"),
        ];
        for (hi, lo) in pairs {
            let a = HlcTimestamp::parse(hi).unwrap();
            let b = HlcTimestamp::parse(lo).unwrap();
            assert!(a > b);
            assert!(
                a.to_string() > b.to_string(),
                "text order must match numeric order: {} vs {}",
                a,
                b
            );
        }
        // Fixed widths are exactly as documented.
        let t = HlcTimestamp::parse("1.2.3").unwrap();
        assert_eq!(t.to_string(), "00000000000000000001.00002.00003");
        // All encodings sort as a prefix-free, fixed-width key space:
        let samples = [
            HlcTimestamp {
                physical: 0,
                counter: 0,
                device: 0,
            },
            HlcTimestamp {
                physical: u64::MAX,
                counter: u16::MAX,
                device: u16::MAX,
            },
            HlcTimestamp {
                physical: 9,
                counter: 10,
                device: 9,
            },
        ];
        let mut texts: Vec<_> = samples.iter().map(|t| t.to_string()).collect();
        let mut sorted = samples;
        sorted.sort();
        texts.sort();
        let re_sorted: Vec<_> = texts
            .iter()
            .map(|s| HlcTimestamp::parse(s).unwrap())
            .collect();
        assert_eq!(re_sorted, sorted);
    }

    #[test]
    fn concurrent_devices_converge_ordering() {
        // Two devices ticking independently: whenever pt+ctr collide,
        // device tie-break gives a deterministic global order.
        let h1 = Hlc::new();
        let h2 = Hlc::new();
        let a = HlcTimestamp {
            physical: 500,
            counter: 3,
            device: 1,
        };
        let b = HlcTimestamp {
            physical: 500,
            counter: 3,
            device: 2,
        };
        // Both devices compare these the same way regardless of local state.
        assert_eq!(a.cmp(&b), Ordering::Less);
        assert!(h1.observe(&a, 1) > a);
        assert!(h2.observe(&b, 2) > b);
    }
}
