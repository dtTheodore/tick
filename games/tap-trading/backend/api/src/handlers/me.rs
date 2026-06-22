//! `GET /v1/me` and `GET /v1/me/history`. ADR-0009 §1 supplies the `AccountCtx`.

use axum::extract::{Query, State};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::account_ctx::AccountCtx;
use crate::error::ApiError;
use crate::state::AppState;

#[derive(Debug, Serialize, FromRow)]
pub struct MeResponse {
    pub account_id: i64,
    pub external_id: String,
    pub balance: i64,
    pub lifetime_points_won: i64,
    pub tier: i16,
    pub current_streak: i32,
}

pub async fn get_me(
    State(state): State<AppState>,
    Extension(ctx): Extension<AccountCtx>,
) -> Result<Json<MeResponse>, ApiError> {
    let row = sqlx::query_as::<_, (i64, i64, i16)>(
        "SELECT balance, lifetime_points_won, tier FROM accounts WHERE id = $1",
    )
    .bind(ctx.id)
    .fetch_one(&state.pg)
    .await?;
    let streak: Option<(i32,)> =
        sqlx::query_as("SELECT current_streak FROM streaks WHERE account_id = $1")
            .bind(ctx.id)
            .fetch_optional(&state.pg)
            .await?;
    Ok(Json(MeResponse {
        account_id: ctx.id,
        external_id: ctx.external_id,
        balance: row.0,
        lifetime_points_won: row.1,
        tier: row.2,
        current_streak: streak.map(|s| s.0).unwrap_or(0),
    }))
}

#[derive(Debug, Deserialize)]
pub struct HistoryQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
    /// Opaque keyset cursor: the previous page's last `position_id`. Clients
    /// echo back `next_cursor`; they should not construct it themselves.
    #[serde(default)]
    pub cursor: Option<i64>,
}

fn default_limit() -> i64 {
    50
}

#[derive(Debug, Serialize)]
pub struct HistorySettlement {
    pub outcome: String,
    pub points_delta: i64,
    pub settled_at_ms: i64,
}

#[derive(Debug, Serialize)]
pub struct HistoryItem {
    pub position_id: i64,
    pub asset: String,
    // Numeric fields are JSON numbers across every endpoint that returns them
    // (TapResponse, PositionResponse, here). The DB stores `NUMERIC`; we read
    // via a `::TEXT` cast to avoid a `bigdecimal` dep, then parse to `f64`.
    pub strike_lo: f64,
    pub strike_hi: f64,
    pub t_open_ms: i64,
    pub t_close_ms: i64,
    pub stake_points: i64,
    pub multiplier_at_tap: f64,
    pub status: String,
    pub created_at_ms: i64,
    pub settlement: Option<HistorySettlement>,
    // Provability surface for the client-side "verify this tap" flow. Every
    // settlement publishes a self-contained Walrus proof, batched many-per-blob
    // by the flusher; the UI shows the verify affordance when `proof_status ==
    // "published"`, fetches the batch blob by `walrus_blob_id`, and extracts this
    // tap's entry at `proof_index`.
    pub walrus_blob_id: Option<String>,
    pub proof_status: Option<String>,
    pub proof_index: Option<i32>,
}

#[derive(Debug, Serialize)]
pub struct HistoryResponse {
    pub positions: Vec<HistoryItem>,
    pub next_cursor: Option<i64>,
}

// Avoid complex type alias — use a newtype instead.
type HistoryRow = (
    i64,
    String,
    String,
    String,
    i64,
    i64,
    i64,
    String,
    String,
    i64,
    Option<String>,
    Option<i64>,
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<i32>,
);

pub async fn get_history(
    State(state): State<AppState>,
    Extension(ctx): Extension<AccountCtx>,
    Query(q): Query<HistoryQuery>,
) -> Result<Json<HistoryResponse>, ApiError> {
    let limit = q.limit.clamp(1, 200);
    let cursor = q.cursor.unwrap_or(i64::MAX);
    // Keyset by `id`, not `created_at_ms`: `id` is unique and monotonic with
    // insertion, so a strict `<` can never drop rows that share a millisecond
    // (a burst of taps yields several rows with the same `created_at_ms`).
    let rows: Vec<HistoryRow> = sqlx::query_as(
        r#"SELECT p.id, p.asset,
                  p.strike_lo::TEXT, p.strike_hi::TEXT,
                  p.t_open_ms, p.t_close_ms, p.stake_points,
                  p.multiplier_at_tap::TEXT, p.status, p.created_at_ms,
                  s.outcome, s.points_delta, s.settled_at_ms,
                  s.walrus_blob_id, s.proof_status, s.proof_index
           FROM positions p
           LEFT JOIN settlements s ON s.position_id = p.id
           WHERE p.account_id = $1 AND p.id < $2
           ORDER BY p.id DESC
           LIMIT $3"#,
    )
    .bind(ctx.id)
    .bind(cursor)
    .bind(limit + 1)
    .fetch_all(&state.pg)
    .await?;

    let has_more = rows.len() as i64 > limit;
    let items: Vec<HistoryItem> = rows
        .into_iter()
        .take(limit as usize)
        .map(|r| HistoryItem {
            position_id: r.0,
            asset: r.1,
            strike_lo: r.2.parse::<f64>().unwrap_or(0.0),
            strike_hi: r.3.parse::<f64>().unwrap_or(0.0),
            t_open_ms: r.4,
            t_close_ms: r.5,
            stake_points: r.6,
            multiplier_at_tap: r.7.parse::<f64>().unwrap_or(0.0),
            status: r.8,
            created_at_ms: r.9,
            settlement: match (r.10, r.11, r.12) {
                (Some(o), Some(d), Some(s)) => Some(HistorySettlement {
                    outcome: o,
                    points_delta: d,
                    settled_at_ms: s,
                }),
                _ => None,
            },
            walrus_blob_id: r.13,
            proof_status: r.14,
            proof_index: r.15,
        })
        .collect();
    let next_cursor = if has_more {
        items.last().map(|i| i.position_id)
    } else {
        None
    };
    Ok(Json(HistoryResponse {
        positions: items,
        next_cursor,
    }))
}
