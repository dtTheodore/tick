mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::TestApp;
use tower::util::ServiceExt;

async fn ping(app: &TestApp, account: &str) -> StatusCode {
    let router = tap_trading_api::router_with_rate_limit_probe(app.state.clone());
    let resp = router
        .oneshot(
            Request::builder()
                .uri("/v1/rl-probe")
                .header("x-account-id", account)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    resp.status()
}

#[tokio::test]
async fn eleventh_tap_in_a_second_is_rate_limited() {
    let app = TestApp::start().await;
    // Pin time so refill doesn't sneak tokens back in.
    app.state.clock.set(1_000_000_000_000);

    for _ in 0..10 {
        assert_eq!(ping(&app, "speedy").await, StatusCode::OK);
    }
    assert_eq!(ping(&app, "speedy").await, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn bucket_isolated_per_account() {
    let app = TestApp::start().await;
    app.state.clock.set(2_000_000_000_000);

    for _ in 0..10 {
        let _ = ping(&app, "alice").await;
    }
    assert_eq!(ping(&app, "alice").await, StatusCode::TOO_MANY_REQUESTS);
    // bob's bucket is untouched.
    assert_eq!(ping(&app, "bob").await, StatusCode::OK);
}

#[tokio::test]
async fn bucket_refills_one_token_per_100ms() {
    let app = TestApp::start().await;
    app.state.clock.set(3_000_000_000_000);
    for _ in 0..10 {
        let _ = ping(&app, "patient").await;
    }
    assert_eq!(ping(&app, "patient").await, StatusCode::TOO_MANY_REQUESTS);
    // Advance 200 ms → 2 tokens recovered.
    app.state.clock.set(3_000_000_000_200);
    assert_eq!(ping(&app, "patient").await, StatusCode::OK);
    assert_eq!(ping(&app, "patient").await, StatusCode::OK);
    assert_eq!(ping(&app, "patient").await, StatusCode::TOO_MANY_REQUESTS);
}
