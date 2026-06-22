//! The self-contained proof blob published to Walrus for every USDC settlement
//! (ADR-0011 §1). It carries everything a third party needs to replay one tap:
//! the locked multiplier inputs and the full oracle path over the window. The
//! verifier (`tap-trading-proof-verifier`) re-derives the multiplier and the
//! outcome from these fields and nothing else.
//!
//! This crate is intentionally dependency-light (serde only) so it — and the
//! verifier built on it — can compile to WASM for the in-browser "Verify this
//! tap" button.

use serde::{Deserialize, Serialize};

/// Proof schema version. Bumped if the blob shape changes.
pub const PROOF_SCHEMA_VERSION: u32 = 1;

/// Verifier multiplier-equality slack, in basis points. The conversion floors
/// (see `multiplier_f64_to_bps`), so a correct recompute matches exactly; the
/// ±1 bps covers the rare integer-bps boundary where f64 rounding across
/// platforms could differ by one unit.
pub const BPS_EPSILON: u64 = 1;

/// Canonical float→bps conversion for the on-chain multiplier (MATH_SPEC §4.4).
/// **Floor**, not round: the player is never charged for a fractional bps they
/// didn't receive, and the verifier's check becomes an exact integer equality.
/// Both the USDC mint path (which writes `multiplier_bps` on-chain) and the
/// verifier MUST use this exact function.
pub fn multiplier_f64_to_bps(m: f64) -> u64 {
    (m * 10_000.0).floor() as u64
}

/// Price band in oracle base units (USD price × 1e9), matching the on-chain
/// `Position.strike_lo/hi`. The verifier divides by 1e9 to get the dollar prices
/// the pricing engine and touch detection use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Band {
    pub lo: u64,
    pub hi: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    pub t_open_ms: u64,
    pub t_close_ms: u64,
}

/// What the pricing engine saw at tap. `tap_ms` is the server's tap timestamp
/// (= `positions.created_at_ms`): the multiplier depends on it via
/// `tau = t_close − tap_ms`, so it is required for an exact replay. (ADR-0011's
/// sketch omitted it; this field is the documented deviation that makes the
/// recompute reproducible.)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuoteAtTap {
    pub oracle_run_id: u64,
    pub oracle_seq: u64,
    pub tap_ms: u64,
    pub mid: f64,
    pub vol_annualized: f64,
    pub formula_version: String,
    pub floor_curve: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Outcome {
    Won,
    Lost,
    Void,
}

/// One oracle tick on the settlement path. A subset of `OracleTick` — only the
/// fields touch re-detection needs.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EvidenceTick {
    pub seq: u64,
    pub ts_ms: i64,
    pub mid: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settlement {
    pub outcome: Outcome,
    /// Seq of the tick that closed the first band-crossing segment (None for
    /// LOST/VOID).
    pub touch_seq: Option<u64>,
    pub touch_mid: Option<f64>,
    /// The full oracle path over `[t_open, t_close]`, ascending by seq.
    pub evidence_ticks: Vec<EvidenceTick>,
    pub settled_at_ms: u64,
    pub sui_tx_digest: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProofBlob {
    pub v: u32,
    pub position_id: String,
    pub vault_id: String,
    pub owner: String,
    pub asset: String,
    pub band: Band,
    pub window: Window,
    pub stake: u64,
    pub multiplier_bps: u64,
    pub quote_at_tap: QuoteAtTap,
    pub settlement: Settlement,
}

/// One Walrus blob carries a batch of proofs (settlements are flushed every
/// ~60s, not per-tap, so per-tap Walrus cost ≈ 0). A settlement row records the
/// blob id plus its `proof_index` into `proofs`; consumers extract that single
/// `ProofBlob` and verify it independently (the verifier is unchanged).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatchProofBlob {
    pub v: u32,
    pub proofs: Vec<ProofBlob>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_conversion_truncates() {
        assert_eq!(multiplier_f64_to_bps(1.9580), 19_580);
        assert_eq!(multiplier_f64_to_bps(1.95809), 19_580); // floors, not rounds
        assert_eq!(multiplier_f64_to_bps(1.0), 10_000);
        assert_eq!(multiplier_f64_to_bps(100.0), 1_000_000);
    }

    #[test]
    fn blob_round_trips_through_json() {
        let blob = ProofBlob {
            v: PROOF_SCHEMA_VERSION,
            position_id: "0x1".into(),
            vault_id: "0x2".into(),
            owner: "0x3".into(),
            asset: "BTC".into(),
            band: Band { lo: 75_832_000_000_000, hi: 75_842_000_000_000 },
            window: Window { t_open_ms: 1_779_564_600_000, t_close_ms: 1_779_564_660_000 },
            stake: 100_000,
            multiplier_bps: 19_580,
            quote_at_tap: QuoteAtTap {
                oracle_run_id: 173,
                oracle_seq: 48_213,
                tap_ms: 1_779_564_599_000,
                mid: 75_837.06,
                vol_annualized: 0.61,
                formula_version: "tick_v2_window_aware".into(),
                floor_curve: "1.00+0.0000*tau".into(),
            },
            settlement: Settlement {
                outcome: Outcome::Won,
                touch_seq: Some(48_999),
                touch_mid: Some(75_838.40),
                evidence_ticks: vec![EvidenceTick { seq: 48_213, ts_ms: 1, mid: 75_837.06 }],
                settled_at_ms: 1_779_564_661_000,
                sui_tx_digest: "abc".into(),
            },
        };
        let json = serde_json::to_string(&blob).unwrap();
        let back: ProofBlob = serde_json::from_str(&json).unwrap();
        assert_eq!(back, blob);
        assert_eq!(back.settlement.outcome, Outcome::Won);
    }
}
