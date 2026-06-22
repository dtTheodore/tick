//! Per-asset aggregation state and pure helpers.
//!
//! `ORACLE_SPEC §4.4` defines the 7-step pipeline that runs every 50 ms.
//! Vol-state, ring-buffer, and broadcast wiring live in sibling modules.
//! This file owns only steps 1–6 (price aggregation) — step 7 (`OracleTick`
//! assembly with `seq`, `run_id`, `vol_annualized`) is the caller's.

use crate::constants::{
    EMA_ALPHA_PRICE, PYTH_CONF_REJECT_BPS, SOURCE_DIVERGENCE_REJECT_BPS, SOURCE_FRESHNESS_MS,
};
use crate::sources::{SourceId, SourceTick};
use std::collections::BTreeMap;

/// Median of a non-empty slice of `f64`. For even length, returns the mean
/// of the two middle elements (statistical median). Uses `total_cmp` so a stray
/// NaN sorts to an end rather than panicking the driver (the sole owner of all
/// per-asset state); `apply_sources` filters non-finite prices upstream anyway.
pub fn median_of(prices: &[f64]) -> f64 {
    assert!(!prices.is_empty(), "median_of: empty slice");
    let mut sorted = prices.to_vec();
    sorted.sort_by(f64::total_cmp);
    let n = sorted.len();
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        0.5 * (sorted[n / 2 - 1] + sorted[n / 2])
    }
}

/// EMA step: `smoothed_t = α · raw + (1 − α) · smoothed_{t−1}`.
/// Returns `raw` if `prev` is `None` (cold start).
pub fn ema_step(prev: Option<f64>, raw: f64, alpha: f64) -> f64 {
    match prev {
        None => raw,
        Some(p) => alpha * raw + (1.0 - alpha) * p,
    }
}

/// Result of one aggregation step. ORACLE_SPEC §4.4.
#[derive(Debug, Clone, PartialEq)]
pub enum AggregateOutcome {
    /// New aggregated price ready (steps 1–6 succeeded).
    Emit {
        mid: f64,
        median: f64,
        source_count: u8,
    },
    /// Too few active sources (`< 2`). Caller decides whether to emit Status.
    InsufficientSources { reason: String },
}

/// Per-asset price state. Owns the EMA carrier (`smoothed`).
#[derive(Debug, Default)]
pub struct AssetPriceState {
    smoothed: Option<f64>,
}

impl AssetPriceState {
    /// Run steps 1–6 against the latest tick per source.
    pub fn apply_sources(
        &mut self,
        now_ms: i64,
        latest: &BTreeMap<SourceId, SourceTick>,
    ) -> AggregateOutcome {
        // Steps 2–3: drop stale and low-confidence sources in one pass.
        let mut dropped = Vec::new();
        let mut active = Vec::new();
        for t in latest.values() {
            // Defense-in-depth: a non-finite price would poison the median/EMA
            // and a NaN would skew the sort. Sources already guard `is_finite`,
            // so this should never fire — but the driver is single-owner, so one
            // bad tick reaching the median must not be able to corrupt all assets.
            if !t.price.is_finite() {
                dropped.push(format!("{} non-finite price", t.source.as_str()));
                continue;
            }
            let age = now_ms - t.ts_ms;
            let fresh = age <= SOURCE_FRESHNESS_MS as i64 && age >= 0;
            if !fresh {
                dropped.push(format!("{} stale {}ms", t.source.as_str(), age));
                continue;
            }
            // Step 3: drop low-confidence Pyth.
            if let Some(bps) = t.pyth_conf_bps {
                if bps > PYTH_CONF_REJECT_BPS {
                    dropped.push(format!("pyth conf {bps} bps"));
                    continue;
                }
            }
            active.push(t);
        }

        // Step 4: minimum active count. Reset the EMA carrier on every gap so a
        // later recovery re-seeds from the fresh median instead of blending in a
        // stale pre-gap price — symmetric with the vol path's re-baseline.
        if active.len() < 2 {
            self.smoothed = None;
            return AggregateOutcome::InsufficientSources {
                reason: dropped.join("; "),
            };
        }

        // Step 5: median, then reject divergent sources. ORACLE_SPEC §6/§7.
        let provisional = median_of(&active.iter().map(|t| t.price).collect::<Vec<_>>());

        // With >= 3 sources the median is robust: a lone rogue venue's distance
        // from the median ≈ its full deviation, so a per-source threshold drops
        // it cleanly. With exactly 2 sources the median is their mean, so a
        // distance-from-median test sees only *half* the inter-source spread —
        // that case is handled by the spread check below instead.
        if active.len() >= 3 {
            active.retain(|t| {
                let diverged = ((t.price - provisional).abs() / provisional) * 10_000.0
                    > SOURCE_DIVERGENCE_REJECT_BPS as f64;
                if diverged {
                    dropped.push(format!("{} diverged from median", t.source.as_str()));
                }
                !diverged
            });
            if active.len() < 2 {
                self.smoothed = None;
                tracing::warn!(reason = %dropped.join("; "), "divergent sources left < 2 active");
                return AggregateOutcome::InsufficientSources {
                    reason: dropped.join("; "),
                };
            }
        }

        let median = median_of(&active.iter().map(|t| t.price).collect::<Vec<_>>());

        // Exactly 2 active sources (originally, or after the reject above): the
        // median is their mean, which neither venue may support. Test the spread
        // directly — two sources disagreeing beyond the threshold can't both be
        // trusted, so pause rather than emit a fiction. Without this, a spread up
        // to ~2× the threshold slips through (each source sits half the spread
        // from the mean). ORACLE_SPEC §6/§7.
        if active.len() == 2 {
            let spread_bps = ((active[0].price - active[1].price).abs() / median) * 10_000.0;
            if spread_bps > SOURCE_DIVERGENCE_REJECT_BPS as f64 {
                self.smoothed = None;
                dropped.push(format!("2-source spread {spread_bps:.0} bps"));
                tracing::warn!(reason = %dropped.join("; "), "2 sources diverge; pausing");
                return AggregateOutcome::InsufficientSources {
                    reason: dropped.join("; "),
                };
            }
        }

        // Step 6: EMA blend.
        let mid = ema_step(self.smoothed, median, EMA_ALPHA_PRICE);
        self.smoothed = Some(mid);

        AggregateOutcome::Emit {
            mid,
            median,
            source_count: active.len() as u8,
        }
    }
}

use crate::constants::DEGRADED_HYSTERESIS_MS;
use tap_trading_oracle_types::OracleStreamState;

/// What the aggregator decided to emit on a given asset for one 50 ms tick.
#[derive(Debug, Clone, PartialEq)]
pub enum TickDecision {
    /// Emit a tick (price + vol assembled by the caller).
    Tick {
        mid: f64,
        median: f64,
        source_count: u8,
    },
    /// Emit a status frame because the asset just transitioned phase.
    Status {
        state: OracleStreamState,
        reason: String,
    },
    /// No emission: still in current phase, or insufficient data without hysteresis fired.
    Silence,
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
enum Phase {
    #[default]
    Normal,
    Degraded,
}

#[derive(Debug, Default)]
pub struct AssetStreamPhase {
    phase: Phase,
    /// Ms-stamp of the first observation that started moving toward the
    /// opposite phase. `None` if no transition is pending.
    pending_since_ms: Option<i64>,
}

impl AssetStreamPhase {
    /// Drive the hysteresis state machine. Returns the side-effect to emit.
    pub fn step(&mut self, now_ms: i64, outcome: &AggregateOutcome) -> TickDecision {
        match (self.phase, outcome) {
            (
                Phase::Normal,
                AggregateOutcome::Emit {
                    mid,
                    median,
                    source_count,
                },
            ) => {
                self.pending_since_ms = None;
                TickDecision::Tick {
                    mid: *mid,
                    median: *median,
                    source_count: *source_count,
                }
            }
            (Phase::Normal, AggregateOutcome::InsufficientSources { reason }) => {
                let started = *self.pending_since_ms.get_or_insert(now_ms);
                if now_ms - started >= DEGRADED_HYSTERESIS_MS as i64 {
                    self.phase = Phase::Degraded;
                    self.pending_since_ms = None;
                    TickDecision::Status {
                        state: OracleStreamState::Degraded,
                        reason: reason.clone(),
                    }
                } else {
                    TickDecision::Silence
                }
            }
            (
                Phase::Degraded,
                AggregateOutcome::Emit {
                    mid,
                    median,
                    source_count,
                },
            ) => {
                let started = *self.pending_since_ms.get_or_insert(now_ms);
                if now_ms - started >= DEGRADED_HYSTERESIS_MS as i64 {
                    self.phase = Phase::Normal;
                    self.pending_since_ms = None;
                    // Recovery emits a Status FIRST; the next 50 ms tick will emit data.
                    TickDecision::Status {
                        state: OracleStreamState::Normal,
                        reason: format!("recovered with {source_count} sources"),
                    }
                } else {
                    // Not enough sustained recovery; still degraded → no tick emission.
                    let _ = (mid, median);
                    TickDecision::Silence
                }
            }
            (Phase::Degraded, AggregateOutcome::InsufficientSources { .. }) => {
                self.pending_since_ms = None;
                TickDecision::Silence
            }
        }
    }
}
