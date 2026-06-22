//! sqlx query helpers. Keep handlers free of SQL string literals.

use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::error::ApiError;

pub struct InsertPositionInput<'a> {
    pub account_id: i64,
    pub asset: &'a str,
    pub strike_lo: f64,
    pub strike_hi: f64,
    pub t_open_ms: i64,
    pub t_close_ms: i64,
    pub stake_points: i64,
    pub multiplier_at_tap: f64,
    pub oracle_seq_at_tap: i64,
    pub oracle_run_id_at_tap: i64,
    pub client_request_id: Uuid,
    pub client_fingerprint: Option<&'a str>,
    pub now_ms: i64,
}

/// Select balance with row-level lock; serializes concurrent taps for one account.
pub async fn select_balance_for_update(
    tx: &mut Transaction<'_, Postgres>,
    account_id: i64,
) -> Result<i64, ApiError> {
    let (balance,): (i64,) =
        sqlx::query_as("SELECT balance FROM accounts WHERE id = $1 FOR UPDATE")
            .bind(account_id)
            .fetch_one(&mut **tx)
            .await?;
    Ok(balance)
}

/// INSERT with ON CONFLICT DO NOTHING; returns Some(id) if newly inserted,
/// None if the (account_id, client_request_id) pair already exists.
pub async fn insert_position(
    tx: &mut Transaction<'_, Postgres>,
    i: &InsertPositionInput<'_>,
) -> Result<Option<i64>, ApiError> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        INSERT INTO positions (
            account_id, asset, strike_lo, strike_hi, t_open_ms, t_close_ms,
            stake_points, multiplier_at_tap, status,
            oracle_seq_at_tap, oracle_run_id_at_tap, client_request_id,
            client_fingerprint, created_at_ms
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'OPEN', $9, $10, $11, $12, $13)
        ON CONFLICT ON CONSTRAINT positions_dedup_request DO NOTHING
        RETURNING id
        "#,
    )
    .bind(i.account_id)
    .bind(i.asset)
    .bind(i.strike_lo)
    .bind(i.strike_hi)
    .bind(i.t_open_ms)
    .bind(i.t_close_ms)
    .bind(i.stake_points)
    .bind(i.multiplier_at_tap)
    .bind(i.oracle_seq_at_tap)
    .bind(i.oracle_run_id_at_tap)
    .bind(i.client_request_id)
    .bind(i.client_fingerprint)
    .bind(i.now_ms)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(|(id,)| id))
}

pub async fn insert_tap_stake_ledger(
    tx: &mut Transaction<'_, Postgres>,
    account_id: i64,
    stake_points: i64,
    position_id: i64,
    now_ms: i64,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"INSERT INTO points_ledger (account_id, kind, delta, ref_id, created_at_ms)
           VALUES ($1, 'TAP_STAKE', $2, $3, $4)"#,
    )
    .bind(account_id)
    .bind(-stake_points)
    .bind(position_id)
    .bind(now_ms)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub async fn debit_balance(
    tx: &mut Transaction<'_, Postgres>,
    account_id: i64,
    stake_points: i64,
    now_ms: i64,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"UPDATE accounts
           SET balance = balance - $2, last_active_ms = $3
           WHERE id = $1"#,
    )
    .bind(account_id)
    .bind(stake_points)
    .bind(now_ms)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct ExistingPosition {
    pub id: i64,
    pub multiplier_at_tap: f64,
    pub status: String,
    pub t_close_ms: i64,
}

/// Look up an existing (account_id, client_request_id) row. Used by Task 10's
/// idempotency path; ships here so `db.rs` is a single PR-reviewable module.
pub async fn find_position_by_request_id(
    pool: &PgPool,
    account_id: i64,
    request_id: Uuid,
) -> Result<Option<ExistingPosition>, ApiError> {
    // Cast NUMERIC to TEXT then parse to f64; avoids a bigdecimal dep.
    let row: Option<(i64, String, String, i64)> = sqlx::query_as(
        r#"SELECT id, multiplier_at_tap::TEXT, status, t_close_ms
           FROM positions WHERE account_id = $1 AND client_request_id = $2"#,
    )
    .bind(account_id)
    .bind(request_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(id, mult_str, status, t_close)| ExistingPosition {
        id,
        multiplier_at_tap: mult_str.parse::<f64>().unwrap_or(0.0),
        status,
        t_close_ms: t_close,
    }))
}
