//! Env-derived process config.
//!
//! Per repo CLAUDE.md: no string-literal port fallbacks. A missing env var
//! is a setup bug — fail loudly so the worktree dev-env can wire the value.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::env;
use tap_trading_oracle_types::AssetSymbol;

#[derive(Debug, Clone)]
pub struct AggregatorConfig {
    pub bind_addr: String,
    pub hermes_base_url: String,
    pub binance_ws_url: String,
    pub bybit_ws_url: String,
    pub okx_ws_url: String,
}

/// Pyth Hermes mainnet (Stable channel) feed IDs, 32-byte hex without a leading
/// `0x`. Keys match Hermes' `parsed[].id`. Source: ORACLE_SPEC §2 (docs.pyth.network).
mod pyth_feed_ids {
    pub const ETH: &str = "ff61491a931112ddf1bd8147cd1b641375f79f5825126d665480874634fd0ace";
    pub const BTC: &str = "e62df6c8b4a85fe1a67db44dc12de5db330f7ac66b72dc658afedf0f4a415b43";
    pub const SUI: &str = "23d7315113f5b1d3ba7a83604c44b94d79f4fd69af77f804fc7f920a6dc65744";
}

impl AggregatorConfig {
    /// Load from env. `TAP_AGGREGATOR_PORT` is mandatory.
    pub fn from_env() -> Result<Self> {
        let port: u16 = env::var("TAP_AGGREGATOR_PORT")
            .context("TAP_AGGREGATOR_PORT must be set by the worktree dev-env")?
            .parse()
            .context("TAP_AGGREGATOR_PORT must parse as u16")?;
        let bind_addr = format!("0.0.0.0:{port}");

        Ok(Self {
            bind_addr,
            hermes_base_url: "https://hermes.pyth.network".to_string(),
            binance_ws_url: "wss://stream.binance.com:9443/ws".to_string(),
            bybit_ws_url: "wss://stream.bybit.com/v5/public/spot".to_string(),
            okx_ws_url: "wss://ws.okx.com:8443/ws/v5/public".to_string(),
        })
    }

    /// Feed-id → asset map for Pyth mainnet, ready to hand to `PythSource::new`.
    /// Keys match Hermes' `parsed[].id` (hex, no `0x`).
    pub fn pyth_feeds(&self) -> HashMap<String, AssetSymbol> {
        use pyth_feed_ids::*;
        [
            (ETH, AssetSymbol::Eth),
            (BTC, AssetSymbol::Btc),
            (SUI, AssetSymbol::Sui),
        ]
        .into_iter()
        .map(|(id, asset)| (id.to_string(), asset))
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> AggregatorConfig {
        AggregatorConfig {
            bind_addr: "0.0.0.0:0".to_string(),
            hermes_base_url: "https://example".to_string(),
            binance_ws_url: String::new(),
            bybit_ws_url: String::new(),
            okx_ws_url: String::new(),
        }
    }

    #[test]
    fn pyth_feeds_cover_all_supported_assets() {
        let feeds = cfg().pyth_feeds();
        let assets: std::collections::HashSet<_> = feeds.values().copied().collect();
        for a in crate::constants::SUPPORTED_ASSETS {
            assert!(assets.contains(&a), "missing feed for {a:?}");
        }
    }
}
