//! `quote_bps <lo$> <hi$> <t_open_ms> <t_close_ms> <mid> <vol> <tap_ms>` — print
//! the locked multiplier in basis points for a cell, computed with the live
//! pricing engine and the canonical floor conversion. The e2e harness uses this
//! so the on-chain `multiplier_bps` it mints equals what the verifier recomputes
//! (the consistency the >1-bps verify check depends on).

use tap_trading_pricing_engine::{compute_multiplier, AssetSymbol, Cell, OracleState, PricingConfig};
use tap_trading_proof_types::multiplier_f64_to_bps;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() != 7 {
        eprintln!("usage: quote_bps <lo$> <hi$> <t_open_ms> <t_close_ms> <mid> <vol> <tap_ms>");
        std::process::exit(2);
    }
    let p = |i: usize| a[i].parse::<f64>().expect("number");
    let cell = Cell {
        asset: AssetSymbol::Btc, // label only; pricing is asset-agnostic
        strike_lo: p(0),
        strike_hi: p(1),
        t_open_ms: a[2].parse().expect("t_open"),
        t_close_ms: a[3].parse().expect("t_close"),
    };
    let oracle = OracleState {
        asset: AssetSymbol::Btc,
        spot: p(4),
        sigma_annualized: p(5),
        timestamp_ms: a[6].parse().expect("tap_ms"),
    };
    let tap_ms: u64 = a[6].parse().expect("tap_ms");
    let m = compute_multiplier(&cell, &oracle, &PricingConfig::default(), tap_ms).expect("multiplier");
    println!("{}", multiplier_f64_to_bps(m));
}
