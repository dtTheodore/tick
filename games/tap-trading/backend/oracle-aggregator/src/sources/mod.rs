//! Source-side types. Concrete clients land in Tasks 9–12.

// Trait and helpers are added incrementally across tasks.
#![allow(dead_code)]

use tap_trading_oracle_types::AssetSymbol;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SourceId {
    Pyth,
    Binance,
    Bybit,
    Okx,
}

impl SourceId {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceId::Pyth => "pyth",
            SourceId::Binance => "binance",
            SourceId::Bybit => "bybit",
            SourceId::Okx => "okx",
        }
    }
}

/// One observation from one source, normalized into our units.
///
/// `ts_ms` is the **server-received** timestamp — never the exchange-provided
/// one (ORACLE_SPEC §4.5 rationale).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SourceTick {
    pub source: SourceId,
    pub asset: AssetSymbol,
    pub price: f64,
    pub ts_ms: i64,
    /// Pyth confidence interval in basis points of price; `None` for non-Pyth.
    pub pyth_conf_bps: Option<u32>,
}

use crate::constants::BACKOFF_MAX_MS;
use async_trait::async_trait;
use std::cell::Cell;
use std::hash::{Hash, Hasher};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

/// Connected source that pushes `SourceTick` into a channel.
///
/// `run` consumes `self` and returns when the source's supervisor decides
/// to stop (process shutdown). Reconnect is internal to the implementation
/// — callers do not see individual disconnects.
#[async_trait]
pub trait Source: Send + 'static {
    fn id(&self) -> SourceId;
    async fn run(self: Box<Self>, tx: mpsc::Sender<SourceTick>);
}

/// Exponential backoff with jitter. Capped at `BACKOFF_MAX_MS`. ORACLE_SPEC §4.2.
pub async fn sleep_jittered(backoff_ms: &mut u64) {
    let jitter_amplitude = (*backoff_ms as f64) * 0.1;
    let jitter = (rand_unit() - 0.5) * 2.0 * jitter_amplitude;
    // Clamp the jittered sleep to the cap as well — otherwise +10% jitter on a
    // maxed backoff sleeps up to ~33 s, past the documented 30 s ceiling.
    let with_jitter = (*backoff_ms as f64 + jitter).clamp(50.0, BACKOFF_MAX_MS as f64) as u64;
    tokio::time::sleep(Duration::from_millis(with_jitter)).await;
    *backoff_ms = (*backoff_ms * 2).min(BACKOFF_MAX_MS);
}

thread_local! {
    /// Per-thread PRNG state for backoff jitter. Seeded once per worker thread
    /// from time XOR thread-id so jitter is decorrelated across source tasks —
    /// the old `subsec_nanos() % 1000` returned near-identical values to sources
    /// reconnecting in the same instant, defeating the anti-herd jitter.
    static JITTER_RNG: Cell<u64> = Cell::new(seed_jitter_rng());
}

fn seed_jitter_rng() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::thread::current().id().hash(&mut hasher);
    nanos ^ hasher.finish().rotate_left(32) ^ 0x9E37_79B9_7F4A_7C15
}

/// Uniform value in `[0, 1)` via SplitMix64 — adequate for backoff jitter
/// (decorrelation), not cryptography.
pub fn rand_unit() -> f64 {
    JITTER_RNG.with(|state| {
        let mut z = state.get().wrapping_add(0x9E37_79B9_7F4A_7C15);
        state.set(z);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        // Top 53 bits map to f64 mantissa precision in [0, 1).
        (z >> 11) as f64 / (1u64 << 53) as f64
    })
}

pub mod binance;
pub mod bybit;
pub mod okx;
pub mod pyth;
