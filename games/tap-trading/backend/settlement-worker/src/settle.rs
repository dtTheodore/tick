//! Settlement transactions. SYSTEM_DESIGN.md §5.2; ADR-0009 §7-§8.
//!
//! Each public function is ONE atomic transaction. The `settlements` row is
//! the idempotency canary: `INSERT ... ON CONFLICT (position_id) DO NOTHING RETURNING id`.
//! If no row is returned, the position was already settled — return Ok(false).

use sqlx::types::BigDecimal;
use sqlx::PgPool;
use std::str::FromStr;
use tap_trading_oracle_types::OracleTick;

use crate::cache::PositionRef;
use crate::error::{Result, WorkerError};

// Per-tap on-chain settlement (the `usdc` settle_mode) has been removed: Tick is
// off-chain USDC, with every settlement publishing a BATCHED Walrus proof via
// the proof flusher. `evidence_to_seq` (the tick that triggered the outcome) is
// recorded on win/loss so the flusher can reassemble the proof's tick window.

/// Win settlement. Pays out `floor(stake * multiplier_at_tap)`.
/// Updates both `balance` AND `lifetime_points_won` per SYSTEM_DESIGN §5.2.
/// Returns Ok(true) if credited, Ok(false) if already settled.
pub async fn settle_win(pool: &PgPool, position: &PositionRef, tick: &OracleTick) -> Result<bool> {
    let payout: i64 = ((position.stake_points as f64) * position.multiplier_at_tap).floor() as i64;
    let oracle_price = f64_to_numeric(tick.mid, "tick.mid")?;
    let multiplier_used = f64_to_numeric(position.multiplier_at_tap, "multiplier_at_tap")?;

    let mut tx = pool.begin().await?;

    let inserted: Option<(i64,)> = sqlx::query_as(
        r#"
        INSERT INTO settlements
          (position_id, account_id, outcome, points_delta,
           oracle_price, settled_at_ms, multiplier_used,
           streak_at_credit, streak_bonus, evidence_to_seq)
        VALUES ($1, $2, 'W', $3, $4, $5, $6, 0, 1.000, $7)
        ON CONFLICT (position_id) DO NOTHING
        RETURNING id
        "#,
    )
    .bind(position.id)
    .bind(position.account_id)
    .bind(payout)
    .bind(oracle_price)
    .bind(tick.ts_ms)
    .bind(multiplier_used)
    .bind(tick.seq as i64)
    .fetch_optional(&mut *tx)
    .await?;

    if inserted.is_none() {
        tx.rollback().await?;
        return Ok(false);
    }

    sqlx::query(
        "UPDATE positions SET status='WON', settled_at_ms=$2 WHERE id=$1 AND status='OPEN'",
    )
    .bind(position.id)
    .bind(tick.ts_ms)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"INSERT INTO points_ledger (account_id, kind, delta, ref_id, created_at_ms)
           VALUES ($1, 'TAP_PAYOUT', $2, $3, $4)"#,
    )
    .bind(position.account_id)
    .bind(payout)
    .bind(position.id)
    .bind(tick.ts_ms)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"UPDATE accounts
              SET balance = balance + $2,
                  lifetime_points_won = lifetime_points_won + $2
            WHERE id = $1"#,
    )
    .bind(position.account_id)
    .bind(payout)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(true)
}

/// Loss settlement. Tick arrived past `t_close_ms` with no prior touch.
/// No ledger row, no balance change — stake debited at tap-commit (ADR-0009 §4).
pub async fn settle_loss(pool: &PgPool, position: &PositionRef, tick: &OracleTick) -> Result<bool> {
    let oracle_price = f64_to_numeric(tick.mid, "tick.mid")?;
    let multiplier_used = f64_to_numeric(position.multiplier_at_tap, "multiplier_at_tap")?;

    let mut tx = pool.begin().await?;

    let inserted: Option<(i64,)> = sqlx::query_as(
        r#"
        INSERT INTO settlements
          (position_id, account_id, outcome, points_delta,
           oracle_price, settled_at_ms, multiplier_used,
           streak_at_credit, streak_bonus, evidence_to_seq)
        VALUES ($1, $2, 'L', 0, $3, $4, $5, 0, 1.000, $6)
        ON CONFLICT (position_id) DO NOTHING
        RETURNING id
        "#,
    )
    .bind(position.id)
    .bind(position.account_id)
    .bind(oracle_price)
    .bind(tick.ts_ms)
    .bind(multiplier_used)
    .bind(tick.seq as i64)
    .fetch_optional(&mut *tx)
    .await?;

    if inserted.is_none() {
        tx.rollback().await?;
        return Ok(false);
    }

    sqlx::query(
        "UPDATE positions SET status='LOST', settled_at_ms=$2 WHERE id=$1 AND status='OPEN'",
    )
    .bind(position.id)
    .bind(tick.ts_ms)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(true)
}

/// Void refund. Position's window was fully covered by an oracle gap.
/// Refunds stake via TAP_REFUND ledger, updates balance only (not lifetime_points_won).
/// `oracle_price` is `last_known_mid` (must be > 0 per schema CHECK); falls back to
/// `strike_lo` if no mid has been observed.
pub async fn settle_void(
    pool: &PgPool,
    position: &PositionRef,
    last_known_mid: Option<f64>,
    settled_at_ms: i64,
) -> Result<bool> {
    let oracle_price_f64 = match last_known_mid {
        Some(v) if v.is_finite() && v > 0.0 => v,
        _ => {
            tracing::warn!(position_id = position.id, "voiding with no last-known mid; using strike_lo");
            position.strike_lo
        }
    };
    let oracle_price = f64_to_numeric(oracle_price_f64, "void_oracle_price")?;
    let multiplier_used = f64_to_numeric(position.multiplier_at_tap, "multiplier_at_tap")?;

    let mut tx = pool.begin().await?;

    let inserted: Option<(i64,)> = sqlx::query_as(
        r#"
        INSERT INTO settlements
          (position_id, account_id, outcome, points_delta,
           oracle_price, settled_at_ms, multiplier_used,
           streak_at_credit, streak_bonus)
        VALUES ($1, $2, 'V', $3, $4, $5, $6, 0, 1.000)
        ON CONFLICT (position_id) DO NOTHING
        RETURNING id
        "#,
    )
    .bind(position.id)
    .bind(position.account_id)
    .bind(position.stake_points)
    .bind(oracle_price)
    .bind(settled_at_ms)
    .bind(multiplier_used)
    .fetch_optional(&mut *tx)
    .await?;

    if inserted.is_none() {
        tx.rollback().await?;
        return Ok(false);
    }

    sqlx::query(
        "UPDATE positions SET status='VOIDED', settled_at_ms=$2 WHERE id=$1 AND status='OPEN'",
    )
    .bind(position.id)
    .bind(settled_at_ms)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"INSERT INTO points_ledger (account_id, kind, delta, ref_id, created_at_ms)
           VALUES ($1, 'TAP_REFUND', $2, $3, $4)"#,
    )
    .bind(position.account_id)
    .bind(position.stake_points)
    .bind(position.id)
    .bind(settled_at_ms)
    .execute(&mut *tx)
    .await?;

    sqlx::query("UPDATE accounts SET balance = balance + $2 WHERE id = $1")
        .bind(position.account_id)
        .bind(position.stake_points)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(true)
}

/// Stamp the published batch blob id and this row's index into that batch's
/// `proofs` vector. `proof_index` lets a consumer extract the single `ProofBlob`
/// for this settlement from the shared blob.
pub async fn mark_proof_published(
    pool: &PgPool,
    position_id: i64,
    walrus_blob_id: &str,
    proof_index: i32,
) -> Result<()> {
    sqlx::query(
        "UPDATE settlements SET proof_status='published', walrus_blob_id=$2, proof_index=$3 WHERE position_id=$1",
    )
    .bind(position_id)
    .bind(walrus_blob_id)
    .bind(proof_index)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_proof_failed(pool: &PgPool, position_id: i64) -> Result<()> {
    sqlx::query("UPDATE settlements SET proof_status='failed' WHERE position_id=$1")
        .bind(position_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Format an f64 as `NUMERIC(_, 8)` text and parse to `BigDecimal`.
///
/// Rejects NaN/±Inf — those format as "NaN"/"inf" which `BigDecimal::from_str`
/// would refuse, panicking on a data-driven path (a bad oracle tick). Surface
/// as `WorkerError` so the caller can log and let the dispatch site retry.
pub(crate) fn f64_to_numeric(v: f64, context: &'static str) -> Result<BigDecimal> {
    if !v.is_finite() {
        return Err(WorkerError::NonFiniteNumeric { context, value: format!("{v}") });
    }
    BigDecimal::from_str(&format!("{v:.8}"))
        .map_err(|e| WorkerError::NonFiniteNumeric { context, value: e.to_string() })
}
