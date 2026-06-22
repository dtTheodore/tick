//! Pure path-segment touch detection — the single definition of "did the price
//! enter the band" shared by the settlement worker (which settles) and the proof
//! verifier (which replays). Keeping it in one crate is what guarantees a
//! settlement and its published proof agree: a win the worker awards on a
//! between-ticks segment leap replays as the same win, not an `OutcomeMismatch`.
//!
//! The band is half-open `[lo, hi)`: `lo` belongs to this cell, `hi` to the cell
//! above, so adjacent cells don't both claim a mid sitting exactly on the shared
//! boundary. Settlement is on the continuous price PATH, not on discrete tick
//! samples — a fast wick can cross a narrow band entirely between two 20 Hz
//! ticks, so we test the straight segment from the previous mid to the current
//! mid (`MATH_SPEC §1` first-touch; see settlement-worker `touch.rs` for why the
//! discrete-sample model under-counts).

/// Does the closed segment `[min(prev,cur), max(prev,cur)]` intersect the
/// half-open band `[lo, hi)`? `seg_hi >= lo` admits a path reaching `lo`;
/// `seg_lo < hi` excludes a segment sitting entirely at/above `hi` (which
/// belongs to the cell above) while still catching a segment passing *through*
/// `hi`, since such a segment necessarily covers values in `[lo, hi)`.
pub fn segment_intersects_band(prev: f64, cur: f64, lo: f64, hi: f64) -> bool {
    let seg_lo = prev.min(cur);
    let seg_hi = prev.max(cur);
    seg_hi >= lo && seg_lo < hi
}

/// Did the price touch `[lo, hi)` arriving at `cur_mid`?
///
/// When `prev_mid` is present and finite, the segment `[prev_mid, cur_mid]` is
/// tested (continuous path). When absent — the first tick after the worker
/// started or after a gap, or the first evidence tick — we fall back to the
/// point sample `cur_mid ∈ [lo, hi)`.
pub fn path_touches_band(prev_mid: Option<f64>, cur_mid: f64, lo: f64, hi: f64) -> bool {
    match prev_mid {
        Some(prev) if prev.is_finite() => segment_intersects_band(prev, cur_mid, lo, hi),
        _ => cur_mid >= lo && cur_mid < hi,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_inside_band_touches() {
        assert!(path_touches_band(None, 70_005.0, 70_000.0, 70_010.0));
    }

    #[test]
    fn lower_edge_is_inside_upper_edge_is_not() {
        assert!(path_touches_band(None, 70_000.0, 70_000.0, 70_010.0));
        assert!(!path_touches_band(None, 70_010.0, 70_000.0, 70_010.0));
    }

    #[test]
    fn segment_leaping_over_band_touches() {
        // Neither endpoint inside, but the path swept through the band.
        assert!(path_touches_band(Some(69_995.0), 70_020.0, 70_000.0, 70_010.0));
    }

    #[test]
    fn segment_staying_below_band_does_not_touch() {
        assert!(!path_touches_band(Some(69_990.0), 69_998.0, 70_000.0, 70_010.0));
    }

    #[test]
    fn segment_touching_lower_edge_touches() {
        assert!(path_touches_band(Some(69_990.0), 70_000.0, 70_000.0, 70_010.0));
    }

    #[test]
    fn segment_only_at_upper_edge_from_above_does_not_touch() {
        assert!(!path_touches_band(Some(70_020.0), 70_010.0, 70_000.0, 70_010.0));
    }

    #[test]
    fn non_finite_prev_falls_back_to_point() {
        assert!(path_touches_band(Some(f64::NAN), 70_005.0, 70_000.0, 70_010.0));
        assert!(!path_touches_band(Some(f64::NAN), 70_020.0, 70_000.0, 70_010.0));
    }
}
