use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64};

use tap_trading_settlement_worker::error::{Result, WorkerError};

fn env(name: &'static str) -> Result<String> {
    std::env::var(name).map_err(|_| WorkerError::MissingEnv(name))
}

fn env_port(name: &'static str) -> Result<u16> {
    env(name)?.parse().map_err(|e: std::num::ParseIntError| WorkerError::InvalidEnv {
        name,
        reason: e.to_string(),
    })
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let db_url = env("TAP_DB_URL")?;
    let aggregator_ws = env("TAP_AGGREGATOR_WS_URL")?;
    let http_port = env_port("TAP_WORKER_METRICS_PORT")?;

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(10)
        .connect(&db_url)
        .await?;

    let mut leader = tap_trading_settlement_worker::leader::LeaderGuard::acquire_or_wait(&db_url).await?;

    let cache = tap_trading_settlement_worker::cache::OpenPositionCache::new();
    cache.hydrate(&pool).await?;

    let is_leader = Arc::new(AtomicBool::new(true));
    let last_tick_received_ms = Arc::new(AtomicI64::new(0));
    let metrics = Arc::new(tap_trading_settlement_worker::health::Metrics::default());

    let ctx = tap_trading_settlement_worker::loop_runner::LoopContext {
        pool: pool.clone(),
        cache: cache.clone(),
        last_tick_received_ms: last_tick_received_ms.clone(),
        gap_tracker: Arc::new(tap_trading_settlement_worker::loop_runner::GapTracker::default()),
        metrics: metrics.clone(),
    };

    let listen_cache = cache.clone();
    let listen_pool = pool.clone();
    let listen_db_url = db_url.clone();
    tokio::spawn(async move {
        if let Err(e) = listen_cache.listen_loop(&listen_pool, &listen_db_url).await {
            tracing::error!(error = %e, "listen_loop exited");
        }
    });

    let ws_ctx = ctx.clone();
    tokio::spawn(async move {
        if let Err(e) = tap_trading_settlement_worker::loop_runner::run(ws_ctx, &aggregator_ws).await {
            tracing::error!(error = %e, "ws loop exited");
        }
    });

    let sweep_ctx = ctx.clone();
    tokio::spawn(async move {
        tap_trading_settlement_worker::loop_runner::periodic_sweep(sweep_ctx).await;
    });

    // Batched Walrus proof publishing (ADR-0011): every settlement is 'pending'
    // until the flusher gathers it into a batch, stores ONE blob, and stamps the
    // rows. Payout already happened synchronously; this only catches up proof
    // durability. Disabled (no `walrus` CLI needed) when TICK_PROOFS_ENABLED=false.
    use tap_trading_settlement_worker::onchain::{ProofConfig, WalrusCliPublisher};
    use tap_trading_settlement_worker::proof_flusher::{run_flusher, ProofFlusher};
    match ProofConfig::from_env()
        .map_err(|e| WorkerError::InvalidEnv { name: "TICK_PROOFS_*", reason: e.to_string() })?
    {
        Some(cfg) => {
            tracing::info!(epochs = cfg.walrus_store_epochs, "proof publishing enabled");
            let publisher = Box::new(WalrusCliPublisher::new(cfg.walrus_store_epochs));
            let flusher = ProofFlusher::new(publisher, cfg);
            let flush_pool = pool.clone();
            // Default 60s; overridable so tests/ops can tune the batch cadence.
            let flush_secs = std::env::var("TICK_PROOF_FLUSH_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(60);
            tokio::spawn(async move {
                run_flusher(flush_pool, flusher, flush_secs).await;
            });
        }
        None => tracing::info!("proof publishing disabled (TICK_PROOFS_ENABLED=false)"),
    }

    let health_state = tap_trading_settlement_worker::health::HealthState {
        is_leader: is_leader.clone(),
        last_tick_received_ms,
        metrics,
    };

    let addr = SocketAddr::from(([0, 0, 0, 0], http_port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "settlement worker http listening");

    // Graceful shutdown: on SIGINT/SIGTERM, stop serving HTTP, flip /healthz
    // off, then explicitly release the advisory lock so a standby can promote
    // inside the documented ≤2 s budget instead of waiting for TCP keepalive.
    let shutdown_signal = is_leader.clone();
    axum::serve(listener, tap_trading_settlement_worker::health::router(health_state))
        .with_graceful_shutdown(async move {
            wait_for_shutdown_signal().await;
            shutdown_signal.store(false, std::sync::atomic::Ordering::Relaxed);
            tracing::info!("shutdown signal received");
        })
        .await?;

    if let Err(e) = leader.release().await {
        tracing::warn!(error = %e, "leader release failed on shutdown");
    }
    Ok(())
}

#[cfg(unix)]
async fn wait_for_shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut sigterm = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    let mut sigint = signal(SignalKind::interrupt()).expect("install SIGINT handler");
    tokio::select! {
        _ = sigterm.recv() => {}
        _ = sigint.recv() => {}
    }
}

#[cfg(not(unix))]
async fn wait_for_shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
