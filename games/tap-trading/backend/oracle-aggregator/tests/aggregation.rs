//! Aggregation core tests. ORACLE_SPEC §4.4 and TESTING_STRATEGY §4.1.

use std::collections::BTreeMap;
use tap_trading_oracle_types::{AssetSymbol, OracleStreamState};

// Re-export module paths from the bin crate. Cargo allows integration tests
// to reach into the bin's library — but a bin crate has no library. We add
// `#[path = ...]` includes here to pull the source files directly.
//
// (Avoiding the alternative of splitting the bin into a lib + bin is
// deliberate: this crate's only public surface is the binary; tests are
// the only callers of these modules.)
#[path = "../src/aggregator.rs"]
mod aggregator;
#[path = "../src/constants.rs"]
mod constants;
#[path = "../src/sources/mod.rs"]
mod sources;

use aggregator::{ema_step, median_of, AggregateOutcome, AssetPriceState};
use sources::{SourceId, SourceTick};

fn tick(src: SourceId, asset: AssetSymbol, price: f64, ts_ms: i64) -> SourceTick {
    SourceTick {
        source: src,
        asset,
        price,
        ts_ms,
        pyth_conf_bps: None,
    }
}

fn pyth(asset: AssetSymbol, price: f64, ts_ms: i64, conf_bps: u32) -> SourceTick {
    SourceTick {
        source: SourceId::Pyth,
        asset,
        price,
        ts_ms,
        pyth_conf_bps: Some(conf_bps),
    }
}

#[test]
fn median_of_odd_length_picks_middle() {
    assert_eq!(median_of(&[3.0, 1.0, 2.0]), 2.0);
}

#[test]
fn median_of_even_length_averages_middle_two() {
    assert_eq!(median_of(&[1.0, 2.0, 3.0, 4.0]), 2.5);
}

#[test]
fn median_robust_to_single_outlier() {
    // ORACLE_SPEC §4.5 rationale: one bad Pyth update should not shift the mid.
    let m = median_of(&[100.0, 100.1, 100.0, 50_000.0]);
    assert!((m - 100.05).abs() < 1e-9, "got {m}");
}

#[test]
fn ema_cold_start_equals_raw() {
    assert_eq!(ema_step(None, 3812.5, 0.6), 3812.5);
}

#[test]
fn ema_blends_with_prior() {
    let s = ema_step(Some(100.0), 200.0, 0.6);
    assert!((s - 160.0).abs() < 1e-9, "got {s}");
}

#[test]
fn four_active_sources_produce_emit() {
    let now = 1_000_000;
    let mut state = AssetPriceState::default();
    let mut latest = BTreeMap::new();
    latest.insert(SourceId::Pyth, pyth(AssetSymbol::Eth, 3812.0, now, 50));
    latest.insert(
        SourceId::Binance,
        tick(SourceId::Binance, AssetSymbol::Eth, 3812.10, now),
    );
    latest.insert(
        SourceId::Bybit,
        tick(SourceId::Bybit, AssetSymbol::Eth, 3812.20, now),
    );
    latest.insert(
        SourceId::Okx,
        tick(SourceId::Okx, AssetSymbol::Eth, 3812.30, now),
    );
    match state.apply_sources(now, &latest) {
        AggregateOutcome::Emit { source_count, .. } => assert_eq!(source_count, 4),
        other => panic!("expected Emit, got {other:?}"),
    }
}

#[test]
fn stale_source_dropped_above_5000ms() {
    let now = 1_000_000;
    let mut state = AssetPriceState::default();
    let mut latest = BTreeMap::new();
    // Pyth is 6000ms old (beyond the 5s freshness window) — must be dropped.
    latest.insert(
        SourceId::Pyth,
        pyth(AssetSymbol::Eth, 3812.0, now - 6_000, 50),
    );
    latest.insert(
        SourceId::Binance,
        tick(SourceId::Binance, AssetSymbol::Eth, 3812.1, now),
    );
    latest.insert(
        SourceId::Bybit,
        tick(SourceId::Bybit, AssetSymbol::Eth, 3812.2, now),
    );
    latest.insert(
        SourceId::Okx,
        tick(SourceId::Okx, AssetSymbol::Eth, 3812.3, now),
    );
    match state.apply_sources(now, &latest) {
        AggregateOutcome::Emit { source_count, .. } => assert_eq!(source_count, 3),
        other => panic!("expected Emit, got {other:?}"),
    }
}

#[test]
fn pyth_dropped_when_confidence_above_100bps() {
    let now = 1_000_000;
    let mut state = AssetPriceState::default();
    let mut latest = BTreeMap::new();
    latest.insert(SourceId::Pyth, pyth(AssetSymbol::Eth, 3812.0, now, 145));
    latest.insert(
        SourceId::Binance,
        tick(SourceId::Binance, AssetSymbol::Eth, 3812.1, now),
    );
    latest.insert(
        SourceId::Bybit,
        tick(SourceId::Bybit, AssetSymbol::Eth, 3812.2, now),
    );
    latest.insert(
        SourceId::Okx,
        tick(SourceId::Okx, AssetSymbol::Eth, 3812.3, now),
    );
    match state.apply_sources(now, &latest) {
        AggregateOutcome::Emit { source_count, .. } => assert_eq!(source_count, 3),
        other => panic!("expected Emit, got {other:?}"),
    }
}

#[test]
fn insufficient_sources_yields_status_signal() {
    let now = 1_000_000;
    let mut state = AssetPriceState::default();
    let mut latest = BTreeMap::new();
    latest.insert(
        SourceId::Pyth,
        pyth(AssetSymbol::Eth, 3812.0, now - 6_000, 50),
    );
    latest.insert(
        SourceId::Binance,
        tick(SourceId::Binance, AssetSymbol::Eth, 3812.1, now),
    );
    latest.insert(
        SourceId::Bybit,
        tick(SourceId::Bybit, AssetSymbol::Eth, 3812.2, now - 6_000),
    );
    latest.insert(
        SourceId::Okx,
        tick(SourceId::Okx, AssetSymbol::Eth, 3812.3, now - 6_000),
    );
    match state.apply_sources(now, &latest) {
        AggregateOutcome::InsufficientSources { reason } => {
            assert!(
                reason.contains("stale"),
                "reason should mention staleness: {reason}"
            );
        }
        other => panic!("expected InsufficientSources, got {other:?}"),
    }
}

#[test]
fn divergent_source_rejected_keeps_clustered_median() {
    // ORACLE_SPEC §7: a lone source > 5% from the median is dropped; the
    // surviving cluster sets the price, not the mean that includes the outlier.
    let now = 1_000_000;
    let mut state = AssetPriceState::default();
    let mut latest = BTreeMap::new();
    latest.insert(
        SourceId::Binance,
        tick(SourceId::Binance, AssetSymbol::Eth, 100.0, now),
    );
    latest.insert(
        SourceId::Bybit,
        tick(SourceId::Bybit, AssetSymbol::Eth, 100.1, now),
    );
    latest.insert(
        SourceId::Okx,
        tick(SourceId::Okx, AssetSymbol::Eth, 130.0, now),
    ); // +30%
    match state.apply_sources(now, &latest) {
        AggregateOutcome::Emit {
            median,
            source_count,
            ..
        } => {
            assert_eq!(source_count, 2, "the +30% outlier must be dropped");
            assert!(
                (median - 100.05).abs() < 1e-9,
                "median must be the clustered pair, not pulled toward 130; got {median}"
            );
        }
        other => panic!("expected Emit, got {other:?}"),
    }
}

#[test]
fn two_sources_disagreeing_hard_pause_rather_than_average() {
    // Two sources 20% apart: the spread exceeds the 5% threshold, so we pause
    // instead of emitting a mean neither venue supports.
    let now = 1_000_000;
    let mut state = AssetPriceState::default();
    let mut latest = BTreeMap::new();
    latest.insert(
        SourceId::Binance,
        tick(SourceId::Binance, AssetSymbol::Eth, 100.0, now),
    );
    latest.insert(
        SourceId::Bybit,
        tick(SourceId::Bybit, AssetSymbol::Eth, 120.0, now),
    );
    match state.apply_sources(now, &latest) {
        AggregateOutcome::InsufficientSources { .. } => {}
        other => panic!("expected InsufficientSources, got {other:?}"),
    }
}

#[test]
fn two_sources_moderate_spread_pauses() {
    // Regression: 100 vs 109 is an ~8.6% spread, but each source sits only
    // ~4.3% from their mean — below the 5% per-source threshold. Measuring the
    // spread directly (not distance-from-mean) is what catches this; the old
    // code emitted the mean of two ~8.6%-apart venues.
    let now = 1_000_000;
    let mut state = AssetPriceState::default();
    let mut latest = BTreeMap::new();
    latest.insert(
        SourceId::Binance,
        tick(SourceId::Binance, AssetSymbol::Eth, 100.0, now),
    );
    latest.insert(
        SourceId::Bybit,
        tick(SourceId::Bybit, AssetSymbol::Eth, 109.0, now),
    );
    match state.apply_sources(now, &latest) {
        AggregateOutcome::InsufficientSources { .. } => {}
        other => panic!("expected InsufficientSources (spread > 5%), got {other:?}"),
    }
}

#[test]
fn two_sources_within_threshold_emit() {
    // 100 vs 102 is a ~2% spread (< 5%): emit the pair's mean.
    let now = 1_000_000;
    let mut state = AssetPriceState::default();
    let mut latest = BTreeMap::new();
    latest.insert(
        SourceId::Binance,
        tick(SourceId::Binance, AssetSymbol::Eth, 100.0, now),
    );
    latest.insert(
        SourceId::Bybit,
        tick(SourceId::Bybit, AssetSymbol::Eth, 102.0, now),
    );
    match state.apply_sources(now, &latest) {
        AggregateOutcome::Emit {
            source_count,
            median,
            ..
        } => {
            assert_eq!(source_count, 2);
            assert!((median - 101.0).abs() < 1e-9, "got {median}");
        }
        other => panic!("expected Emit, got {other:?}"),
    }
}

#[test]
fn ema_resets_after_insufficient_gap() {
    // A gap (insufficient sources) must re-baseline the EMA so the next emit
    // starts from the fresh median, not a stale pre-gap price.
    let now = 1_000_000;
    let mut state = AssetPriceState::default();
    let four = |p: f64, t: i64| {
        let mut m = BTreeMap::new();
        m.insert(SourceId::Pyth, pyth(AssetSymbol::Eth, p, t, 50));
        m.insert(
            SourceId::Binance,
            tick(SourceId::Binance, AssetSymbol::Eth, p, t),
        );
        m.insert(SourceId::Bybit, tick(SourceId::Bybit, AssetSymbol::Eth, p, t));
        m.insert(SourceId::Okx, tick(SourceId::Okx, AssetSymbol::Eth, p, t));
        m
    };
    // 1) Establish the EMA carrier at 100.
    state.apply_sources(now, &four(100.0, now));
    // 2) Gap: a single fresh source → InsufficientSources, which clears the carrier.
    let mut one = BTreeMap::new();
    one.insert(
        SourceId::Binance,
        tick(SourceId::Binance, AssetSymbol::Eth, 100.0, now + 50),
    );
    match state.apply_sources(now + 50, &one) {
        AggregateOutcome::InsufficientSources { .. } => {}
        other => panic!("expected InsufficientSources, got {other:?}"),
    }
    // 3) Recovery at 200: mid must be the raw median (200), NOT the stale blend
    //    0.6·200 + 0.4·100 = 160.
    let mid = match state.apply_sources(now + 100, &four(200.0, now + 100)) {
        AggregateOutcome::Emit { mid, .. } => mid,
        other => panic!("expected Emit, got {other:?}"),
    };
    assert!(
        (mid - 200.0).abs() < 1e-9,
        "EMA must re-seed from the fresh median after a gap; got {mid}"
    );
}

#[test]
fn ema_carries_over_across_calls() {
    // Two consecutive apply_sources: second result should be blended with first.
    let now = 1_000_000;
    let mut state = AssetPriceState::default();
    let four = |p: f64, t: i64| {
        let mut m = BTreeMap::new();
        m.insert(SourceId::Pyth, pyth(AssetSymbol::Eth, p, t, 50));
        m.insert(
            SourceId::Binance,
            tick(SourceId::Binance, AssetSymbol::Eth, p, t),
        );
        m.insert(
            SourceId::Bybit,
            tick(SourceId::Bybit, AssetSymbol::Eth, p, t),
        );
        m.insert(SourceId::Okx, tick(SourceId::Okx, AssetSymbol::Eth, p, t));
        m
    };
    let r1 = state.apply_sources(now, &four(100.0, now));
    let r2 = state.apply_sources(now + 50, &four(200.0, now + 50));
    let mid2 = match r2 {
        AggregateOutcome::Emit { mid, .. } => mid,
        _ => panic!("expected Emit"),
    };
    // r1.mid = 100; raw2 = 200; blended = 0.6·200 + 0.4·100 = 160.
    assert!((mid2 - 160.0).abs() < 1e-6, "got {mid2}");
    let _ = r1;
}

use aggregator::{AssetStreamPhase, TickDecision};

fn insufficient() -> AggregateOutcome {
    AggregateOutcome::InsufficientSources {
        reason: "test".into(),
    }
}

fn emit() -> AggregateOutcome {
    AggregateOutcome::Emit {
        mid: 100.0,
        median: 100.0,
        source_count: 4,
    }
}

#[test]
fn normal_to_degraded_requires_full_hysteresis_window() {
    let mut p = AssetStreamPhase::default();
    // First insufficient — pending transition starts; still emit Silence.
    assert_eq!(p.step(0, &insufficient()), TickDecision::Silence);
    // 1999 ms later — still pending.
    assert_eq!(p.step(1_999, &insufficient()), TickDecision::Silence);
    // 2000 ms — phase flips to Degraded; emit Status.
    match p.step(2_000, &insufficient()) {
        TickDecision::Status { state, .. } => assert_eq!(state, OracleStreamState::Degraded),
        other => panic!("expected Status(Degraded), got {other:?}"),
    }
}

#[test]
fn normal_emit_after_brief_insufficient_does_not_flip() {
    let mut p = AssetStreamPhase::default();
    p.step(0, &insufficient());
    // Recovery within window → pending cleared, still Normal.
    match p.step(500, &emit()) {
        TickDecision::Tick { .. } => {}
        other => panic!("expected Tick, got {other:?}"),
    }
    // Subsequent insufficient must restart the timer.
    p.step(600, &insufficient());
    assert_eq!(p.step(2_000, &insufficient()), TickDecision::Silence);
    match p.step(2_600, &insufficient()) {
        TickDecision::Status { state, .. } => assert_eq!(state, OracleStreamState::Degraded),
        other => panic!("expected Status(Degraded), got {other:?}"),
    }
}

#[test]
fn degraded_to_normal_requires_full_hysteresis_window() {
    let mut p = AssetStreamPhase::default();
    // Force into Degraded.
    p.step(0, &insufficient());
    p.step(2_000, &insufficient());
    // Recovery starts; less than 2000 ms → still Silence.
    assert_eq!(p.step(2_100, &emit()), TickDecision::Silence);
    assert_eq!(p.step(3_999, &emit()), TickDecision::Silence);
    // At 2000 ms of sustained Emit, flip back.
    match p.step(4_100, &emit()) {
        TickDecision::Status { state, .. } => assert_eq!(state, OracleStreamState::Normal),
        other => panic!("expected Status(Normal), got {other:?}"),
    }
}

#[test]
fn degraded_insufficient_resets_recovery_timer() {
    let mut p = AssetStreamPhase::default();
    p.step(0, &insufficient());
    p.step(2_000, &insufficient());
    p.step(2_100, &emit()); // recovery pending at 2100
    p.step(2_500, &insufficient()); // resets: pending = None
                                    // Recovery restarts at 3000 when we send emit again.
    assert_eq!(p.step(3_000, &emit()), TickDecision::Silence); // pending=3000
    assert_eq!(p.step(4_999, &emit()), TickDecision::Silence); // 4999-3000=1999 < 2000
    match p.step(5_001, &emit()) {
        TickDecision::Status { state, .. } => assert_eq!(state, OracleStreamState::Normal),
        other => panic!("expected Status(Normal), got {other:?}"),
    }
}
