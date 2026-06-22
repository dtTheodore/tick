mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::TestApp;
use tower::util::ServiceExt;

#[tokio::test]
async fn missing_header_returns_401() {
    let app = TestApp::start().await;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/ping")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn unknown_header_lazy_creates_account_and_signup_ledger() {
    let app = TestApp::start().await;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/ping")
                .header("x-account-id", "brand-new-user")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let (count_accounts,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM accounts")
        .fetch_one(&app.pg)
        .await
        .unwrap();
    assert_eq!(count_accounts, 1);
    let (balance,): (i64,) = sqlx::query_as("SELECT balance FROM accounts WHERE external_id = $1")
        .bind("brand-new-user")
        .fetch_one(&app.pg)
        .await
        .unwrap();
    assert_eq!(balance, 10_000);
    let (count_signup,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM points_ledger WHERE kind = 'SIGNUP'")
            .fetch_one(&app.pg)
            .await
            .unwrap();
    assert_eq!(count_signup, 1);
}

#[tokio::test]
async fn second_call_reuses_account() {
    let app = TestApp::start().await;
    for _ in 0..3 {
        let _ = app
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/ping")
                    .header("x-account-id", "repeat-user")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
    }
    let (count_accounts,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM accounts")
        .fetch_one(&app.pg)
        .await
        .unwrap();
    assert_eq!(count_accounts, 1);
    let (count_signup,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM points_ledger WHERE kind = 'SIGNUP'")
            .fetch_one(&app.pg)
            .await
            .unwrap();
    assert_eq!(count_signup, 1);
}

#[tokio::test]
async fn empty_header_returns_400() {
    let app = TestApp::start().await;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/ping")
                .header("x-account-id", "")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn too_long_header_returns_400() {
    let app = TestApp::start().await;
    let huge = "x".repeat(200);
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/ping")
                .header("x-account-id", huge)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
