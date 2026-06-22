//! Tick oracle aggregator — process entrypoint.

// Sub-modules add constants and config fields incrementally across tasks.
// Not all are consumed until the final task, so suppress dead_code warnings
// at the crate level rather than annotating each item individually.
#![allow(dead_code)]

mod aggregator;
mod api;
mod broadcast;
mod config;
mod constants;
mod driver;
mod ring_buffer;
mod runtime;
mod sources;
mod vol_state;

use anyhow::{Context, Result};
use config::AggregatorConfig;
use sources::{
    binance::BinanceSource, bybit::BybitSource, okx::OkxSource, pyth::PythSource, Source,
};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();

    let cfg = AggregatorConfig::from_env()?;
    let run_id = assign_run_id();
    tracing::info!(bind_addr = %cfg.bind_addr, run_id, "oracle-aggregator starting");

    let listener = tokio::net::TcpListener::bind(&cfg.bind_addr)
        .await
        .with_context(|| format!("bind {}", cfg.bind_addr))?;

    let (app, _handles) = runtime::wire(run_id, build_sources(&cfg));

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("axum serve")?;

    Ok(())
}

/// The four price sources the aggregator ingests, built from env-derived URLs
/// and the network's Pyth feed IDs. ORACLE_SPEC §2.
fn build_sources(cfg: &AggregatorConfig) -> Vec<Box<dyn Source>> {
    vec![
        Box::new(BinanceSource {
            base_url: cfg.binance_ws_url.clone(),
        }),
        Box::new(BybitSource {
            url: cfg.bybit_ws_url.clone(),
        }),
        Box::new(OkxSource {
            url: cfg.okx_ws_url.clone(),
        }),
        Box::new(PythSource::new(
            cfg.hermes_base_url.clone(),
            cfg.pyth_feeds(),
        )),
    ]
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("tap_trading_oracle_aggregator=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

/// Pick a `run_id`. ADR-0008 §4 allows unix-ms; we use it for human readability
/// in logs. Restart → fresh `run_id`, so clients with cached `oracle_seq_at_tap`
/// get a 409 Conflict from `/ring` and re-fetch a fresh quote.
fn assign_run_id() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(1)
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.ok();
    };
    #[cfg(unix)]
    let term = async {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut s) = signal(SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();

    tokio::select! { _ = ctrl_c => {}, _ = term => {} }
    tracing::info!("oracle-aggregator shutting down");
}
