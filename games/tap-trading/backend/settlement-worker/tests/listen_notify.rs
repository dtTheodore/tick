mod common;

use std::time::Duration;

use tap_trading_pricing_engine::AssetSymbol;
use tap_trading_settlement_worker::cache::OpenPositionCache;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notify_inserts_into_cache() {
    let db = common::setup_test_postgres().await;
    let acct = common::insert_account(&db.pool, "user-N", 10_000).await;

    let cache = OpenPositionCache::new();
    let cache_clone = cache.clone();
    let url = db.url.clone();
    let pool = db.pool.clone();
    let listener_task = tokio::spawn(async move {
        let _ = cache_clone.listen_loop(&pool, &url).await;
    });

    tokio::time::sleep(Duration::from_millis(500)).await;

    let pid = common::insert_open_position(&db.pool, acct, "BTC", 70_000.0, 70_010.0, 0, 5_000, 100, 2.5).await;
    sqlx::query("SELECT pg_notify('tap_new_position', $1::text)")
        .bind(pid.to_string())
        .execute(&db.pool)
        .await
        .expect("notify");

    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let active = cache.settleable_for_asset(AssetSymbol::Btc, 0).await;
        if !active.is_empty() {
            assert_eq!(active[0].id, pid);
            listener_task.abort();
            return;
        }
    }
    panic!("NOTIFY did not propagate to cache within 2s");
}

/// Verifies that a bare `hydrate()` call loads pre-existing OPEN positions.
/// This is a *baseline* — it does NOT simulate a dropped listener connection.
/// A true reconnect-rehydrate test requires aborting and restarting the
/// listener task between the insert and the assert; tracked as a TODO.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hydrate_loads_pre_existing_open_positions() {
    let db = common::setup_test_postgres().await;
    let acct = common::insert_account(&db.pool, "user-R", 10_000).await;

    let pid = common::insert_open_position(&db.pool, acct, "ETH", 3_800.0, 3_801.0, 0, 5_000, 100, 2.5).await;

    let cache = OpenPositionCache::new();
    let count = cache.hydrate(&db.pool).await.expect("hydrate");
    assert_eq!(count, 1);

    let active = cache.settleable_for_asset(AssetSymbol::Eth, 1_000).await;
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].id, pid);
}
