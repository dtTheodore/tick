//! Broadie–Glasserman–Kou (1997) continuity correction for discretely
//! monitored barrier options.
//!
//! Reference: Broadie, M., Glasserman, P., Kou, S. (1997). "A Continuity
//! Correction for Discrete Barrier Options." *Mathematical Finance*
//! 7(4):325–349. Spec: `MATH_SPEC.md §2.2`.

use crate::constants::BETA_BGK;

/// Widen the barriers `(l, h)` outward by `β_BGK · σ · √(τ/m)` so that
/// the continuous-monitoring Hui formula approximates the discretely-
/// monitored touch probability.
///
/// Arguments
/// - `l`             lower barrier
/// - `h`             upper barrier (`h > l > 0`)
/// - `sigma_per_sec` per-second volatility
/// - `tau_sec`       window length in seconds
/// - `m`             number of monitoring ticks in the window (= `tau_sec / Δt_tick`)
///
/// Returns `(l_corrected, h_corrected)` with `l_corrected < l` and
/// `h_corrected > h`.
pub fn apply_bgk_correction(
    l: f64,
    h: f64,
    sigma_per_sec: f64,
    tau_sec: f64,
    m: f64,
) -> (f64, f64) {
    if m <= 0.0 || tau_sec <= 0.0 {
        return (l, h);
    }
    let shift = BETA_BGK * sigma_per_sec * (tau_sec / m).sqrt();
    let l_corrected = l * (-shift).exp();
    let h_corrected = h * shift.exp();
    (l_corrected, h_corrected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn beta_bgk_constant_is_correct() {
        // β = −ζ(½) / √(2π) with ζ(½) ≈ −1.4603545088…
        let expected = 1.4603545088_f64 / (2.0 * std::f64::consts::PI).sqrt();
        assert!((BETA_BGK - expected).abs() < 1e-3, "got {BETA_BGK}");
    }

    #[test]
    fn correction_widens_barriers() {
        let (l_c, h_c) = apply_bgk_correction(99.0, 101.0, 0.001, 5.0, 100.0);
        assert!(l_c < 99.0, "lower barrier should move down: {l_c}");
        assert!(h_c > 101.0, "upper barrier should move up: {h_c}");
    }

    #[test]
    fn correction_symmetric_in_log_space() {
        // The shift in log space is the same magnitude on both sides:
        //   ln(H_c / H) == -ln(L_c / L)
        let (l_c, h_c) = apply_bgk_correction(99.0, 101.0, 0.001, 5.0, 100.0);
        let up = (h_c / 101.0).ln();
        let down = (99.0 / l_c).ln();
        assert!((up - down).abs() < 1e-12, "up={up} down={down}");
    }

    proptest! {
        /// As `m → ∞`, the continuous limit, shift → 0 and barriers approach (l, h).
        ///
        /// This test catches the §4-typo bug (forgetting `/m`): without it, the
        /// shift wouldn't vanish at high `m`.
        #[test]
        fn continuous_limit_recovers_input_barriers(
            l in 50.0_f64..150.0,
            band_width in 1.0_f64..50.0,
            sigma in 0.30_f64..2.0,
            tau in 0.5_f64..30.0,
        ) {
            let h = l + band_width;
            let sigma_per_sec = sigma / crate::constants::SECONDS_PER_YEAR.sqrt();
            let (l_c, h_c) = apply_bgk_correction(l, h, sigma_per_sec, tau, 1e9);
            prop_assert!((l_c - l).abs() < 1e-3, "l_c={l_c} l={l}");
            prop_assert!((h_c - h).abs() < 1e-3, "h_c={h_c} h={h}");
        }

        /// Shift magnitude scales with σ — more vol means wider correction.
        #[test]
        fn shift_monotonic_in_sigma(
            l in 50.0_f64..150.0,
            band_width in 1.0_f64..50.0,
            sigma_low in 0.10_f64..1.0,
            sigma_bump in 0.05_f64..1.0,
            tau in 0.5_f64..30.0,
            m in 10.0_f64..10_000.0,
        ) {
            let h = l + band_width;
            let sigma_per_sec_low = sigma_low / crate::constants::SECONDS_PER_YEAR.sqrt();
            let sigma_per_sec_high =
                (sigma_low + sigma_bump) / crate::constants::SECONDS_PER_YEAR.sqrt();

            let (_, h_low) = apply_bgk_correction(l, h, sigma_per_sec_low, tau, m);
            let (_, h_high) = apply_bgk_correction(l, h, sigma_per_sec_high, tau, m);
            prop_assert!(
                h_high >= h_low,
                "higher σ should produce a wider shift: h_low={h_low} h_high={h_high}"
            );
        }
    }
}
