//! OKX v5 public WS source.
//!
//! Subscribe: `{"op":"subscribe","args":[{"channel":"bbo-tbt","instId":"ETH-USDT"},…]}`.
//! Data: `{"arg":{"channel":"bbo-tbt",…},"data":[{"asks":[["1668.95",…]],"bids":[["1668.94",…]],…}]}`.
//! `bbo-tbt` is tick-by-tick best bid/offer (continuous, ~10 ms). We emit the
//! book mid `(bid+ask)/2`, not last-trade: trade prints go silent on a calm
//! asset, dropping the source out of the consensus fresh-set and teleporting
//! the median.

use crate::constants::{BACKOFF_INITIAL_MS, SOURCE_READ_IDLE_TIMEOUT_MS, WS_KEEPALIVE_PERIOD_S};
use crate::sources::{sleep_jittered, Source, SourceId, SourceTick};
use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use std::time::Duration;
use tap_trading_oracle_types::AssetSymbol;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

pub struct OkxSource {
    pub url: String,
}

fn instid_to_asset(inst: &str) -> Option<AssetSymbol> {
    match inst {
        "ETH-USDT" => Some(AssetSymbol::Eth),
        "BTC-USDT" => Some(AssetSymbol::Btc),
        "SUI-USDT" => Some(AssetSymbol::Sui),
        _ => None,
    }
}

#[async_trait]
impl Source for OkxSource {
    fn id(&self) -> SourceId {
        SourceId::Okx
    }

    async fn run(self: Box<Self>, tx: mpsc::Sender<SourceTick>) {
        let url = self.url.clone();
        let mut backoff_ms: u64 = BACKOFF_INITIAL_MS;
        loop {
            tracing::info!(%url, "okx connecting");
            let (mut ws, _resp) = match tokio_tungstenite::connect_async(&url).await {
                Ok(pair) => pair,
                Err(e) => {
                    tracing::warn!(error = %e, "okx connect failed");
                    sleep_jittered(&mut backoff_ms).await;
                    continue;
                }
            };

            let sub = serde_json::json!({
                "op": "subscribe",
                "args": [
                    {"channel": "bbo-tbt", "instId": "ETH-USDT"},
                    {"channel": "bbo-tbt", "instId": "BTC-USDT"},
                    {"channel": "bbo-tbt", "instId": "SUI-USDT"}
                ]
            });
            if ws.send(Message::Text(sub.to_string())).await.is_err() {
                // Back off before retrying — otherwise a persistently failing
                // subscribe spins a tight reconnect loop.
                sleep_jittered(&mut backoff_ms).await;
                continue;
            }

            // Split so the keepalive sender and the reader can borrow the socket
            // independently inside the `select!`.
            let (mut write, mut read) = ws.split();
            // OKX drops a socket after ~30 s of no client activity; ping under that.
            let mut keepalive = tokio::time::interval(Duration::from_secs(WS_KEEPALIVE_PERIOD_S));
            keepalive.tick().await; // consume the immediate first tick
            // Reset backoff only after real data arrives, not on TCP connect.
            let mut proven = false;
            loop {
                tokio::select! {
                    _ = keepalive.tick() => {
                        // OKX expects the literal text "ping"; it replies "pong".
                        if write.send(Message::Text("ping".to_string())).await.is_err() {
                            break;
                        }
                    }
                    next = tokio::time::timeout(
                        Duration::from_millis(SOURCE_READ_IDLE_TIMEOUT_MS),
                        read.next(),
                    ) => {
                        let msg = match next {
                            Err(_) => {
                                tracing::warn!("okx read idle; reconnecting");
                                break;
                            }
                            Ok(None) => break,
                            Ok(Some(Ok(m))) => m,
                            Ok(Some(Err(_))) => break,
                        };
                        if !proven {
                            backoff_ms = BACKOFF_INITIAL_MS;
                            proven = true;
                        }
                        match msg {
                            Message::Text(text) => {
                                if is_okx_error(&text) {
                                    tracing::warn!(%text, "okx subscribe error; reconnecting");
                                    break;
                                }
                                if let Some(ticks) = parse_okx_frame(&text) {
                                    // Non-blocking — a blocked send would starve
                                    // the keepalive path and drop the socket.
                                    for t in ticks {
                                        let _ = tx.try_send(t);
                                    }
                                }
                            }
                            Message::Ping(p) => {
                                let _ = write.send(Message::Pong(p)).await;
                            }
                            Message::Close(_) => break,
                            _ => {}
                        }
                    }
                }
            }
            sleep_jittered(&mut backoff_ms).await;
        }
    }
}

/// OKX signals a rejected subscription with `{"event":"error", …}` (no `arg`),
/// which `parse_okx_frame` would silently drop — leaving the task blocked on a
/// socket it never joined. Detect it so the caller can reconnect.
fn is_okx_error(raw: &str) -> bool {
    #[derive(Deserialize)]
    struct OkxEventFrame {
        event: Option<String>,
    }
    serde_json::from_str::<OkxEventFrame>(raw)
        .ok()
        .and_then(|f| f.event)
        .as_deref()
        == Some("error")
}

#[derive(Deserialize)]
struct OkxFrame {
    arg: Option<OkxArg>,
    data: Option<Vec<OkxBbo>>,
}

#[derive(Deserialize)]
struct OkxArg {
    channel: String,
    #[serde(rename = "instId")]
    inst_id: String,
}

/// One BBO snapshot. `asks`/`bids` are `[price, size, _, order_count]` rows;
/// row 0 is the top of book, and only its price (`[0][0]`) is load-bearing here.
#[derive(Deserialize)]
struct OkxBbo {
    asks: Vec<Vec<String>>,
    bids: Vec<Vec<String>>,
}

fn parse_okx_frame(raw: &str) -> Option<Vec<SourceTick>> {
    let frame: OkxFrame = serde_json::from_str(raw).ok()?;
    let arg = frame.arg?;
    if arg.channel != "bbo-tbt" {
        return None;
    }
    let asset = instid_to_asset(&arg.inst_id)?;
    let data = frame.data?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis() as i64;
    let ticks: Vec<SourceTick> = data
        .into_iter()
        .filter_map(|b| {
            let bid: f64 = b.bids.first()?.first()?.parse().ok()?;
            let ask: f64 = b.asks.first()?.first()?.parse().ok()?;
            let price = 0.5 * (bid + ask);
            if !price.is_finite() || bid <= 0.0 || ask <= 0.0 {
                return None;
            }
            Some(SourceTick {
                source: SourceId::Okx,
                asset,
                price,
                ts_ms: now_ms,
                pyth_conf_bps: None,
            })
        })
        .collect();
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
    fn parse_bbo_uses_mid() {
        let raw = r#"{"arg":{"channel":"bbo-tbt","instId":"ETH-USDT"},"data":[{"asks":[["1668.96","30.8","0","14"]],"bids":[["1668.94","9.7","0","9"]],"ts":"1","seqId":1}]}"#;
        let ticks = parse_okx_frame(raw).unwrap();
        assert_eq!(ticks.len(), 1);
        assert_eq!(ticks[0].asset, AssetSymbol::Eth);
        assert!((ticks[0].price - 1668.95).abs() < 1e-9); // (1668.94 + 1668.96) / 2
        assert_eq!(ticks[0].source, SourceId::Okx);
    }

    #[test]
    fn parse_ignores_subscribe_ack() {
        let raw = r#"{"event":"subscribe","arg":{"channel":"bbo-tbt","instId":"ETH-USDT"}}"#;
        // No `data` field → drop.
        assert!(parse_okx_frame(raw).is_none());
    }

    #[test]
    fn parse_ignores_wrong_channel() {
        let raw = r#"{"arg":{"channel":"books","instId":"ETH-USDT"},"data":[{"asks":[["1"]],"bids":[["1"]]}]}"#;
        assert!(parse_okx_frame(raw).is_none());
    }
}
