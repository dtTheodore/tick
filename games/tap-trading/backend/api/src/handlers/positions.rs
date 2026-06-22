//! POST /v1/positions — tap commit. ADR-0009 §4.
//!
//! Pipeline steps 3 → 9 land here. Step 1 (rate limit) is a tower layer;
//! step 2 (idempotency) lands in Task 10.

use axum::extract::{Path, State};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use tap_trading_oracle_types::AssetSymbol;
use tap_trading_pricing_engine::{compute_multiplier, Cell, OracleState, PricingConfig};
use uuid::Uuid;

use crate::account_ctx::AccountCtx;
use crate::aggregator_client::ReplayError;
use crate::db::{
    debit_balance, insert_position, insert_tap_stake_ledger, select_balance_for_update,
    InsertPositionInput,
};
use crate::error::ApiError;
use crate::state::AppState;
use crate::validation::{drift_exceeded, parse_asset, validate_cell, validate_stake};

#[derive(Debug, Deserialize)]
pub struct TapRequest {
    pub client_request_id: Uuid,
    pub asset: String,
    pub strike_lo: f64,
    pub strike_hi: f64,
    pub t_open_ms: i64,
    pub t_close_ms: i64,
    pub stake_points: i64,
    pub client_multiplier: f64,
    pub oracle_seq_at_tap: i64,
    pub oracle_run_id_at_tap: i64,
    #[serde(default)]
    pub client_fingerprint: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TapResponse {
    pub position_id: i64,
    pub multiplier_at_tap: f64,
    pub status: String,
    pub t_close_ms: i64,
}

#[derive(Debug, Serialize)]
pub struct PositionResponse {
    pub id: i64,
    pub asset: String,
    pub status: String,
    pub t_open_ms: i64,
    pub t_close_ms: i64,
    pub stake_points: i64,
    pub multiplier_at_tap: f64,
}

fn replay_response(existing: crate::db::ExistingPosition) -> Json<TapResponse> {
    Json(TapResponse {
        position_id: existing.id,
        multiplier_at_tap: existing.multiplier_at_tap,
        status: existing.status,
        t_close_ms: existing.t_close_ms,
    })
}

async fn tap_inner(
    state: &AppState,
    ctx: &AccountCtx,
    req: &TapRequest,
) -> Result<(axum::http::StatusCode, Json<TapResponse>), ApiError> {
    let now = state.clock.now_ms();

    // ADR-0009 §4 step 2 — idempotency pre-flight.
    if let Some(existing) =
        crate::db::find_position_by_request_id(&state.pg, ctx.id, req.client_request_id).await?
    {
        return Ok((axum::http::StatusCode::OK, replay_response(existing)));
    }

    // Step 3 — cell + stake tier validation.
    let asset = parse_asset(&req.asset)?;
    validate_stake(req.stake_points)?;
    validate_cell(
        req.t_open_ms,
        req.t_close_ms,
        req.strike_lo,
        req.strike_hi,
        now,
    )?;
    if req.oracle_seq_at_tap < 0 || req.oracle_run_id_at_tap < 0 {
        return Err(ApiError::InvalidCell);
    }

    // Step 4 — replay quote from aggregator.
    let tick = match state
        .aggregator
        .replay(
            asset,
            req.oracle_seq_at_tap as u64,
            req.oracle_run_id_at_tap as u64,
        )
        .await
    {
        Ok(t) => t,
        Err(ReplayError::Stale) => return Err(ApiError::StaleQuote),
        Err(ReplayError::UnknownAsset) => return Err(ApiError::UnknownAsset),
        Err(_) => return Err(ApiError::StaleQuote),
    };

    // Step 5 — server recompute.
    let cell = Cell {
        asset,
        strike_lo: req.strike_lo,
        strike_hi: req.strike_hi,
        t_open_ms: req.t_open_ms as u64,
        t_close_ms: req.t_close_ms as u64,
    };
    let oracle = OracleState {
        asset,
        spot: tick.mid,
        sigma_annualized: tick.vol_annualized,
        timestamp_ms: tick.ts_ms as u64,
    };
    let server_mult = compute_multiplier(&cell, &oracle, &PricingConfig::default(), now as u64)
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("pricing engine: {e}")))?;
    // Round to the persisted NUMERIC(10,4) precision once, so the 201 response,
    // the stored row, and any later idempotent-replay (which reads the row back)
    // all report the identical locked multiplier.
    let server_mult = (server_mult * 10_000.0).round() / 10_000.0;

    // Step 6 — drift gate.
    if drift_exceeded(server_mult, req.client_multiplier) {
        return Err(ApiError::DriftExceeded {
            server_multiplier: server_mult,
        });
    }

    // Steps 7+8 — balance + atomic commit.
    let mut tx = state.pg.begin().await?;
    let balance = select_balance_for_update(&mut tx, ctx.id).await?;
    if balance < req.stake_points {
        // A concurrent duplicate of this request_id may have committed (and
        // debited) after our pre-flight check but before we took the lock.
        // Release the lock and replay it rather than reporting a false 422.
        tx.rollback().await?;
        if let Some(existing) =
            crate::db::find_position_by_request_id(&state.pg, ctx.id, req.client_request_id).await?
        {
            return Ok((axum::http::StatusCode::OK, replay_response(existing)));
        }
        return Err(ApiError::InsufficientBalance);
    }
    let asset_text = match asset {
        AssetSymbol::Eth => "ETH",
        AssetSymbol::Btc => "BTC",
        AssetSymbol::Sui => "SUI",
    };
    let inserted = insert_position(
        &mut tx,
        &InsertPositionInput {
            account_id: ctx.id,
            asset: asset_text,
            strike_lo: req.strike_lo,
            strike_hi: req.strike_hi,
            t_open_ms: req.t_open_ms,
            t_close_ms: req.t_close_ms,
            stake_points: req.stake_points,
            multiplier_at_tap: server_mult,
            oracle_seq_at_tap: req.oracle_seq_at_tap,
            oracle_run_id_at_tap: req.oracle_run_id_at_tap,
            client_request_id: req.client_request_id,
            client_fingerprint: req.client_fingerprint.as_deref(),
            now_ms: now,
        },
    )
    .await?;
    let position_id = match inserted {
        Some(id) => id,
        None => {
            // Concurrent retry won the INSERT race. Rollback and replay.
            tx.rollback().await?;
            let existing =
                crate::db::find_position_by_request_id(&state.pg, ctx.id, req.client_request_id)
                    .await?
                    .ok_or_else(|| {
                        ApiError::Internal(anyhow::anyhow!("conflict-but-row-missing"))
                    })?;
            return Ok((axum::http::StatusCode::OK, replay_response(existing)));
        }
    };
    insert_tap_stake_ledger(&mut tx, ctx.id, req.stake_points, position_id, now).await?;
    debit_balance(&mut tx, ctx.id, req.stake_points, now).await?;
    // Step 9 — ADR-0009 §5 NOTIFY.
    sqlx::query("SELECT pg_notify('tap_new_position', $1)")
        .bind(position_id.to_string())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    Ok((
        axum::http::StatusCode::CREATED,
        Json(TapResponse {
            position_id,
            multiplier_at_tap: server_mult,
            status: "OPEN".to_string(),
            t_close_ms: req.t_close_ms,
        }),
    ))
}

pub async fn post_position(
    State(state): State<AppState>,
    Extension(ctx): Extension<AccountCtx>,
    Json(req): Json<TapRequest>,
) -> Result<(axum::http::StatusCode, Json<TapResponse>), ApiError> {
    let started = std::time::Instant::now();
    let result = tap_inner(&state, &ctx, &req).await;
    let elapsed = started.elapsed().as_secs_f64();
    match &result {
        Ok((status, _)) => {
            // Only a fresh 201 is a new commit; a 200 is an idempotent replay of
            // an already-counted tap and must not bump the committed counter.
            if *status == axum::http::StatusCode::CREATED {
                state
                    .metrics
                    .taps_committed_total
                    .with_label_values(&[&req.asset])
                    .inc();
            }
            state
                .metrics
                .tap_handler_duration_seconds
                .with_label_values(&["ok"])
                .observe(elapsed);
        }
        Err(e) => {
            let reason = match e {
                ApiError::InvalidStake => "invalid_stake",
                ApiError::UnknownAsset => "unknown_asset",
                ApiError::LockWindow => "lock_window",
                ApiError::InvalidCell => "invalid_cell",
                ApiError::StaleQuote => "stale_quote",
                ApiError::DriftExceeded { .. } => "drift_exceeded",
                ApiError::InsufficientBalance => "insufficient_balance",
                ApiError::RateLimited { .. } => "rate_limited",
                _ => "internal",
            };
            state
                .metrics
                .taps_rejected_total
                .with_label_values(&[reason])
                .inc();
            state
                .metrics
                .tap_handler_duration_seconds
                .with_label_values(&["err"])
                .observe(elapsed);
        }
    }
    result
}

pub async fn get_position_by_id(
    State(state): State<AppState>,
    Extension(ctx): Extension<AccountCtx>,
    Path(position_id): Path<i64>,
) -> Result<Json<PositionResponse>, ApiError> {
    // Fetch without ownership filter first so we can distinguish 403 vs 404.
    type Row = (i64, i64, String, String, i64, i64, i64, String);
    let row: Option<Row> = sqlx::query_as(
        r#"SELECT id, account_id, asset, status, t_open_ms, t_close_ms, stake_points,
                  multiplier_at_tap::TEXT
           FROM positions WHERE id = $1"#,
    )
    .bind(position_id)
    .fetch_optional(&state.pg)
    .await?;
    let (id, acct_id, asset, status, t_open, t_close, stake, mult_str) =
        row.ok_or(ApiError::NotFound)?;
    if acct_id != ctx.id {
        return Err(ApiError::Forbidden);
    }
    Ok(Json(PositionResponse {
        id,
        asset,
        status,
        t_open_ms: t_open,
        t_close_ms: t_close,
        stake_points: stake,
        multiplier_at_tap: mult_str.parse::<f64>().unwrap_or(0.0),
    }))
}
