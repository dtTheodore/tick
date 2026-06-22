//! Per-asset vol state.
//!
//! Pipeline per spec MATH_SPEC §3.4: `EWMA → jump-adjust → broadcast`.
//! The deque holds 1-second log returns; we sample one return per second
//! and call `tap_trading_pricing_engine::estimate_realized_vol(λ=0.94)`
//! followed by `jump_adjusted_sigma`. Cold-start (ADR-0008 §7): until we
//! have at least `COLD_START_RETURN_THRESHOLD` returns, emit the constant
//! `COLD_START_VOL_ANNUALIZED`.
//!
//! The pricing engine's signature returns `Result<f64, PricingError>` and
//! we treat any `Err(_)` as cold-start fallback — defense-in-depth against
//! unexpected NaN propagation from upstream sources.

use crate::constants::{
    COLD_START_RETURN_THRESHOLD, COLD_START_VOL_ANNUALIZED, EWMA_LAMBDA_VOL, MAX_SAMPLE_GAP_MS,
    RETURN_DEQUE_CAP,
};
use std::collections::VecDeque;
use tap_trading_pricing_engine::{estimate_realized_vol, jump_adjusted_sigma};

#[derive(Debug, Default)]
pub struct AssetVolState {
    /// Most recent 1-second log returns, oldest first.
    returns: VecDeque<f64>,
    /// Last EWMA result, needed by `jump_adjusted_sigma`.
    prev_sigma_annualized: f64,
    /// Mid at the last 1-second sample boundary; used to compute the next return.
    last_sampled_mid: Option<f64>,
    /// Wall-clock-ms of the last 1-s sample; sample boundary fires when (now − last) ≥ 1000.
    last_sample_ts_ms: Option<i64>,
}

impl AssetVolState {
    /// Update with the most recent `mid` at server time `now_ms`. Returns the
    /// annualized vol to publish on the next `OracleTick`.
    ///
    /// Called on every aggregator tick (20 Hz). Internally rate-limits the
    /// 1-second sampling so the deque grows at exactly 1 Hz independent of
    /// emit cadence.
    pub fn next_vol(&mut self, now_ms: i64, mid: f64) -> f64 {
        self.maybe_sample(now_ms, mid);

        if self.returns.len() < COLD_START_RETURN_THRESHOLD {
            return COLD_START_VOL_ANNUALIZED;
        }

        let slice: Vec<f64> = self.returns.iter().copied().collect();
        let raw = match estimate_realized_vol(&slice, EWMA_LAMBDA_VOL) {
            Ok(v) if v.is_finite() => v,
            // Err or a non-finite estimate (defense-in-depth): fall back rather
            // than store NaN, which would be sticky (NaN <= 0.0 is false) and
            // serialize to clients as `vol_annualized: null`.
            _ => {
                tracing::warn!("estimate_realized_vol unusable; using cold-start vol");
                return COLD_START_VOL_ANNUALIZED;
            }
        };

        // First real estimate after cold-start: no prior σ̂ exists, so the spike
        // detector (`|r| > 5·σ̂_prev`) would fire on any non-zero return because
        // σ̂_prev is 0, laundering a benign return into `|r|·√yr`. Seed the
        // carrier from the raw EWMA and publish it unadjusted instead. Gate on
        // `raw > 0` so a dead-flat warmup (raw == 0) does not latch here and
        // skip the spike absorber forever once volatility returns.
        if self.prev_sigma_annualized <= 0.0 {
            if raw > 0.0 {
                self.prev_sigma_annualized = raw;
            }
            return raw;
        }

        // Apply the spike absorber from MATH_SPEC §3.4.
        let last_return = self.returns.back().copied().unwrap_or(0.0);
        let (adjusted, was_spike) =
            jump_adjusted_sigma(raw, last_return, self.prev_sigma_annualized);
        if was_spike {
            tracing::info!(raw, adjusted, "vol spike absorbed");
        }
        self.prev_sigma_annualized = adjusted;
        adjusted
    }

    fn maybe_sample(&mut self, now_ms: i64, mid: f64) {
        match self.last_sample_ts_ms {
            None => {
                self.last_sample_ts_ms = Some(now_ms);
                self.last_sampled_mid = Some(mid);
            }
            Some(prev_ts) if now_ms - prev_ts >= 1_000 => {
                // Only record the return when the elapsed span is close to the
                // 1-second cadence. A larger gap means sampling was suspended
                // (DEGRADED/silence); recording it would mislabel a multi-second
                // move as one second of vol — re-baseline and skip instead.
                let elapsed_ms = (now_ms - prev_ts) as f64;
                let within_cadence = now_ms - prev_ts <= MAX_SAMPLE_GAP_MS;
                if within_cadence {
                    if let Some(prev_mid) = self.last_sampled_mid {
                        if mid > 0.0 && prev_mid > 0.0 {
                            // Normalize to a 1-second-equivalent return: variance
                            // scales with time, so a Δt-second return contributes
                            // r²·(1000/Δt) of 1-second variance. Scheduler slack
                            // or a post-silence resumption can stretch elapsed to
                            // ~2 s; recorded unscaled that overstates σ by ~41%.
                            let r = (mid / prev_mid).ln() * (1_000.0 / elapsed_ms).sqrt();
                            if r.is_finite() {
                                if self.returns.len() == RETURN_DEQUE_CAP {
                                    self.returns.pop_front();
                                }
                                self.returns.push_back(r);
                            }
                        }
                    }
                }
                self.last_sampled_mid = Some(mid);
                self.last_sample_ts_ms = Some(now_ms);
            }
            _ => { /* not yet a 1-s boundary */ }
        }
    }

    #[cfg(test)]
    pub fn returns_len(&self) -> usize {
        self.returns.len()
    }

    #[cfg(test)]
    pub fn last_return(&self) -> Option<f64> {
        self.returns.back().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cold_start_returns_constant_until_threshold() {
        let mut v = AssetVolState::default();
        for i in 0..(COLD_START_RETURN_THRESHOLD - 1) {
            // One sample per second.
            let now = (i as i64) * 1_000;
            let mid = 100.0 + i as f64 * 0.01;
            let vol = v.next_vol(now, mid);
            assert!(
                (vol - COLD_START_VOL_ANNUALIZED).abs() < 1e-12,
                "expected cold-start at i={i}, got {vol}"
            );
        }
    }

    #[test]
    fn deque_grows_by_one_per_second() {
        let mut v = AssetVolState::default();
        // First call seeds last_sampled_mid; no return yet.
        v.next_vol(0, 100.0);
        assert_eq!(v.returns_len(), 0);
        // 999 ms later — still no return.
        v.next_vol(999, 100.1);
        assert_eq!(v.returns_len(), 0);
        // 1000 ms past the seed — one return.
        v.next_vol(1_000, 100.1);
        assert_eq!(v.returns_len(), 1);
    }

    #[test]
    fn deque_capped_at_cap() {
        let mut v = AssetVolState::default();
        // Push 700 samples at 1-s cadence; capacity 600.
        for i in 0..=700 {
            v.next_vol((i as i64) * 1_000, 100.0 + (i as f64).sin() * 0.01);
        }
        assert_eq!(v.returns_len(), RETURN_DEQUE_CAP);
    }

    #[test]
    fn vol_finite_and_positive_after_warmup() {
        let mut v = AssetVolState::default();
        // 60 s of 1% per-second returns → non-cold-start, expect a real vol.
        let mut mid = 100.0;
        let mut last_vol = COLD_START_VOL_ANNUALIZED;
        for i in 0..=60 {
            mid *= 1.001;
            last_vol = v.next_vol((i as i64) * 1_000, mid);
        }
        assert!(last_vol.is_finite(), "vol must be finite");
        assert!(
            (last_vol - COLD_START_VOL_ANNUALIZED).abs() > 1e-6,
            "vol should have moved off the cold-start default, got {last_vol}"
        );
        assert!(last_vol > 0.0, "vol must be positive");
    }

    #[test]
    fn nan_mid_does_not_corrupt_deque() {
        let mut v = AssetVolState::default();
        v.next_vol(0, 100.0);
        v.next_vol(1_000, f64::NAN);
        // ln(NaN / 100) is NaN → must NOT push.
        assert_eq!(v.returns_len(), 0);
    }

    #[test]
    fn first_real_vol_equals_raw_ewma_not_a_false_spike() {
        // Regression: with prev_sigma=0 the spike detector's threshold is 5·0=0,
        // so the OLD code forced the first post-cold-start return through the
        // spike path and published `max(raw, |r_last|·√yr)`. With a flat history
        // and one final jump, that bumped value is ~4× the true EWMA. The first
        // published vol must equal the raw EWMA instead.
        let mut v = AssetVolState::default();
        // 30 flat samples (returns 0), then a 1% jump as the 30th return so the
        // deque hits COLD_START_RETURN_THRESHOLD on the jump tick.
        let mut returns = vec![0.0_f64; COLD_START_RETURN_THRESHOLD - 1];
        for i in 0..COLD_START_RETURN_THRESHOLD {
            v.next_vol((i as i64) * 1_000, 100.0);
        }
        let jump = 1.01_f64;
        let vol = v.next_vol((COLD_START_RETURN_THRESHOLD as i64) * 1_000, 100.0 * jump);
        returns.push(jump.ln());

        let raw = estimate_realized_vol(&returns, EWMA_LAMBDA_VOL).unwrap();
        let bumped = jump.ln().abs() * tap_trading_pricing_engine::SECONDS_PER_YEAR.sqrt();
        assert!(
            (vol - raw).abs() < 1e-9,
            "first vol must be raw EWMA {raw}, got {vol} (bumped would be {bumped})"
        );
        assert!(
            vol < bumped,
            "must not publish the jump-bumped value {bumped}"
        );
    }

    #[test]
    fn two_second_sample_scaled_to_one_second_equivalent() {
        // A return sampled over ~2 s must be scaled down to a 1-second-equivalent
        // (·√(1000/Δt)) so it doesn't overstate vol; recorded unscaled a 2 s
        // move carries ~2× the variance of a 1 s move.
        let mut v = AssetVolState::default();
        v.next_vol(0, 100.0); // seed baseline
        let raw_r = 0.02_f64; // log return over the gap
        v.next_vol(2_000, 100.0 * raw_r.exp()); // 2 s later
        let recorded = v.last_return().expect("a return was recorded");
        let expected = raw_r * (1_000.0_f64 / 2_000.0).sqrt(); // = raw_r / √2
        assert!(
            (recorded - expected).abs() < 1e-12,
            "2 s return must be scaled to 1 s-equivalent: got {recorded}, want {expected}"
        );
    }

    #[test]
    fn recovery_after_long_gap_does_not_record_a_jumbo_return() {
        // next_vol runs only on emitted ticks; a DEGRADED gap freezes the sample
        // clock. On recovery the elapsed span is huge — that move must NOT be
        // recorded as a single 1-second return.
        let mut v = AssetVolState::default();
        v.next_vol(0, 100.0); // seed baseline
        v.next_vol(1_000, 100.1); // normal 1s sample → 1 return
        assert_eq!(v.returns_len(), 1);
        // 5-minute gap with a 20% move (degraded period), then recovery tick.
        v.next_vol(301_000, 120.0);
        assert_eq!(v.returns_len(), 1, "gap sample must re-baseline, not push");
        // Sampling resumes cleanly one second later.
        v.next_vol(302_000, 120.12);
        assert_eq!(v.returns_len(), 2, "post-gap sampling resumes at 1 Hz");
    }
}
