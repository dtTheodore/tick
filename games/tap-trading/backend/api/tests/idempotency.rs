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

#[tokio::test]
async fn same_request_id_returns_identical_response_and_one_ledger_row() {
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

    let body = json!({
        "client_request_id": "00000000-0000-0000-0000-0000000000ff",
        "asset": "BTC", "strike_lo": 49999.5, "strike_hi": 50000.5,
        "t_open_ms": 1_748_345_670_000_i64, "t_close_ms": 1_748_345_675_000_i64,
        "stake_points": 100, "client_multiplier": 1.0,
        "oracle_seq_at_tap": 12345, "oracle_run_id_at_tap": 999
    });

    async fn fire(app: &TestApp, body: &serde_json::Value) -> (StatusCode, serde_json::Value) {
        let r = app
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/positions")
                    .header("x-account-id", "idem-tester")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let s = r.status();
        let bytes = to_bytes(r.into_body(), 8 * 1024).await.unwrap();
        (s, serde_json::from_slice(&bytes).unwrap())
    }

    let (s1, b1) = fire(&app, &body).await;
    assert_eq!(s1, StatusCode::CREATED);
    let (s2, b2) = fire(&app, &body).await;
    assert_eq!(s2, StatusCode::OK);

    assert_eq!(b1["position_id"], b2["position_id"]);
    assert_eq!(b1["multiplier_at_tap"], b2["multiplier_at_tap"]);
    assert_eq!(b1["t_close_ms"], b2["t_close_ms"]);

    let (positions,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM positions")
        .fetch_one(&app.pg)
        .await
        .unwrap();
    assert_eq!(positions, 1);
    let (ledger,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM points_ledger WHERE kind = 'TAP_STAKE'")
            .fetch_one(&app.pg)
            .await
            .unwrap();
    assert_eq!(ledger, 1);
    let (balance,): (i64,) = sqlx::query_as("SELECT balance FROM accounts WHERE external_id = $1")
        .bind("idem-tester")
        .fetch_one(&app.pg)
        .await
        .unwrap();
    assert_eq!(balance, 9_900); // debited once, not twice.
}
