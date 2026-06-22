//! Error type for the pricing engine boundary. Spec: `MATH_SPEC.md §7.1`.
//!
//! The engine returns `Result` from every public function that consumes
//! oracle-derived inputs (`compute_multiplier`, `compute_p_touch`,
//! `estimate_realized_vol`, `hui_no_touch`). NaN/negative spot, NaN log
//! returns, and out-of-range λ surface as `Err` so callers can implement
//! the spec-mandated "pause taps" path (§7.1, §3.4) instead of receiving
//! a silently corrupt multiplier.

use core::fmt;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PricingError {
    /// Spot price was NaN, negative, or zero. Spec §7.1: reject and pause.
    InvalidSpot(f64),
    /// Annualized σ was NaN or negative. Caller should pause taps.
    InvalidSigma(f64),
    /// EWMA λ outside `[0, 1)`. λ=1 freezes the estimator; λ>1 diverges.
    InvalidLambda(f64),
    /// A log-return sample was NaN or non-finite.
    InvalidLogReturn(f64),
    /// `terms == 0` in `hui_no_touch`. Series with zero terms is meaningless.
    InvalidTerms,
    /// Hui band degenerate: requires `0 < l < h`.
    InvalidBand { l: f64, h: f64 },
    /// Cold-start with no observations and no prior. Caller must bootstrap.
    InsufficientHistory,
    /// Hui series did not converge within the requested `max_terms`.
    /// Caller can retry with a higher `max_terms` (spec §7.1 caps at 20)
    /// or fall back to an external numerical method.
    HuiConvergenceFailure { last_term_mag: f64, terms: u32 },
}

impl fmt::Display for PricingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSpot(v) => write!(f, "invalid spot: {v}"),
            Self::InvalidSigma(v) => write!(f, "invalid sigma: {v}"),
            Self::InvalidLambda(v) => write!(f, "invalid lambda {v} (must be in [0, 1))"),
            Self::InvalidLogReturn(v) => write!(f, "invalid log return: {v}"),
            Self::InvalidTerms => write!(f, "hui terms must be >= 1"),
            Self::InvalidBand { l, h } => write!(f, "invalid band: l={l}, h={h} (need 0 < l < h)"),
            Self::InsufficientHistory => {
                write!(f, "insufficient history for vol estimate (no observations)")
            }
            Self::HuiConvergenceFailure {
                last_term_mag,
                terms,
            } => write!(
                f,
                "hui series did not converge: last_term={last_term_mag} after {terms} terms"
            ),
        }
    }
}

impl std::error::Error for PricingError {}
