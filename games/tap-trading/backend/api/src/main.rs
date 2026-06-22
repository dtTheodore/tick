//! `tap-trading-api` binary.

use anyhow::{anyhow, Result};
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use tap_trading_api::aggregator_client::AggregatorClient;
use tap_trading_api::history::TickHistory;
use tap_trading_api::metrics::Metrics;
use tap_trading_api::now::Clock;
use tap_trading_api::{router, AppState};
use tokio::sync::broadcast;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .json()
        .init();

    let database_url =
        std::env::var("TAP_DB_URL").map_err(|_| anyhow!("TAP_DB_URL is required"))?;
    let redis_url =
        std::env::var("TAP_REDIS_URL").map_err(|_| anyhow!("TAP_REDIS_URL is required"))?;
    let aggregator_url = std::env::var("TAP_AGGREGATOR_URL")
        .map_err(|_| anyhow!("TAP_AGGREGATOR_URL is required"))?;
    let port = std::env::var("TAP_API_PORT")
        .map_err(|_| anyhow!("TAP_API_PORT is required"))?
        .parse::<u16>()
        .map_err(|e| anyhow!("TAP_API_PORT invalid: {e}"))?;

    let pg = PgPoolOptions::new()
        .max_connections(20)
        .connect(&database_url)
        .await?;
    let redis_client = redis::Client::open(redis_url)?;
    let redis = redis::aio::ConnectionManager::new(redis_client).await?;
    let (broadcast_tx, _) = broadcast::channel(256);
    let history = Arc::new(TickHistory::new());
    tap_trading_api::aggregator_client::spawn_aggregator_subscriber(
        aggregator_url.clone(),
        broadcast_tx.clone(),
        history.clone(),
    );

    let state = AppState {
        pg,
        redis,
        aggregator: Arc::new(AggregatorClient::new(aggregator_url)),
        broadcast: broadcast_tx,
        history,
        metrics: Metrics::new(),
        clock: Clock::real(),
    };

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!(port, "tap-trading-api listening");
    axum::serve(listener, router(state)).await?;
    Ok(())
}
