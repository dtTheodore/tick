mod common;

use tap_trading_oracle_types::OracleTick;
use tap_trading_pricing_engine::AssetSymbol;
use tap_trading_settlement_worker::cache::PositionRef;
use tap_trading_settlement_worker::settle::settle_loss;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loss_writes_settlement_with_zero_delta_and_no_ledger() {
    let db = common::setup_test_postgres().await;
    let acct = common::insert_account(&db.pool, "loser", 1_000).await;
    let pid = common::insert_open_position(&db.pool, acct, "BTC", 70_000.0, 70_010.0, 1_000, 6_000, 100, 2.5).await;

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
        ts_ms: 7_000,
        mid: 70_005.0,
        vol_annualized: 0.80,
        source_count: 3,
    };

    let credited = settle_loss(&db.pool, &pos, &tick).await.expect("settle_loss");
    assert!(credited);

    assert_eq!(common::get_balance(&db.pool, acct).await, 1_000);

    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM points_ledger WHERE ref_id = $1")
        .bind(pid)
        .fetch_one(&db.pool)
        .await
        .expect("ledger count");
    assert_eq!(count.0, 0);

    let row: (String, i64) =
        sqlx::query_as("SELECT outcome, points_delta FROM settlements WHERE position_id = $1")
            .bind(pid)
            .fetch_one(&db.pool)
            .await
            .expect("settlement row");
    assert_eq!(row, ("L".to_string(), 0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_loss_is_noop() {
    let db = common::setup_test_postgres().await;
    let acct = common::insert_account(&db.pool, "loser2", 1_000).await;
    let pid = common::insert_open_position(&db.pool, acct, "BTC", 70_000.0, 70_010.0, 1_000, 6_000, 100, 2.5).await;

    let pos = PositionRef {
        id: pid, account_id: acct, asset: AssetSymbol::Btc,
        strike_lo: 70_000.0, strike_hi: 70_010.0,
        t_open_ms: 1_000, t_close_ms: 6_000,
        stake_points: 100, multiplier_at_tap: 2.5,
        oracle_seq_at_tap: 0, oracle_run_id_at_tap: 0, created_at_ms: 0,
    };
    let tick = OracleTick {
        asset: AssetSymbol::Btc, run_id: 1, seq: 1, ts_ms: 7_000,
        mid: 70_005.0, vol_annualized: 0.80, source_count: 3,
    };

    assert!(settle_loss(&db.pool, &pos, &tick).await.expect("loss 1"));
    assert!(!settle_loss(&db.pool, &pos, &tick).await.expect("loss 2"));
    assert_eq!(common::count_settlements_for_position(&db.pool, pid).await, 1);
}
