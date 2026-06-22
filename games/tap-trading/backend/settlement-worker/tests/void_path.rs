mod common;

use std::sync::Arc;
use std::sync::atomic::AtomicI64;

use tap_trading_oracle_types::{OracleMessage, OracleStatus, OracleStreamState, OracleTick};
use tap_trading_pricing_engine::AssetSymbol;
use tap_trading_settlement_worker::cache::OpenPositionCache;
use tap_trading_settlement_worker::loop_runner::{GapTracker, LoopContext, handle_message};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_window_gap_voids_and_refunds() {
    let db = common::setup_test_postgres().await;
    let acct = common::insert_account(&db.pool, "voided", 500).await;

    let now_ms = chrono::Utc::now().timestamp_millis();
    let t_open = now_ms - 60_000;
    let t_close = now_ms - 55_000;
    let pid = common::insert_open_position(&db.pool, acct, "BTC", 70_000.0, 70_010.0, t_open, t_close, 100, 2.5).await;

    let cache = OpenPositionCache::new();
    cache.hydrate(&db.pool).await.expect("hydrate");
    cache.record_last_mid(AssetSymbol::Btc, 70_005.0).await;

    let ctx = LoopContext {
        pool: db.pool.clone(),
        cache: cache.clone(),
        last_tick_received_ms: Arc::new(AtomicI64::new(0)),
        gap_tracker: Arc::new(GapTracker::default()),
        metrics: Arc::new(tap_trading_settlement_worker::health::Metrics::default()),
    };

    // Seed a tick BEFORE t_open so it doesn't trigger a touch.
    handle_message(&ctx, OracleMessage::Tick(OracleTick {
        asset: AssetSymbol::Btc, run_id: 1, seq: 1,
        ts_ms: t_open - 10_000,
        mid: 70_005.0, vol_annualized: 0.80, source_count: 3,
    })).await;

    // Manually insert gap_start before t_open.
    ctx.gap_tracker.enter_degraded(AssetSymbol::Btc, t_open - 1_000);

    // Exit Degraded — wall-clock now > t_close, so the window is fully contained.
    handle_message(&ctx, OracleMessage::Status(OracleStatus {
        asset: AssetSymbol::Btc, state: OracleStreamState::Normal,
        reason: "sources recovered".into(), run_id: 1,
    })).await;

    let status: (String,) = sqlx::query_as("SELECT status FROM positions WHERE id = $1")
        .bind(pid).fetch_one(&db.pool).await.expect("status");
    assert_eq!(status.0, "VOIDED");

    assert_eq!(common::get_balance(&db.pool, acct).await, 500 + 100);

    let refund: (String, i64) = sqlx::query_as(
        "SELECT kind, delta FROM points_ledger WHERE ref_id = $1 AND kind = 'TAP_REFUND'",
    ).bind(pid).fetch_one(&db.pool).await.expect("refund row");
    assert_eq!(refund, ("TAP_REFUND".to_string(), 100));

    let outcome: (String,) = sqlx::query_as("SELECT outcome FROM settlements WHERE position_id = $1")
        .bind(pid).fetch_one(&db.pool).await.expect("settlement");
    assert_eq!(outcome.0, "V");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn partial_window_gap_does_not_void() {
    let db = common::setup_test_postgres().await;
    let acct = common::insert_account(&db.pool, "partial", 500).await;

    let now_ms = chrono::Utc::now().timestamp_millis();
    let t_open = now_ms - 5_000;
    let t_close = now_ms + 60_000; // far in the future — gap won't fully cover
    let pid = common::insert_open_position(&db.pool, acct, "BTC", 70_000.0, 70_010.0, t_open, t_close, 100, 2.5).await;

    let cache = OpenPositionCache::new();
    cache.hydrate(&db.pool).await.expect("hydrate");
    cache.record_last_mid(AssetSymbol::Btc, 70_005.0).await;

    let ctx = LoopContext {
        pool: db.pool.clone(),
        cache: cache.clone(),
        last_tick_received_ms: Arc::new(AtomicI64::new(0)),
        gap_tracker: Arc::new(GapTracker::default()),
        metrics: Arc::new(tap_trading_settlement_worker::health::Metrics::default()),
    };

    ctx.gap_tracker.enter_degraded(AssetSymbol::Btc, t_open - 1_000);
    handle_message(&ctx, OracleMessage::Status(OracleStatus {
        asset: AssetSymbol::Btc, state: OracleStreamState::Normal,
        reason: "sources recovered".into(), run_id: 1,
    })).await;

    let status: (String,) = sqlx::query_as("SELECT status FROM positions WHERE id = $1")
        .bind(pid).fetch_one(&db.pool).await.expect("status");
    assert_eq!(status.0, "OPEN");
    assert_eq!(common::get_balance(&db.pool, acct).await, 500);
}
