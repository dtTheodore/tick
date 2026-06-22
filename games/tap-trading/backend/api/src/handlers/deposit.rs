//! POST /v1/deposit — credit the off-chain USDC ledger from a verified on-chain
//! deposit into the operator custody PlayerBalance.
//!
//! Trust model: anyone may `vault::deposit` into a shared PlayerBalance, so the
//! player signs a deposit of USDC into the custody balance and posts the digest
//! here. We verify on-chain via fullnode RPC — tx succeeded, the custody PB was
//! mutated, and the sender's USDC balance fell by `amount` — then credit that
//! exact micro-amount exactly once (idempotent by digest) and bind the sender
//! as the account's withdrawal address.

use axum::{extract::State, Extension, Json};
use serde::{Deserialize, Serialize};

use crate::account_ctx::AccountCtx;
use crate::error::ApiError;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct DepositRequest {
    pub tx_digest: String,
}

#[derive(Debug, Serialize)]
pub struct DepositResponse {
    pub credited_micro: i64,
    pub balance: i64,
    pub already_credited: bool,
}

fn env_var(k: &str) -> Result<String, ApiError> {
    std::env::var(k).map_err(|_| ApiError::Internal(anyhow::anyhow!("{k} unset")))
}

pub async fn post_deposit(
    State(state): State<AppState>,
    Extension(ctx): Extension<AccountCtx>,
    Json(req): Json<DepositRequest>,
) -> Result<Json<DepositResponse>, ApiError> {
    // Idempotency: a digest is creditable at most once.
    if let Some((amt, bal)) = lookup_deposit(&state.pg, &req.tx_digest).await? {
        return Ok(Json(DepositResponse {
            credited_micro: amt,
            balance: bal,
            already_credited: true,
        }));
    }

    let rpc = env_var("SUI_RPC_URL")?;
    let usdc = env_var("TICK_QUOTE_TYPE")?;
    let custody = env_var("TICK_CUSTODY_PB_ID")?;

    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "sui_getTransactionBlock",
        "params": [req.tx_digest, {
            "showBalanceChanges": true, "showEffects": true,
            "showObjectChanges": true, "showInput": true
        }]
    });
    let resp: serde_json::Value = reqwest::Client::new()
        .post(&rpc)
        .json(&body)
        .send()
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("sui rpc send: {e}")))?
        .json()
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("sui rpc decode: {e}")))?;
    let result = resp.get("result").ok_or(ApiError::DepositUnverifiable)?;

    // Verify: tx succeeded, the custody PB was mutated, and the sender's USDC
    // balance fell by `deposited`.
    if result
        .pointer("/effects/status/status")
        .and_then(|s| s.as_str())
        != Some("success")
    {
        return Err(ApiError::DepositUnverifiable);
    }
    let sender = result
        .pointer("/transaction/data/sender")
        .and_then(|s| s.as_str())
        .ok_or(ApiError::DepositUnverifiable)?;
    let touched_custody = result
        .get("objectChanges")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .any(|c| c.get("objectId").and_then(|o| o.as_str()) == Some(custody.as_str()))
        })
        .unwrap_or(false);
    if !touched_custody {
        return Err(ApiError::DepositUnverifiable);
    }
    let mut deposited: i64 = 0;
    if let Some(bcs) = result.get("balanceChanges").and_then(|v| v.as_array()) {
        for bc in bcs {
            let ct = bc.get("coinType").and_then(|c| c.as_str()).unwrap_or("");
            let owner = bc
                .pointer("/owner/AddressOwner")
                .and_then(|o| o.as_str())
                .unwrap_or("");
            let amt: i64 = bc
                .get("amount")
                .and_then(|a| a.as_str())
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            if ct == usdc && owner == sender && amt < 0 {
                deposited = -amt;
            }
        }
    }
    if deposited <= 0 {
        return Err(ApiError::DepositUnverifiable);
    }

    let bal = credit_deposit(
        &state.pg,
        ctx.id,
        &req.tx_digest,
        deposited,
        sender,
        state.clock.now_ms(),
    )
    .await?;
    Ok(Json(DepositResponse {
        credited_micro: deposited,
        balance: bal,
        already_credited: false,
    }))
}

async fn lookup_deposit(pg: &sqlx::PgPool, digest: &str) -> Result<Option<(i64, i64)>, ApiError> {
    let row: Option<(i64, i64)> = sqlx::query_as(
        r#"SELECT d.amount_micro, a.balance
           FROM deposits d JOIN accounts a ON a.id = d.account_id
           WHERE d.tx_digest = $1"#,
    )
    .bind(digest)
    .fetch_optional(pg)
    .await?;
    Ok(row)
}

async fn credit_deposit(
    pg: &sqlx::PgPool,
    account_id: i64,
    digest: &str,
    amount: i64,
    from: &str,
    now: i64,
) -> Result<i64, ApiError> {
    let mut tx = pg.begin().await?;
    // The UNIQUE digest insert is the idempotency guard: a concurrent duplicate
    // returns None here, so we never double-credit.
    let inserted: Option<(i64,)> = sqlx::query_as(
        r#"INSERT INTO deposits (account_id, tx_digest, amount_micro, from_address, created_at_ms)
           VALUES ($1,$2,$3,$4,$5) ON CONFLICT (tx_digest) DO NOTHING RETURNING id"#,
    )
    .bind(account_id)
    .bind(digest)
    .bind(amount)
    .bind(from)
    .bind(now)
    .fetch_optional(&mut *tx)
    .await?;
    if inserted.is_none() {
        tx.rollback().await?;
        let (bal,): (i64,) = sqlx::query_as("SELECT balance FROM accounts WHERE id=$1")
            .bind(account_id)
            .fetch_one(pg)
            .await?;
        return Ok(bal);
    }
    let (bal,): (i64,) = sqlx::query_as(
        r#"UPDATE accounts SET balance = balance + $2,
             sui_address = COALESCE(sui_address, $3), last_active_ms = $4
           WHERE id = $1 RETURNING balance"#,
    )
    .bind(account_id)
    .bind(amount)
    .bind(from)
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(bal)
}
