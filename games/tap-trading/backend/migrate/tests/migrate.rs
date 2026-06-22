//! Integration test: run `run_migrations` against a Testcontainers Postgres
//! and assert every Tick table exists. The same fixtures power Task 3.

use sqlx::postgres::PgPoolOptions;
use sqlx::Row;
use tap_trading_migrate::{run_migrations, MIGRATION_TABLE};
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

/// Names of the eight tables Plan A's genesis migration creates.
const TICK_TABLES: &[&str] = &[
    "accounts",
    "daily_quests",
    "flags",
    "points_ledger",
    "positions",
    "settlements",
    "snapshots",
    "streaks",
];

async fn fresh_pool() -> (testcontainers::ContainerAsync<Postgres>, sqlx::PgPool) {
    let container = Postgres::default()
        .start()
        .await
        .expect("postgres container");
    let host_port = container.get_host_port_ipv4(5432).await.expect("host port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{host_port}/postgres");
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect");
    (container, pool)
}

#[tokio::test]
async fn applies_all_tick_tables() {
    let (_container, pool) = fresh_pool().await;

    run_migrations(&pool).await.expect("run_migrations");

    for table in TICK_TABLES {
        let row = sqlx::query(
            "SELECT COUNT(*)::BIGINT AS n
             FROM information_schema.tables
             WHERE table_schema = 'public' AND table_name = $1",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .expect("query");
        let n: i64 = row.try_get("n").expect("n");
        assert_eq!(n, 1, "table {table} missing after migrate");
    }
}

#[tokio::test]
async fn rerun_is_idempotent() {
    let (_container, pool) = fresh_pool().await;
    run_migrations(&pool).await.expect("first run");
    let after_first = tap_trading_migrate::list_applied(&pool)
        .await
        .expect("list after first run");
    run_migrations(&pool).await.expect("second run no-op");
    let after_second = tap_trading_migrate::list_applied(&pool)
        .await
        .expect("list after second run");

    // Idempotency — not just "doesn't error". The second run must not re-apply,
    // add, or duplicate any bookkeeping row.
    assert_eq!(
        after_first.len(),
        after_second.len(),
        "rerun changed the applied-migration count"
    );
    assert_eq!(
        after_second.len(),
        1,
        "exactly one genesis migration after rerun"
    );
}

#[tokio::test]
async fn uses_custom_migration_table() {
    let (_container, pool) = fresh_pool().await;
    run_migrations(&pool).await.expect("run");

    let row = sqlx::query(
        "SELECT COUNT(*)::BIGINT AS n
         FROM information_schema.tables
         WHERE table_schema = 'public' AND table_name = $1",
    )
    .bind(MIGRATION_TABLE)
    .fetch_one(&pool)
    .await
    .expect("query");
    let n: i64 = row.try_get("n").expect("n");
    assert_eq!(n, 1, "{MIGRATION_TABLE} should be created by sqlx");

    // The default sqlx table must NOT exist — that's the whole point of the override.
    let row = sqlx::query(
        "SELECT COUNT(*)::BIGINT AS n
         FROM information_schema.tables
         WHERE table_schema = 'public' AND table_name = '_sqlx_migrations'",
    )
    .fetch_one(&pool)
    .await
    .expect("query");
    let n: i64 = row.try_get("n").expect("n");
    assert_eq!(n, 0, "default _sqlx_migrations must not exist");
}

#[tokio::test]
async fn list_applied_after_run_returns_one_row() {
    let (_container, pool) = fresh_pool().await;
    run_migrations(&pool).await.expect("run");

    let applied = tap_trading_migrate::list_applied(&pool)
        .await
        .expect("list_applied");

    // Plan A shipped one genesis migration: 20260523120000_create_tick_schema.sql.
    assert_eq!(applied.len(), 1, "expected exactly 1 migration row");
    let row = &applied[0];
    assert_eq!(row.version, 20_260_523_120_000);
    // sqlx replaces underscores with spaces in descriptions derived from filenames.
    assert!(
        row.description.contains("create tick schema"),
        "unexpected description: {}",
        row.description
    );
    assert!(row.success, "migration must be marked success");
    // Exercise the two non-trivially projected columns (lib.rs derives
    // installed_on_ms via EXTRACT(EPOCH ...)*1000 and aliases execution_time):
    // a broken cast or a placeholder execution_time would otherwise pass silently.
    assert!(
        row.installed_on_ms > 0,
        "installed_on_ms must be real epoch-ms, got {}",
        row.installed_on_ms
    );
    assert!(
        row.execution_time_ns >= 0,
        "execution_time_ns must be populated, got {}",
        row.execution_time_ns
    );
}

#[tokio::test]
async fn list_applied_on_empty_db_returns_empty_vec() {
    let (_container, pool) = fresh_pool().await;
    // Don't run migrations — the table doesn't exist yet.
    let applied = tap_trading_migrate::list_applied(&pool)
        .await
        .expect("list_applied tolerates missing table");
    assert!(applied.is_empty(), "expected no rows: {applied:?}");
}
