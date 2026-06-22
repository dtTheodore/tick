//! `X-Account-Id` middleware. ADR-0009 §1.
//!
//! Validates the header, lazy-creates the account row, attaches `AccountCtx` to
//! request extensions. USDC economy: a new account starts at 0 balance — it is
//! funded by an on-chain vault USDC deposit, not a free points faucet.

use axum::extract::{Request, State};
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::response::Response;

use crate::account_ctx::AccountCtx;
use crate::error::ApiError;
use crate::state::AppState;

const HEADER: &str = "x-account-id";
const MAX_LEN: usize = 128;
// No free faucet in the USDC economy: a new account starts at 0 and is funded
// by a real on-chain USDC deposit. (Kept as a named constant so the bootstrap
// INSERT/ledger shape is unchanged; flip non-zero only for a promo airdrop.)
const SIGNUP_BONUS: i64 = 0;

pub async fn account_id_middleware(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let value = headers
        .get(HEADER)
        .ok_or(ApiError::MissingAccountId)?
        .to_str()
        .map_err(|_| ApiError::InvalidAccountId)?;
    if value.is_empty() || value.len() > MAX_LEN {
        return Err(ApiError::InvalidAccountId);
    }
    let ctx = lookup_or_create(&state, value).await?;
    req.extensions_mut().insert(ctx);
    Ok(next.run(req).await)
}

async fn lookup_or_create(state: &AppState, external_id: &str) -> Result<AccountCtx, ApiError> {
    // Fast path: existing account is the overwhelming common case. A plain
    // SELECT avoids a write transaction (and the `accounts` sequence burn that
    // `INSERT … ON CONFLICT DO NOTHING` incurs even when it inserts nothing) on
    // every authenticated request.
    if let Some((id,)) =
        sqlx::query_as::<_, (i64,)>("SELECT id FROM accounts WHERE external_id = $1")
            .bind(external_id)
            .fetch_optional(&state.pg)
            .await?
    {
        return Ok(AccountCtx {
            id,
            external_id: external_id.to_string(),
        });
    }

    // Slow path: lazy-create the account + one-time SIGNUP ledger row. The
    // ON CONFLICT guard handles two concurrent first-requests racing the insert;
    // the loser falls through to the SELECT and reads the winner's id.
    let now = state.clock.now_ms();
    let mut tx = state.pg.begin().await?;

    let inserted: Option<(i64,)> = sqlx::query_as(
        r#"
        INSERT INTO accounts
          (external_id, zklogin_sub, zklogin_iss, balance, lifetime_points_won,
           signup_bonus_at_ms, created_at_ms, last_active_ms)
        VALUES ($1, 'dev', 'dev', $2, 0, $3, $3, $3)
        ON CONFLICT (external_id) DO NOTHING
        RETURNING id
        "#,
    )
    .bind(external_id)
    .bind(SIGNUP_BONUS)
    .bind(now)
    .fetch_optional(&mut *tx)
    .await?;

    let id = if let Some((id,)) = inserted {
        sqlx::query(
            r#"INSERT INTO points_ledger (account_id, kind, delta, ref_id, created_at_ms)
               VALUES ($1, 'SIGNUP', $2, NULL, $3)"#,
        )
        .bind(id)
        .bind(SIGNUP_BONUS)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        id
    } else {
        let (id,): (i64,) = sqlx::query_as("SELECT id FROM accounts WHERE external_id = $1")
            .bind(external_id)
            .fetch_one(&mut *tx)
            .await?;
        id
    };
    tx.commit().await?;
    Ok(AccountCtx {
        id,
        external_id: external_id.to_string(),
    })
}
