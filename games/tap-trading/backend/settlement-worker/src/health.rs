//! HTTP surface — `/healthz` and `/metrics`.
//!
//! `/healthz`: 200 iff this process is the leader AND a tick has arrived in
//! the last 2 s. 503 otherwise.
//!
//! `/metrics`: hand-rolled Prometheus text exposition.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};

use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Router,
};

#[derive(Default)]
pub struct Metrics {
    pub positions_settled_win: AtomicU64,
    pub positions_settled_loss: AtomicU64,
    pub positions_voided: AtomicU64,
    pub tick_processing_ms_le_1: AtomicU64,
    pub tick_processing_ms_le_5: AtomicU64,
    pub tick_processing_ms_le_25: AtomicU64,
    pub tick_processing_ms_le_inf: AtomicU64,
}

impl Metrics {
    pub fn observe_tick_ms(&self, ms: u64) {
        if ms <= 1  { self.tick_processing_ms_le_1.fetch_add(1, Ordering::Relaxed); }
        if ms <= 5  { self.tick_processing_ms_le_5.fetch_add(1, Ordering::Relaxed); }
        if ms <= 25 { self.tick_processing_ms_le_25.fetch_add(1, Ordering::Relaxed); }
        self.tick_processing_ms_le_inf.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Clone)]
pub struct HealthState {
    pub is_leader: Arc<AtomicBool>,
    pub last_tick_received_ms: Arc<AtomicI64>,
    pub metrics: Arc<Metrics>,
}

const STALE_TICK_THRESHOLD_MS: i64 = 2_000;

pub fn router(state: HealthState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics_handler))
        .with_state(state)
}

async fn healthz(State(state): State<HealthState>) -> impl IntoResponse {
    if !state.is_leader.load(Ordering::Relaxed) {
        return (StatusCode::SERVICE_UNAVAILABLE, "not leader").into_response();
    }
    let now_ms = chrono::Utc::now().timestamp_millis();
    let last = state.last_tick_received_ms.load(Ordering::Relaxed);
    if now_ms - last > STALE_TICK_THRESHOLD_MS {
        return (StatusCode::SERVICE_UNAVAILABLE, "tick stream stale").into_response();
    }
    (StatusCode::OK, "ok").into_response()
}

async fn metrics_handler(State(state): State<HealthState>) -> impl IntoResponse {
    let m = &state.metrics;
    let win  = m.positions_settled_win.load(Ordering::Relaxed);
    let loss = m.positions_settled_loss.load(Ordering::Relaxed);
    let void = m.positions_voided.load(Ordering::Relaxed);
    let b1   = m.tick_processing_ms_le_1.load(Ordering::Relaxed);
    let b5   = m.tick_processing_ms_le_5.load(Ordering::Relaxed);
    let b25  = m.tick_processing_ms_le_25.load(Ordering::Relaxed);
    let binf = m.tick_processing_ms_le_inf.load(Ordering::Relaxed);

    let body = format!(
        "# HELP positions_settled_total Position settlements by outcome.\n\
         # TYPE positions_settled_total counter\n\
         positions_settled_total{{outcome=\"W\"}} {win}\n\
         positions_settled_total{{outcome=\"L\"}} {loss}\n\
         positions_settled_total{{outcome=\"V\"}} {void}\n\
         # HELP tick_processing_duration_ms Tick to settle latency, ms.\n\
         # TYPE tick_processing_duration_ms histogram\n\
         tick_processing_duration_ms_bucket{{le=\"1\"}} {b1}\n\
         tick_processing_duration_ms_bucket{{le=\"5\"}} {b5}\n\
         tick_processing_duration_ms_bucket{{le=\"25\"}} {b25}\n\
         tick_processing_duration_ms_bucket{{le=\"+Inf\"}} {binf}\n\
         tick_processing_duration_ms_count {binf}\n",
    );
    (StatusCode::OK, body).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn healthz_503_when_not_leader() {
        let state = HealthState {
            is_leader: Arc::new(AtomicBool::new(false)),
            last_tick_received_ms: Arc::new(AtomicI64::new(chrono::Utc::now().timestamp_millis())),
            metrics: Arc::new(Metrics::default()),
        };
        let resp = healthz(State(state)).await.into_response();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn healthz_503_when_tick_stale() {
        let state = HealthState {
            is_leader: Arc::new(AtomicBool::new(true)),
            last_tick_received_ms: Arc::new(AtomicI64::new(0)),
            metrics: Arc::new(Metrics::default()),
        };
        let resp = healthz(State(state)).await.into_response();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn healthz_200_when_leader_and_fresh() {
        let state = HealthState {
            is_leader: Arc::new(AtomicBool::new(true)),
            last_tick_received_ms: Arc::new(AtomicI64::new(chrono::Utc::now().timestamp_millis())),
            metrics: Arc::new(Metrics::default()),
        };
        let resp = healthz(State(state)).await.into_response();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
