//! In-memory recent-tick ring buffer for chart backfill.
//!
//! The `WS /stream` handler sends a one-shot history snapshot to every new
//! client so the price chart paints the real recent shape immediately, instead
//! of a flat seed line. This works for ANY client — fresh browser, new device,
//! first-ever visit — which client-side persistence (localStorage) cannot.
//!
//! Bounded by wall-clock window, so memory is `O(assets × rate × window)` and
//! independent of process uptime. The aggregator's `/ring` replay buffer serves
//! a different need (per-seq settlement lookup); this is the fan-out-side cache
//! for the live chart, kept where the stream is already consumed so a connect
//! never blocks on an upstream HTTP round trip.

use std::collections::VecDeque;
use std::sync::Mutex;

use serde::Deserialize;

/// Match the client chart's trail window (`TRAIL_WINDOW_MS` = 150 s in
/// tick-store.ts) so a fresh connect's backfill spans the chart's full visible
/// past at any viewport. A narrower 35 s window left the chart's left edge blank
/// on reload: the on-screen past reaches ~52 s at 1280 px and ~106 s ultrawide,
/// so 35 s only filled ~60% from the now-line. The client windows this down to
/// whatever's actually visible, so over-sending is harmless.
const WINDOW_MS: i64 = 150_000;

/// Hard cap so an upstream burst (e.g. replay flood) can't grow the buffer
/// unbounded between time-trims. Must cover the full window or it silently
/// re-truncates the left edge: 3 assets × 20 Hz × 150 s ≈ 9 000; 10 000 leaves
/// headroom at ~1 MB of small JSON strings — not a memory concern.
const MAX_ENTRIES: usize = 10_000;

/// Just enough of a stream frame to classify and time-bound it. Tick frames
/// are retained verbatim; status/heartbeat carry no chart data and are dropped.
#[derive(Deserialize)]
struct FrameMeta {
    #[serde(rename = "type")]
    ty: String,
    ts_ms: Option<i64>,
}

/// Time-bounded ring buffer of raw `tick` stream frames, newest at the back.
pub struct TickHistory {
    inner: Mutex<VecDeque<(i64, String)>>,
}

impl TickHistory {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(VecDeque::new()),
        }
    }

    /// Record a raw stream frame. No-op for non-tick or unparseable frames.
    /// Trimming uses the incoming tick's `ts_ms` as "now" rather than wall
    /// clock, so a stalled upstream can't silently evict a still-valid window.
    pub fn record(&self, raw: &str) {
        let Ok(meta) = serde_json::from_str::<FrameMeta>(raw) else {
            return;
        };
        if meta.ty != "tick" {
            return;
        }
        let Some(ts) = meta.ts_ms else {
            return;
        };
        let mut buf = self.inner.lock().unwrap();
        buf.push_back((ts, raw.to_string()));
        let cutoff = ts - WINDOW_MS;
        while buf.front().is_some_and(|(t, _)| *t < cutoff) {
            buf.pop_front();
        }
        while buf.len() > MAX_ENTRIES {
            buf.pop_front();
        }
    }

    /// Build a `{"type":"history","ticks":[...]}` frame from the retained tick
    /// frames (oldest→newest). Returns `None` when empty so the caller skips the
    /// send. Raw tick JSON is embedded verbatim — each entry is already a valid
    /// JSON object, so no re-serialization is needed.
    pub fn snapshot_frame(&self) -> Option<String> {
        let buf = self.inner.lock().unwrap();
        if buf.is_empty() {
            return None;
        }
        let mut s = String::from(r#"{"type":"history","ticks":["#);
        for (i, (_, raw)) in buf.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push_str(raw);
        }
        s.push_str("]}");
        Some(s)
    }
}

impl Default for TickHistory {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tick(seq: u64, ts_ms: i64) -> String {
        format!(
            r#"{{"type":"tick","asset":"ETH","run_id":1,"seq":{seq},"ts_ms":{ts_ms},"mid":2000.0,"vol_annualized":0.8,"source_count":3}}"#
        )
    }

    #[test]
    fn retains_only_tick_frames() {
        let h = TickHistory::new();
        h.record(&tick(1, 1000));
        h.record(r#"{"type":"heartbeat","ts_ms":1001}"#);
        h.record(r#"{"type":"status","asset":"ETH","state":"normal","reason":"","run_id":1,"ts_ms":1002}"#);
        h.record("not json at all");
        let frame = h.snapshot_frame().unwrap();
        // Exactly one tick embedded; the non-tick frames were dropped.
        assert_eq!(frame.matches(r#""type":"tick""#).count(), 1);
    }

    #[test]
    fn snapshot_is_valid_json_oldest_first() {
        let h = TickHistory::new();
        h.record(&tick(1, 1000));
        h.record(&tick(2, 1050));
        let frame = h.snapshot_frame().unwrap();
        let v: serde_json::Value = serde_json::from_str(&frame).expect("valid json");
        let ticks = v["ticks"].as_array().unwrap();
        assert_eq!(ticks.len(), 2);
        assert_eq!(ticks[0]["seq"], 1, "oldest first");
        assert_eq!(ticks[1]["seq"], 2);
    }

    #[test]
    fn trims_entries_older_than_window() {
        let h = TickHistory::new();
        h.record(&tick(1, 0));
        h.record(&tick(2, 10_000));
        // 160 s after the first tick → first is now outside the 150 s window.
        h.record(&tick(3, 160_000));
        let v: serde_json::Value = serde_json::from_str(&h.snapshot_frame().unwrap()).unwrap();
        let seqs: Vec<u64> = v["ticks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["seq"].as_u64().unwrap())
            .collect();
        assert_eq!(seqs, vec![2, 3], "seq 1 evicted by window trim");
    }

    #[test]
    fn empty_buffer_yields_no_frame() {
        let h = TickHistory::new();
        assert!(h.snapshot_frame().is_none());
    }
}
