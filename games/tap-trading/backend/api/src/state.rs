//! Shared handler state. Cloneable via `Arc`s inside; tests construct directly.

use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::broadcast;

use crate::aggregator_client::AggregatorClient;
use crate::history::TickHistory;
use crate::metrics::Metrics;
use crate::now::Clock;

#[derive(Clone)]
pub struct AppState {
    pub pg: PgPool,
    pub redis: redis::aio::ConnectionManager,
    pub aggregator: Arc<AggregatorClient>,
    pub broadcast: broadcast::Sender<String>,
    /// Recent-tick ring buffer; snapshotted to each new WS client for chart
    /// backfill. Shared with the aggregator subscriber task that fills it.
    pub history: Arc<TickHistory>,
    pub metrics: Arc<Metrics>,
    pub clock: Clock,
}
