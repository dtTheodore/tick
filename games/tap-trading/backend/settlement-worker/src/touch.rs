//! Pure touch-detection. SYSTEM_DESIGN.md §5.2.
//!
//! A cell is a price *zone* `[strike_lo, strike_hi)`; the player wins if the mid
//! enters that zone at any time in `[t_open, t_close]` (MATH_SPEC §1 first-touch:
//! "Win if spot(t) ∈ [L, H] for some t"). Half-open on the high edge so adjacent
//! cells in the ladder don't both claim a mid sitting exactly on their shared
//! boundary — same convention the client uses for the live hit cue and pricing.
//!
//! **Settlement is on the price PATH, not on discrete tick samples.** The oracle
//! feed is discrete (~20 Hz) but the underlying price is continuous, and a fast
//! wick can cross a narrow band entirely *between* two ticks — empirically a
//! single inter-tick jump exceeds the band step on ETH. Sampling only the
//! discrete `tick.mid` would miss those crossings, so the player loses on a band
//! the price visibly swept through (the chart line is drawn as that same
//! continuous path). We therefore treat the straight segment from the previous
//! tick's mid to this tick's mid as the path and win if that segment intersects
//! the band. This makes settlement agree with the line the player watches and
//! removes the discrete-monitoring under-count (so BGK band-widening in pricing,
//! which existed to bridge discrete→continuous, is no longer needed).

use tap_trading_oracle_types::OracleTick;

use crate::cache::PositionRef;

#[derive(Debug, Clone, PartialEq)]
pub enum TouchOutcome {
    /// The price path entered the zone within the monitoring window. Settle WIN.
    Win,
    /// Tick is past `t_close_ms` with no prior touch. Settle LOST.
    Expire,
    /// Path stayed outside the zone and tick is in-window. Keep monitoring.
    Hold,
}

/// Pure decision: should this position be settled given this tick?
///
/// `prev_mid` is the asset's previous tick mid (None on the first tick after the
/// worker started / a gap). When present, the path from `prev_mid` to `tick.mid`
/// is tested against the band; when absent, only the current point is.
///
/// Precondition: caller has confirmed `tick.asset == position.asset`.
pub fn evaluate_position(
    position: &PositionRef,
    prev_mid: Option<f64>,
    tick: &OracleTick,
) -> TouchOutcome {
    debug_assert_eq!(position.asset, tick.asset, "evaluate_position called across assets");

    if tick.ts_ms < position.t_open_ms {
        return TouchOutcome::Hold;
    }

    if tick.ts_ms > position.t_close_ms {
        return TouchOutcome::Expire;
    }

    // Shared with the proof verifier (`tap-trading-touch`) so a settlement and
    // its published proof agree by construction — see that crate's docs.
    let touched = tap_trading_touch::path_touches_band(
        prev_mid,
        tick.mid,
        position.strike_lo,
        position.strike_hi,
    );

    if touched {
        TouchOutcome::Win
    } else {
        TouchOutcome::Hold
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tap_trading_pricing_engine::AssetSymbol;

    fn pos(strike_lo: f64, strike_hi: f64, t_open_ms: i64, t_close_ms: i64) -> PositionRef {
        PositionRef {
            id: 1,
            account_id: 1,
            asset: AssetSymbol::Btc,
            strike_lo,
            strike_hi,
            t_open_ms,
            t_close_ms,
            stake_points: 100,
            multiplier_at_tap: 2.5,
            oracle_seq_at_tap: 0,
            oracle_run_id_at_tap: 0,
            created_at_ms: 0,
        }
    }

    fn tick(mid: f64, ts_ms: i64) -> OracleTick {
        OracleTick {
            asset: AssetSymbol::Btc,
            run_id: 1,
            seq: 1,
            ts_ms,
            mid,
            vol_annualized: 0.80,
            source_count: 3,
        }
    }

    #[test]
    fn mid_inside_zone_during_window_is_win() {
        assert_eq!(evaluate_position(&pos(70_000.0, 70_010.0, 1_000, 6_000), None, &tick(70_005.0, 3_000)), TouchOutcome::Win);
    }

    #[test]
    fn mid_at_lower_edge_is_win() {
        // Half-open [lo, hi): the low edge belongs to this cell.
        assert_eq!(evaluate_position(&pos(70_000.0, 70_010.0, 1_000, 6_000), None, &tick(70_000.0, 3_000)), TouchOutcome::Win);
    }

    #[test]
    fn mid_at_upper_edge_holds() {
        // Half-open [lo, hi): the high edge belongs to the cell above, not this one.
        assert_eq!(evaluate_position(&pos(70_000.0, 70_010.0, 1_000, 6_000), None, &tick(70_010.0, 3_000)), TouchOutcome::Hold);
    }

    #[test]
    fn mid_below_zone_holds() {
        assert_eq!(evaluate_position(&pos(70_000.0, 70_010.0, 1_000, 6_000), None, &tick(69_999.99, 3_000)), TouchOutcome::Hold);
    }

    #[test]
    fn mid_above_zone_holds() {
        assert_eq!(evaluate_position(&pos(70_000.0, 70_010.0, 1_000, 6_000), None, &tick(70_010.01, 3_000)), TouchOutcome::Hold);
    }

    #[test]
    fn tick_before_window_holds() {
        assert_eq!(evaluate_position(&pos(70_000.0, 70_010.0, 1_000, 6_000), None, &tick(70_005.0, 500)), TouchOutcome::Hold);
    }

    #[test]
    fn tick_after_window_without_touch_expires() {
        assert_eq!(evaluate_position(&pos(70_000.0, 70_010.0, 1_000, 6_000), None, &tick(80_000.0, 7_000)), TouchOutcome::Expire);
    }

    #[test]
    fn tick_at_close_is_in_window() {
        let p = pos(70_000.0, 70_010.0, 1_000, 6_000);
        assert_eq!(evaluate_position(&p, None, &tick(70_005.0, 6_000)), TouchOutcome::Win);
        assert_eq!(evaluate_position(&p, None, &tick(80_000.0, 6_000)), TouchOutcome::Hold);
    }

    #[test]
    fn tick_at_open_inside_zone_is_win() {
        assert_eq!(evaluate_position(&pos(70_000.0, 70_010.0, 1_000, 6_000), None, &tick(70_005.0, 1_000)), TouchOutcome::Win);
    }

    // --- Path-crossing settlement: the segment from prev_mid to tick.mid ---

    #[test]
    fn segment_leaping_over_band_between_ticks_is_win() {
        // The defect this fixes: prev tick below the band, this tick above it —
        // the price jumped clean over a narrow band between two ticks. Point
        // sampling would miss it (neither endpoint is inside); the path caught it.
        let p = pos(70_000.0, 70_010.0, 1_000, 6_000);
        assert_eq!(evaluate_position(&p, Some(69_995.0), &tick(70_020.0, 3_000)), TouchOutcome::Win);
    }

    #[test]
    fn segment_staying_below_band_holds() {
        let p = pos(70_000.0, 70_010.0, 1_000, 6_000);
        assert_eq!(evaluate_position(&p, Some(69_990.0), &tick(69_998.0, 3_000)), TouchOutcome::Hold);
    }

    #[test]
    fn segment_touching_lower_edge_is_win() {
        // Path reaches exactly lo: lo ∈ [lo, hi) so it has touched this cell.
        let p = pos(70_000.0, 70_010.0, 1_000, 6_000);
        assert_eq!(evaluate_position(&p, Some(69_990.0), &tick(70_000.0, 3_000)), TouchOutcome::Win);
    }

    #[test]
    fn segment_only_reaching_upper_edge_from_above_holds() {
        // Both endpoints ≥ hi and the segment never dips below hi → no overlap
        // with [lo, hi). (hi belongs to the cell above.)
        let p = pos(70_000.0, 70_010.0, 1_000, 6_000);
        assert_eq!(evaluate_position(&p, Some(70_020.0), &tick(70_010.0, 3_000)), TouchOutcome::Hold);
    }

    #[test]
    fn segment_crossing_band_after_close_expires() {
        // Past the window: expiry wins over any path crossing.
        let p = pos(70_000.0, 70_010.0, 1_000, 6_000);
        assert_eq!(evaluate_position(&p, Some(69_990.0), &tick(70_020.0, 7_000)), TouchOutcome::Expire);
    }
}
