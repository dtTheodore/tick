//! Dev tool: emit a numerically-consistent golden WON proof blob to stdout, so
//! `proof-verify` and the verifier's golden test have a real fixture to check.
//! The multiplier is computed with the live pricing engine, so the fixture is
//! correct by construction. Run:
//!   cargo run -p tap-trading-proof-verifier --bin gen_proof_fixture \
//!     > proof-verifier/tests/fixtures/proof_won.json

use tap_trading_pricing_engine::{compute_multiplier, AssetSymbol, Cell, OracleState, PricingConfig};
use tap_trading_proof_types::{
    multiplier_f64_to_bps, Band, EvidenceTick, Outcome, ProofBlob, QuoteAtTap, Settlement, Window,
    PROOF_SCHEMA_VERSION,
};

fn main() {
    let (band_lo, band_hi) = (70_000.0_f64, 70_010.0_f64);
    let (t_open, t_close, tap_ms) = (2_000_u64, 12_000_u64, 1_000_u64);
    let (mid, vol) = (70_005.0_f64, 0.6_f64);

    let cell = Cell {
        asset: AssetSymbol::Btc,
        strike_lo: band_lo,
        strike_hi: band_hi,
        t_open_ms: t_open,
        t_close_ms: t_close,
    };
    let oracle = OracleState {
        asset: AssetSymbol::Btc,
        spot: mid,
        sigma_annualized: vol,
        timestamp_ms: tap_ms,
    };
    let m = compute_multiplier(&cell, &oracle, &PricingConfig::default(), tap_ms).unwrap();

    let blob = ProofBlob {
        v: PROOF_SCHEMA_VERSION,
        position_id: "0xexample_position".into(),
        vault_id: "0xexample_vault".into(),
        owner: "0xexample_owner".into(),
        asset: "BTC".into(),
        band: Band { lo: (band_lo * 1e9) as u64, hi: (band_hi * 1e9) as u64 },
        window: Window { t_open_ms: t_open, t_close_ms: t_close },
        stake: 1_000_000,
        multiplier_bps: multiplier_f64_to_bps(m),
        quote_at_tap: QuoteAtTap {
            oracle_run_id: 1,
            oracle_seq: 0,
            tap_ms,
            mid,
            vol_annualized: vol,
            formula_version: "tick_v2_window_aware".into(),
            floor_curve: "1.00+0.0000*tau".into(),
        },
        settlement: Settlement {
            outcome: Outcome::Won,
            touch_seq: Some(1),
            touch_mid: Some(70_005.0),
            evidence_ticks: vec![
                EvidenceTick { seq: 0, ts_ms: 1_000, mid: 69_990.0 },
                EvidenceTick { seq: 1, ts_ms: 3_000, mid: 70_005.0 },
            ],
            settled_at_ms: 3_000,
            sui_tx_digest: "EXAMPLEdigest1111111111111111111111111111111".into(),
        },
    };
    println!("{}", serde_json::to_string_pretty(&blob).unwrap());
}
