//! Canonical API error type. All handlers return `Result<T, ApiError>`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("missing_account_id")]
    MissingAccountId,
    #[error("invalid_account_id")]
    InvalidAccountId,
    #[error("invalid_stake")]
    InvalidStake,
    #[error("unknown_asset")]
    UnknownAsset,
    #[error("lock_window")]
    LockWindow,
    #[error("invalid_cell")]
    InvalidCell,
    #[error("stale_quote")]
    StaleQuote,
    #[error("drift_exceeded")]
    DriftExceeded { server_multiplier: f64 },
    #[error("insufficient_balance")]
    InsufficientBalance,
    #[error("rate_limited")]
    RateLimited { retry_after_secs: u64 },
    #[error("not_found")]
    NotFound,
    #[error("forbidden")]
    Forbidden,
    #[error("deposit_unverifiable")]
    DepositUnverifiable,
    #[error("no_withdraw_address")]
    NoWithdrawAddress,
    #[error("withdraw_failed")]
    WithdrawFailed,
    #[error("internal")]
    Internal(#[from] anyhow::Error),
    #[error("db")]
    Db(#[from] sqlx::Error),
    #[error("redis")]
    Redis(#[from] redis::RedisError),
}

#[derive(Serialize)]
struct ErrBody<'a> {
    error: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    server_multiplier: Option<f64>,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, body, retry_after) = match &self {
            ApiError::MissingAccountId => (StatusCode::UNAUTHORIZED, "missing_account_id", None),
            ApiError::InvalidAccountId => (StatusCode::BAD_REQUEST, "invalid_account_id", None),
            ApiError::InvalidStake => (StatusCode::BAD_REQUEST, "invalid_stake", None),
            ApiError::UnknownAsset => (StatusCode::BAD_REQUEST, "unknown_asset", None),
            ApiError::LockWindow => (StatusCode::BAD_REQUEST, "lock_window", None),
            ApiError::InvalidCell => (StatusCode::BAD_REQUEST, "invalid_cell", None),
            ApiError::StaleQuote => (StatusCode::UNPROCESSABLE_ENTITY, "stale_quote", None),
            ApiError::DriftExceeded { .. } => {
                (StatusCode::UNPROCESSABLE_ENTITY, "drift_exceeded", None)
            }
            ApiError::InsufficientBalance => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "insufficient_balance",
                None,
            ),
            ApiError::RateLimited { retry_after_secs } => (
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                Some(*retry_after_secs),
            ),
            ApiError::NotFound => (StatusCode::NOT_FOUND, "not_found", None),
            ApiError::Forbidden => (StatusCode::FORBIDDEN, "forbidden", None),
            ApiError::DepositUnverifiable => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "deposit_unverifiable",
                None,
            ),
            ApiError::NoWithdrawAddress => (StatusCode::BAD_REQUEST, "no_withdraw_address", None),
            ApiError::WithdrawFailed => (StatusCode::BAD_GATEWAY, "withdraw_failed", None),
            ApiError::Internal(_) | ApiError::Db(_) | ApiError::Redis(_) => {
                tracing::error!(error = ?self, "internal error");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal", None)
            }
        };
        let server_multiplier = match &self {
            ApiError::DriftExceeded { server_multiplier } => Some(*server_multiplier),
            _ => None,
        };
        let mut resp = (
            status,
            Json(ErrBody {
                error: body,
                server_multiplier,
            }),
        )
            .into_response();
        if let Some(secs) = retry_after {
            resp.headers_mut()
                .insert("retry-after", secs.to_string().parse().unwrap());
        }
        resp
    }
}
