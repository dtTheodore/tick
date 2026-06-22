//! The single wiring path for the aggregator topology.
//!
//! `main` and the end-to-end test both call `wire` so the production graph
//! (heartbeat + driver + sources → ring + broadcast → HTTP/WS router) can
//! never drift from what the test exercises. The caller owns serving: `main`
//! adds graceful shutdown, the test binds an ephemeral port and injects
//! synthetic ticks through `RuntimeHandles::source_tx`.

use crate::api::{router, AppState};
use crate::broadcast::Broadcaster;
use crate::driver;
use crate::ring_buffer::RingBuffers;
use crate::sources::{Source, SourceTick};
use axum::Router;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Handles a caller may need after wiring: `source_tx` lets a test feed ticks
/// without real sources; `broadcaster`/`rings` expose the live state.
pub struct RuntimeHandles {
    pub broadcaster: Broadcaster,
    pub rings: Arc<RingBuffers>,
    pub source_tx: mpsc::Sender<SourceTick>,
}

/// Build the full aggregator graph and return the axum `Router` plus handles.
///
/// Spawns the heartbeat, the 50 ms driver, and every passed source onto the
/// current tokio runtime, then returns without serving. Must be called from
/// within a runtime context.
pub fn wire(run_id: u64, sources: Vec<Box<dyn Source>>) -> (Router, RuntimeHandles) {
    let rings = Arc::new(RingBuffers::new());
    let broadcaster = Broadcaster::new();
    // Detached: the heartbeat task lives for the process; dropping the handle
    // does not cancel it.
    let _heartbeat = broadcaster.spawn_heartbeat();

    let (source_tx, source_rx) = mpsc::channel::<SourceTick>(1024);
    tokio::spawn(driver::run(
        source_rx,
        rings.clone(),
        broadcaster.clone(),
        run_id,
    ));

    // Supervise the source tasks on a JoinSet: `run` loops forever, so a join
    // completing means the task exited or panicked. Log it loudly rather than
    // dropping the handle and losing the feed silently.
    let mut sources_set = tokio::task::JoinSet::new();
    for src in sources {
        let tx = source_tx.clone();
        let id = src.id();
        sources_set.spawn(async move {
            src.run(tx).await;
            id
        });
    }
    tokio::spawn(async move {
        while let Some(res) = sources_set.join_next().await {
            match res {
                Ok(id) => tracing::error!(?id, "source task exited; feed lost until restart"),
                Err(e) => {
                    tracing::error!(error = %e, "source task panicked; feed lost until restart")
                }
            }
        }
    });

    let app = router(AppState {
        run_id,
        rings: rings.clone(),
        broadcaster: broadcaster.clone(),
    });

    (
        app,
        RuntimeHandles {
            broadcaster,
            rings,
            source_tx,
        },
    )
}
