//! 50 ms aggregator driver. Single-owner task that drains source ticks and
//! emits `OracleTick` / `OracleStatus` to the ring + broadcast.
//!
//! Sole writer of `AssetPriceState`, `AssetVolState`, `AssetStreamPhase`,
//! per-asset `next_seq`, and latest-tick-per-source. No locks: the
//! `select!` between `source_rx.recv()` and the 50 ms interval is the
//! mutual-exclusion mechanism.

use crate::aggregator::{AggregateOutcome, AssetPriceState, AssetStreamPhase, TickDecision};
use crate::broadcast::Broadcaster;
use crate::constants::{EMIT_PERIOD_MS, SUPPORTED_ASSETS};
use crate::ring_buffer::RingBuffers;
use crate::sources::{SourceId, SourceTick};
use crate::vol_state::AssetVolState;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;
use tap_trading_oracle_types::{AssetSymbol, OracleMessage, OracleStatus, OracleTick};
use tokio::sync::mpsc;
use tokio::time::{interval, MissedTickBehavior};

/// All per-asset state owned by the driver task. No locks — single owner.
struct DriverState {
    /// Latest tick per (asset, source). Refreshed on every `source_rx.recv()`.
    latest: HashMap<AssetSymbol, BTreeMap<SourceId, SourceTick>>,
    price: HashMap<AssetSymbol, AssetPriceState>,
    vol: HashMap<AssetSymbol, AssetVolState>,
    phase: HashMap<AssetSymbol, AssetStreamPhase>,
    /// Monotonic `seq` per asset under the current `run_id`. Paused (not
    /// advanced) while the asset is DEGRADED.
    next_seq: HashMap<AssetSymbol, u64>,
    rings: Arc<RingBuffers>,
    broadcaster: Broadcaster,
    run_id: u64,
}

impl DriverState {
    fn new(rings: Arc<RingBuffers>, broadcaster: Broadcaster, run_id: u64) -> Self {
        Self {
            latest: HashMap::new(),
            price: HashMap::new(),
            vol: HashMap::new(),
            phase: HashMap::new(),
            next_seq: HashMap::new(),
            rings,
            broadcaster,
            run_id,
        }
    }

    /// Absorb one source observation. Server-stamps `ts_ms` on receive.
    fn ingest(&mut self, tick: SourceTick) {
        self.latest
            .entry(tick.asset)
            .or_default()
            .insert(tick.source, tick);
    }

    /// Run one 50 ms emit step for every *supported* asset — not only those with
    /// a recorded tick. An asset whose sources never connect must still drive its
    /// phase machine and emit `Status(Degraded)`; otherwise the stream is silent
    /// for it (clients can't tell "down" from "quiet") while `/healthz` reports
    /// the whole service unhealthy — the two would disagree.
    pub(crate) fn tick_once(&mut self, now_ms: i64) {
        for asset in SUPPORTED_ASSETS {
            self.tick_asset(asset, now_ms);
        }
    }

    fn tick_asset(&mut self, asset: AssetSymbol, now_ms: i64) {
        let outcome = match self.latest.get(&asset) {
            Some(m) if !m.is_empty() => self.price.entry(asset).or_default().apply_sources(now_ms, m),
            _ => AggregateOutcome::InsufficientSources {
                reason: "no sources connected".to_string(),
            },
        };

        let phase = self.phase.entry(asset).or_default();
        let decision = phase.step(now_ms, &outcome);

        match decision {
            TickDecision::Tick {
                mid, source_count, ..
            } => {
                // Vol is computed against the freshly-aggregated mid.
                let vol = self.vol.entry(asset).or_default().next_vol(now_ms, mid);
                let seq_slot = self.next_seq.entry(asset).or_insert(0);
                let seq = *seq_slot;
                *seq_slot = seq.saturating_add(1);

                let tick = OracleTick {
                    asset,
                    run_id: self.run_id,
                    seq,
                    ts_ms: now_ms,
                    mid,
                    vol_annualized: vol,
                    source_count,
                };
                self.rings.push(tick);
                self.broadcaster.send(OracleMessage::Tick(tick));
            }
            TickDecision::Status { state, reason } => {
                self.broadcaster.send(OracleMessage::Status(OracleStatus {
                    asset,
                    state,
                    reason,
                    run_id: self.run_id,
                }));
            }
            TickDecision::Silence => {}
        }
    }
}

/// Wall-clock-ms since UNIX epoch. Local copy to avoid pubbing the one in
/// `broadcast.rs` — single-line helper, not worth the cross-module coupling.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Run the driver. Returns only when `source_rx` is closed (i.e. process
/// shutdown). Spawn on the tokio runtime.
pub async fn run(
    mut source_rx: mpsc::Receiver<SourceTick>,
    rings: Arc<RingBuffers>,
    broadcaster: Broadcaster,
    run_id: u64,
) {
    let mut state = DriverState::new(rings, broadcaster, run_id);
    let mut ticker = interval(Duration::from_millis(EMIT_PERIOD_MS));
    // Skip catch-up bursts if the loop falls behind — clients want fresh
    // data, not backfill.
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    // Discard the immediate first tick so the first emit happens after
    // EMIT_PERIOD_MS, not at t=0 before any sources have been ingested.
    ticker.tick().await;

    loop {
        tokio::select! {
            maybe = source_rx.recv() => {
                match maybe {
                    Some(tick) => state.ingest(tick),
                    None => {
                        tracing::info!("source channel closed; driver exiting");
                        return;
                    }
                }
            }
            _ = ticker.tick() => {
                state.tick_once(now_ms());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ring_buffer::RingLookup;

    fn src(source: SourceId, asset: AssetSymbol, price: f64, ts_ms: i64) -> SourceTick {
        SourceTick {
            source,
            asset,
            price,
            ts_ms,
            pyth_conf_bps: None,
        }
    }

    fn fresh_state() -> DriverState {
        DriverState::new(Arc::new(RingBuffers::new()), Broadcaster::new(), 7)
    }

    #[test]
    fn emits_tick_with_seq_starting_at_zero() {
        let mut s = fresh_state();
        let mut rx = s.broadcaster.sender().subscribe();
        let t = 1_000_000;
        s.ingest(src(SourceId::Binance, AssetSymbol::Eth, 3812.0, t));
        s.ingest(src(SourceId::Bybit, AssetSymbol::Eth, 3812.1, t));
        s.ingest(src(SourceId::Okx, AssetSymbol::Eth, 3812.2, t));

        s.tick_once(t);

        let msg = rx.try_recv().expect("a broadcast frame");
        match msg {
            OracleMessage::Tick(tick) => {
                assert_eq!(tick.asset, AssetSymbol::Eth);
                assert_eq!(tick.run_id, 7);
                assert_eq!(tick.seq, 0, "first tick seq must be 0");
                assert_eq!(tick.source_count, 3);
                // Ring should also have it.
                match s.rings.get(AssetSymbol::Eth, 7, 0) {
                    RingLookup::Hit(rt) => assert_eq!(rt, tick),
                    other => panic!("expected Hit, got {other:?}"),
                }
            }
            other => panic!("expected Tick, got {other:?}"),
        }
    }

    #[test]
    fn seq_advances_monotonically_across_ticks() {
        let mut s = fresh_state();
        let mut rx = s.broadcaster.sender().subscribe();
        let base = 1_000_000;
        for i in 0..3 {
            let now = base + (i * 50);
            s.ingest(src(
                SourceId::Binance,
                AssetSymbol::Eth,
                3812.0 + i as f64,
                now,
            ));
            s.ingest(src(
                SourceId::Bybit,
                AssetSymbol::Eth,
                3812.1 + i as f64,
                now,
            ));
            s.tick_once(now);
        }
        let mut seqs = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            if let OracleMessage::Tick(t) = msg {
                seqs.push(t.seq);
            }
        }
        assert_eq!(seqs, vec![0, 1, 2]);
    }

    #[test]
    fn single_source_produces_silence_not_tick() {
        // Step 4 of ORACLE_SPEC §4.4: < 2 active sources → no tick.
        // Hysteresis hasn't fired yet either → Silence, no Status.
        let mut s = fresh_state();
        let mut rx = s.broadcaster.sender().subscribe();
        let t = 1_000_000;
        s.ingest(src(SourceId::Binance, AssetSymbol::Eth, 3812.0, t));
        s.tick_once(t);
        assert!(rx.try_recv().is_err(), "no frame expected");
        // Seq must NOT have advanced.
        assert!(!s.next_seq.contains_key(&AssetSymbol::Eth));
    }

    #[test]
    fn asset_with_no_sources_emits_degraded_status() {
        // An asset whose sources never connect must still surface Degraded on
        // the stream (so clients disable taps), not stay silent while healthz
        // reports the whole service 503.
        use tap_trading_oracle_types::OracleStreamState;
        let mut s = fresh_state();
        let mut rx = s.broadcaster.sender().subscribe();
        // Only ETH ever ingests; BTC and SUI have no sources at all.
        s.ingest(src(SourceId::Binance, AssetSymbol::Eth, 3812.0, 0));
        s.ingest(src(SourceId::Bybit, AssetSymbol::Eth, 3812.1, 0));
        s.tick_once(0); // starts the Degraded pending timer for the source-less assets
        s.tick_once(2_001); // 2001 ms later → Status(Degraded) for them

        let mut degraded = std::collections::HashSet::new();
        while let Ok(msg) = rx.try_recv() {
            if let OracleMessage::Status(st) = msg {
                if st.state == OracleStreamState::Degraded {
                    degraded.insert(st.asset);
                }
            }
        }
        assert!(degraded.contains(&AssetSymbol::Sui), "SUI must emit Degraded");
        assert!(degraded.contains(&AssetSymbol::Btc), "BTC must emit Degraded");
    }

    #[test]
    fn degraded_pauses_seq_then_recovers_with_last_plus_one() {
        // seq pauses during DEGRADED and continues (last+1) on recovery — it
        // never resets mid-run. ADR-0008 §4 requires seq monotonic per
        // (asset, run_id); only continuation satisfies that. Resetting to 0
        // would collide with already-emitted ticks and break ring replay.
        let mut s = fresh_state();
        let mut rx = s.broadcaster.sender().subscribe();

        let t0 = 0i64;
        // 1) Normal: 2 sources → tick(seq=0).
        s.ingest(src(SourceId::Binance, AssetSymbol::Eth, 3812.0, t0));
        s.ingest(src(SourceId::Bybit, AssetSymbol::Eth, 3812.1, t0));
        s.tick_once(t0);
        // 2) Make sources stale so apply_sources returns InsufficientSources,
        // driving the hysteresis toward Degraded. Staleness = ts_ms too old.
        // t0=0, SOURCE_FRESHNESS_MS=5000. At now=5001, age=5001 > 5000 → stale.
        // First tick at 5001 starts the Degraded pending timer.
        // Second tick at 7002 (2001ms later) fires the Status(Degraded).
        s.tick_once(5_001);
        s.tick_once(7_002); // 2001ms after first → Status(Degraded) emitted
                            // 3) Restore sources; 2 s of sustained Emit triggers Status(Normal).
                            // Sources must stay fresh (< 5000ms old) on each tick_once call.
        let t_recover = 8_000;
        s.ingest(src(SourceId::Binance, AssetSymbol::Eth, 3812.0, t_recover));
        s.ingest(src(SourceId::Bybit, AssetSymbol::Eth, 3812.1, t_recover));
        s.tick_once(t_recover);
        let t_r2 = t_recover + 2_001;
        s.ingest(src(SourceId::Binance, AssetSymbol::Eth, 3812.0, t_r2));
        s.ingest(src(SourceId::Bybit, AssetSymbol::Eth, 3812.1, t_r2));
        s.tick_once(t_r2); // 2001ms of sustained Emit → Status(Normal)
                           // 4) Next emit-eligible step issues seq=1, NOT seq=0.
        let t_next = t_recover + 2_100;
        s.ingest(src(SourceId::Binance, AssetSymbol::Eth, 3812.0, t_next));
        s.ingest(src(SourceId::Bybit, AssetSymbol::Eth, 3812.1, t_next));
        s.tick_once(t_next);

        let mut tick_seqs = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            if let OracleMessage::Tick(t) = msg {
                tick_seqs.push(t.seq);
            }
        }
        // First two ticks at t0 (seq=0) and t_next (seq=1).
        assert_eq!(tick_seqs, vec![0, 1]);
    }

    #[tokio::test(start_paused = true)]
    async fn run_drains_channel_and_emits_on_interval() {
        let rings = Arc::new(RingBuffers::new());
        let broadcaster = Broadcaster::new();
        let mut rx = broadcaster.sender().subscribe();
        let (tx, source_rx) = mpsc::channel::<SourceTick>(8);

        let handle = tokio::spawn(super::run(source_rx, rings, broadcaster, 9));

        // Use real wall-clock time so the freshness filter (now_ms() - ts_ms <= 1000)
        // passes when the driver runs. tokio::time::advance only moves the tokio
        // clock; SystemTime::now() in now_ms() stays on wall-clock time.
        let t = now_ms();
        tx.send(src(SourceId::Binance, AssetSymbol::Eth, 3812.0, t))
            .await
            .unwrap();
        tx.send(src(SourceId::Bybit, AssetSymbol::Eth, 3812.1, t))
            .await
            .unwrap();

        // Yield a few times so the driver can drain its channel before the
        // interval fires. The select! is non-deterministic when both branches
        // are ready at once, so we drain first then advance time.
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
        tokio::time::advance(Duration::from_millis(60)).await;
        // Additional yields to let the driver process the interval tick.
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }

        // The tick should now be in the broadcast channel. recv() returns
        // immediately; the 50ms timeout is a hang-guard only.
        let msg = tokio::time::timeout(Duration::from_millis(50), async {
            loop {
                if let Ok(OracleMessage::Tick(t)) = rx.recv().await {
                    return t;
                }
            }
        })
        .await
        .expect("a tick within the test budget");
        assert_eq!(msg.run_id, 9);

        drop(tx);
        let _ = handle.await;
    }
}
