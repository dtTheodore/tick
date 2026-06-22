mod common;

use std::sync::Arc;
use std::sync::atomic::AtomicI64;

use tap_trading_pricing_engine::AssetSymbol;
use tap_trading_settlement_worker::cache::OpenPositionCache;
use tap_trading_settlement_worker::health::Metrics;
use tap_trading_settlement_worker::loop_runner::{GapTracker, LoopContext, sweep_once};

fn ctx_for(db_pool: sqlx::PgPool, cache: OpenPositionCache) -> (LoopContext, Arc<GapTracker>) {
    let gap_tracker = Arc::new(GapTracker::default());
    let ctx = LoopContext {
        pool: db_pool,
        cache,
        last_tick_received_ms: Arc::new(AtomicI64::new(0)),
        gap_tracker: gap_tracker.clone(),
        metrics: Arc::new(Metrics::default()),
    };
    (ctx, gap_tracker)
}

/// SYSTEM_DESIGN §5.2 stragglers: a position whose `t_close_ms` is already past
/// must be settled as LOST by the sweep. Asserts the actual `sweep_once`
/// function rather than reimplementing its body inline.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sweep_once_settles_expired_stragglers_as_loss() {
    let db = common::setup_test_postgres().await;
    let acct = common::insert_account(&db.pool, "missed", 500).await;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let pid = common::insert_open_position(
        &db.pool, acct, "BTC", 70_000.0, 70_010.0,
        now_ms - 70_000, now_ms - 65_000, 100, 2.5,
    ).await;

    let cache = OpenPositionCache::new();
    cache.record_last_mid(AssetSymbol::Btc, 70_005.0).await;
    let (ctx, _gap) = ctx_for(db.pool.clone(), cache);

    sweep_once(&ctx).await;

    let outcome: (String,) = sqlx::query_as("SELECT outcome FROM settlements WHERE position_id = $1")
        .bind(pid).fetch_one(&db.pool).await.expect("settlement");
    assert_eq!(outcome.0, "L", "stale untouched straggler must settle LOST");
    assert_eq!(common::get_balance(&db.pool, acct).await, 500, "loss does not credit");
    assert_eq!(ctx.metrics.positions_settled_loss.load(std::sync::atomic::Ordering::Relaxed), 1);
}

/// The sweep MUST NOT settle loss for a position whose asset is currently
/// degraded — the void-on-recovery path is the only legitimate outcome for
/// fully-gap-covered windows (§9.1). Letting the sweep race it would record
/// 'L' and turn a refund into a stake loss.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sweep_once_skips_degraded_asset() {
    let db = common::setup_test_postgres().await;
    let acct = common::insert_account(&db.pool, "gapped", 500).await;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let pid = common::insert_open_position(
        &db.pool, acct, "BTC", 70_000.0, 70_010.0,
        now_ms - 70_000, now_ms - 65_000, 100, 2.5,
    ).await;

    let cache = OpenPositionCache::new();
    cache.record_last_mid(AssetSymbol::Btc, 70_005.0).await;
    let (ctx, gap) = ctx_for(db.pool.clone(), cache);

    gap.enter_degraded(AssetSymbol::Btc, now_ms - 80_000);
    sweep_once(&ctx).await;

    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM settlements WHERE position_id = $1")
        .bind(pid).fetch_one(&db.pool).await.expect("count");
    assert_eq!(count.0, 0, "degraded asset must not be settled by the sweep");
    let status: (String,) = sqlx::query_as("SELECT status FROM positions WHERE id = $1")
        .bind(pid).fetch_one(&db.pool).await.expect("status");
    assert_eq!(status.0, "OPEN", "position stays OPEN until recovery decides");
}
