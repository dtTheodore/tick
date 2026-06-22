//! Aggregator WS subscriber + dispatch loop. See `docs/SYSTEM_DESIGN.md §5.2`.
//!
//! Each `OracleMessage` is dispatched by variant:
//!   - `Tick` → records last-known-mid, scans cache; `evaluate_position`
//!     decides Win/Expire/Hold so both touches AND expiries settle live.
//!   - `Status` → flips per-asset gap state machine; on recovery, voids
//!     positions whose window was fully covered by the gap (§9.1).
//!   - `Heartbeat` → refreshes the last-tick health timer.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use futures_util::StreamExt;
use sqlx::PgPool;
use tap_trading_oracle_types::{OracleMessage, OracleTick, OracleStreamState};
use tap_trading_pricing_engine::AssetSymbol;
use tokio_tungstenite::tungstenite::Message;

use crate::cache::{OpenPositionCache, PositionRef};
use crate::error::Result;
use crate::health::Metrics;

/// A live settle decision routed by `dispatch_settle`. Only Win/Loss settle off
/// a tick; the Void refund is gap-recovery only and calls `settle_void` directly
/// in `process_gap_recovery`, so it isn't a dispatch variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettleOutcome {
    Win,
    Loss,
}

#[derive(Default)]
pub struct GapTracker {
    /// gap_start_ms per asset; absent means currently Normal.
    inner: Mutex<HashMap<AssetSymbol, i64>>,
}

impl GapTracker {
    pub fn enter_degraded(&self, asset: AssetSymbol, now_ms: i64) {
        let mut g = self.inner.lock().expect("gap tracker poisoned");
        g.entry(asset).or_insert(now_ms);
    }

    /// Returns gap_start_ms if a gap was in progress; clears the entry.
    pub fn exit_degraded(&self, asset: AssetSymbol) -> Option<i64> {
        let mut g = self.inner.lock().expect("gap tracker poisoned");
        g.remove(&asset)
    }

    /// Snapshot: is this asset currently in a gap? Used by the sweep to skip
    /// degraded assets so it can't race the void-on-recovery path.
    pub fn is_degraded(&self, asset: AssetSymbol) -> bool {
        let g = self.inner.lock().expect("gap tracker poisoned");
        g.contains_key(&asset)
    }
}

#[derive(Clone)]
pub struct LoopContext {
    pub pool: PgPool,
    pub cache: OpenPositionCache,
    pub last_tick_received_ms: Arc<AtomicI64>,
    pub gap_tracker: Arc<GapTracker>,
    pub metrics: Arc<Metrics>,
}

pub async fn run(ctx: LoopContext, ws_url: &str) -> Result<()> {
    // Outer loop reconnects unconditionally. The inner loop must NEVER let a
    // frame or parse error escape — a single malformed payload or transient
    // protocol error would otherwise return from `run()` and permanently stop
    // settlement (the spawned task in main.rs is not respawned). Connect
    // failures DO bubble out so the task can retry the whole connect cycle.
    loop {
        let (ws, _resp) = match tokio_tungstenite::connect_async(ws_url).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "aggregator ws connect failed; retrying in 1s");
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                continue;
            }
        };
        tracing::info!(ws_url, "aggregator ws connected");
        let (_write, mut read) = ws.split();

        while let Some(frame) = read.next().await {
            let frame = match frame {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!(error = %e, "ws read error; breaking to reconnect");
                    break;
                }
            };
            let msg = match frame {
                Message::Text(t) => t,
                Message::Binary(_) | Message::Ping(_) | Message::Pong(_) => continue,
                Message::Close(_) => break,
                Message::Frame(_) => continue,
            };

            let oracle_msg: OracleMessage = match serde_json::from_str(&msg) {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!(error = %e, payload = %msg, "drop malformed oracle message");
                    continue;
                }
            };
            handle_message(&ctx, oracle_msg).await;
        }

        tracing::warn!("aggregator ws stream ended; reconnecting in 1s");
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

pub async fn handle_message(ctx: &LoopContext, msg: OracleMessage) {
    match msg {
        OracleMessage::Tick(tick) => {
            ctx.last_tick_received_ms.store(tick.ts_ms, Ordering::Relaxed);
            // Snapshot the previous mid BEFORE overwriting it, so touch detection
            // can test the path segment [prev_mid, tick.mid] — a fast wick can
            // cross a narrow band entirely between two ticks (see touch.rs).
            let prev_mid = ctx.cache.last_mid(tick.asset).await;
            ctx.cache.record_last_mid(tick.asset, tick.mid).await;

            // Per SYSTEM_DESIGN §5.2: every tick scans BOTH in-window touches
            // (Win) and post-close untouched positions (Expire→Loss). The cache
            // returns everything that has opened; evaluate_position decides.
            let started = Instant::now();
            let candidates = ctx.cache.settleable_for_asset(tick.asset, tick.ts_ms).await;
            for pos in candidates {
                use crate::touch::{evaluate_position, TouchOutcome};
                match evaluate_position(&pos, prev_mid, &tick) {
                    TouchOutcome::Win => dispatch_settle(ctx, &pos, SettleOutcome::Win, &tick).await,
                    TouchOutcome::Expire => dispatch_settle(ctx, &pos, SettleOutcome::Loss, &tick).await,
                    TouchOutcome::Hold => {}
                }
            }
            ctx.metrics.observe_tick_ms(started.elapsed().as_millis() as u64);
        }
        OracleMessage::Status(status) => {
            let now_ms = chrono::Utc::now().timestamp_millis();
            tracing::info!(
                asset = ?status.asset,
                state = ?status.state,
                reason = %status.reason,
                "oracle status",
            );
            match status.state {
                OracleStreamState::Degraded => {
                    ctx.gap_tracker.enter_degraded(status.asset, now_ms);
                }
                OracleStreamState::Normal => {
                    if let Some(gap_start_ms) = ctx.gap_tracker.exit_degraded(status.asset) {
                        process_gap_recovery(ctx, status.asset, gap_start_ms, now_ms).await;
                    }
                }
            }
        }
        OracleMessage::Heartbeat { ts_ms } => {
            ctx.last_tick_received_ms.store(ts_ms, Ordering::Relaxed);
        }
    }
}

/// Settle a touched/expired position off-chain (Postgres). On success the
/// position is evicted from the cache; on failure it stays OPEN and is retried
/// on the next tick/sweep. The Walrus proof is published out-of-band by the
/// proof flusher — it never blocks this path.
async fn dispatch_settle(ctx: &LoopContext, pos: &PositionRef, outcome: SettleOutcome, tick: &OracleTick) {
    let res = match outcome {
        SettleOutcome::Win => crate::settle::settle_win(&ctx.pool, pos, tick).await,
        SettleOutcome::Loss => crate::settle::settle_loss(&ctx.pool, pos, tick).await,
    };
    match res {
        Ok(fresh) => {
            if fresh {
                bump_outcome_metric(ctx, outcome);
            }
            ctx.cache.remove(pos.asset, pos.id).await;
        }
        Err(e) => tracing::error!(error = %e, position_id = pos.id, ?outcome, "settle failed"),
    }
}

fn bump_outcome_metric(ctx: &LoopContext, outcome: SettleOutcome) {
    match outcome {
        SettleOutcome::Win => ctx.metrics.positions_settled_win.fetch_add(1, Ordering::Relaxed),
        SettleOutcome::Loss => ctx.metrics.positions_settled_loss.fetch_add(1, Ordering::Relaxed),
    };
}

async fn process_gap_recovery(ctx: &LoopContext, asset: AssetSymbol, gap_start_ms: i64, gap_end_ms: i64) {
    let positions = ctx.cache.all_positions().await;
    let last_mid = ctx.cache.last_mid(asset).await;
    for pos in positions {
        if pos.asset != asset {
            continue;
        }
        // SYSTEM_DESIGN §9.1: void only when the gap covers the FULL window
        // (zero ticks across [t_open, t_close]). Partial overlap stays a
        // normal settlement — there were ticks for part of the window.
        if pos.t_open_ms >= gap_start_ms && pos.t_close_ms <= gap_end_ms {
            // A full-gap void has no replayable evidence path, so it never gets a
            // proof (the flusher SELECT excludes 'V'); the refund stands.
            match crate::settle::settle_void(&ctx.pool, &pos, last_mid, gap_end_ms).await {
                Ok(fresh) => {
                    if fresh {
                        ctx.metrics.positions_voided.fetch_add(1, Ordering::Relaxed);
                    }
                    ctx.cache.remove(pos.asset, pos.id).await;
                }
                Err(e) => tracing::error!(error = %e, position_id = pos.id, "settle_void failed"),
            }
        }
    }
}

/// Safety-net sweep: every 30 s, re-hydrate and force-settle expired stragglers
/// the live tick loop somehow missed (no tick for an asset since `t_close_ms`).
/// SYSTEM_DESIGN §5.2: "The cache evicts on settle/expire; periodically we
/// sweep stragglers."
pub async fn periodic_sweep(ctx: LoopContext) {
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(30));
    ticker.tick().await; // skip the immediate first tick
    loop {
        ticker.tick().await;
        sweep_once(&ctx).await;
    }
}

/// One pass of the safety-net sweep — extracted so tests can drive it
/// deterministically without spinning the 30 s ticker.
pub async fn sweep_once(ctx: &LoopContext) {
    if let Err(e) = ctx.cache.hydrate(&ctx.pool).await {
        tracing::warn!(error = %e, "periodic hydrate failed");
        return;
    }
    let now_ms = chrono::Utc::now().timestamp_millis();
    for pos in expired_positions(&ctx.cache, now_ms).await {
        // Skip degraded assets: the void-on-recovery path will refund (§9.1)
        // those positions whose window was fully covered. Letting the sweep
        // record an 'L' settlement first would win the UNIQUE(position_id)
        // race and turn a refund into a stake loss.
        if ctx.gap_tracker.is_degraded(pos.asset) {
            tracing::debug!(position_id = pos.id, asset = ?pos.asset, "sweep: skip degraded asset");
            continue;
        }
        let mid = ctx.cache.last_mid(pos.asset).await.unwrap_or(pos.strike_lo);
        let tick = OracleTick {
            asset: pos.asset, run_id: 0, seq: 0, ts_ms: now_ms,
            mid, vol_annualized: 0.60, source_count: 0,
        };
        // Expire a straggler as a Loss. The synthetic tick carries seq=0, so the
        // proof flusher can't serve its evidence window and marks the proof
        // failed — the settlement (a Loss, no payout) stands regardless.
        dispatch_settle(ctx, &pos, SettleOutcome::Loss, &tick).await;
    }
}

async fn expired_positions(cache: &OpenPositionCache, now_ms: i64) -> Vec<crate::cache::PositionRef> {
    cache.all_positions().await.into_iter().filter(|p| p.t_close_ms < now_ms).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::postgres::PgPoolOptions;

    fn fake_ctx() -> LoopContext {
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy("postgres://nobody:nobody@127.0.0.1:1/none")
            .expect("lazy pool");
        LoopContext {
            pool,
            cache: OpenPositionCache::new(),
            last_tick_received_ms: Arc::new(AtomicI64::new(0)),
            gap_tracker: Arc::new(GapTracker::default()),
            metrics: Arc::new(Metrics::default()),
        }
    }

    #[tokio::test]
    async fn tick_updates_last_known_mid() {
        let ctx = fake_ctx();
        let msg: OracleMessage = serde_json::from_str(
            r#"{"type":"tick","asset":"BTC","run_id":1,"seq":1,"ts_ms":12345,"mid":70000.5,"vol_annualized":0.8,"source_count":3}"#,
        )
        .expect("parse");
        handle_message(&ctx, msg).await;
        assert_eq!(ctx.cache.last_mid(AssetSymbol::Btc).await, Some(70_000.5));
        assert_eq!(ctx.last_tick_received_ms.load(Ordering::Relaxed), 12345);
    }

    #[tokio::test]
    async fn heartbeat_updates_timer() {
        let ctx = fake_ctx();
        let msg: OracleMessage = serde_json::from_str(r#"{"type":"heartbeat","ts_ms":99999}"#)
            .expect("parse");
        handle_message(&ctx, msg).await;
        assert_eq!(ctx.last_tick_received_ms.load(Ordering::Relaxed), 99999);
    }
}

#[cfg(test)]
mod sweep_tests {
    use super::*;
    use tap_trading_pricing_engine::AssetSymbol;

    fn pos(id: i64, t_close_ms: i64) -> crate::cache::PositionRef {
        crate::cache::PositionRef {
            id,
            account_id: 1,
            asset: AssetSymbol::Btc,
            strike_lo: 70_000.0,
            strike_hi: 70_010.0,
            t_open_ms: 0,
            t_close_ms,
            stake_points: 100,
            multiplier_at_tap: 2.5,
            oracle_seq_at_tap: 0,
            oracle_run_id_at_tap: 0,
            created_at_ms: 0,
        }
    }

    #[tokio::test]
    async fn expired_filter_picks_only_past_close() {
        let cache = OpenPositionCache::new();
        cache.upsert(pos(1, 1_000)).await;
        cache.upsert(pos(2, 5_000)).await;
        cache.upsert(pos(3, 999)).await;
        let out = expired_positions(&cache, 3_000).await;
        let mut ids: Vec<i64> = out.iter().map(|p| p.id).collect();
        ids.sort();
        assert_eq!(ids, vec![1, 3]);
    }
}
