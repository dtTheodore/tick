mod common;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use common::TestApp;
use serde_json::json;
use std::sync::Arc;
use tap_trading_api::aggregator_client::AggregatorClient;
use tower::util::ServiceExt;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn happy_path_debits_balance_writes_position() {
    // Pin time to a 5s-aligned boundary, 3s into the window (2s before close,
    // outside the 1s lock window).
    let t_open_ms: i64 = 1_748_345_670_000;
    let t_close_ms: i64 = t_open_ms + 5_000;
    let pinned_now: i64 = t_open_ms + 3_000;

    // Narrow band around spot 50000 BTC, in-play (τ_open=0) and in-band → p=1 →
    // (1−0.03)=0.97, floored to the flat 1.0× minimum (v2 pricing).
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex(r"^/ring/BTC/12345$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "asset": "BTC", "run_id": 999, "seq": 12345,
            "ts_ms": pinned_now, "mid": 50_000.0, "vol_annualized": 0.80,
            "source_count": 3
        })))
        .mount(&mock)
        .await;
    let aggregator_uri = mock.uri();

    let app = TestApp::start_with(|s| {
        s.aggregator = Arc::new(AggregatorClient::new(aggregator_uri));
        s.clock.set(pinned_now);
    })
    .await;

    // client_multiplier = 1.0 (matches the in-band flat floor exactly).
    let req = json!({
        "client_request_id": "00000000-0000-0000-0000-0000000000aa",
        "asset": "BTC",
        "strike_lo": 49_999.5,
        "strike_hi": 50_000.5,
        "t_open_ms": t_open_ms,
        "t_close_ms": t_close_ms,
        "stake_points": 100,
        "client_multiplier": 1.0,
        "oracle_seq_at_tap": 12345,
        "oracle_run_id_at_tap": 999,
        "client_fingerprint": "test-fp"
    });

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/positions")
                .header("x-account-id", "happy-tester")
                .header("content-type", "application/json")
                .body(Body::from(req.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = to_bytes(resp.into_body(), 8 * 1024).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(v["position_id"].as_i64().unwrap() > 0);
    assert_eq!(v["status"], "OPEN");
    assert_eq!(v["t_close_ms"], t_close_ms);
    let mult = v["multiplier_at_tap"].as_f64().unwrap();
    assert!((0.99..=1.01).contains(&mult), "unexpected mult={mult}");

    // Balance debited by 100.
    let (balance,): (i64,) = sqlx::query_as("SELECT balance FROM accounts WHERE external_id = $1")
        .bind("happy-tester")
        .fetch_one(&app.pg)
        .await
        .unwrap();
    assert_eq!(balance, 9_900);

    // One position row, status OPEN.
    let (count, status): (i64, String) =
        sqlx::query_as("SELECT COUNT(*), MAX(status) FROM positions")
            .fetch_one(&app.pg)
            .await
            .unwrap();
    assert_eq!(count, 1);
    assert_eq!(status, "OPEN");

    // One TAP_STAKE ledger row with delta = -100.
    let (delta,): (i64,) =
        sqlx::query_as("SELECT delta FROM points_ledger WHERE kind = 'TAP_STAKE'")
            .fetch_one(&app.pg)
            .await
            .unwrap();
    assert_eq!(delta, -100);
}
