//! Binance Spot WS source.
//!
//! Channel `{symbol}@bookTicker` — best bid/ask, pushed on every BBO change
//! (continuous, sub-second). We emit the book mid `(bid+ask)/2`, not last-trade:
//! trade prints fall silent on a calm asset for seconds, so a trade-based source
//! keeps dropping out of the consensus fresh-set and teleporting the median.
//! One socket per symbol — Binance's combined stream exists but the per-symbol
//! URL is simpler to reconnect.

use crate::constants::{BACKOFF_INITIAL_MS, SOURCE_READ_IDLE_TIMEOUT_MS};
use crate::sources::{sleep_jittered, Source, SourceId, SourceTick};
use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use std::time::Duration;
use tap_trading_oracle_types::AssetSymbol;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

pub struct BinanceSource {
    pub base_url: String,
}

#[derive(Debug, Clone, Copy)]
struct SymbolBinding {
    asset: AssetSymbol,
    binance_path: &'static str,
}

const SYMBOLS: &[SymbolBinding] = &[
    SymbolBinding {
        asset: AssetSymbol::Eth,
        binance_path: "ethusdt@bookTicker",
    },
    SymbolBinding {
        asset: AssetSymbol::Btc,
        binance_path: "btcusdt@bookTicker",
    },
    SymbolBinding {
        asset: AssetSymbol::Sui,
        binance_path: "suiusdt@bookTicker",
    },
];

#[async_trait]
impl Source for BinanceSource {
    fn id(&self) -> SourceId {
        SourceId::Binance
    }

    async fn run(self: Box<Self>, tx: mpsc::Sender<SourceTick>) {
        // One task per symbol on a JoinSet so a panic in any of them surfaces
        // immediately (and loudly) instead of being silently swallowed —
        // `run_one` loops forever, so a completed join means it panicked.
        let mut set = tokio::task::JoinSet::new();
        for sym in SYMBOLS {
            let url = format!("{}/{}", self.base_url, sym.binance_path);
            let asset = sym.asset;
            let tx_cloned = tx.clone();
            set.spawn(async move {
                run_one(url, asset, tx_cloned).await;
                asset
            });
        }
        while let Some(res) = set.join_next().await {
            match res {
                Ok(asset) => {
                    tracing::error!(?asset, "binance symbol task exited; feed lost until restart")
                }
                Err(e) => {
                    tracing::error!(error = %e, "binance symbol task panicked; feed lost until restart")
                }
            }
        }
    }
}

async fn run_one(url: String, asset: AssetSymbol, tx: mpsc::Sender<SourceTick>) {
    let mut backoff_ms: u64 = BACKOFF_INITIAL_MS;
    loop {
        tracing::info!(%url, asset = %format!("{asset:?}"), "binance connecting");
        let (mut ws, _resp) = match tokio_tungstenite::connect_async(&url).await {
            Ok(pair) => pair,
            Err(e) => {
                tracing::warn!(error = %e, "binance connect failed");
                sleep_jittered(&mut backoff_ms).await;
                continue;
            }
        };
        // Reset backoff only after real data arrives (`proven`), not on TCP
        // connect — an accept-then-drop server would otherwise hammer reconnects
        // at the 100 ms floor instead of climbing toward the cap.
        let mut proven = false;
        loop {
            // Read-idle timeout: a half-open socket (silent network drop) would
            // otherwise park `ws.next()` forever with no reconnect.
            let next =
                tokio::time::timeout(Duration::from_millis(SOURCE_READ_IDLE_TIMEOUT_MS), ws.next())
                    .await;
            let msg = match next {
                Err(_) => {
                    tracing::warn!("binance read idle; reconnecting");
                    break;
                }
                Ok(None) => break,
                Ok(Some(Ok(m))) => m,
                Ok(Some(Err(_))) => {
                    tracing::warn!("binance ws error; reconnecting");
                    break;
                }
            };
            if !proven {
                backoff_ms = BACKOFF_INITIAL_MS;
                proven = true;
            }
            match msg {
                Message::Text(text) => {
                    if let Some(tick) = parse_binance_book_ticker(&text, asset) {
                        // Non-blocking: a blocked send on a full channel would
                        // stall this loop and starve the ping/pong below,
                        // dropping the socket. Latest-wins, so dropping a tick
                        // under backpressure is harmless.
                        let _ = tx.try_send(tick);
                    }
                }
                Message::Ping(payload) => {
                    let _ = ws.send(Message::Pong(payload)).await;
                }
                Message::Close(_) => {
                    tracing::warn!("binance ws closed; reconnecting");
                    break;
                }
                _ => {}
            }
        }
        sleep_jittered(&mut backoff_ms).await;
    }
}

#[derive(Deserialize)]
struct BinanceBookTicker {
    /// `"b"` best bid price, `"a"` best ask price (strings) in Binance's
    /// bookTicker schema.
    b: String,
    a: String,
}

fn parse_binance_book_ticker(raw: &str, asset: AssetSymbol) -> Option<SourceTick> {
    let parsed: BinanceBookTicker = serde_json::from_str(raw).ok()?;
    let bid: f64 = parsed.b.parse().ok()?;
    let ask: f64 = parsed.a.parse().ok()?;
    let price = 0.5 * (bid + ask);
    if !price.is_finite() || bid <= 0.0 || ask <= 0.0 {
        return None;
    }
    let ts_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis() as i64;
    Some(SourceTick {
        source: SourceId::Binance,
        asset,
        price,
        ts_ms,
        pyth_conf_bps: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_book_ticker_uses_mid() {
        let raw = r#"{"u":77481318861,"s":"ETHUSDT","b":"1669.17","B":"15.9","a":"1669.19","A":"2.9"}"#;
        let t = parse_binance_book_ticker(raw, AssetSymbol::Eth).unwrap();
        assert_eq!(t.source, SourceId::Binance);
        assert_eq!(t.asset, AssetSymbol::Eth);
        assert!((t.price - 1669.18).abs() < 1e-9); // (1669.17 + 1669.19) / 2
        assert!(t.pyth_conf_bps.is_none());
    }

    #[test]
    fn parse_book_ticker_rejects_unparseable() {
        assert!(parse_binance_book_ticker("not-json", AssetSymbol::Eth).is_none());
    }

    #[test]
    fn parse_book_ticker_rejects_zero_side() {
        let raw = r#"{"b":"0","a":"1669.19"}"#;
        assert!(parse_binance_book_ticker(raw, AssetSymbol::Eth).is_none());
    }
}
