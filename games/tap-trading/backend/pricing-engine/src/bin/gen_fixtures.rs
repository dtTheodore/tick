//! Emits parity fixtures consumed by the TS port's parity test.
//! Run: cargo run --bin gen_fixtures > ../../ui/tests/fixtures/parity.json
//!
//! Stages:
//! - "ok"            → Rust returned Ok(m); TS must match within 1e-6
//! - "boundary_zero" → Rust returned Ok(0.0) (closed cell); TS must match
//! - "invalid_input" → Rust returned Err(InvalidSpot|InvalidSigma); TS must throw the matching error class

use serde::Serialize;
use tap_trading_pricing_engine::{
    compute_multiplier, AssetSymbol, Cell, OracleState, PricingConfig, PricingError,
};

#[derive(Serialize)]
struct Input {
    s0: f64, lo: f64, hi: f64, sigma: f64, tau_sec: f64, now_ms: i64,
}

#[derive(Serialize)]
struct Case {
    input: Input,
    multiplier: Option<f64>,
    stage: &'static str,
    error_kind: Option<&'static str>,
}

fn case(s0: f64, lo: f64, hi: f64, sigma: f64, tau_sec: f64) -> Case {
    let now_ms = 1_000_000i64;
    let t_close_ms = now_ms + (tau_sec * 1000.0) as i64;
    let cell = Cell {
        asset: AssetSymbol::Eth,
        strike_lo: lo, strike_hi: hi,
        t_open_ms: now_ms as u64,
        t_close_ms: if tau_sec > 0.0 { t_close_ms as u64 } else { now_ms as u64 },
    };
    let oracle = OracleState {
        asset: AssetSymbol::Eth,
        spot: s0,
        sigma_annualized: sigma,
        timestamp_ms: now_ms as u64,
    };
    let input = Input { s0, lo, hi, sigma, tau_sec, now_ms };
    match compute_multiplier(&cell, &oracle, &PricingConfig::default(), now_ms as u64) {
        Ok(m) if m == 0.0 => Case { input, multiplier: Some(0.0), stage: "boundary_zero", error_kind: None },
        Ok(m) => Case { input, multiplier: Some(m), stage: "ok", error_kind: None },
        Err(PricingError::InvalidSpot(_)) =>
            Case { input, multiplier: None, stage: "invalid_input", error_kind: Some("InvalidSpot") },
        Err(PricingError::InvalidSigma(_)) =>
            Case { input, multiplier: None, stage: "invalid_input", error_kind: Some("InvalidSigma") },
        Err(_) =>
            Case { input, multiplier: None, stage: "invalid_input", error_kind: Some("other") },
    }
}

fn main() {
    let mut cases: Vec<Case> = Vec::new();
    // Plausible-range coverage
    for s0 in [100.0, 1_000.0, 10_000.0] {
        for band_pct in [0.0001, 0.001, 0.01] {
            for sigma in [0.05, 0.20, 0.50, 1.50] {
                for tau in [0.5, 2.0, 5.0] {
                    let half = s0 * band_pct;
                    cases.push(case(s0, s0 - half, s0 + half, sigma, tau));
                    cases.push(case(s0 - 0.3 * half, s0 - half, s0 + half, sigma, tau));
                    cases.push(case(s0 + 0.4 * half, s0 - half, s0 + half, sigma, tau));
                }
            }
        }
    }
    // Realistic Tick band: ETH Δ$0.5 around $3812 spot, 5s window, 30% σ
    cases.push(case(3812.25, 3812.00, 3812.50, 0.30, 5.0));
    cases.push(case(3812.49, 3812.00, 3812.50, 0.30, 5.0));
    cases.push(case(3812.01, 3812.00, 3812.50, 0.30, 5.0));
    // Boundary cases (Rust returns Ok(0.0) when cell is closed)
    cases.push(case(100.0, 100.0, 110.0, 0.5, 5.0));
    cases.push(case(110.0, 100.0, 110.0, 0.5, 5.0));
    cases.push(case(105.0, 100.0, 110.0, 1e-9, 5.0));
    cases.push(case(105.0, 100.0, 110.0, 0.5, 0.1));
    cases.push(case(105.0, 100.0, 110.0, 0.5, 30.0));
    // Closed cell (tau_sec = 0 → t_close_ms == now_ms → Ok(0.0))
    cases.push(case(105.0, 100.0, 110.0, 0.5, 0.0));
    // Invalid inputs (Rust returns Err)
    cases.push(case(f64::NAN, 100.0, 110.0, 0.5, 5.0));   // InvalidSpot
    cases.push(case(0.0,      100.0, 110.0, 0.5, 5.0));   // InvalidSpot (<=0)
    cases.push(case(-1.0,     100.0, 110.0, 0.5, 5.0));   // InvalidSpot
    cases.push(case(105.0, 100.0, 110.0, f64::NAN, 5.0)); // InvalidSigma
    cases.push(case(105.0, 100.0, 110.0, -0.1, 5.0));     // InvalidSigma

    println!("{}", serde_json::to_string_pretty(&cases).unwrap());
}
