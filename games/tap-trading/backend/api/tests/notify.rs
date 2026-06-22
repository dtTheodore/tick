mod common;

use axum::body::Body;
use axum::http::Request;
use common::TestApp;
use serde_json::json;
use sqlx::postgres::PgListener;
use std::sync::Arc;
use std::time::Duration;
use tap_trading_api::aggregator_client::AggregatorClient;
use tokio::time::timeout;
use tower::util::ServiceExt;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn notify_emitted_on_successful_commit() {
    let pinned_now: i64 = 1_748_345_673_000;
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "asset": "BTC", "run_id": 999, "seq": 12345, "ts_ms": pinned_now,
            "mid": 50000.0, "vol_annualized": 0.80, "source_count": 3
        })))
        .mount(&mock)
        .await;
    let uri = mock.uri();
    let app = TestApp::start_with(|s| {
        s.aggregator = Arc::new(AggregatorClient::new(uri));
        s.clock.set(pinned_now);
    })
    .await;

    let mut listener = PgListener::connect_with(&app.pg).await.unwrap();
    listener.listen("tap_new_position").await.unwrap();

    let body = json!({
        "client_request_id": "00000000-0000-0000-0000-0000000000bb",
        "asset": "BTC", "strike_lo": 49999.5, "strike_hi": 50000.5,
        "t_open_ms": 1_748_345_670_000_i64, "t_close_ms": 1_748_345_675_000_i64,
        "stake_points": 100, "client_multiplier": 1.0,
        "oracle_seq_at_tap": 12345, "oracle_run_id_at_tap": 999
    });
    let _ = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/positions")
                .header("x-account-id", "notify-tester")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    let notif = timeout(Duration::from_secs(2), listener.recv())
        .await
        .expect("NOTIFY did not arrive within 2s")
        .unwrap();
    assert_eq!(notif.channel(), "tap_new_position");
    let payload = notif.payload();
    let id: i64 = payload.parse().expect("payload is a decimal i64");
    assert!(id > 0);
}
