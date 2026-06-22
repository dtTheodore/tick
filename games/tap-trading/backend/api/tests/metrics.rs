//! GET /metrics — Prometheus text exposition smoke test.

mod common;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use std::sync::Arc;
use tap_trading_api::aggregator_client::AggregatorClient;
use tower::ServiceExt;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn metrics_endpoint_returns_prometheus_text() {
    let app = common::TestApp::start().await;

    let resp = app
        .router
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let text = std::str::from_utf8(&body).unwrap();
    // # HELP and # TYPE lines are always emitted for registered metric families,
    // even before any observations have been recorded.
    assert!(
        text.contains("# HELP taps_committed_total"),
        "missing taps_committed_total"
    );
    assert!(
        text.contains("# HELP taps_rejected_total"),
        "missing taps_rejected_total"
    );
    assert!(
        text.contains("# HELP tap_handler_duration_seconds"),
        "missing tap_handler_duration_seconds"
    );
}

#[tokio::test]
async fn committed_counter_excludes_idempotent_replay() {
    // A duplicate request_id returns 200 (replay), not a new commit. The
    // committed counter must move once for the 201, not again for the replay.
    let pinned_now: i64 = 1_748_345_673_000;
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "asset": "BTC", "run_id": 999, "seq": 12345, "ts_ms": pinned_now,
            "mid": 50000.0, "vol_annualized": 0.80, "source_count": 3
        })))
        .mount(&mock)
        .await;
    let uri = mock.uri();
    let app = common::TestApp::start_with(|s| {
        s.aggregator = Arc::new(AggregatorClient::new(uri));
        s.clock.set(pinned_now);
    })
    .await;

    let body = serde_json::json!({
        "client_request_id": "00000000-0000-0000-0000-0000000000ab",
        "asset": "BTC", "strike_lo": 49999.5, "strike_hi": 50000.5,
        "t_open_ms": 1_748_345_670_000_i64, "t_close_ms": 1_748_345_675_000_i64,
        "stake_points": 100, "client_multiplier": 1.0,
        "oracle_seq_at_tap": 12345, "oracle_run_id_at_tap": 999
    })
    .to_string();
    let tap = |b: String| {
        let router = app.router.clone();
        async move {
            router
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/v1/positions")
                        .header("x-account-id", "metric-user")
                        .header("content-type", "application/json")
                        .body(Body::from(b))
                        .unwrap(),
                )
                .await
                .unwrap()
                .status()
        }
    };
    assert_eq!(tap(body.clone()).await, StatusCode::CREATED);
    assert_eq!(tap(body).await, StatusCode::OK);

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let scrape = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let text = std::str::from_utf8(&scrape).unwrap();
    let line = text
        .lines()
        .find(|l| l.starts_with("taps_committed_total{") && l.contains("asset=\"BTC\""))
        .expect("taps_committed_total BTC series");
    let value: f64 = line.rsplit(' ').next().unwrap().parse().unwrap();
    assert_eq!(value, 1.0, "replay double-counted: {line}");
}
