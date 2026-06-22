//! POST /v1/withdraw — debit the off-chain USDC ledger and release real USDC
//! from the operator custody PlayerBalance to the player's bound wallet.
//!
//! Saga: debit + record PENDING in one committed tx (before the chain call, so
//! a crash can't double-spend), then the operator signs the release. On release
//! failure we refund the debit and mark the row FAILED — the player never loses
//! balance to a failed release.

use std::process::Command;

use axum::{extract::State, Extension, Json};
use serde::{Deserialize, Serialize};

use crate::account_ctx::AccountCtx;
use crate::error::ApiError;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct WithdrawRequest {
    pub amount_micro: i64,
}

#[derive(Debug, Serialize)]
pub struct WithdrawResponse {
    pub tx_digest: String,
    pub balance: i64,
}

pub async fn post_withdraw(
    State(state): State<AppState>,
    Extension(ctx): Extension<AccountCtx>,
    Json(req): Json<WithdrawRequest>,
) -> Result<Json<WithdrawResponse>, ApiError> {
    if req.amount_micro <= 0 {
        return Err(ApiError::InvalidStake);
    }
    let (to_addr,): (Option<String>,) =
        sqlx::query_as("SELECT sui_address FROM accounts WHERE id = $1")
            .bind(ctx.id)
            .fetch_one(&state.pg)
            .await?;
    let to = to_addr.ok_or(ApiError::NoWithdrawAddress)?;
    let now = state.clock.now_ms();

    // Debit + record PENDING atomically, committed before the on-chain release.
    let mut tx = state.pg.begin().await?;
    let (bal,): (i64,) = sqlx::query_as("SELECT balance FROM accounts WHERE id=$1 FOR UPDATE")
        .bind(ctx.id)
        .fetch_one(&mut *tx)
        .await?;
    if bal < req.amount_micro {
        tx.rollback().await?;
        return Err(ApiError::InsufficientBalance);
    }
    sqlx::query("UPDATE accounts SET balance = balance - $2, last_active_ms = $3 WHERE id = $1")
        .bind(ctx.id)
        .bind(req.amount_micro)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    let (wid,): (i64,) = sqlx::query_as(
        r#"INSERT INTO withdrawals (account_id, amount_micro, to_address, status, created_at_ms)
           VALUES ($1,$2,$3,'PENDING',$4) RETURNING id"#,
    )
    .bind(ctx.id)
    .bind(req.amount_micro)
    .bind(&to)
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;

    // Operator-signed release (CLI-behind, mirroring the worker's settler).
    match release_usdc(&to, req.amount_micro).await {
        Ok(digest) => {
            sqlx::query("UPDATE withdrawals SET status='SENT', tx_digest=$2 WHERE id=$1")
                .bind(wid)
                .bind(&digest)
                .execute(&state.pg)
                .await?;
            let (newbal,): (i64,) = sqlx::query_as("SELECT balance FROM accounts WHERE id=$1")
                .bind(ctx.id)
                .fetch_one(&state.pg)
                .await?;
            Ok(Json(WithdrawResponse {
                tx_digest: digest,
                balance: newbal,
            }))
        }
        Err(e) => {
            tracing::error!(error = %e, withdrawal_id = wid, "withdraw release failed; refunding");
            let mut tx2 = state.pg.begin().await?;
            sqlx::query("UPDATE accounts SET balance = balance + $2 WHERE id = $1")
                .bind(ctx.id)
                .bind(req.amount_micro)
                .execute(&mut *tx2)
                .await?;
            sqlx::query("UPDATE withdrawals SET status='FAILED' WHERE id=$1")
                .bind(wid)
                .execute(&mut *tx2)
                .await?;
            tx2.commit().await?;
            Err(ApiError::WithdrawFailed)
        }
    }
}

/// Operator signs `vault::withdraw(custodyPB, amount) -> Coin` and transfers the
/// coin to `to` in one PTB. Shells the `sui` CLI — same deliberate pattern as
/// the settlement worker (`sui-sdk` drags the whole monorepo); programmatic
/// signing is a later swap. Blocking, so run on a blocking thread.
async fn release_usdc(to: &str, amount_micro: i64) -> anyhow::Result<String> {
    let pkg = std::env::var("TICK_VAULT_PKG")?;
    let usdc = std::env::var("TICK_QUOTE_TYPE")?;
    let custody = std::env::var("TICK_CUSTODY_PB_ID")?;
    let gas = std::env::var("TICK_WITHDRAW_GAS_BUDGET").unwrap_or_else(|_| "60000000".into());
    let to = to.to_string();
    let amt = amount_micro.to_string();
    tokio::task::spawn_blocking(move || {
        let out = Command::new("sui")
            .args([
                "client",
                "ptb",
                "--move-call",
                &format!("{pkg}::vault::withdraw"),
                &format!("<{usdc}>"),
                &format!("@{custody}"),
                &amt,
                "--assign",
                "c",
                "--transfer-objects",
                "[c]",
                &format!("@{to}"),
                "--gas-budget",
                &gas,
            ])
            .output()?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        if !stdout.contains("Status: Success") {
            anyhow::bail!(
                "withdraw ptb failed: {}",
                if stderr.trim().is_empty() {
                    stdout.trim()
                } else {
                    stderr.trim()
                }
            );
        }
        stdout
            .lines()
            .find_map(|l| l.trim().strip_prefix("Transaction Digest:"))
            .map(|s| s.trim().to_string())
            .ok_or_else(|| anyhow::anyhow!("no digest in ptb output"))
    })
    .await?
}
