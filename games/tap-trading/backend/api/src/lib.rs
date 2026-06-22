//! Tick API service — library entry point.
//!
//! Spec: `docs/decisions/0009-tick-api-cross-service-contracts.md`,
//! `games/tap-trading/docs/SYSTEM_DESIGN.md §3`.

pub mod account_ctx;
pub mod aggregator_client;
pub mod db;
pub mod error;
pub mod handlers;
pub mod history;
pub mod metrics;
pub mod middleware;
pub mod now;
pub mod state;
pub mod validation;

use axum::{
    routing::{any, get, post},
    Router,
};

pub use state::AppState;

/// For tests only — a route that runs the rate limiter and returns 200 on
/// pass. We expose this from the library so integration tests can exercise
/// the limiter in isolation of POST /v1/positions (wired in Task 8).
#[doc(hidden)]
pub fn router_with_rate_limit_probe(state: AppState) -> Router {
    use axum::middleware::from_fn_with_state;
    Router::new()
        .route("/v1/rl-probe", get(|| async { "ok" }))
        .layer(from_fn_with_state(
            state.clone(),
            middleware::rate_limit::rate_limit_middleware,
        ))
        .layer(from_fn_with_state(
            state.clone(),
            middleware::account_id::account_id_middleware,
        ))
        .with_state(state)
}

/// Test-only router without the per-account rate-limit middleware.
/// Used by concurrency tests that fire >10 req/s on one account.
#[doc(hidden)]
pub fn router_without_rate_limit(state: AppState) -> Router {
    use axum::middleware::from_fn_with_state;
    use axum::routing::{get, post};
    let public = Router::new()
        .route("/healthz", get(handlers::health::healthz))
        .route("/metrics", get(handlers::health::metrics));
    let authenticated = Router::new()
        .route("/v1/me", get(handlers::me::get_me))
        .route("/v1/me/history", get(handlers::me::get_history))
        .route("/v1/positions", post(handlers::positions::post_position))
        .route(
            "/v1/positions/:id",
            get(handlers::positions::get_position_by_id),
        )
        .layer(from_fn_with_state(
            state.clone(),
            middleware::account_id::account_id_middleware,
        ));
    public.merge(authenticated).with_state(state)
}

/// Build the router. Middleware and most routes are added by later tasks.
pub fn router(state: AppState) -> Router {
    use axum::middleware::from_fn_with_state;
    use tower_http::cors::CorsLayer;

    let public = Router::new()
        .route("/healthz", get(handlers::health::healthz))
        .route("/metrics", get(handlers::health::metrics))
        .route("/stream", any(handlers::stream::ws_stream));

    let authenticated = Router::new()
        .route("/v1/ping", get(handlers::health::ping))
        .route("/v1/me", get(handlers::me::get_me))
        .route("/v1/me/history", get(handlers::me::get_history))
        .route("/v1/deposit", post(handlers::deposit::post_deposit))
        .route("/v1/withdraw", post(handlers::withdraw::post_withdraw))
        .route(
            "/v1/positions",
            post(handlers::positions::post_position).route_layer(from_fn_with_state(
                state.clone(),
                middleware::rate_limit::rate_limit_middleware,
            )),
        )
        .route(
            "/v1/positions/:id",
            get(handlers::positions::get_position_by_id),
        )
        // WS /stream lands in Task 13
        .layer(from_fn_with_state(
            state.clone(),
            middleware::account_id::account_id_middleware,
        ));

    // Permissive CORS matches the platform gateway. Tightens to an allowlist
    // when prod origins are known.
    public
        .merge(authenticated)
        .layer(CorsLayer::permissive())
        .with_state(state)
}
