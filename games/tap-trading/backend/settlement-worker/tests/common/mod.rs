use sqlx::{postgres::PgPoolOptions, PgPool};
use testcontainers::{runners::AsyncRunner, ContainerAsync};
use testcontainers_modules::postgres::Postgres;
use uuid::Uuid;

#[allow(dead_code)]
pub struct TestDb {
    pub pool: PgPool,
    pub url: String,
    _container: ContainerAsync<Postgres>,
}

// Migrations applied in timestamp order — the same sequence sqlx runs in prod.
// All five are needed: the flusher reads `accounts.sui_address` (deposit/withdraw
// migration) and stamps `settlements.proof_index` (the batched-proof migration).
const MIGRATIONS: &[&str] = &[
    include_str!("../../../migrations/20260523120000_create_tick_schema.sql"),
    include_str!("../../../migrations/20260614000000_add_usdc_settle_mode.sql"),
    include_str!("../../../migrations/20260620150000_add_usdc_deposit_withdraw.sql"),
    include_str!("../../../migrations/20260621000000_swap_positions_asset_check_sol_to_sui.sql"),
    include_str!("../../../migrations/20260622000000_add_settlement_proof_index.sql"),
];

pub async fn setup_test_postgres() -> TestDb {
    let container = Postgres::default().start().await.expect("start postgres");
    let host_port = container.get_host_port_ipv4(5432).await.expect("port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{host_port}/postgres");

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await
        .expect("connect pool");

    for (i, sql) in MIGRATIONS.iter().enumerate() {
        // `*sql` is `&'static str` (the slice holds `include_str!` literals),
        // which is what `raw_sql`'s `SqlSafeStr` bound requires.
        sqlx::raw_sql(*sql).execute(&pool).await.unwrap_or_else(|e| panic!("apply migration {i}: {e}"));
    }

    TestDb { pool, url, _container: container }
}

#[allow(dead_code)]
pub async fn insert_account(pool: &PgPool, external_id: &str, starting_balance: i64) -> i64 {
    let row: (i64,) = sqlx::query_as(
        r#"
        INSERT INTO accounts
          (external_id, zklogin_sub, zklogin_iss, balance,
           lifetime_points_won, created_at_ms, last_active_ms)
        VALUES ($1, 'dev', 'dev', $2, 0, 0, 0)
        RETURNING id
        "#,
    )
    .bind(external_id)
    .bind(starting_balance)
    .fetch_one(pool)
    .await
    .expect("insert account");
    row.0
}

#[allow(dead_code, clippy::too_many_arguments)]
pub async fn insert_open_position(
    pool: &PgPool,
    account_id: i64,
    asset: &str,
    strike_lo: f64,
    strike_hi: f64,
    t_open_ms: i64,
    t_close_ms: i64,
    stake_points: i64,
    multiplier_at_tap: f64,
) -> i64 {
    let row: (i64,) = sqlx::query_as(
        r#"
        INSERT INTO positions
          (account_id, asset, strike_lo, strike_hi, t_open_ms, t_close_ms,
           stake_points, multiplier_at_tap, status, created_at_ms,
           oracle_seq_at_tap, oracle_run_id_at_tap, client_request_id)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'OPEN', $9, 0, 0, $10)
        RETURNING id
        "#,
    )
    .bind(account_id)
    .bind(asset)
    .bind(strike_lo)
    .bind(strike_hi)
    .bind(t_open_ms)
    .bind(t_close_ms)
    .bind(stake_points)
    .bind(multiplier_at_tap)
    .bind(t_open_ms)
    .bind(Uuid::new_v4())
    .fetch_one(pool)
    .await
    .expect("insert open position");
    row.0
}

#[allow(dead_code)]
pub async fn get_balance(pool: &PgPool, account_id: i64) -> i64 {
    let row: (i64,) = sqlx::query_as("SELECT balance FROM accounts WHERE id = $1")
        .bind(account_id)
        .fetch_one(pool)
        .await
        .expect("get balance");
    row.0
}

#[allow(dead_code)]
pub async fn get_lifetime_points(pool: &PgPool, account_id: i64) -> i64 {
    let row: (i64,) = sqlx::query_as("SELECT lifetime_points_won FROM accounts WHERE id = $1")
        .bind(account_id)
        .fetch_one(pool)
        .await
        .expect("get lifetime");
    row.0
}

#[allow(dead_code)]
pub async fn count_settlements_for_position(pool: &PgPool, position_id: i64) -> i64 {
    let row: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM settlements WHERE position_id = $1")
            .bind(position_id)
            .fetch_one(pool)
            .await
            .expect("count settlements");
    row.0
}
