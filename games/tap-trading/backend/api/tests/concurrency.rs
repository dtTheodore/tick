mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::TestApp;
use serde_json::json;
use std::sync::Arc;
use tap_trading_api::aggregator_client::AggregatorClient;
use tokio::task::JoinSet;
use tower::util::ServiceExt;
use uuid::Uuid;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn n_of_100_concurrent_taps_succeed() {
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

    // Lazy-create the account, then set balance to exactly 5 · 100 = 500.
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/me")
                .header("x-account-id", "concurrent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    sqlx::query("UPDATE accounts SET balance = 500 WHERE external_id = $1")
        .bind("concurrent")
        .execute(&app.pg)
        .await
        .unwrap();

    // Use the rate-limit-free router so we measure only the balance gate.
    let router = tap_trading_api::router_without_rate_limit(app.state.clone());

    let mut joins = JoinSet::new();
    for _ in 0..100 {
        let r = router.clone();
        let rid = Uuid::new_v4();
        joins.spawn(async move {
            let body = json!({
                "client_request_id": rid,
                "asset": "BTC",
                "strike_lo": 49999.5,
                "strike_hi": 50000.5,
                "t_open_ms": 1_748_345_670_000_i64,
                "t_close_ms": 1_748_345_675_000_i64,
                "stake_points": 100,
                "client_multiplier": 1.0,
                "oracle_seq_at_tap": 12345,
                "oracle_run_id_at_tap": 999
            });
            r.oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/positions")
                    .header("x-account-id", "concurrent")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
        });
    }

    let mut ok = 0;
    let mut insuf = 0;
    let mut other = 0;
    while let Some(r) = joins.join_next().await {
        match r.unwrap() {
            StatusCode::CREATED => ok += 1,
            StatusCode::UNPROCESSABLE_ENTITY => insuf += 1,
            s => {
                eprintln!("unexpected status: {s}");
                other += 1;
            }
        }
    }
    assert_eq!(ok, 5, "expected exactly 5 successful taps");
    assert_eq!(
        insuf, 95,
        "expected exactly 95 insufficient_balance rejections"
    );
    assert_eq!(other, 0, "unexpected status codes");

    let (balance,): (i64,) = sqlx::query_as("SELECT balance FROM accounts WHERE external_id = $1")
        .bind("concurrent")
        .fetch_one(&app.pg)
        .await
        .unwrap();
    assert_eq!(balance, 0);

    let (ledger_count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM points_ledger WHERE kind = 'TAP_STAKE'")
            .fetch_one(&app.pg)
            .await
            .unwrap();
    assert_eq!(ledger_count, 5);
}
