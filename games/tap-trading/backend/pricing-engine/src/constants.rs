//! Numerical constants. Spec: `MATH_SPEC.md §4.2`.

/// Broadie–Glasserman–Kou continuity-correction constant.
///
/// `β = −ζ(½) / √(2π) ≈ 0.5826` (BGK 1997).
pub const BETA_BGK: f64 = 0.5826;

/// Seconds in a year, RiskMetrics standard (365.25 days × 86_400).
pub const SECONDS_PER_YEAR: f64 = 31_557_600.0;

/// Numerical floor on `P_touch` below which we treat the cell as "untouchable".
pub const EPSILON: f64 = 1e-9;
