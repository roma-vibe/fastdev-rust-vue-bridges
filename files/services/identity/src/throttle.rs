//! Sign-in and registration throttling (in memory, per process; `limiter.rs` has the mechanics).
//!
//! - Failed sign-ins, per normalized email and per client: 5 are free within 15 minutes, then every
//!   attempt waits 1 s, 2 s, 4 s … up to 15 minutes after the last failure. A success clears both.
//! - Per client: at most [`SIGN_INS_PER_WINDOW`] sign-in attempts per 10 minutes and
//!   [`REGISTRATIONS_PER_HOUR`] registrations per hour.
//!
//! Throttled requests get 429 with `Retry-After`, the same for known and unknown emails. There is
//! no lockout: a stranger could otherwise lock the owner out. The client is `x-client-ip`, set by
//! the edge (`app_core::client_ip`; IPv6 grouped by /64).

use std::{
    sync::{Mutex, PoisonError},
    time::{Duration, Instant},
};

use app_core::{ApiError, client_ip};
use axum::http::HeaderMap;

use crate::limiter::{Backoff, Store, Window};

pub const SIGN_INS_PER_WINDOW: u32 = 30;
pub const SIGN_IN_WINDOW: Duration = Duration::from_secs(10 * 60);
pub const REGISTRATIONS_PER_HOUR: u32 = 10;
/// Keys kept per kind (failures, sign-in windows, registration windows).
const MAX_KEYS: usize = 10_000;

#[derive(Debug)]
pub struct Throttle(Mutex<Inner>);

#[derive(Debug)]
struct Inner {
    failures: Store<Backoff>,
    sign_ins: Store<Window>,
    registrations: Store<Window>,
}

impl Default for Throttle {
    fn default() -> Self {
        Self(Mutex::new(Inner {
            failures: Store::new(MAX_KEYS),
            sign_ins: Store::new(MAX_KEYS),
            registrations: Store::new(MAX_KEYS),
        }))
    }
}

/// Who is signing in: the normalized email and the client of the request.
pub struct Attempt {
    email: String,
    client: String,
}

impl Attempt {
    #[must_use]
    pub fn new(email: &str, headers: &HeaderMap) -> Self {
        Self {
            email: format!("email:{}", email.trim().to_lowercase()),
            client: client_key(headers),
        }
    }
}

fn client_key(headers: &HeaderMap) -> String {
    format!(
        "ip:{}",
        client_ip::limit_key(client_ip::from_headers(headers))
    )
}

fn too_many(wait: Duration) -> ApiError {
    ApiError::too_many_requests("Too many attempts. Try again later.", wait)
}

impl Throttle {
    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Counts a sign-in attempt; 429 while the client is over its limit or the email or the
    /// client must still wait after failures.
    ///
    /// # Errors
    /// [`ApiError::TooManyRequests`] with the time to wait.
    pub fn sign_in(&self, attempt: &Attempt) -> Result<(), ApiError> {
        let now = Instant::now();
        let mut inner = self.inner();
        inner
            .sign_ins
            .entry(&attempt.client, now)
            .hit(now, SIGN_INS_PER_WINDOW, SIGN_IN_WINDOW)
            .map_err(too_many)?;
        let wait = [&attempt.email, &attempt.client]
            .into_iter()
            .filter_map(|key| inner.failures.get(key)?.blocked_for(now))
            .max();
        wait.map_or(Ok(()), |wait| Err(too_many(wait)))
    }

    pub fn failed(&self, attempt: &Attempt) {
        let now = Instant::now();
        let mut inner = self.inner();
        for key in [&attempt.email, &attempt.client] {
            inner.failures.entry(key, now).fail(now);
        }
    }

    pub fn succeeded(&self, attempt: &Attempt) {
        let mut inner = self.inner();
        inner.failures.remove(&attempt.email);
        inner.failures.remove(&attempt.client);
    }

    /// Counts a registration of the request's client; 429 over [`REGISTRATIONS_PER_HOUR`].
    ///
    /// # Errors
    /// [`ApiError::TooManyRequests`] with the time to wait.
    pub fn register(&self, headers: &HeaderMap) -> Result<(), ApiError> {
        let now = Instant::now();
        self.inner()
            .registrations
            .entry(&client_key(headers), now)
            .hit(now, REGISTRATIONS_PER_HOUR, Duration::from_secs(60 * 60))
            .map_err(too_many)
    }
}
