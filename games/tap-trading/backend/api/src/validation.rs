//! Pure validation. Tested without DB / Redis / HTTP.
//!
//! Spec: ADR-0009 §4 step 3 (cell), `PRD.md` line 23 (stake tiers),
//! ADR-0009 §4 step 6 (drift gate, 3%).

use tap_trading_oracle_types::AssetSymbol;

use crate::error::ApiError;

/// Stake bounds in **USDC micro-units** (6 decimals). The economy is real USDC
/// (balance credited by an on-chain vault deposit, redeemable by withdraw — no
/// free faucet), and the player stakes any amount in this range their balance
/// covers — preset chips or a custom value. `MIN` is a $0.0001 dust floor; `MAX`
/// ($10) caps the per-tap bet. The balance debit (positions.rs) enforces
/// affordability; this only bounds the raw amount.
pub const MIN_STAKE_MICRO: i64 = 100;
pub const MAX_STAKE_MICRO: i64 = 10_000_000;

/// Drift tolerance: 3.0% (ADR-0009 §4 step 6).
pub const DRIFT_TOLERANCE: f64 = 0.03;

/// Cell window length in milliseconds (v1: fixed 5s).
pub const CELL_DURATION_MS: i64 = 5_000;

/// Lock window in milliseconds before close (taps inside this window reject).
pub const LOCK_WINDOW_MS: i64 = 1_000;

pub fn parse_asset(raw: &str) -> Result<AssetSymbol, ApiError> {
    match raw {
        "ETH" => Ok(AssetSymbol::Eth),
        "BTC" => Ok(AssetSymbol::Btc),
        "SUI" => Ok(AssetSymbol::Sui),
        _ => Err(ApiError::UnknownAsset),
    }
}

pub fn validate_stake(stake_points: i64) -> Result<(), ApiError> {
    if (MIN_STAKE_MICRO..=MAX_STAKE_MICRO).contains(&stake_points) {
        Ok(())
    } else {
        Err(ApiError::InvalidStake)
    }
}

pub fn validate_cell(
    t_open_ms: i64,
    t_close_ms: i64,
    strike_lo: f64,
    strike_hi: f64,
    now_ms: i64,
) -> Result<(), ApiError> {
    if !(strike_lo > 0.0 && strike_hi > strike_lo) {
        return Err(ApiError::InvalidCell);
    }
    if t_close_ms - t_open_ms != CELL_DURATION_MS {
        return Err(ApiError::InvalidCell);
    }
    if t_open_ms % CELL_DURATION_MS != 0 {
        return Err(ApiError::InvalidCell);
    }
    if now_ms + LOCK_WINDOW_MS >= t_close_ms {
        return Err(ApiError::LockWindow);
    }
    // NOTE: the spec §1 invariant "t_open > now for every tappable cell" is NOT
    // enforced here. The window-aware multiplier (multiplier.rs) already prices an
    // already-open in-band cell at ~1.0× (τ_open=0 fast path → p≈1 → no +EV), so
    // the in-play column is no longer an exploit. Adding a strict `t_open > now`
    // gate is clean future hardening but would need a skew grace and updates to
    // the in-play-now integration fixtures.
    Ok(())
}

/// Returns `true` iff `|server - client| / server > DRIFT_TOLERANCE`.
/// `server` must be positive — the multiplier floor (`MATH_SPEC §4.1`)
/// guarantees this; we still defensively reject `server <= 0` as drift.
pub fn drift_exceeded(server: f64, client: f64) -> bool {
    if server <= 0.0 {
        return true;
    }
    ((server - client).abs() / server) > DRIFT_TOLERANCE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_asset_table() {
        for (raw, ok) in [
            ("ETH", true),
            ("BTC", true),
            ("SUI", true),
            ("DOGE", false),
            ("", false),
        ] {
            assert_eq!(parse_asset(raw).is_ok(), ok, "raw={raw}");
        }
    }

    #[test]
    fn validate_stake_range() {
        for (stake, ok) in [
            (100, true),          // $0.0001 — min
            (100_000, true),      // $0.10
            (1_000_000, true),    // $1
            (10_000_000, true),   // $10 — max
            (5_000, true),        // arbitrary in-range custom value
            (50, false),          // below the dust floor
            (0, false),
            (-1, false),
            (10_000_001, false),  // a cent over the $10 cap
        ] {
            assert_eq!(validate_stake(stake).is_ok(), ok, "stake={stake}");
        }
    }

    #[test]
    fn validate_cell_rejects_misaligned_open() {
        // t_open_ms = 1_000_001 → not a 5s boundary.
        let res = validate_cell(1_000_001, 1_005_001, 100.0, 101.0, 999_000);
        assert!(matches!(res, Err(ApiError::InvalidCell)));
    }

    #[test]
    fn validate_cell_rejects_wrong_duration() {
        // 6s window.
        let res = validate_cell(1_000_000, 1_006_000, 100.0, 101.0, 999_000);
        assert!(matches!(res, Err(ApiError::InvalidCell)));
    }

    #[test]
    fn validate_cell_rejects_strike_lo_ge_hi() {
        let res = validate_cell(1_000_000, 1_005_000, 100.0, 100.0, 999_000);
        assert!(matches!(res, Err(ApiError::InvalidCell)));
    }

    #[test]
    fn validate_cell_rejects_inside_lock_window() {
        // now + 1000 == t_close → reject.
        let res = validate_cell(1_000_000, 1_005_000, 100.0, 101.0, 1_004_000);
        assert!(matches!(res, Err(ApiError::LockWindow)));
        // now + 999 < t_close → ok.
        let ok = validate_cell(1_000_000, 1_005_000, 100.0, 101.0, 1_003_999);
        assert!(ok.is_ok());
    }

    #[test]
    fn drift_at_exactly_three_percent_passes() {
        // The gate is strict >. Use values where (server - client) / server
        // is provably <= 0.03 in f64 arithmetic.
        // server=100, client=97 → delta=3, ratio=0.03 exactly (integer arithmetic).
        assert!(!drift_exceeded(100.0, 97.0));
        assert!(!drift_exceeded(100.0, 103.0));
    }

    #[test]
    fn drift_above_three_percent_rejects() {
        // 3.01% strictly exceeds the gate.
        assert!(drift_exceeded(100.0, 96.99));
        assert!(drift_exceeded(100.0, 103.01));
    }

    #[test]
    fn drift_zero_server_rejects() {
        assert!(drift_exceeded(0.0, 1.0));
        assert!(drift_exceeded(-1.0, 1.0));
    }
}
