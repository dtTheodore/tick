//! End-to-end multiplier orchestration. Spec: `MATH_SPEC.md §4`.

use crate::constants::{EPSILON, SECONDS_PER_YEAR};
use crate::error::PricingError;
use crate::types::{Cell, OracleState, PricingConfig};

// Fixed Gauss-weighted grid for the t_open spot integral. MUST be byte-identical
// to the TS port (`multiplier.ts`) so client preview and server price agree well
// inside the 3% drift gate. Midpoint rule over a ±6σ-truncated standard normal;
// the 1/√(2π) normalisation cancels in `acc / wsum`.
const Z_LO: f64 = -6.0;
const Z_HI: f64 = 6.0;
const Z_STEPS: usize = 64;

/// Probability that the *continuous* price path enters `[strike_lo, strike_hi)`
/// at some time during the cell's monitoring window `[t_open, t_close]`.
///
/// `now_ms` is passed in (rather than read from wall-clock) so the function
/// stays pure and testable. Callers pass `chrono::Utc::now()` in epoch-ms.
///
/// Unlike a naïve `[now, t_close]` model, this respects the spec invariant that
/// the cell is only live in the *future* window `[t_open, t_close]` and the path
/// from `now` to `t_open` is unobserved (MATH_SPEC §1). The price at `t_open` is
/// therefore a random variable `S(t_open) = S₀·exp(σ_o·Z − σ_o²/2)` with
/// `σ_o = σ_sec·√τ_open` (driftless martingale). We integrate the
/// continuous-monitoring touch probability over that distribution:
///   `P_touch = E_Z[ touch(S(t_open) → band over τ_win) ]`.
/// This is why a center cell isn't a certainty: by `t_open` the price may have
/// drifted out of the band — so its fair multiplier is naturally `> 1` without
/// any floor, which is what removes the old in-band→1 over-payout.
///
/// Settlement is on the continuous path (see settlement-worker `touch.rs`), so
/// there is no discrete-monitoring correction here (BGK was removed with it).
///
/// Returns `Err(InvalidSpot)`/`Err(InvalidSigma)` for bad oracle inputs (spec
/// §7.1) and `Ok(0.0)` for a cell already past its close (not tappable).
pub fn compute_p_touch(
    cell: &Cell,
    oracle: &OracleState,
    cfg: &PricingConfig,
    now_ms: u64,
) -> Result<f64, PricingError> {
    if !(oracle.spot.is_finite() && oracle.spot > 0.0) {
        return Err(PricingError::InvalidSpot(oracle.spot));
    }
    if !(oracle.sigma_annualized.is_finite() && oracle.sigma_annualized >= 0.0) {
        return Err(PricingError::InvalidSigma(oracle.sigma_annualized));
    }
    if cell.t_close_ms <= now_ms {
        return Ok(0.0);
    }
    let sigma_per_sec = (oracle.sigma_annualized / SECONDS_PER_YEAR.sqrt()) * cfg.jump_buffer;
    let t_open_eff = cell.t_open_ms.max(now_ms);
    let tau_open_sec = (t_open_eff - now_ms) as f64 / 1000.0;
    // Window we actually monitor. For an already-open cell (t_open ≤ now) this is
    // the remaining time to close; for a future cell it's the full window.
    let tau_win_sec = (cell.t_close_ms - t_open_eff) as f64 / 1000.0;

    // Fast path: cell is open now (τ_open = 0) → S(t_open) = S₀ deterministically,
    // the integral collapses to a single evaluation. Also keeps parity exact with
    // the closed-form first-passage on the fixture set (all τ_open = 0).
    if tau_open_sec <= 0.0 {
        return Ok(touch_prob_from(oracle.spot, cell.strike_lo, cell.strike_hi, sigma_per_sec, tau_win_sec));
    }

    let sigma_open = sigma_per_sec * tau_open_sec.sqrt();
    let dz = (Z_HI - Z_LO) / Z_STEPS as f64;
    let mut acc = 0.0;
    let mut wsum = 0.0;
    for k in 0..Z_STEPS {
        let z = Z_LO + (k as f64 + 0.5) * dz;
        let w = (-0.5 * z * z).exp();
        let s_open = oracle.spot * (sigma_open * z - 0.5 * sigma_open * sigma_open).exp();
        acc += w * touch_prob_from(s_open, cell.strike_lo, cell.strike_hi, sigma_per_sec, tau_win_sec);
        wsum += w;
    }
    Ok((acc / wsum).clamp(0.0, 1.0))
}

/// Continuous-monitoring touch probability of band `[lo, hi)` over `tau_sec`,
/// starting from `s`. In-band start → certain (1.0); otherwise first-passage to
/// the near edge only — under first-touch the far edge and band width are
/// irrelevant once the near edge is reached.
fn touch_prob_from(s: f64, lo: f64, hi: f64, sigma_per_sec: f64, tau_sec: f64) -> f64 {
    if s >= lo && s < hi {
        return 1.0;
    }
    if sigma_per_sec <= 0.0 || tau_sec <= 0.0 {
        return 0.0;
    }
    let barrier = if s < lo { lo } else { hi };
    first_passage_touch_prob(s, barrier, sigma_per_sec, tau_sec).clamp(0.0, 1.0)
}

/// Probability that GBM touches `barrier` within `tau_sec`, given a spot
/// `s0` on the opposite side of the band (out-of-band first passage). Spec §2.3.
///
/// Reflection-principle result for zero-log-drift Brownian motion:
///   `P(touch ≤ τ) = 2 · Φ(−|ln(barrier / s0)| / (σ_sec · √τ))`
/// This is an approximation of the exact BSM r=q=0 result (log-drift −σ²/2);
/// rel-err ≤ ~5e-4 at Tick scales (σ·√τ ≤ 1.1e-3 for τ ≤ 60s). Pinned against
/// the exact Karatzas–Shreve form by `otm_approximation_matches_exact_within_1e_3`
/// and against QuantLib by the `otm_quantlib_parity` integration test.
pub fn first_passage_touch_prob(s0: f64, barrier: f64, sigma_per_sec: f64, tau_sec: f64) -> f64 {
    if sigma_per_sec <= 0.0 || tau_sec <= 0.0 {
        return 0.0;
    }
    let b = (barrier / s0).ln().abs();
    let v = sigma_per_sec * tau_sec.sqrt();
    (2.0 * normal_cdf(-b / v)).clamp(0.0, 1.0)
}

/// Standard normal CDF Φ(x) = ½ · erfc(−x/√2).
fn normal_cdf(x: f64) -> f64 {
    0.5 * libm::erfc(-x / std::f64::consts::SQRT_2)
}

/// Final multiplier = `max(floor(τ), (1 − house_margin) / P_touch)`, capped.
///
/// Returns `Ok(0.0)` if the cell has already closed. Returns `Err` if the
/// underlying `compute_p_touch` rejects the oracle inputs (spec §7.1).
pub fn compute_multiplier(
    cell: &Cell,
    oracle: &OracleState,
    cfg: &PricingConfig,
    now_ms: u64,
) -> Result<f64, PricingError> {
    if cell.t_close_ms <= now_ms {
        return Ok(0.0);
    }
    let tau_close_sec = (cell.t_close_ms - now_ms) as f64 / 1000.0;
    let floor = cfg.floor_a + cfg.floor_b * tau_close_sec;

    let p_touch = compute_p_touch(cell, oracle, cfg, now_ms)?;
    let raw = if p_touch < EPSILON {
        cfg.multiplier_cap
    } else {
        (1.0 - cfg.house_margin) / p_touch
    };

    Ok(raw.max(floor).min(cfg.multiplier_cap))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AssetSymbol;
    use proptest::prelude::*;

    fn default_cfg() -> PricingConfig {
        PricingConfig::default()
    }

    fn cell_at(strike_lo: f64, strike_hi: f64, t_open_ms: u64, t_close_ms: u64) -> Cell {
        Cell {
            asset: AssetSymbol::Eth,
            strike_lo,
            strike_hi,
            t_open_ms,
            t_close_ms,
        }
    }

    fn oracle_at(spot: f64, sigma_annualized: f64, ts: u64) -> OracleState {
        OracleState {
            asset: AssetSymbol::Eth,
            spot,
            sigma_annualized,
            timestamp_ms: ts,
        }
    }

    #[test]
    fn closed_cell_returns_zero() {
        let now = 1_000_000;
        let cell = cell_at(3812.0, 3812.5, now - 10_000, now - 5_000);
        let oracle = oracle_at(3812.25, 0.80, now);
        let m = compute_multiplier(&cell, &oracle, &default_cfg(), now).unwrap();
        assert_eq!(m, 0.0);
    }

    #[test]
    fn nan_sigma_rejected() {
        // Spec §7.1: bad oracle data must surface as Err, not silent floor.
        let now = 1_000_000;
        let cell = cell_at(3812.0, 3812.5, now, now + 5_000);
        let oracle = oracle_at(3812.25, f64::NAN, now);
        match compute_multiplier(&cell, &oracle, &default_cfg(), now) {
            Err(PricingError::InvalidSigma(v)) => assert!(v.is_nan()),
            other => panic!("expected InvalidSigma, got {other:?}"),
        }
    }

    #[test]
    fn negative_sigma_rejected() {
        let now = 1_000_000;
        let cell = cell_at(3812.0, 3812.5, now, now + 5_000);
        let oracle = oracle_at(3812.25, -0.1, now);
        match compute_multiplier(&cell, &oracle, &default_cfg(), now) {
            Err(PricingError::InvalidSigma(_)) => {}
            other => panic!("expected InvalidSigma, got {other:?}"),
        }
    }

    #[test]
    fn zero_spot_rejected() {
        let now = 1_000_000;
        let cell = cell_at(3812.0, 3812.5, now, now + 5_000);
        let oracle = oracle_at(0.0, 0.80, now);
        match compute_multiplier(&cell, &oracle, &default_cfg(), now) {
            Err(PricingError::InvalidSpot(v)) => assert_eq!(v, 0.0),
            other => panic!("expected InvalidSpot, got {other:?}"),
        }
    }

    #[test]
    fn nan_spot_rejected() {
        let now = 1_000_000;
        let cell = cell_at(3812.0, 3812.5, now, now + 5_000);
        let oracle = oracle_at(f64::NAN, 0.80, now);
        match compute_multiplier(&cell, &oracle, &default_cfg(), now) {
            Err(PricingError::InvalidSpot(v)) => assert!(v.is_nan()),
            other => panic!("expected InvalidSpot, got {other:?}"),
        }
    }

    #[test]
    fn negative_spot_rejected() {
        let now = 1_000_000;
        let cell = cell_at(3812.0, 3812.5, now, now + 5_000);
        let oracle = oracle_at(-1.0, 0.80, now);
        match compute_multiplier(&cell, &oracle, &default_cfg(), now) {
            Err(PricingError::InvalidSpot(_)) => {}
            other => panic!("expected InvalidSpot, got {other:?}"),
        }
    }

    #[test]
    fn in_band_cell_at_open_pays_flat_floor() {
        // A cell already open (t_open = now, τ_open = 0) whose band holds the
        // spot is a certainty over its window → p = 1 → raw = (1−0.03) = 0.97,
        // floored to the flat 1.0× minimum. No τ-growing giveaway, at any window.
        let now = 1_000_000;
        let oracle = oracle_at(3812.25, 0.80, now);
        for tau in [5_000u64, 30_000] {
            let cell = cell_at(3812.0, 3812.5, now, now + tau);
            let m = compute_multiplier(&cell, &oracle, &default_cfg(), now).unwrap();
            assert!((m - 1.0).abs() < 0.001, "τ={tau}: expected flat floor 1.0, got {m}");
        }
    }

    #[test]
    fn future_in_band_cell_pays_above_floor() {
        // The fix's crux: the SAME center band, but a FUTURE column (τ_open > 0),
        // is genuinely uncertain — the spot may drift out before the window opens
        // — so its fair multiplier is naturally > 1 WITHOUT any incentive floor.
        let now = 1_000_000;
        let cell = cell_at(3812.0, 3812.5, now + 5_000, now + 10_000);
        let oracle = oracle_at(3812.25, 0.80, now);
        let m = compute_multiplier(&cell, &oracle, &default_cfg(), now).unwrap();
        assert!(m > 1.0, "future center cell should price above the floor, got {m}");
    }

    proptest! {
        #[test]
        fn p_touch_in_unit_interval(
            spot in 100.0_f64..5_000.0,
            band_offset in -0.05_f64..0.05,
            band_width_pct in 0.0001_f64..0.05,
            sigma in 0.30_f64..2.0,
            tau_ms in 1_000_u64..30_000,
        ) {
            let now = 1_000_000_u64;
            let lo = spot * (1.0 + band_offset);
            let hi = lo + spot * band_width_pct;
            let cell = cell_at(lo, hi, now, now + tau_ms);
            let oracle = oracle_at(spot, sigma, now);
            let p = compute_p_touch(&cell, &oracle, &default_cfg(), now).unwrap();
            prop_assert!((0.0..=1.0).contains(&p), "out of range: {p}");
        }

        #[test]
        fn multiplier_at_least_floor(
            spot in 100.0_f64..5_000.0,
            band_offset in -0.05_f64..0.05,
            band_width_pct in 0.0001_f64..0.05,
            sigma in 0.30_f64..2.0,
            tau_ms in 1_000_u64..30_000,
        ) {
            let now = 1_000_000_u64;
            let lo = spot * (1.0 + band_offset);
            let hi = lo + spot * band_width_pct;
            let cell = cell_at(lo, hi, now, now + tau_ms);
            let oracle = oracle_at(spot, sigma, now);
            let cfg = default_cfg();
            let m = compute_multiplier(&cell, &oracle, &cfg, now).unwrap();
            let tau_sec = tau_ms as f64 / 1000.0;
            let floor = cfg.floor_a + cfg.floor_b * tau_sec;
            prop_assert!(m >= floor - 1e-9, "m={m} floor={floor}");
        }

        #[test]
        fn multiplier_at_most_cap(
            spot in 100.0_f64..5_000.0,
            band_offset in -0.20_f64..0.20,
            band_width_pct in 0.0001_f64..0.05,
            sigma in 0.30_f64..2.0,
            tau_ms in 1_000_u64..30_000,
        ) {
            let now = 1_000_000_u64;
            let lo = spot * (1.0 + band_offset);
            let hi = lo + spot * band_width_pct;
            let cell = cell_at(lo, hi, now, now + tau_ms);
            let oracle = oracle_at(spot, sigma, now);
            let cfg = default_cfg();
            let m = compute_multiplier(&cell, &oracle, &cfg, now).unwrap();
            prop_assert!(m <= cfg.multiplier_cap + 1e-9, "m={m}");
        }
    }

    /// The floor is a flat 1.0× minimum (a tap never returns less than stake),
    /// independent of τ. Guards against a regression that reintroduces the old
    /// τ-growing incentive floor, which over-paid near cells (the "too generous"
    /// leak): fair value comes from `(1−margin)/p`, not from the floor.
    #[test]
    fn floor_is_flat_unit_minimum() {
        let cfg = default_cfg();
        for tau in [5.0_f64, 10.0, 30.0, 50.0] {
            let floor = cfg.floor_a + cfg.floor_b * tau;
            assert!((floor - 1.0).abs() < 1e-9, "floor({tau})={floor}, expected flat 1.0");
        }
    }

    #[test]
    fn deep_otm_below_band_pays_high_multiplier() {
        let now = 1_000_000;
        let cell = cell_at(4000.0, 4001.0, now, now + 5_000);
        let oracle = oracle_at(3812.0, 0.80, now);
        let m = compute_multiplier(&cell, &oracle, &default_cfg(), now).unwrap();
        assert!(m > 10.0, "expected high multiplier, got {m}");
        assert!(m <= 1000.0, "expected ≤ cap, got {m}");
    }

    #[test]
    fn deep_otm_above_band_pays_high_multiplier() {
        let now = 1_000_000;
        let cell = cell_at(3700.0, 3701.0, now, now + 5_000);
        let oracle = oracle_at(3812.0, 0.80, now);
        let m = compute_multiplier(&cell, &oracle, &default_cfg(), now).unwrap();
        assert!(m > 10.0, "expected high multiplier, got {m}");
        assert!(m <= 1000.0, "expected ≤ cap, got {m}");
    }

    #[test]
    fn cap_enforced_for_extreme_otm() {
        let now = 1_000_000;
        let cell = cell_at(100_000.0, 100_001.0, now, now + 5_000);
        let oracle = oracle_at(3812.0, 0.30, now);
        let m = compute_multiplier(&cell, &oracle, &default_cfg(), now).unwrap();
        assert_eq!(m, 1000.0);
    }

    #[test]
    fn p_touch_decreases_with_otm_distance_below() {
        let now = 1_000_000;
        let oracle = oracle_at(3812.0, 0.80, now);
        let cfg = default_cfg();
        let mut prev_p: Option<f64> = None;
        for offset in [10.0, 50.0, 200.0, 500.0] {
            let cell = cell_at(3812.0 + offset, 3812.5 + offset, now, now + 5_000);
            let p = compute_p_touch(&cell, &oracle, &cfg, now).unwrap();
            if let Some(prev) = prev_p {
                assert!(
                    p <= prev + 1e-9,
                    "p={p} should be ≤ prev={prev} at offset {offset}"
                );
            }
            prev_p = Some(p);
        }
    }

    #[test]
    fn p_touch_decreases_with_otm_distance_above() {
        let now = 1_000_000;
        let oracle = oracle_at(3812.0, 0.80, now);
        let cfg = default_cfg();
        let mut prev_p: Option<f64> = None;
        for offset in [10.0, 50.0, 200.0, 500.0] {
            let cell = cell_at(3812.0 - offset - 0.5, 3812.0 - offset, now, now + 5_000);
            let p = compute_p_touch(&cell, &oracle, &cfg, now).unwrap();
            if let Some(prev) = prev_p {
                assert!(
                    p <= prev + 1e-9,
                    "p={p} should be ≤ prev={prev} at offset {offset}"
                );
            }
            prev_p = Some(p);
        }
    }

    #[test]
    fn spot_at_lower_strike_edge_is_in_band() {
        let now = 1_000_000;
        let cell = cell_at(3812.0, 3812.5, now, now + 5_000);
        let oracle = oracle_at(3812.0, 0.80, now);
        let cfg = default_cfg();
        let p = compute_p_touch(&cell, &oracle, &cfg, now).unwrap();
        assert!((p - 1.0).abs() < 1e-9, "expected p=1, got {p}");
    }

    /// Pins the reflection-principle approximation `2·Φ(−b/v)` against the
    /// exact Karatzas–Shreve first-passage formula with drift μ = −σ²/2
    /// (spec §2.3). Spec claims rel-err ≈ 4e-4 at Tick scales; empirically
    /// the worst case in this sweep (closest OTM distance, smallest b/v) is
    /// ~5e-4, so we set the gate at 1e-3 — tight enough that dropping the
    /// leading `2·`, mis-scaling v, or sign-flipping the drift fails loudly,
    /// loose enough to absorb the spec's "≈".
    #[test]
    fn otm_approximation_matches_exact_within_1e_3() {
        let sigma_per_sec = 0.80 / SECONDS_PER_YEAR.sqrt();
        let tau_sec: f64 = 5.0;
        let v = sigma_per_sec * tau_sec.sqrt();

        // Restrict to OTM offsets where Φ(-b/v) stays in [1e-9, 1] — beyond
        // that, multiplier_cap kicks in (EPSILON = 1e-9) and parity rel-err
        // becomes meaningless catastrophic-cancellation noise in the tail.
        let s0 = 3812.0_f64;
        for offset_pct in [0.00005, 0.0001, 0.0005, 0.001] {
            let l = s0 * (1.0 + offset_pct);
            let b = (l / s0).ln();

            let approx = 2.0 * normal_cdf(-b / v);

            // Exact: μ = -σ²/2; μτ = -v²/2; exp(2μb/σ²) = exp(-b) = S_0/L.
            let mu_tau = -0.5 * v * v;
            let exact = normal_cdf((mu_tau - b) / v) + (-b).exp() * normal_cdf(-(mu_tau + b) / v);

            let rel_err = (approx - exact).abs() / exact;
            assert!(
                rel_err < 1e-3,
                "offset={offset_pct} approx={approx} exact={exact} rel_err={rel_err}"
            );
        }
    }
}
