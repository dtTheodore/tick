//! HTTP + WS router.

use crate::broadcast::Broadcaster;
use crate::constants::{HEALTHZ_FRESHNESS_MS, RING_SIZE, SUPPORTED_ASSETS};
use crate::ring_buffer::{RangeLookup, RingBuffers, RingLookup};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, Query, State,
    },
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use std::sync::Arc;
use tap_trading_oracle_types::AssetSymbol;
use tokio::sync::broadcast::error::RecvError;

#[derive(Clone)]
pub struct AppState {
    pub run_id: u64,
    pub rings: Arc<RingBuffers>,
    pub broadcaster: Broadcaster,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics))
        .route("/latest/:asset", get(get_latest))
        .route("/ring/:asset/range", get(get_ring_range))
        .route("/ring/:asset/:seq", get(get_ring))
        .route("/stream", get(ws_upgrade))
        .with_state(state)
}

/// ADR-0008 §5: 200 only if every supported asset has a fresh tick with
/// `source_count >= 2`; 503 otherwise. "Fresh" = newest tick is younger than
/// `HEALTHZ_FRESHNESS_MS` (a degraded asset stops emitting, so its newest tick
/// ages out and trips this).
async fn healthz(State(state): State<AppState>) -> Response {
    let now = now_ms();
    for asset in SUPPORTED_ASSETS {
        let healthy = match state.rings.newest(asset) {
            Some(tick) => tick.source_count >= 2 && now - tick.ts_ms <= HEALTHZ_FRESHNESS_MS,
            None => false,
        };
        if !healthy {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                format!("{asset:?} below 2 fresh sources"),
            )
                .into_response();
        }
    }
    (StatusCode::OK, "ok").into_response()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

async fn metrics() -> &'static str {
    "# tap_trading_oracle_aggregator metrics stub\n"
}

#[derive(Debug, Deserialize)]
struct RingQuery {
    run_id: Option<u64>,
}

async fn get_ring(
    State(state): State<AppState>,
    Path((asset, seq)): Path<(String, u64)>,
    Query(query): Query<RingQuery>,
) -> Response {
    let asset = match parse_asset(&asset) {
        Some(a) => a,
        None => return (StatusCode::NOT_FOUND, "unknown asset").into_response(),
    };
    let Some(run_id) = query.run_id else {
        return (StatusCode::CONFLICT, "missing run_id").into_response();
    };
    // Early-out on a stale run_id (409). Necessary here, not only in the ring,
    // because an asset with no ticks yet has no stored run_id for the ring to
    // compare against; the ring's own run_id check is defense-in-depth.
    if run_id != state.run_id {
        return (StatusCode::CONFLICT, "stale run_id").into_response();
    }
    match state.rings.get(asset, run_id, seq) {
        RingLookup::Hit(tick) => Json(tick).into_response(),
        RingLookup::Gone => (StatusCode::GONE, "seq rotated").into_response(),
        RingLookup::Conflict => (StatusCode::CONFLICT, "stale run_id").into_response(),
    }
}

/// `GET /latest/:asset` — the newest tick for an asset (current `run_id`, `seq`,
/// `mid`, `vol`). A convenience read for clients and the e2e harness that need a
/// current `(run_id, seq)` anchor without subscribing to the WS stream.
async fn get_latest(State(state): State<AppState>, Path(asset): Path<String>) -> Response {
    let Some(asset) = parse_asset(&asset) else {
        return (StatusCode::NOT_FOUND, "unknown asset").into_response();
    };
    match state.rings.newest(asset) {
        Some(tick) => Json(tick).into_response(),
        None => (StatusCode::SERVICE_UNAVAILABLE, "no tick yet").into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct RingRangeQuery {
    run_id: u64,
    from_seq: u64,
    to_seq: u64,
}

/// `GET /ring/:asset/range?run_id&from_seq&to_seq` — the contiguous tick path
/// over a window, for Walrus proof evidence (ADR-0011 §6). One call replaces the
/// N single-seq lookups the worker would otherwise make per settlement.
async fn get_ring_range(
    State(state): State<AppState>,
    Path(asset): Path<String>,
    Query(query): Query<RingRangeQuery>,
) -> Response {
    let asset = match parse_asset(&asset) {
        Some(a) => a,
        None => return (StatusCode::NOT_FOUND, "unknown asset").into_response(),
    };
    if query.to_seq < query.from_seq {
        return (StatusCode::BAD_REQUEST, "to_seq < from_seq").into_response();
    }
    // Bound the span to the ring capacity so a pathological request can't ask us
    // to scan an unbounded range.
    if query.to_seq - query.from_seq >= RING_SIZE as u64 {
        return (StatusCode::BAD_REQUEST, "span exceeds ring capacity").into_response();
    }
    if query.run_id != state.run_id {
        return (StatusCode::CONFLICT, "stale run_id").into_response();
    }
    match state.rings.range(asset, query.run_id, query.from_seq, query.to_seq) {
        RangeLookup::Hit(ticks) => Json(ticks).into_response(),
        RangeLookup::Gone => (StatusCode::GONE, "from_seq rotated out").into_response(),
        RangeLookup::Conflict => (StatusCode::CONFLICT, "stale run_id").into_response(),
    }
}

fn parse_asset(raw: &str) -> Option<AssetSymbol> {
    // Derived from SUPPORTED_ASSETS so adding an asset there is the single edit
    // — keeps the ring API and healthz on one source of truth.
    let want = raw.to_ascii_uppercase();
    SUPPORTED_ASSETS.into_iter().find(|a| a.ticker() == want)
}

async fn ws_upgrade(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| ws_session(socket, state))
}

async fn ws_session(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let mut rx = state.broadcaster.sender().subscribe();

    // Reader task: ignore client frames except Close (per ADR-0008 §2 the
    // client never sends app frames; only subscribe-by-default).
    let reader = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            if matches!(msg, Message::Close(_)) {
                break;
            }
        }
    });

    // Writer loop: pump broadcast → WS.
    while let Ok(()) = async {
        match rx.recv().await {
            Ok(msg) => {
                let json = match serde_json::to_string(&msg) {
                    Ok(j) => j,
                    Err(e) => {
                        tracing::error!(error = %e, "OracleMessage serialize failed");
                        return Err(());
                    }
                };
                sender.send(Message::Text(json)).await.map_err(|_| ())?;
                Ok(())
            }
            Err(RecvError::Lagged(n)) => {
                tracing::warn!(skipped = n, "ws client lagged; closing");
                Err(())
            }
            Err(RecvError::Closed) => Err(()),
        }
    }
    .await
    {}

    reader.abort();
}
