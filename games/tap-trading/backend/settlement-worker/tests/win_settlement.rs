mod common;

use tap_trading_oracle_types::OracleTick;
use tap_trading_pricing_engine::AssetSymbol;
use tap_trading_settlement_worker::cache::PositionRef;
use tap_trading_settlement_worker::settle::settle_win;

fn position_for(id: i64, account_id: i64, multiplier_at_tap: f64, stake: i64) -> PositionRef {
    PositionRef {
        id,
        account_id,
        asset: AssetSymbol::Btc,
        strike_lo: 70_000.0,
        strike_hi: 70_010.0,
        t_open_ms: 1_000,
        t_close_ms: 6_000,
        stake_points: stake,
        multiplier_at_tap,
        oracle_seq_at_tap: 0,
        oracle_run_id_at_tap: 0,
        created_at_ms: 0,
    }
}

fn touching_tick(ts_ms: i64) -> OracleTick {
    OracleTick {
        asset: AssetSymbol::Btc,
        run_id: 1,
        seq: 1,
        ts_ms,
        mid: 70_010.0,
        vol_annualized: 0.80,
        source_count: 3,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn win_credits_balance_ledger_and_lifetime_points() {
    let db = common::setup_test_postgres().await;
    let acct = common::insert_account(&db.pool, "winner", 1_000).await;
    let pid = common::insert_open_position(&db.pool, acct, "BTC", 70_000.0, 70_010.0, 1_000, 6_000, 100, 2.5).await;

    let credited = settle_win(&db.pool, &position_for(pid, acct, 2.5, 100), &touching_tick(3_000))
        .await
        .expect("settle_win");
    assert!(credited);

    assert_eq!(common::get_balance(&db.pool, acct).await, 1_000 + 250);
    assert_eq!(common::get_lifetime_points(&db.pool, acct).await, 250);
    assert_eq!(common::count_settlements_for_position(&db.pool, pid).await, 1);

    let status: (String,) = sqlx::query_as("SELECT status FROM positions WHERE id = $1")
        .bind(pid)
        .fetch_one(&db.pool)
        .await
        .expect("status");
    assert_eq!(status.0, "WON");

    let ledger: (String, i64) = sqlx::query_as(
        "SELECT kind, delta FROM points_ledger WHERE ref_id = $1 AND kind = 'TAP_PAYOUT'",
    )
    .bind(pid)
    .fetch_one(&db.pool)
    .await
    .expect("ledger");
    assert_eq!(ledger, ("TAP_PAYOUT".to_string(), 250));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_settle_is_noop() {
    let db = common::setup_test_postgres().await;
    let acct = common::insert_account(&db.pool, "winner2", 1_000).await;
    let pid = common::insert_open_position(&db.pool, acct, "BTC", 70_000.0, 70_010.0, 1_000, 6_000, 100, 2.5).await;

    let first = settle_win(&db.pool, &position_for(pid, acct, 2.5, 100), &touching_tick(3_000))
        .await.expect("settle 1");
    assert!(first);
    let balance_1 = common::get_balance(&db.pool, acct).await;

    let second = settle_win(&db.pool, &position_for(pid, acct, 2.5, 100), &touching_tick(3_500))
        .await.expect("settle 2");
    assert!(!second, "second call must be a no-op");

    assert_eq!(common::get_balance(&db.pool, acct).await, balance_1, "balance unchanged on duplicate");
    assert_eq!(common::count_settlements_for_position(&db.pool, pid).await, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settle_uses_locked_multiplier() {
    let db = common::setup_test_postgres().await;
    let acct = common::insert_account(&db.pool, "locked", 1_000).await;
    let pid = common::insert_open_position(&db.pool, acct, "BTC", 70_000.0, 70_010.0, 1_000, 6_000, 100, 5.0).await;

    settle_win(&db.pool, &position_for(pid, acct, 5.0, 100), &touching_tick(3_000))
        .await.expect("settle");

    assert_eq!(common::get_balance(&db.pool, acct).await, 1_500);
    let row: (sqlx::types::BigDecimal,) =
        sqlx::query_as("SELECT multiplier_used FROM settlements WHERE position_id = $1")
            .bind(pid)
            .fetch_one(&db.pool)
            .await
            .expect("mult");
    assert_eq!(row.0.to_string().parse::<f64>().unwrap(), 5.0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn payout_floors() {
    // 100 * 2.4999 = 249.99 → floor to 249.
    let db = common::setup_test_postgres().await;
    let acct = common::insert_account(&db.pool, "floored", 1_000).await;
    let pid = common::insert_open_position(&db.pool, acct, "BTC", 70_000.0, 70_010.0, 1_000, 6_000, 100, 2.4999).await;

    settle_win(&db.pool, &position_for(pid, acct, 2.4999, 100), &touching_tick(3_000))
        .await.expect("settle");

    assert_eq!(common::get_balance(&db.pool, acct).await, 1_249);
}
