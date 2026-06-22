//! Pure, dependency-light replay verifier for Walrus proof blobs (ADR-0011 §5).
//!
//! `verify` re-derives two things from the blob and nothing else, then compares
//! them to what the blob claims:
//!   (a) the locked multiplier, recomputed with the SAME `tap-trading-pricing-engine`
//!       the server used (no formula reimplementation — drift-proof by sharing code), and
//!   (b) the outcome, by re-running `tap-trading-touch` over `evidence_ticks`
//!       (the SAME path-segment logic the settlement worker settled on).
//!
//! No IO, no clock: callers fetch the blob (e.g. via the `walrus` CLI) and hand
//! it in. This compiles to WASM unchanged for the in-browser "Verify this tap".

use tap_trading_pricing_engine::{compute_multiplier, AssetSymbol, Cell, OracleState, PricingConfig};
use tap_trading_proof_types::{multiplier_f64_to_bps, Outcome, ProofBlob, BPS_EPSILON};
use tap_trading_touch::path_touches_band;

/// Oracle base-unit scale: on-chain/blob prices are USD × 1e9.
const ORACLE_PRICE_SCALE: f64 = 1e9;

#[derive(Debug, Clone, PartialEq)]
pub enum VerifyResult {
    Valid,
    /// The recomputed multiplier disagrees with the locked `multiplier_bps`.
    MultiplierMismatch { claimed_bps: u64, recomputed_bps: u64 },
    /// Replaying touch over the evidence yields a different outcome than claimed.
    OutcomeMismatch { claimed: Outcome, recomputed: Outcome },
    /// The evidence ticks don't span the part of the window needed to judge the
    /// claimed outcome (empty, missing the window head, or — for a LOSS —
    /// missing the tail through `t_close`).
    InsufficientEvidence,
}

pub fn verify(blob: &ProofBlob) -> VerifyResult {
    let ticks = &blob.settlement.evidence_ticks;
    let t_open = blob.window.t_open_ms as i64;
    let t_close = blob.window.t_close_ms as i64;

    // --- Evidence head coverage (cheap structural gate) ---
    let Some(first) = ticks.first() else {
        return VerifyResult::InsufficientEvidence;
    };
    // The entry into the window must be observable: a tick at or before t_open.
    if first.ts_ms > t_open {
        return VerifyResult::InsufficientEvidence;
    }

    // --- (a) Recompute the locked multiplier ---
    let asset = asset_from_str(&blob.asset);
    let cell = Cell {
        asset,
        strike_lo: blob.band.lo as f64 / ORACLE_PRICE_SCALE,
        strike_hi: blob.band.hi as f64 / ORACLE_PRICE_SCALE,
        t_open_ms: blob.window.t_open_ms,
        t_close_ms: blob.window.t_close_ms,
    };
    let oracle = OracleState {
        asset,
        spot: blob.quote_at_tap.mid,
        sigma_annualized: blob.quote_at_tap.vol_annualized,
        timestamp_ms: blob.quote_at_tap.tap_ms,
    };
    let recomputed_bps = match compute_multiplier(
        &cell,
        &oracle,
        &PricingConfig::default(),
        blob.quote_at_tap.tap_ms,
    ) {
        Ok(m) => multiplier_f64_to_bps(m),
        // Bad oracle inputs in the blob → it can't reproduce the claim.
        Err(_) => {
            return VerifyResult::MultiplierMismatch {
                claimed_bps: blob.multiplier_bps,
                recomputed_bps: 0,
            }
        }
    };
    if recomputed_bps.abs_diff(blob.multiplier_bps) > BPS_EPSILON {
        return VerifyResult::MultiplierMismatch {
            claimed_bps: blob.multiplier_bps,
            recomputed_bps,
        };
    }

    // --- (b) Re-run touch over the evidence path ---
    let lo = blob.band.lo as f64 / ORACLE_PRICE_SCALE;
    let hi = blob.band.hi as f64 / ORACLE_PRICE_SCALE;
    let detected_touch = detect_touch(ticks, t_open, t_close, lo, hi);

    match blob.settlement.outcome {
        Outcome::Won => {
            if detected_touch.is_none() {
                VerifyResult::OutcomeMismatch {
                    claimed: Outcome::Won,
                    recomputed: Outcome::Lost,
                }
            } else {
                VerifyResult::Valid
            }
        }
        Outcome::Lost => {
            // A no-touch claim must be backed by evidence through t_close.
            if ticks.last().map(|t| t.ts_ms).unwrap_or(i64::MIN) < t_close {
                return VerifyResult::InsufficientEvidence;
            }
            if detected_touch.is_some() {
                VerifyResult::OutcomeMismatch {
                    claimed: Outcome::Lost,
                    recomputed: Outcome::Won,
                }
            } else {
                VerifyResult::Valid
            }
        }
        // VOID is an oracle-gap refund (SYSTEM_DESIGN §9.1). The gap is an
        // aggregator Status event, not a tick, so it can't be replayed from the
        // tick path; v1 accepts a VOID proof's multiplier + structure without
        // re-deriving the gap. Documented limitation.
        Outcome::Void => VerifyResult::Valid,
    }
}

/// First in-window tick whose arriving segment crosses the band, mirroring the
/// worker: each in-window tick is tested against the immediately preceding tick
/// (which may be pre-window — that entry segment matters), point-sampled when
/// there is no predecessor. Returns the crossing tick's seq.
fn detect_touch(
    ticks: &[tap_trading_proof_types::EvidenceTick],
    t_open: i64,
    t_close: i64,
    lo: f64,
    hi: f64,
) -> Option<u64> {
    let mut prev: Option<f64> = None;
    for t in ticks {
        if t.ts_ms > t_close {
            break;
        }
        if t.ts_ms >= t_open && path_touches_band(prev, t.mid, lo, hi) {
            return Some(t.seq);
        }
        prev = Some(t.mid);
    }
    None
}

fn asset_from_str(s: &str) -> AssetSymbol {
    // Asset is a label; the pricing math is asset-agnostic. Unknown strings
    // fall back to BTC (they don't change the recomputed multiplier).
    match s {
        "ETH" => AssetSymbol::Eth,
        "SUI" => AssetSymbol::Sui,
        _ => AssetSymbol::Btc,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tap_trading_proof_types::{Band, EvidenceTick, QuoteAtTap, Settlement, Window, PROOF_SCHEMA_VERSION};

    // Band [70_000, 70_010), window [2000, 12000], tap at 1000.
    const BAND_LO_USD: f64 = 70_000.0;
    const BAND_HI_USD: f64 = 70_010.0;
    const T_OPEN: u64 = 2_000;
    const T_CLOSE: u64 = 12_000;
    const TAP_MS: u64 = 1_000;
    const MID: f64 = 70_005.0;
    const VOL: f64 = 0.6;

    fn correct_multiplier_bps() -> u64 {
        let cell = Cell {
            asset: AssetSymbol::Btc,
            strike_lo: BAND_LO_USD,
            strike_hi: BAND_HI_USD,
            t_open_ms: T_OPEN,
            t_close_ms: T_CLOSE,
        };
        let oracle = OracleState {
            asset: AssetSymbol::Btc,
            spot: MID,
            sigma_annualized: VOL,
            timestamp_ms: TAP_MS,
        };
        let m = compute_multiplier(&cell, &oracle, &PricingConfig::default(), TAP_MS).unwrap();
        multiplier_f64_to_bps(m)
    }

    fn blob(outcome: Outcome, evidence: Vec<EvidenceTick>) -> ProofBlob {
        ProofBlob {
            v: PROOF_SCHEMA_VERSION,
            position_id: "0xpos".into(),
            vault_id: "0xvault".into(),
            owner: "0xowner".into(),
            asset: "BTC".into(),
            band: Band {
                lo: (BAND_LO_USD * 1e9) as u64,
                hi: (BAND_HI_USD * 1e9) as u64,
            },
            window: Window { t_open_ms: T_OPEN, t_close_ms: T_CLOSE },
            stake: 1_000_000,
            multiplier_bps: correct_multiplier_bps(),
            quote_at_tap: QuoteAtTap {
                oracle_run_id: 1,
                oracle_seq: 0,
                tap_ms: TAP_MS,
                mid: MID,
                vol_annualized: VOL,
                formula_version: "tick_v2_window_aware".into(),
                floor_curve: "1.00+0.0000*tau".into(),
            },
            settlement: Settlement {
                outcome,
                touch_seq: None,
                touch_mid: None,
                evidence_ticks: evidence,
                settled_at_ms: T_CLOSE,
                sui_tx_digest: "digest".into(),
            },
        }
    }

    fn tick(seq: u64, ts_ms: i64, mid: f64) -> EvidenceTick {
        EvidenceTick { seq, ts_ms, mid }
    }

    // pre-window tick below band, then an in-window tick inside the band.
    fn won_evidence() -> Vec<EvidenceTick> {
        vec![tick(0, 1_000, 69_990.0), tick(1, 3_000, 70_005.0)]
    }

    #[test]
    fn valid_won_proof_verifies() {
        assert_eq!(verify(&blob(Outcome::Won, won_evidence())), VerifyResult::Valid);
    }

    #[test]
    fn valid_lost_proof_verifies() {
        // Spans the full window, never enters the band.
        let evidence = vec![
            tick(0, 1_000, 69_990.0),
            tick(1, 5_000, 69_995.0),
            tick(2, 12_000, 69_998.0),
        ];
        assert_eq!(verify(&blob(Outcome::Lost, evidence)), VerifyResult::Valid);
    }

    #[test]
    fn tampered_multiplier_is_caught() {
        let mut b = blob(Outcome::Won, won_evidence());
        b.multiplier_bps += 50;
        assert!(matches!(verify(&b), VerifyResult::MultiplierMismatch { .. }));
    }

    #[test]
    fn won_claim_with_no_touch_evidence_is_outcome_mismatch() {
        // Claims WON but the path never enters the band.
        let evidence = vec![tick(0, 1_000, 69_990.0), tick(1, 3_000, 69_995.0)];
        assert!(matches!(
            verify(&blob(Outcome::Won, evidence)),
            VerifyResult::OutcomeMismatch { claimed: Outcome::Won, .. }
        ));
    }

    #[test]
    fn lost_claim_truncated_before_close_is_insufficient() {
        // No tick reaches t_close, so "never touched" is unprovable.
        let evidence = vec![tick(0, 1_000, 69_990.0), tick(1, 5_000, 69_995.0)];
        assert_eq!(verify(&blob(Outcome::Lost, evidence)), VerifyResult::InsufficientEvidence);
    }

    #[test]
    fn empty_evidence_is_insufficient() {
        assert_eq!(verify(&blob(Outcome::Won, vec![])), VerifyResult::InsufficientEvidence);
    }

    #[test]
    fn missing_window_head_is_insufficient() {
        // First tick is after t_open → the entry into the window is unobserved.
        let evidence = vec![tick(5, 4_000, 70_005.0)];
        assert_eq!(verify(&blob(Outcome::Won, evidence)), VerifyResult::InsufficientEvidence);
    }

    #[test]
    fn golden_fixture_verifies_valid() {
        // The committed fixture (generated by `gen_proof_fixture`) must always
        // verify Valid — guards the schema + the verify path against drift.
        let json = include_str!("../tests/fixtures/proof_won.json");
        let blob: ProofBlob = serde_json::from_str(json).unwrap();
        assert_eq!(verify(&blob), VerifyResult::Valid);
    }

    #[test]
    fn segment_leap_win_verifies_valid() {
        // The regression this whole shared-touch design exists to prevent:
        // no single tick sits inside the band, but the path LEAPS over it
        // between two ticks. Point-sampling would call this LOST and the proof
        // would (wrongly) read as OutcomeMismatch; the shared segment logic
        // confirms the worker's WON.
        let evidence = vec![tick(0, 1_000, 69_990.0), tick(1, 3_000, 70_050.0)];
        assert_eq!(verify(&blob(Outcome::Won, evidence)), VerifyResult::Valid);
    }
}
