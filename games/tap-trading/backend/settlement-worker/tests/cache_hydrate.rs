mod common;

use tap_trading_pricing_engine::AssetSymbol;
use tap_trading_settlement_worker::cache::OpenPositionCache;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hydrate_loads_only_open_positions() {
    let db = common::setup_test_postgres().await;
    let acct = common::insert_account(&db.pool, "user-1", 10_000).await;

    let p1 = common::insert_open_position(&db.pool, acct, "BTC", 70_000.0, 70_010.0, 0, 5_000, 100, 2.5).await;
    let p2 = common::insert_open_position(&db.pool, acct, "BTC", 70_020.0, 70_030.0, 0, 5_000, 100, 2.5).await;
    let p3 = common::insert_open_position(&db.pool, acct, "ETH", 3_800.0, 3_801.0, 0, 5_000, 100, 2.5).await;
    let p_won = common::insert_open_position(&db.pool, acct, "BTC", 71_000.0, 71_001.0, 0, 5_000, 100, 2.5).await;
    sqlx::query("UPDATE positions SET status = 'WON' WHERE id = $1")
        .bind(p_won)
        .execute(&db.pool)
        .await
        .expect("flip to WON");

    let cache = OpenPositionCache::new();
    let count = cache.hydrate(&db.pool).await.expect("hydrate");

    assert_eq!(count, 3, "cache should have 3 OPEN positions");

    let btc = cache.settleable_for_asset(AssetSymbol::Btc, 1_000).await;
    assert_eq!(btc.len(), 2);
    let btc_ids: Vec<i64> = btc.iter().map(|p| p.id).collect();
    assert!(btc_ids.contains(&p1) && btc_ids.contains(&p2));
    assert!(!btc_ids.contains(&p_won), "WON position must not be in cache");

    let eth = cache.settleable_for_asset(AssetSymbol::Eth, 1_000).await;
    assert_eq!(eth.len(), 1);
    assert_eq!(eth[0].id, p3);
}

/// `settleable_for_asset` MUST include post-close positions so the live loop
/// can reach the `Expire`→`settle_loss` arm (SYSTEM_DESIGN §5.2). The probe at
/// t_open (1_000) and one past t_close (7_000) pins the closed-lower-bound /
/// no-upper-bound contract.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settleable_for_asset_includes_post_close() {
    let db = common::setup_test_postgres().await;
    let acct = common::insert_account(&db.pool, "user-2", 10_000).await;
    common::insert_open_position(&db.pool, acct, "BTC", 70_000.0, 70_010.0, 1_000, 6_000, 100, 2.5).await;

    let cache = OpenPositionCache::new();
    cache.hydrate(&db.pool).await.expect("hydrate");

    // Before t_open: not yet settleable.
    assert_eq!(cache.settleable_for_asset(AssetSymbol::Btc, 500).await.len(), 0);
    // Exactly at t_open: settleable (closed lower bound).
    assert_eq!(cache.settleable_for_asset(AssetSymbol::Btc, 1_000).await.len(), 1);
    // Inside the window.
    assert_eq!(cache.settleable_for_asset(AssetSymbol::Btc, 3_000).await.len(), 1);
    // Past t_close: STILL settleable — evaluate_position will return Expire→Loss.
    assert_eq!(cache.settleable_for_asset(AssetSymbol::Btc, 7_000).await.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn last_known_mid_persists_across_hydrate() {
    let db = common::setup_test_postgres().await;

    let cache = OpenPositionCache::new();
    cache.record_last_mid(AssetSymbol::Btc, 70_000.0).await;
    cache.hydrate(&db.pool).await.expect("hydrate");

    assert_eq!(cache.last_mid(AssetSymbol::Btc).await, Some(70_000.0));
}
