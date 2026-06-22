//! Per-asset ring buffer for `(run_id, seq)` replay. ADR-0008 §5.
//!
//! At 20 Hz × 10 entries = 500 ms of history per asset — the budget the
//! client has between displaying a multiplier and committing the tap.

use crate::constants::RING_SIZE;
use dashmap::DashMap;
use std::collections::VecDeque;
use std::sync::Mutex;
use tap_trading_oracle_types::{AssetSymbol, OracleTick};

/// Reply shape for `GET /ring/:asset/:seq?run_id=N`. ADR-0008 §5.
#[derive(Debug, Clone, PartialEq)]
pub enum RingLookup {
    Hit(OracleTick),
    /// `seq` is older than the oldest entry we still retain. 410 Gone.
    Gone,
    /// `run_id` doesn't match the aggregator's current `run_id`. 409 Conflict.
    Conflict,
}

/// Reply shape for `GET /ring/:asset/range?run_id&from_seq&to_seq` (ADR-0011 §6
/// proof-evidence span). The settlement worker pulls the contiguous tick path
/// over `[t_open, t_close]` in one call rather than N single-seq lookups.
#[derive(Debug, Clone, PartialEq)]
pub enum RangeLookup {
    /// The full inclusive span, in ascending seq order.
    Hit(Vec<OracleTick>),
    /// `from_seq` is older than the oldest entry retained — evidence would be
    /// incomplete. 410 Gone. (The caller marks the proof failed; payout stands.)
    Gone,
    /// `run_id` mismatch. 409 Conflict.
    Conflict,
}

#[derive(Debug, Default)]
pub struct AssetRing {
    entries: VecDeque<OracleTick>,
}

impl AssetRing {
    pub fn push(&mut self, tick: OracleTick) {
        if self.entries.len() == RING_SIZE {
            self.entries.pop_front();
        }
        self.entries.push_back(tick);
    }

    fn newest(&self) -> Option<OracleTick> {
        self.entries.back().copied()
    }

    fn lookup(&self, run_id: u64, seq: u64) -> RingLookup {
        let Some(front) = self.entries.front() else {
            // No data yet for this asset.
            return RingLookup::Gone;
        };
        // Defense-in-depth: the ring verifies run_id itself so it is correct in
        // isolation (and unit-tested as such). The HTTP layer additionally
        // early-outs on a run_id mismatch (api.rs) to cover the empty-ring case
        // this branch can't — an asset with no ticks has no stored run_id to
        // compare. Both layers map a mismatch to 409.
        if front.run_id != run_id {
            return RingLookup::Conflict;
        }
        if seq < front.seq {
            return RingLookup::Gone;
        }
        for t in &self.entries {
            if t.seq == seq {
                return RingLookup::Hit(*t);
            }
        }
        // seq is newer than the newest entry — treat as not-yet-emitted; 410.
        RingLookup::Gone
    }

    fn range(&self, run_id: u64, from_seq: u64, to_seq: u64) -> RangeLookup {
        let Some(front) = self.entries.front() else {
            return RangeLookup::Gone;
        };
        if front.run_id != run_id {
            return RangeLookup::Conflict;
        }
        // Evidence must be complete: if the window opens before our oldest
        // retained seq, the path is missing its head — refuse rather than
        // return a truncated array a verifier would reject as insufficient.
        if from_seq < front.seq {
            return RangeLookup::Gone;
        }
        let span = self
            .entries
            .iter()
            .filter(|t| t.seq >= from_seq && t.seq <= to_seq)
            .copied()
            .collect();
        RangeLookup::Hit(span)
    }
}

#[derive(Debug, Default)]
pub struct RingBuffers {
    inner: DashMap<AssetSymbol, Mutex<AssetRing>>,
}

impl RingBuffers {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, tick: OracleTick) {
        let entry = self.inner.entry(tick.asset).or_default();
        let mut ring = entry.lock().expect("ring mutex poisoned");
        ring.push(tick);
    }

    pub fn get(&self, asset: AssetSymbol, run_id: u64, seq: u64) -> RingLookup {
        let Some(entry) = self.inner.get(&asset) else {
            return RingLookup::Gone;
        };
        let guard = entry.lock().expect("ring mutex poisoned");
        guard.lookup(run_id, seq)
    }

    /// Contiguous tick span `[from_seq, to_seq]` for proof evidence (ADR-0011).
    pub fn range(
        &self,
        asset: AssetSymbol,
        run_id: u64,
        from_seq: u64,
        to_seq: u64,
    ) -> RangeLookup {
        let Some(entry) = self.inner.get(&asset) else {
            return RangeLookup::Gone;
        };
        let guard = entry.lock().expect("ring mutex poisoned");
        guard.range(run_id, from_seq, to_seq)
    }

    /// Newest tick recorded for `asset`, or `None` if none yet. Used by
    /// `GET /healthz` to read the current `source_count` and emit-recency.
    pub fn newest(&self, asset: AssetSymbol) -> Option<OracleTick> {
        let entry = self.inner.get(&asset)?;
        let guard = entry.lock().expect("ring mutex poisoned");
        guard.newest()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tick(seq: u64, run_id: u64) -> OracleTick {
        OracleTick {
            asset: AssetSymbol::Btc,
            run_id,
            seq,
            ts_ms: 1_000 + seq as i64,
            mid: 70_000.0 + seq as f64,
            vol_annualized: 0.6,
            source_count: 3,
        }
    }

    fn ring_with(seqs: std::ops::RangeInclusive<u64>, run_id: u64) -> RingBuffers {
        let rings = RingBuffers::new();
        for seq in seqs {
            rings.push(tick(seq, run_id));
        }
        rings
    }

    #[test]
    fn range_returns_inclusive_span_in_seq_order() {
        let rings = ring_with(100..=130, 1);
        let RangeLookup::Hit(v) = rings.range(AssetSymbol::Btc, 1, 110, 120) else {
            panic!("expected Hit");
        };
        assert_eq!(v.len(), 11);
        assert_eq!(v.first().unwrap().seq, 110);
        assert_eq!(v.last().unwrap().seq, 120);
        // strictly ascending
        assert!(v.windows(2).all(|w| w[0].seq < w[1].seq));
    }

    #[test]
    fn range_gone_when_from_seq_evicted() {
        // RING_SIZE caps retention; pushing more than RING_SIZE evicts the head.
        // We can't push 2400+ cheaply here, so test the logical guard directly:
        // a from_seq below the oldest retained seq is Gone.
        let rings = ring_with(500..=510, 1);
        assert_eq!(rings.range(AssetSymbol::Btc, 1, 499, 505), RangeLookup::Gone);
    }

    #[test]
    fn range_conflict_on_run_id_mismatch() {
        let rings = ring_with(100..=110, 7);
        assert_eq!(rings.range(AssetSymbol::Btc, 8, 100, 110), RangeLookup::Conflict);
    }

    #[test]
    fn range_gone_for_unknown_asset() {
        let rings = ring_with(100..=110, 1);
        assert_eq!(rings.range(AssetSymbol::Eth, 1, 100, 110), RangeLookup::Gone);
    }

    #[test]
    fn range_clamps_to_available_high_end() {
        // to_seq past the newest retained seq returns up to the newest.
        let rings = ring_with(100..=110, 1);
        let RangeLookup::Hit(v) = rings.range(AssetSymbol::Btc, 1, 108, 200) else {
            panic!("expected Hit");
        };
        assert_eq!(v.first().unwrap().seq, 108);
        assert_eq!(v.last().unwrap().seq, 110);
    }
}
