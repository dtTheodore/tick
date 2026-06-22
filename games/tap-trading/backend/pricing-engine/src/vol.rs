//! EWMA volatility estimator. Spec: `MATH_SPEC.md §3.1`.
//!
//! `σ²_i = λ · σ²_{i-1} + (1 − λ) · r_i²`, annualized via √SECONDS_PER_YEAR.
//!
//! Cold-start: callers SHOULD bootstrap `σ²_0` from 5 minutes of historical
//! Pyth ticks per spec §3.1 and feed the resulting log-returns through this
//! function. With a single observation the function still returns a value
//! (seeded with `r²`), but that estimate carries no historical context —
//! see `MATH_SPEC §3.1` for the recommended cold-start procedure.

use crate::constants::SECONDS_PER_YEAR;
use crate::error::PricingError;

/// Annualized EWMA realized vol from a slice of log returns.
///
/// `log_returns[0]` is treated as the oldest. `lambda` defaults to 0.94
/// (RiskMetrics standard) per spec §3.1; values outside `[0, 1)` are
/// rejected because λ=1 freezes the estimator at the cold-start seed and
/// λ>1 makes variance diverge.
///
/// Returns `Err(InsufficientHistory)` for an empty slice so the caller can
/// distinguish "no data yet" from "zero realized vol" (spec §3.4: pause
/// taps until σ̂ confidence ≥ threshold).
pub fn estimate_realized_vol(log_returns: &[f64], lambda: f64) -> Result<f64, PricingError> {
    if !(0.0..1.0).contains(&lambda) || lambda.is_nan() {
        return Err(PricingError::InvalidLambda(lambda));
    }
    if log_returns.is_empty() {
        return Err(PricingError::InsufficientHistory);
    }
    for &r in log_returns {
        if !r.is_finite() {
            return Err(PricingError::InvalidLogReturn(r));
        }
    }

    // Cold-start: seed variance with the first observation's r².
    let mut variance = log_returns[0] * log_returns[0];
    for &r in &log_returns[1..] {
        variance = lambda * variance + (1.0 - lambda) * r * r;
    }

    Ok(variance.max(0.0).sqrt() * SECONDS_PER_YEAR.sqrt())
}

/// Post-EWMA spike absorption. Spec §3.4: when `|r_i| > 5·σ̂_{i-1}` (in
/// per-second units), bump σ̂ immediately to `|r_i|·√seconds_per_year` so
/// the next multiplier reflects the new regime without waiting for EWMA's
/// ~10s half-life. Returns `(adjusted_sigma, was_spike)` so the caller
/// can audit-log the incident.
///
/// The function is pure — no internal state. The aggregator owns the
/// EWMA buffer and `prev_sigma_annualized`; this helper is just the
/// transform applied per tick.
pub fn jump_adjusted_sigma(
    raw_sigma_annualized: f64,
    last_log_return: f64,
    prev_sigma_annualized: f64,
) -> (f64, bool) {
    // Spec §7.1: NaN σ̂ must surface, not be silently sanitized. Rust's
    // f64::max(NaN, finite) returns the finite value, so without this guard
    // the spike branch below would launder a NaN raw into a plausible bumped
    // value, defeating the caller's pause-on-NaN policy.
    if !raw_sigma_annualized.is_finite() {
        return (raw_sigma_annualized, false);
    }
    let yearly_root = SECONDS_PER_YEAR.sqrt();
    let prev_per_sec = prev_sigma_annualized / yearly_root;
    let is_spike = last_log_return.abs() > 5.0 * prev_per_sec;
    if !is_spike {
        return (raw_sigma_annualized, false);
    }
    let bumped = last_log_return.abs() * yearly_root;
    (raw_sigma_annualized.max(bumped), true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn empty_returns_insufficient_history_error() {
        assert_eq!(
            estimate_realized_vol(&[], 0.94),
            Err(PricingError::InsufficientHistory)
        );
    }

    #[test]
    fn lambda_one_rejected() {
        // λ=1 freezes the estimator — disallow per spec §3.1 (RiskMetrics λ=0.94).
        assert_eq!(
            estimate_realized_vol(&[0.01, 0.02], 1.0),
            Err(PricingError::InvalidLambda(1.0))
        );
    }

    #[test]
    fn lambda_above_one_rejected() {
        assert_eq!(
            estimate_realized_vol(&[0.01, 0.02], 1.5),
            Err(PricingError::InvalidLambda(1.5))
        );
    }

    #[test]
    fn lambda_negative_rejected() {
        match estimate_realized_vol(&[0.01, 0.02], -0.1) {
            Err(PricingError::InvalidLambda(_)) => {}
            other => panic!("expected InvalidLambda, got {other:?}"),
        }
    }

    #[test]
    fn nan_log_return_rejected() {
        // A NaN tick must not silently zero out σ via the f64::max quirk.
        match estimate_realized_vol(&[0.01, f64::NAN, 0.02], 0.94) {
            Err(PricingError::InvalidLogReturn(v)) => assert!(v.is_nan()),
            other => panic!("expected InvalidLogReturn, got {other:?}"),
        }
    }

    #[test]
    fn infinite_log_return_rejected() {
        match estimate_realized_vol(&[0.01, f64::INFINITY], 0.94) {
            Err(PricingError::InvalidLogReturn(v)) => assert!(v.is_infinite()),
            other => panic!("expected InvalidLogReturn, got {other:?}"),
        }
    }

    #[test]
    fn single_return_initializes_with_square() {
        // With no history, the first variance estimate IS r².
        // σ_annualized = |r| · √seconds_per_year. Documented caller-aware
        // limitation: the result is plausibly noisy until ~half-life ticks.
        let r = 0.001;
        let sigma = estimate_realized_vol(&[r], 0.94).unwrap();
        let expected = r.abs() * SECONDS_PER_YEAR.sqrt();
        assert!(
            (sigma - expected).abs() < 1e-6,
            "got {sigma}, expected {expected}"
        );
    }

    #[test]
    fn constant_returns_converge_to_input_vol() {
        let r = 0.01;
        let log_returns: Vec<f64> = std::iter::repeat_n(r, 500).collect();
        let sigma = estimate_realized_vol(&log_returns, 0.94).unwrap();
        let expected = r * SECONDS_PER_YEAR.sqrt();
        let rel_err = (sigma - expected).abs() / expected;
        assert!(
            rel_err < 0.01,
            "got {sigma}, expected {expected}, rel_err={rel_err}"
        );
    }

    #[test]
    fn final_sigma_elevated_after_vol_spike() {
        // 100 quiet ticks then 10 spike ticks. Catches an estimator that
        // ignores recent observations or weights the prior too heavily.
        let mut log_returns: Vec<f64> = std::iter::repeat_n(0.0001, 100).collect();
        log_returns.extend(std::iter::repeat_n(0.01, 10));
        let sigma = estimate_realized_vol(&log_returns, 0.94).unwrap();
        assert!(sigma > 5.0, "expected elevated σ, got {sigma}");
    }

    #[test]
    fn jump_adjusted_passes_through_normal_returns() {
        // r = 0.5 · σ̂_prev / √yearly is well under the 5· threshold → no adjustment.
        let prev_annual = 0.80;
        let prev_per_sec = prev_annual / SECONDS_PER_YEAR.sqrt();
        let normal_r = 0.5 * prev_per_sec;
        let (adj, was_spike) = jump_adjusted_sigma(0.81, normal_r, prev_annual);
        assert!(!was_spike);
        assert_eq!(adj, 0.81);
    }

    #[test]
    fn jump_adjusted_bumps_on_spike() {
        // r = 10· σ̂_prev_per_sec → spike → σ̂ bumped to |r|·√yearly.
        let prev_annual = 0.80;
        let prev_per_sec = prev_annual / SECONDS_PER_YEAR.sqrt();
        let spike_r = 10.0 * prev_per_sec;
        let (adj, was_spike) = jump_adjusted_sigma(0.81, spike_r, prev_annual);
        assert!(was_spike);
        let expected = spike_r.abs() * SECONDS_PER_YEAR.sqrt();
        assert!(
            (adj - expected).abs() < 1e-9,
            "adj={adj} expected={expected}"
        );
    }

    #[test]
    fn jump_adjusted_propagates_nan_raw_through_spike_path() {
        // Spec §7.1: NaN σ̂ must reach the caller so taps pause. Pre-fix,
        // f64::max(NaN, finite) returned the finite value and the spike path
        // laundered NaN into a plausible bumped σ.
        let prev_annual = 0.80;
        let prev_per_sec = prev_annual / SECONDS_PER_YEAR.sqrt();
        let spike_r = 10.0 * prev_per_sec;
        let (adj, _) = jump_adjusted_sigma(f64::NAN, spike_r, prev_annual);
        assert!(adj.is_nan(), "NaN raw must propagate, got {adj}");
    }

    #[test]
    fn jump_adjusted_preserves_raw_when_raw_already_higher() {
        // If EWMA already absorbed the spike (raw > bumped), don't downgrade.
        let prev_annual = 0.80;
        let prev_per_sec = prev_annual / SECONDS_PER_YEAR.sqrt();
        let spike_r = 6.0 * prev_per_sec;
        let huge_raw = 100.0;
        let (adj, was_spike) = jump_adjusted_sigma(huge_raw, spike_r, prev_annual);
        assert!(was_spike);
        assert_eq!(adj, huge_raw);
    }

    proptest! {
        #[test]
        fn never_returns_negative(
            returns in proptest::collection::vec(-0.5_f64..0.5, 1..200),
            lambda in 0.0_f64..0.999,
        ) {
            prop_assert!(estimate_realized_vol(&returns, lambda).unwrap() >= 0.0);
        }

        #[test]
        fn lambda_zero_collapses_to_last_return_magnitude(
            returns in proptest::collection::vec(-0.1_f64..0.1, 1..50),
        ) {
            // λ = 0 means the estimate is just the most recent r², annualized.
            let sigma = estimate_realized_vol(&returns, 0.0).unwrap();
            let last = *returns.last().unwrap();
            let expected = last.abs() * SECONDS_PER_YEAR.sqrt();
            prop_assert!(
                (sigma - expected).abs() < 1e-6,
                "got {sigma}, expected {expected}"
            );
        }
    }
}
