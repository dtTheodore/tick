mod common;

use axum::body::Body;
use axum::http::Request;
use common::TestApp;
use serde_json::json;
use std::sync::Arc;
use tap_trading_api::aggregator_client::AggregatorClient;
use tap_trading_oracle_types::AssetSymbol;
use tap_trading_pricing_engine::{compute_multiplier, Cell, OracleState, PricingConfig};
use tower::util::ServiceExt;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn committed_multiplier_matches_server_recompute_not_client_claim() {
    let pinned_now: i64 = 1_748_345_673_000;
    let mid = 50_000.0_f64;
    let vol = 0.80_f64;
    let t_open_ms: i64 = 1_748_345_670_000;
    let t_close_ms: i64 = t_open_ms + 5_000;
    let strike_lo = 49_999.5_f64;
    let strike_hi = 50_000.5_f64;

    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "asset": "BTC", "run_id": 999, "seq": 12345, "ts_ms": pinned_now,
            "mid": mid, "vol_annualized": vol, "source_count": 3
        })))
        .mount(&mock)
        .await;
    let uri = mock.uri();
    let app = TestApp::start_with(|s| {
        s.aggregator = Arc::new(AggregatorClient::new(uri));
        s.clock.set(pinned_now);
    })
    .await;

    // Independent recompute using the same inputs the handler will use.
    let expected_server_mult = compute_multiplier(
        &Cell {
            asset: AssetSymbol::Btc,
            strike_lo,
            strike_hi,
            t_open_ms: t_open_ms as u64,
            t_close_ms: t_close_ms as u64,
        },
        &OracleState {
            asset: AssetSymbol::Btc,
            spot: mid,
            sigma_annualized: vol,
            timestamp_ms: pinned_now as u64,
        },
        &PricingConfig::default(),
        pinned_now as u64,
    )
    .expect("expected_server_mult");

    // Client sends 2% higher — within the 3% gate but distinct from server.
    let client_mult = expected_server_mult * 1.02;

    let body = json!({
        "client_request_id": "00000000-0000-0000-0000-0000000000cc",
        "asset": "BTC",
        "strike_lo": strike_lo,
        "strike_hi": strike_hi,
        "t_open_ms": t_open_ms,
        "t_close_ms": t_close_ms,
        "stake_points": 100,
        "client_multiplier": client_mult,
        "oracle_seq_at_tap": 12345,
        "oracle_run_id_at_tap": 999
    });
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/positions")
                .header("x-account-id", "lock-tester")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    // Read via TEXT cast — no bigdecimal dep needed.
    let (committed_str,): (String,) =
        sqlx::query_as("SELECT multiplier_at_tap::TEXT FROM positions")
            .fetch_one(&app.pg)
            .await
            .unwrap();
    let committed: f64 = committed_str.parse().unwrap();

    let delta_to_server = (committed - expected_server_mult).abs();
    let delta_to_client = (committed - client_mult).abs();

    assert!(
        delta_to_server < 1e-3,
        "committed {committed} should match server {expected_server_mult} (delta {delta_to_server})"
    );
    assert!(
        delta_to_client > delta_to_server,
        "committed {committed} should NOT match client {client_mult} \
         (delta_client {delta_to_client} vs delta_server {delta_to_server})"
    );
}
