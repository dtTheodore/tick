mod common;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use common::TestApp;
use serde_json::json;
use std::sync::Arc;
use tap_trading_api::aggregator_client::AggregatorClient;
use tower::util::ServiceExt;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const PINNED_NOW: i64 = 1_748_345_673_000;

fn base_request() -> serde_json::Value {
    json!({
        "client_request_id": "00000000-0000-0000-0000-000000000001",
        "asset": "BTC",
        "strike_lo": 49_999.5,
        "strike_hi": 50_000.5,
        "t_open_ms": 1_748_345_670_000_i64,
        "t_close_ms": 1_748_345_675_000_i64,
        "stake_points": 100,
        "client_multiplier": 1.0,
        "oracle_seq_at_tap": 12345,
        "oracle_run_id_at_tap": 999,
    })
}

async fn post(
    app: &TestApp,
    account: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/positions")
                .header("x-account-id", account)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 8 * 1024).await.unwrap();
    let v = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, v)
}

#[tokio::test]
async fn invalid_stake_returns_400() {
    let app = TestApp::start().await;
    app.state.clock.set(PINNED_NOW);
    let mut req = base_request();
    req["stake_points"] = json!(77);
    let (status, body) = post(&app, "alice", req).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_stake");
}

#[tokio::test]
async fn unknown_asset_returns_400() {
    let app = TestApp::start().await;
    app.state.clock.set(PINNED_NOW);
    let mut req = base_request();
    req["asset"] = json!("DOGE");
    let (status, body) = post(&app, "bob", req).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "unknown_asset");
}

#[tokio::test]
async fn lock_window_returns_400() {
    // now = t_close - 500ms → inside the 1s lock window.
    let app = TestApp::start().await;
    app.state.clock.set(1_748_345_674_500);
    let (status, body) = post(&app, "carol", base_request()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "lock_window");
}

#[tokio::test]
async fn stale_quote_returns_422() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(410))
        .mount(&mock)
        .await;
    let uri = mock.uri();
    let app = TestApp::start_with(|s| {
        s.aggregator = Arc::new(AggregatorClient::new(uri));
        s.clock.set(PINNED_NOW);
    })
    .await;
    let (status, body) = post(&app, "dave", base_request()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "stale_quote");
}

#[tokio::test]
async fn drift_exceeded_returns_422_with_server_mult() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "asset": "BTC", "run_id": 999, "seq": 12345,
            "ts_ms": PINNED_NOW,
            "mid": 50000.0, "vol_annualized": 0.80, "source_count": 3
        })))
        .mount(&mock)
        .await;
    let uri = mock.uri();
    let app = TestApp::start_with(|s| {
        s.aggregator = Arc::new(AggregatorClient::new(uri));
        s.clock.set(PINNED_NOW);
    })
    .await;
    // Client claims 5.0 — server recompute is the in-band ~1.0× → drift > 3%.
    let mut req = base_request();
    req["client_multiplier"] = json!(5.0);
    let (status, body) = post(&app, "eve", req).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "drift_exceeded");
    assert!(body["server_multiplier"].as_f64().is_some());
}

#[tokio::test]
async fn insufficient_balance_returns_422() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "asset": "BTC", "run_id": 999, "seq": 12345,
            "ts_ms": PINNED_NOW,
            "mid": 50000.0, "vol_annualized": 0.80, "source_count": 3
        })))
        .mount(&mock)
        .await;
    let uri = mock.uri();
    let app = TestApp::start_with(|s| {
        s.aggregator = Arc::new(AggregatorClient::new(uri));
        s.clock.set(PINNED_NOW);
    })
    .await;

    // Create account via first tap (returns CREATED), then drain balance to 50.
    let _ = post(&app, "frank", base_request()).await;
    sqlx::query("UPDATE accounts SET balance = 50 WHERE external_id = $1")
        .bind("frank")
        .execute(&app.pg)
        .await
        .unwrap();

    let mut req = base_request();
    req["stake_points"] = json!(100);
    req["client_request_id"] = json!("00000000-0000-0000-0000-000000000002");
    let (status, body) = post(&app, "frank", req).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "insufficient_balance");
}

#[tokio::test]
async fn malformed_body_returns_400() {
    let app = TestApp::start().await;
    app.state.clock.set(PINNED_NOW);
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/positions")
                .header("x-account-id", "garbage")
                .header("content-type", "application/json")
                .body(Body::from("{bad json"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(resp.status().is_client_error());
}
