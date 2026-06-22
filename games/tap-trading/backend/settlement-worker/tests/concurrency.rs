mod common;

use std::sync::Arc;
use std::sync::atomic::AtomicI64;

use tap_trading_oracle_types::OracleTick;
use tap_trading_pricing_engine::AssetSymbol;
use tap_trading_settlement_worker::cache::{OpenPositionCache, PositionRef};
use tap_trading_settlement_worker::health::Metrics;
use tap_trading_settlement_worker::loop_runner::{GapTracker, LoopContext, sweep_once};
use tap_trading_settlement_worker::settle::{settle_loss, settle_win};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_concurrent_settle_win_calls_yield_one_credit() {
    let db = common::setup_test_postgres().await;
    let acct = common::insert_account(&db.pool, "racer", 1_000).await;
    let pid = common::insert_open_position(
        &db.pool, acct, "BTC", 70_000.0, 70_010.0, 1_000, 6_000, 100, 2.5,
    ).await;

    let pos = PositionRef {
        id: pid,
        account_id: acct,
        asset: AssetSymbol::Btc,
        strike_lo: 70_000.0,
        strike_hi: 70_010.0,
        t_open_ms: 1_000,
        t_close_ms: 6_000,
        stake_points: 100,
        multiplier_at_tap: 2.5,
        oracle_seq_at_tap: 0,
        oracle_run_id_at_tap: 0,
        created_at_ms: 0,
    };
    let tick = OracleTick {
        asset: AssetSymbol::Btc,
        run_id: 1,
        seq: 1,
        ts_ms: 3_000,
        mid: 70_010.0,
        vol_annualized: 0.80,
        source_count: 3,
    };

    let pool_a = db.pool.clone();
    let pool_b = db.pool.clone();
    let (a, b) = tokio::join!(
        tokio::spawn({let pos = pos.clone(); async move { settle_win(&pool_a, &pos, &tick).await }}),
        tokio::spawn(async move { settle_win(&pool_b, &pos, &tick).await })
    );
    let a = a.expect("task a panicked").expect("a result");
    let b = b.expect("task b panicked").expect("b result");

    assert!(a ^ b, "exactly one task credits: a={a} b={b}");
    assert_eq!(common::count_settlements_for_position(&db.pool, pid).await, 1);
    assert_eq!(common::get_balance(&db.pool, acct).await, 1_000 + 250);
    let ledger_count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM points_ledger WHERE ref_id = $1 AND kind = 'TAP_PAYOUT'",
    )
    .bind(pid)
    .fetch_one(&db.pool)
    .await
    .expect("ledger count");
    assert_eq!(ledger_count.0, 1);
}

/// The realistic race: a live touching tick fires `settle_win` while the 30s
/// sweep concurrently fires `settle_loss` on the same position. The
/// `settlements.position_id UNIQUE` index guarantees ONE row, but the test
/// must verify the surviving outcome is consistent with the balance —
/// crediting the win means the ledger and balance both agree, NOT a loss row
/// with a credited balance.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_win_racing_sweep_loss_is_consistent() {
    let db = common::setup_test_postgres().await;
    let acct = common::insert_account(&db.pool, "race-cross", 1_000).await;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let pid = common::insert_open_position(
        &db.pool, acct, "BTC", 70_000.0, 70_010.0,
        now_ms - 10_000, now_ms - 5_000, 100, 2.5,
    ).await;

    let pos = PositionRef {
        id: pid, account_id: acct, asset: AssetSymbol::Btc,
        strike_lo: 70_000.0, strike_hi: 70_010.0,
        t_open_ms: now_ms - 10_000, t_close_ms: now_ms - 5_000,
        stake_points: 100, multiplier_at_tap: 2.5,
        oracle_seq_at_tap: 0, oracle_run_id_at_tap: 0, created_at_ms: 0,
    };
    // Touching tick within the window.
    let win_tick = OracleTick {
        asset: AssetSymbol::Btc, run_id: 1, seq: 1, ts_ms: now_ms - 7_000,
        mid: 70_010.0, vol_annualized: 0.80, source_count: 3,
    };
    let loss_tick = OracleTick {
        asset: AssetSymbol::Btc, run_id: 1, seq: 2, ts_ms: now_ms,
        mid: 70_005.0, vol_annualized: 0.80, source_count: 3,
    };

    let pool_w = db.pool.clone();
    let pool_l = db.pool.clone();
    let pos_w = pos.clone();
    let (w, l) = tokio::join!(
        tokio::spawn(async move { settle_win(&pool_w, &pos_w, &win_tick).await }),
        tokio::spawn(async move { settle_loss(&pool_l, &pos, &loss_tick).await }),
    );
    let w = w.expect("win task").expect("win result");
    let l = l.expect("loss task").expect("loss result");
    assert!(w ^ l, "exactly one tx wins the UNIQUE gate: win={w} loss={l}");

    // The surviving outcome must match balance + position status (no torn settle).
    let outcome: (String,) = sqlx::query_as("SELECT outcome FROM settlements WHERE position_id = $1")
        .bind(pid).fetch_one(&db.pool).await.expect("settlement");
    let balance = common::get_balance(&db.pool, acct).await;
    let status: (String,) = sqlx::query_as("SELECT status FROM positions WHERE id = $1")
        .bind(pid).fetch_one(&db.pool).await.expect("status");
    match outcome.0.as_str() {
        "W" => {
            assert_eq!(balance, 1_000 + 250, "win credits floor(100 * 2.5) = 250");
            assert_eq!(status.0, "WON");
        }
        "L" => {
            assert_eq!(balance, 1_000, "loss does not credit");
            assert_eq!(status.0, "LOST");
        }
        other => panic!("unexpected outcome {other}"),
    }
    assert_eq!(common::count_settlements_for_position(&db.pool, pid).await, 1);
}

/// `sweep_once` must NOT race a live win that already settled the position.
/// Sequence: live `settle_win` commits, then sweep fires on a stale cache
/// snapshot. The UNIQUE gate prevents a duplicate row; balance stays correct.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sweep_once_after_live_win_does_not_overwrite() {
    let db = common::setup_test_postgres().await;
    let acct = common::insert_account(&db.pool, "post-win", 1_000).await;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let pid = common::insert_open_position(
        &db.pool, acct, "BTC", 70_000.0, 70_010.0,
        now_ms - 10_000, now_ms - 5_000, 100, 2.5,
    ).await;

    let pos = PositionRef {
        id: pid, account_id: acct, asset: AssetSymbol::Btc,
        strike_lo: 70_000.0, strike_hi: 70_010.0,
        t_open_ms: now_ms - 10_000, t_close_ms: now_ms - 5_000,
        stake_points: 100, multiplier_at_tap: 2.5,
        oracle_seq_at_tap: 0, oracle_run_id_at_tap: 0, created_at_ms: 0,
    };
    let win_tick = OracleTick {
        asset: AssetSymbol::Btc, run_id: 1, seq: 1, ts_ms: now_ms - 7_000,
        mid: 70_010.0, vol_annualized: 0.80, source_count: 3,
    };
    let credited = settle_win(&db.pool, &pos, &win_tick).await.expect("live win");
    assert!(credited);

    // The sweep sees the position only if it's OPEN. After settle_win the row
    // is WON, so hydrate excludes it — but defensively run the sweep anyway.
    let cache = OpenPositionCache::new();
    cache.record_last_mid(AssetSymbol::Btc, 70_005.0).await;
    let ctx = LoopContext {
        pool: db.pool.clone(),
        cache,
        last_tick_received_ms: Arc::new(AtomicI64::new(0)),
        gap_tracker: Arc::new(GapTracker::default()),
        metrics: Arc::new(Metrics::default()),
    };
    sweep_once(&ctx).await;

    let outcome: (String,) = sqlx::query_as("SELECT outcome FROM settlements WHERE position_id = $1")
        .bind(pid).fetch_one(&db.pool).await.expect("outcome");
    assert_eq!(outcome.0, "W", "settled WIN survives a later sweep pass");
    assert_eq!(common::get_balance(&db.pool, acct).await, 1_000 + 250);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn second_leader_blocked_while_first_holds() {
    let db = common::setup_test_postgres().await;
    let first = tap_trading_settlement_worker::leader::LeaderGuard::acquire_or_wait(&db.url)
        .await
        .expect("first acquires");

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let second = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        tap_trading_settlement_worker::leader::LeaderGuard::acquire_or_wait(&db.url),
    )
    .await;
    assert!(second.is_err(), "second leader must NOT acquire while first holds");
    drop(first);
}
