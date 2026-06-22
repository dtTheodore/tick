//! Liveness + metrics.

use axum::extract::State;
use axum::{Extension, Json};
use prometheus::Encoder;
use serde_json::json;

use crate::account_ctx::AccountCtx;
use crate::state::AppState;

pub async fn healthz() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok" }))
}

pub async fn metrics(State(state): State<AppState>) -> String {
    let encoder = prometheus::TextEncoder::new();
    let mf = state.metrics.registry.gather();
    let mut buf = Vec::new();
    encoder.encode(&mf, &mut buf).ok();
    String::from_utf8(buf).unwrap_or_default()
}

/// Debug route used by middleware integration tests.
pub async fn ping(Extension(_ctx): Extension<AccountCtx>) -> &'static str {
    "pong"
}
