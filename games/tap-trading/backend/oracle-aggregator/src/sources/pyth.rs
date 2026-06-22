//! Pyth Hermes SSE source.
//!
//! Endpoint: `${HERMES}/v2/updates/price/stream?ids[]=…&ids[]=…&parsed=true`.
//! Each event line is JSON: `{ "parsed": [{ "id": "<hex>", "price": {"price":"N","conf":"N","expo":-8,"publish_time":…} }] }`.
//! See https://hermes.pyth.network/docs/.

use crate::constants::{BACKOFF_INITIAL_MS, SOURCE_READ_IDLE_TIMEOUT_MS};
use crate::sources::{sleep_jittered, Source, SourceId, SourceTick};
use async_trait::async_trait;
use futures::StreamExt;
use reqwest_eventsource::{Event, EventSource};
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;
use tap_trading_oracle_types::AssetSymbol;
use tokio::sync::mpsc;

/// Feed-id → asset mapping. Mainnet/testnet feed IDs differ — caller wires
/// the correct set via `PythSource::new`. ORACLE_SPEC §2.
pub struct PythSource {
    hermes_base_url: String,
    feeds: HashMap<String, AssetSymbol>,
}

impl PythSource {
    pub fn new(hermes_base_url: String, feeds: HashMap<String, AssetSymbol>) -> Self {
        Self {
            hermes_base_url,
            feeds,
        }
    }

    fn build_url(&self) -> String {
        let ids: Vec<String> = self.feeds.keys().map(|id| format!("ids[]={id}")).collect();
        format!(
            "{}/v2/updates/price/stream?{}&parsed=true",
            self.hermes_base_url,
            ids.join("&")
        )
    }
}

#[async_trait]
impl Source for PythSource {
    fn id(&self) -> SourceId {
        SourceId::Pyth
    }

    async fn run(self: Box<Self>, tx: mpsc::Sender<SourceTick>) {
        let url = self.build_url();
        let mut backoff_ms: u64 = BACKOFF_INITIAL_MS;
        loop {
            tracing::info!(%url, "pyth hermes connecting");
            let mut stream = EventSource::get(&url);
            // Reset backoff only after a real event arrives, not on SSE open —
            // an accept-then-stall server would otherwise reconnect at the floor.
            let mut proven = false;
            loop {
                // Read-idle timeout: a half-open SSE connection that stops
                // delivering events would otherwise park `next()` forever.
                let next = tokio::time::timeout(
                    Duration::from_millis(SOURCE_READ_IDLE_TIMEOUT_MS),
                    stream.next(),
                )
                .await;
                let ev = match next {
                    Err(_) => {
                        tracing::warn!("pyth hermes read idle; reconnecting");
                        break;
                    }
                    Ok(None) => break,
                    Ok(Some(ev)) => ev,
                };
                match ev {
                    Ok(Event::Open) => {
                        tracing::info!("pyth hermes SSE open");
                    }
                    Ok(Event::Message(msg)) => {
                        if !proven {
                            backoff_ms = BACKOFF_INITIAL_MS;
                            proven = true;
                        }
                        if let Some(ticks) = parse_hermes_frame(&msg.data, &self.feeds) {
                            // Non-blocking — a blocked send would stall this SSE
                            // loop under backpressure; latest-wins.
                            for t in ticks {
                                let _ = tx.try_send(t);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "pyth hermes stream error; reconnecting");
                        break;
                    }
                }
            }
            sleep_jittered(&mut backoff_ms).await;
        }
    }
}

#[derive(Deserialize)]
struct HermesFrame {
    parsed: Vec<HermesParsed>,
}

#[derive(Deserialize)]
struct HermesParsed {
    id: String,
    price: HermesPrice,
}

#[derive(Deserialize)]
struct HermesPrice {
    price: String,
    conf: String,
    expo: i32,
    #[allow(dead_code)]
    publish_time: i64,
}

fn parse_hermes_frame(raw: &str, feeds: &HashMap<String, AssetSymbol>) -> Option<Vec<SourceTick>> {
    let frame: HermesFrame = serde_json::from_str(raw).ok()?;
    let ticks = frame
        .parsed
        .into_iter()
        .filter_map(|p| {
            let asset = feeds.get(p.id.trim_start_matches("0x"))?;
            let raw_price = p.price.price.parse::<i128>().ok()?;
            let raw_conf = p.price.conf.parse::<i128>().ok()?;
            let scale = 10_f64.powi(p.price.expo);
            let price = raw_price as f64 * scale;
            // Guard explicitly like the exchange sources: a non-finite price
            // (e.g. an out-of-range expo overflowing `scale` to ∞) would slip
            // past the conf-bps gate (conf/∞ rounds to 0 bps) and inject ∞ into
            // the median/EMA. ORACLE_SPEC §4.4.
            if !price.is_finite() || price <= 0.0 {
                return None;
            }
            // Reject a malformed (negative or non-finite) confidence rather than
            // clamp it: clamping a negative conf to 0 bps would mark a garbage
            // tick as *highest* confidence and let it pass the conf-reject gate.
            let conf = raw_conf as f64 * scale;
            if !conf.is_finite() || conf < 0.0 {
                return None;
            }
            // `price > 0.0` is guaranteed by the guard above, so the ratio is
            // always well-defined and non-negative.
            let conf_bps = ((conf / price) * 10_000.0).round() as u32;
            Some(SourceTick {
                source: SourceId::Pyth,
                asset: *asset,
                price,
                // ORACLE_SPEC §4.5: server-received timestamp.
                ts_ms: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()?
                    .as_millis() as i64,
                pyth_conf_bps: Some(conf_bps),
            })
        })
        .collect::<Vec<_>>();
    if ticks.is_empty() {
        None
    } else {
        Some(ticks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hermes_frame_yields_one_tick_per_known_feed() {
        let raw = r#"{"parsed":[{"id":"ff61491a931112ddf1bd8147cd1b641375f79f5825126d665480874634fd0ace","price":{"price":"381225000000","conf":"100000000","expo":-8,"publish_time":1747526400}}]}"#;
        let mut feeds = HashMap::new();
        feeds.insert(
            "ff61491a931112ddf1bd8147cd1b641375f79f5825126d665480874634fd0ace".to_string(),
            AssetSymbol::Eth,
        );
        let ticks = parse_hermes_frame(raw, &feeds).unwrap();
        assert_eq!(ticks.len(), 1);
        let t = &ticks[0];
        assert_eq!(t.asset, AssetSymbol::Eth);
        assert!((t.price - 3812.25).abs() < 1e-6, "got {}", t.price);
        // conf = 100_000_000 · 1e-8 = 1.0; bps = (1.0 / 3812.25) · 10000 ≈ 3 bps.
        assert!(t.pyth_conf_bps.unwrap() <= 5);
    }

    #[test]
    fn parse_hermes_frame_drops_unknown_feed() {
        let raw = r#"{"parsed":[{"id":"deadbeef","price":{"price":"1","conf":"1","expo":-8,"publish_time":0}}]}"#;
        let feeds = HashMap::new();
        assert!(parse_hermes_frame(raw, &feeds).is_none());
    }

    #[test]
    fn parse_hermes_frame_rejects_malformed_json() {
        assert!(parse_hermes_frame("not json", &HashMap::new()).is_none());
    }

    #[test]
    fn parse_hermes_frame_rejects_negative_confidence() {
        // A negative conf is malformed; it must be dropped, not clamped to 0 bps
        // (which would let it pass the conf-reject gate as "perfect confidence").
        let raw = r#"{"parsed":[{"id":"feed","price":{"price":"381225000000","conf":"-100000000","expo":-8,"publish_time":0}}]}"#;
        let mut feeds = HashMap::new();
        feeds.insert("feed".to_string(), AssetSymbol::Eth);
        assert!(parse_hermes_frame(raw, &feeds).is_none());
    }

    #[test]
    fn parse_hermes_frame_rejects_non_finite_price() {
        // An out-of-range expo overflows `scale` to ∞; the resulting ∞ price
        // must be dropped, not passed through with conf_bps rounded to 0.
        let raw = r#"{"parsed":[{"id":"feed","price":{"price":"1","conf":"1","expo":400,"publish_time":0}}]}"#;
        let mut feeds = HashMap::new();
        feeds.insert("feed".to_string(), AssetSymbol::Btc);
        assert!(parse_hermes_frame(raw, &feeds).is_none());
    }
}
