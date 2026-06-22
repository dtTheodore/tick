//! In-process test harness. Boots Postgres + Redis containers, runs migrations,
//! returns an `axum::Router` exercised via `tower::ServiceExt::oneshot`.

#![allow(dead_code)] // members are added as tests grow

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::sync::Arc;
use tap_trading_api::aggregator_client::AggregatorClient;
use tap_trading_api::metrics::Metrics;
use tap_trading_api::now::Clock;
use tap_trading_api::AppState;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage};
use testcontainers_modules::postgres::Postgres;
use tokio::sync::broadcast;

pub struct TestApp {
    pub pg: PgPool,
    pub state: AppState,
    pub router: axum::Router,
    // Hold container handles so they stay alive for the test's lifetime.
    _pg_container: ContainerAsync<Postgres>,
    _redis_container: ContainerAsync<GenericImage>,
}

impl TestApp {
    pub async fn start() -> Self {
        Self::start_with(|_state| {}).await
    }

    /// Allow per-test state customization (e.g. swap aggregator base URL).
    pub async fn start_with<F: FnOnce(&mut AppState)>(customize: F) -> Self {
        let pg_container = Postgres::default()
            .start()
            .await
            .expect("postgres container");
        let pg_port = pg_container.get_host_port_ipv4(5432).await.unwrap();
        let pg_url = format!("postgres://postgres:postgres@127.0.0.1:{pg_port}/postgres");

        let redis_container = GenericImage::new("redis", "7-alpine")
            .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
            .with_exposed_port(6379.tcp())
            .start()
            .await
            .expect("redis container");
        let redis_host = redis_container.get_host().await.unwrap();
        let redis_port = redis_container.get_host_port_ipv4(6379).await.unwrap();
        let redis_url = format!("redis://{redis_host}:{redis_port}");

        let pg = PgPoolOptions::new()
            .max_connections(10)
            .connect(&pg_url)
            .await
            .expect("connect pg");
        tap_trading_migrate::run_migrations(&pg)
            .await
            .expect("migrations");

        let redis_client = redis::Client::open(redis_url).unwrap();
        let redis = redis::aio::ConnectionManager::new(redis_client)
            .await
            .unwrap();
        let (broadcast_tx, _) = broadcast::channel(256);

        // Default aggregator base URL: nonsense. Tests that exercise the
        // aggregator path override it via `start_with`.
        let mut state = AppState {
            pg: pg.clone(),
            redis,
            aggregator: Arc::new(AggregatorClient::new("http://127.0.0.1:1".to_string())),
            broadcast: broadcast_tx,
            history: Arc::new(tap_trading_api::history::TickHistory::new()),
            metrics: Metrics::new(),
            // Per-app test clock — each `TestApp` gets its own atomic, so
            // parallel tests in the same binary can never race the clock.
            clock: Clock::test(0),
        };
        customize(&mut state);
        let router = tap_trading_api::router(state.clone());

        Self {
            pg,
            state,
            router,
            _pg_container: pg_container,
            _redis_container: redis_container,
        }
    }
}
