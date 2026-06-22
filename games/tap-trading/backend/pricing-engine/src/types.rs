//! Public input/output types. Spec: `MATH_SPEC.md §5.1`.

use serde::{Deserialize, Serialize};

/// Asset symbol. Phase 1 supports ETH, BTC, SUI (`ORACLE_SPEC.md §2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum AssetSymbol {
    Eth,
    Btc,
    Sui,
}

impl AssetSymbol {
    /// Uppercase ticker — matches the serde representation and the symbols used
    /// in oracle WS/ring URLs. Exhaustive, so a new variant is a compile error
    /// rather than a silently-unhandled asset.
    pub const fn ticker(self) -> &'static str {
        match self {
            AssetSymbol::Eth => "ETH",
            AssetSymbol::Btc => "BTC",
            AssetSymbol::Sui => "SUI",
        }
    }
}

/// A single tappable cell on the price grid.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Cell {
    pub asset: AssetSymbol,
    pub strike_lo: f64,
    pub strike_hi: f64,
    pub t_open_ms: u64,
    pub t_close_ms: u64,
}

/// Snapshot of the oracle aggregator state at a point in time.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct OracleState {
    pub asset: AssetSymbol,
    pub spot: f64,
    pub sigma_annualized: f64,
    pub timestamp_ms: u64,
}

/// Tunable pricing parameters. See `MATH_SPEC.md §4.2` for v1 defaults.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PricingConfig {
    pub house_margin: f64,
    pub jump_buffer: f64,
    pub tick_period_seconds: f64,
    pub floor_a: f64,
    pub floor_b: f64,
    pub multiplier_cap: f64,
}

impl Default for PricingConfig {
    fn default() -> Self {
        Self {
            // 3% house edge (RTP 97%). Uniform across every cell via
            // `multiplier = (1 − house_margin) / P_touch`, so EV = −house_margin
            // regardless of which cell is tapped — the fairness guarantee.
            // Industry band is 1–4% (Stake/BC.Game 1%, Aviator 3%, Rollbit 5%);
            // 3% buys headroom against σ mis-estimation, the dominant house risk.
            house_margin: 0.03,
            // No σ inflation: with window-aware pricing + continuous-path
            // settlement (no discrete BGK correction), the multiplier is priced
            // at fair σ. A value > 1 here would silently bias the house. Retune
            // only as σ-conservatism in shadow mode (MATH_SPEC §6).
            jump_buffer: 1.0,
            tick_period_seconds: 0.05,
            // Flat 1.0× minimum: a tap can never return less than the stake.
            // The old τ-growing floor (1.50 + 0.025·τ) paid in-band cells far
            // ABOVE fair value (RTP 150%+ on the near cells players actually
            // tap) — the source of the "too generous" leak. Window-aware pricing
            // now makes future near cells genuinely uncertain (p < 1), so their
            // fair `(1 − margin)/p` is naturally > 1 without any incentive floor.
            floor_a: 1.0,
            floor_b: 0.0,
            multiplier_cap: 1000.0,
        }
    }
}
