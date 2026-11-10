//! A small in-memory rate limiter: "at most N hits per key in a sliding window".
//!
//! State lives in this process only, so it resets on restart and is not shared between
//! several instances. That is enough for a single-node app; use a shared store (the
//! database or Redis) before running more than one instance.

use std::{
    collections::{HashMap, VecDeque},
    sync::{Mutex, PoisonError},
    time::{Duration, Instant},
};

/// The most keys tracked at once, which bounds memory however many distinct clients or
/// emails an attacker invents.
const MAX_KEYS: usize = 10_000;

/// A rate: at most `max` hits within `window`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limit {
    pub max: usize,
    pub window: Duration,
}

#[derive(Debug)]
struct Entry {
    hits: VecDeque<Instant>,
    window: Duration,
}

impl Entry {
    /// Forgets hits that have left the window.
    fn prune(&mut self, now: Instant) {
        while let Some(&oldest) = self.hits.front() {
            if now.saturating_duration_since(oldest) >= self.window {
                self.hits.pop_front();
            } else {
                break;
            }
        }
    }
}

/// Counts hits per key. Callers pass the current time, so tests can control the clock.
#[derive(Debug, Default)]
pub struct Throttle {
    entries: Mutex<HashMap<String, Entry>>,
}

impl Throttle {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether `key` has already used up `limit` within the window.
    pub fn is_blocked(&self, key: &str, limit: Limit, now: Instant) -> bool {
        let mut entries = self.lock();
        let Some(entry) = entries.get_mut(key) else {
            return false;
        };
        entry.prune(now);
        if entry.hits.is_empty() {
            entries.remove(key);
            return false;
        }
        entry.hits.len() >= limit.max
    }

    /// Counts one hit against `key`.
    pub fn record(&self, key: &str, limit: Limit, now: Instant) {
        let mut entries = self.lock();
        if !entries.contains_key(key) && entries.len() >= MAX_KEYS {
            // Drop keys whose hits have all expired; if the table is still full of live
            // keys, start over rather than grow without bound.
            entries.retain(|_, entry| {
                entry.prune(now);
                !entry.hits.is_empty()
            });
            if entries.len() >= MAX_KEYS {
                entries.clear();
            }
        }
        let entry = entries.entry(key.to_owned()).or_insert_with(|| Entry {
            hits: VecDeque::new(),
            window: limit.window,
        });
        entry.window = limit.window;
        entry.prune(now);
        entry.hits.push_back(now);
    }

    /// Forgets every hit for `key`, for example after a successful login.
    pub fn clear(&self, key: &str) {
        self.lock().remove(key);
    }

    /// How many keys are tracked.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.lock().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMIT: Limit = Limit {
        max: 3,
        window: Duration::from_secs(60),
    };

    fn after(start: Instant, seconds: u64) -> Instant {
        start + Duration::from_secs(seconds)
    }

    #[test]
    fn allows_up_to_the_limit_then_blocks() {
        let throttle = Throttle::new();
        let t0 = Instant::now();
        for _ in 0..3 {
            assert!(!throttle.is_blocked("k", LIMIT, t0));
            throttle.record("k", LIMIT, t0);
        }
        assert!(throttle.is_blocked("k", LIMIT, t0));
    }

    #[test]
    fn a_blocked_key_is_freed_when_old_hits_leave_the_window() {
        let throttle = Throttle::new();
        let t0 = Instant::now();
        throttle.record("k", LIMIT, t0);
        throttle.record("k", LIMIT, after(t0, 10));
        throttle.record("k", LIMIT, after(t0, 20));
        assert!(throttle.is_blocked("k", LIMIT, after(t0, 59)));
        // The first hit expires at 60 s: two hits remain, below the limit of three.
        assert!(!throttle.is_blocked("k", LIMIT, after(t0, 60)));
        // The rest follow.
        assert!(!throttle.is_blocked("k", LIMIT, after(t0, 81)));
        assert_eq!(throttle.len(), 0, "an idle key is forgotten");
    }

    #[test]
    fn the_window_slides_instead_of_resetting_all_at_once() {
        let throttle = Throttle::new();
        let t0 = Instant::now();
        throttle.record("k", LIMIT, t0);
        throttle.record("k", LIMIT, after(t0, 30));
        throttle.record("k", LIMIT, after(t0, 50));
        assert!(throttle.is_blocked("k", LIMIT, after(t0, 59)));
        assert!(!throttle.is_blocked("k", LIMIT, after(t0, 61)));
        throttle.record("k", LIMIT, after(t0, 61));
        assert!(
            throttle.is_blocked("k", LIMIT, after(t0, 62)),
            "30, 50 and 61 are in the window"
        );
    }

    #[test]
    fn keys_are_independent() {
        let throttle = Throttle::new();
        let t0 = Instant::now();
        for _ in 0..3 {
            throttle.record("a", LIMIT, t0);
        }
        assert!(throttle.is_blocked("a", LIMIT, t0));
        assert!(!throttle.is_blocked("b", LIMIT, t0));
    }

    #[test]
    fn clear_forgets_a_key() {
        let throttle = Throttle::new();
        let t0 = Instant::now();
        for _ in 0..3 {
            throttle.record("a", LIMIT, t0);
        }
        throttle.clear("a");
        assert!(!throttle.is_blocked("a", LIMIT, t0));
        throttle.clear("never-seen");
    }

    #[test]
    fn an_unknown_key_is_not_blocked_and_checking_does_not_create_it() {
        let throttle = Throttle::new();
        assert!(!throttle.is_blocked("x", LIMIT, Instant::now()));
        assert_eq!(throttle.len(), 0);
    }

    #[test]
    fn memory_is_bounded_when_many_keys_are_invented() {
        let throttle = Throttle::new();
        let t0 = Instant::now();
        for i in 0..(MAX_KEYS * 3) {
            throttle.record(&format!("key-{i}"), LIMIT, t0);
            assert!(throttle.len() <= MAX_KEYS);
        }
    }

    #[test]
    fn expired_keys_are_swept_before_live_ones_are_dropped() {
        let throttle = Throttle::new();
        let t0 = Instant::now();
        for i in 0..MAX_KEYS {
            throttle.record(&format!("old-{i}"), LIMIT, t0);
        }
        // A minute later every old key has expired, so a new key evicts nothing live.
        let later = after(t0, 120);
        throttle.record("fresh", LIMIT, later);
        assert_eq!(throttle.len(), 1);
        assert!(!throttle.is_blocked("fresh", LIMIT, later));
    }
}
