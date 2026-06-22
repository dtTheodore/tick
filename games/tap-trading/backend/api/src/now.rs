//! Clock dependency, injected through `AppState`.
//!
//! Production uses `Clock::real()`. Tests use `Clock::test(initial_ms)` and
//! `clock.set(ms)` to pin or advance time *per-`AppState`* — each test sees
//! only its own clock, so parallel tests can't race on a shared global.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone)]
pub struct Clock(ClockKind);

#[derive(Clone)]
enum ClockKind {
    Real,
    Test(Arc<AtomicI64>),
}

impl Clock {
    pub fn real() -> Self {
        Self(ClockKind::Real)
    }

    /// Test clock initialized to `initial_ms`. The returned `Clock` is `Clone`
    /// and every clone shares the same atomic, so a `set` on any handle is
    /// visible to all (including the one held inside `AppState` after cloning).
    pub fn test(initial_ms: i64) -> Self {
        Self(ClockKind::Test(Arc::new(AtomicI64::new(initial_ms))))
    }

    pub fn now_ms(&self) -> i64 {
        match &self.0 {
            ClockKind::Real => SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0),
            ClockKind::Test(t) => t.load(Ordering::Relaxed),
        }
    }

    /// No-op on `Clock::real()`. Tests use this to pin or advance time.
    pub fn set(&self, ms: i64) {
        if let ClockKind::Test(t) = &self.0 {
            t.store(ms, Ordering::Relaxed);
        }
    }
}
