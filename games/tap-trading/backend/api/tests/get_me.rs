mod common;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use common::TestApp;
use tower::util::ServiceExt;

#[tokio::test]
async fn returns_default_state_for_new_account() {
    let app = TestApp::start().await;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/me")
                .header("x-account-id", "new-me")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 8 * 1024).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["external_id"], "new-me");
    assert_eq!(v["balance"], 10_000);
    assert_eq!(v["lifetime_points_won"], 0);
    assert_eq!(v["tier"], 1);
    assert_eq!(v["current_streak"], 0);
}
