//! Ring-buffer semantics. ADR-0008 §5.

#[path = "../src/constants.rs"]
mod constants;
#[path = "../src/ring_buffer.rs"]
mod ring_buffer;

use ring_buffer::{RingBuffers, RingLookup};
use tap_trading_oracle_types::{AssetSymbol, OracleTick};

fn make_tick(run_id: u64, seq: u64) -> OracleTick {
    OracleTick {
        asset: AssetSymbol::Eth,
        run_id,
        seq,
        ts_ms: 1_000_000 + (seq as i64) * 50,
        mid: 3812.0 + seq as f64,
        vol_annualized: 0.60,
        source_count: 4,
    }
}

#[test]
fn unknown_asset_returns_gone() {
    let rb = RingBuffers::new();
    // BTC never pushed.
    assert_eq!(rb.get(AssetSymbol::Btc, 1, 0), RingLookup::Gone);
}

#[test]
fn fresh_push_is_a_hit() {
    let rb = RingBuffers::new();
    rb.push(make_tick(1, 0));
    match rb.get(AssetSymbol::Eth, 1, 0) {
        RingLookup::Hit(t) => assert_eq!(t.seq, 0),
        other => panic!("expected Hit, got {other:?}"),
    }
}

#[test]
fn wrong_run_id_returns_conflict() {
    let rb = RingBuffers::new();
    rb.push(make_tick(42, 0));
    assert_eq!(rb.get(AssetSymbol::Eth, 99, 0), RingLookup::Conflict);
}

#[test]
fn rotated_past_seq_returns_gone() {
    let rb = RingBuffers::new();
    // Push one full ring + 5; the first 5 seqs rotate out of retention.
    let cap = constants::RING_SIZE as u64;
    for seq in 0..(cap + 5) {
        rb.push(make_tick(1, seq));
    }
    let oldest_retained = 5; // seqs 0..=4 evicted
    // Seq 4 fell out → Gone.
    assert_eq!(rb.get(AssetSymbol::Eth, 1, 4), RingLookup::Gone);
    // Oldest still-retained seq → Hit.
    match rb.get(AssetSymbol::Eth, 1, oldest_retained) {
        RingLookup::Hit(t) => assert_eq!(t.seq, oldest_retained),
        other => panic!("expected Hit, got {other:?}"),
    }
    // Newest seq → Hit.
    let newest = cap + 4;
    match rb.get(AssetSymbol::Eth, 1, newest) {
        RingLookup::Hit(t) => assert_eq!(t.seq, newest),
        other => panic!("expected Hit, got {other:?}"),
    }
}

#[test]
fn future_seq_returns_gone() {
    let rb = RingBuffers::new();
    rb.push(make_tick(1, 0));
    // Asking for seq 100 when only 0 exists → Gone (caller re-fetches a fresh quote).
    assert_eq!(rb.get(AssetSymbol::Eth, 1, 100), RingLookup::Gone);
}

#[test]
fn per_asset_isolation() {
    let rb = RingBuffers::new();
    rb.push(make_tick(1, 0));
    // BTC is independent.
    assert_eq!(rb.get(AssetSymbol::Btc, 1, 0), RingLookup::Gone);
}

#[test]
fn newest_returns_last_pushed_or_none() {
    // Backs the healthz freshness/source_count check (ADR-0008 §5).
    let rb = RingBuffers::new();
    assert_eq!(
        rb.newest(AssetSymbol::Eth),
        None,
        "empty asset has no newest tick"
    );
    rb.push(make_tick(1, 0));
    rb.push(make_tick(1, 1));
    assert_eq!(rb.newest(AssetSymbol::Eth).map(|t| t.seq), Some(1));
    assert_eq!(
        rb.newest(AssetSymbol::Btc),
        None,
        "untouched asset stays empty"
    );
}
