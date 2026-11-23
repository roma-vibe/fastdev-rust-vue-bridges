//! The building blocks of sign-in throttling (`throttle.rs`), in memory and bounded: a keyed
//! [`Store`] that evicts the least recently used key when full, a failure [`Backoff`] and a
//! fixed-[`Window`] counter. Every method takes the current time, so tests control the clock.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

/// At most `cap` keys; a new key beyond that evicts the expired ones, then the oldest.
#[derive(Debug)]
pub struct Store<V> {
    entries: HashMap<String, (V, Instant)>,
    cap: usize,
}

/// What a [`Store`] needs to drop old entries.
pub trait Expires {
    fn expired(&self, now: Instant) -> bool;
}

impl<V: Expires + Default> Store<V> {
    #[must_use]
    pub fn new(cap: usize) -> Self {
        Self {
            entries: HashMap::new(),
            cap: cap.max(1),
        }
    }

    /// The entry of `key` (created when missing), marked as used now.
    pub fn entry(&mut self, key: &str, now: Instant) -> &mut V {
        if !self.entries.contains_key(key) && self.entries.len() >= self.cap {
            self.entries.retain(|_, (value, _)| !value.expired(now));
            if self.entries.len() >= self.cap
                && let Some(oldest) = self
                    .entries
                    .iter()
                    .min_by_key(|(_, (_, used))| *used)
                    .map(|(k, _)| k.clone())
            {
                self.entries.remove(&oldest);
            }
        }
        let slot = self
            .entries
            .entry(key.to_owned())
            .or_insert_with(|| (V::default(), now));
        slot.1 = now;
        &mut slot.0
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&V> {
        self.entries.get(key).map(|(value, _)| value)
    }

    pub fn remove(&mut self, key: &str) {
        self.entries.remove(key);
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Failures are free up to [`Backoff::FREE`]; then every attempt waits 1 s, 2 s, 4 s … up to
/// [`Backoff::MAX_WAIT`] after the last failure. Forgotten after [`Backoff::WINDOW`] of quiet
/// (counted from the end of the wait).
#[derive(Debug, Default)]
pub struct Backoff {
    failures: u32,
    last: Option<Instant>,
}

impl Backoff {
    pub const FREE: u32 = 5;
    pub const WINDOW: Duration = Duration::from_secs(15 * 60);
    pub const MAX_WAIT: Duration = Duration::from_secs(15 * 60);

    fn wait(&self) -> Duration {
        if self.failures < Self::FREE {
            return Duration::ZERO;
        }
        let doublings = (self.failures - Self::FREE).min(20);
        Duration::from_secs(1 << doublings).min(Self::MAX_WAIT)
    }

    /// How long the next attempt must still wait (`None`: it may go now).
    #[must_use]
    pub fn blocked_for(&self, now: Instant) -> Option<Duration> {
        let until = self.last? + self.wait();
        until.checked_duration_since(now).filter(|d| !d.is_zero())
    }

    pub fn fail(&mut self, now: Instant) {
        if self.expired(now) {
            self.failures = 0;
        }
        self.failures = self.failures.saturating_add(1);
        self.last = Some(now);
    }
}

impl Expires for Backoff {
    fn expired(&self, now: Instant) -> bool {
        self.last
            .is_none_or(|last| now >= last + self.wait() + Self::WINDOW)
    }
}

/// At most `limit` hits per `period`, counted from the first hit of the period.
#[derive(Debug, Default)]
pub struct Window {
    start: Option<Instant>,
    hits: u32,
    period: Duration,
}

impl Window {
    /// Counts a hit; `Err(wait)` when the period's `limit` is used up.
    pub fn hit(&mut self, now: Instant, limit: u32, period: Duration) -> Result<(), Duration> {
        self.period = period;
        let start = match self.start {
            Some(start) if now < start + period => start,
            _ => {
                self.hits = 0;
                self.start = Some(now);
                now
            }
        };
        if self.hits >= limit {
            return Err(start + period - now);
        }
        self.hits += 1;
        Ok(())
    }
}

impl Expires for Window {
    fn expired(&self, now: Instant) -> bool {
        self.start.is_none_or(|start| now >= start + self.period)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEC: Duration = Duration::from_secs(1);

    #[test]
    fn backoff_starts_after_five_failures_and_doubles_up_to_fifteen_minutes() {
        let t0 = Instant::now();
        let mut backoff = Backoff::default();
        for _ in 0..4 {
            backoff.fail(t0);
        }
        assert_eq!(backoff.blocked_for(t0), None, "four failures are free");
        backoff.fail(t0);
        assert_eq!(backoff.blocked_for(t0), Some(SEC));
        assert_eq!(backoff.blocked_for(t0 + SEC), None);
        backoff.fail(t0 + SEC);
        assert_eq!(backoff.blocked_for(t0 + SEC), Some(2 * SEC));
        let mut now = t0 + SEC;
        for _ in 0..20 {
            now += Backoff::MAX_WAIT;
            backoff.fail(now);
        }
        assert_eq!(backoff.blocked_for(now), Some(Backoff::MAX_WAIT), "capped");
        assert!(!backoff.expired(now + Backoff::MAX_WAIT));
        assert!(backoff.expired(now + Backoff::MAX_WAIT + Backoff::WINDOW));
        backoff.fail(now + Backoff::MAX_WAIT + Backoff::WINDOW);
        assert_eq!(backoff.failures, 1, "a quiet window starts over");
    }

    #[test]
    fn windows_count_hits_per_period() {
        let t0 = Instant::now();
        let mut window = Window::default();
        let period = 10 * SEC;
        for _ in 0..3 {
            assert!(window.hit(t0, 3, period).is_ok());
        }
        assert_eq!(window.hit(t0 + 4 * SEC, 3, period), Err(6 * SEC));
        assert!(window.hit(t0 + period, 3, period).is_ok(), "a new period");
    }

    #[test]
    fn the_store_is_bounded_and_evicts_the_oldest_key() {
        let t0 = Instant::now();
        let mut store: Store<Backoff> = Store::new(2);
        store.entry("a", t0).fail(t0);
        store.entry("b", t0 + SEC).fail(t0 + SEC);
        store.entry("a", t0 + 2 * SEC);
        store.entry("c", t0 + 3 * SEC);
        assert_eq!(store.len(), 2);
        assert!(store.get("b").is_none(), "least recently used");
        assert!(store.get("a").is_some() && store.get("c").is_some());
        store.remove("a");
        assert!(store.get("a").is_none());
    }
}
