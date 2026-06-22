mod common;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use common::TestApp;
use serde_json::json;
use std::sync::Arc;
use tap_trading_api::aggregator_client::AggregatorClient;
use tower::util::ServiceExt;
use uuid::Uuid;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn tap_once(app: &TestApp, account: &str, rid: Uuid, t_open_ms: i64) {
    let body = json!({
        "client_request_id": rid,
        "asset": "BTC", "strike_lo": 49999.5, "strike_hi": 50000.5,
        "t_open_ms": t_open_ms, "t_close_ms": t_open_ms + 5_000,
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
                .header("x-account-id", account)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn history_returns_open_positions() {
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

    tap_once(&app, "histuser", Uuid::from_u128(1), 1_748_345_670_000).await;

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/me/history?limit=10")
                .header("x-account-id", "histuser")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 16 * 1024).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["positions"].as_array().unwrap().len(), 1);
    assert_eq!(v["positions"][0]["status"], "OPEN");
    assert!(v["positions"][0]["settlement"].is_null());
}

#[tokio::test]
async fn position_by_id_forbidden_for_other_account() {
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

    tap_once(&app, "owner", Uuid::from_u128(2), 1_748_345_670_000).await;
    let (pid,): (i64,) = sqlx::query_as("SELECT id FROM positions LIMIT 1")
        .fetch_one(&app.pg)
        .await
        .unwrap();

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/v1/positions/{pid}"))
                .header("x-account-id", "intruder")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn pagination_keeps_rows_that_share_a_millisecond() {
    // Two taps under one pinned clock land with identical `created_at_ms`.
    // A `created_at_ms`-keyed cursor with strict `<` would drop the second on
    // page two; the `id`-keyed cursor must surface both.
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

    tap_once(&app, "pager", Uuid::from_u128(10), 1_748_345_670_000).await;
    tap_once(&app, "pager", Uuid::from_u128(11), 1_748_345_670_000).await;

    async fn page(app: &TestApp, cursor: Option<i64>) -> serde_json::Value {
        let uri = match cursor {
            Some(c) => format!("/v1/me/history?limit=1&cursor={c}"),
            None => "/v1/me/history?limit=1".to_string(),
        };
        let resp = app
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header("x-account-id", "pager")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(resp.into_body(), 16 * 1024).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    let p1 = page(&app, None).await;
    assert_eq!(p1["positions"].as_array().unwrap().len(), 1);
    let id1 = p1["positions"][0]["position_id"].as_i64().unwrap();
    let cursor = p1["next_cursor"]
        .as_i64()
        .expect("next_cursor present when more rows remain");

    let p2 = page(&app, Some(cursor)).await;
    assert_eq!(p2["positions"].as_array().unwrap().len(), 1);
    let id2 = p2["positions"][0]["position_id"].as_i64().unwrap();

    assert_ne!(id1, id2, "page two must not repeat the page-one row");
    let mut ids = [id1, id2];
    ids.sort_unstable();
    assert_eq!(
        ids,
        [1, 2],
        "both same-millisecond positions must be reachable"
    );
}

#[tokio::test]
async fn position_by_id_404_for_unknown() {
    let app = TestApp::start().await;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/positions/999999")
                .header("x-account-id", "ghost")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
