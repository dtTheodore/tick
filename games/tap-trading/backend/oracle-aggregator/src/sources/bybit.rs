//! Bybit v5 public Spot WS source.
//!
//! One socket, multiple subscriptions:
//!   `{"op":"subscribe","args":["publicTrade.ETHUSDT","publicTrade.BTCUSDT","publicTrade.SUIUSDT"]}`
//! Trade frames arrive as `{"topic":"publicTrade.ETHUSDT","data":[{"p":"3812.45",…}]}`.

use crate::constants::{BACKOFF_INITIAL_MS, SOURCE_READ_IDLE_TIMEOUT_MS, WS_KEEPALIVE_PERIOD_S};
use crate::sources::{sleep_jittered, Source, SourceId, SourceTick};
use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use std::time::Duration;
use tap_trading_oracle_types::AssetSymbol;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

pub struct BybitSource {
    pub url: String,
}

fn symbol_to_asset(sym: &str) -> Option<AssetSymbol> {
    match sym {
        "ETHUSDT" => Some(AssetSymbol::Eth),
        "BTCUSDT" => Some(AssetSymbol::Btc),
        "SUIUSDT" => Some(AssetSymbol::Sui),
        _ => None,
    }
}

#[async_trait]
impl Source for BybitSource {
    fn id(&self) -> SourceId {
        SourceId::Bybit
    }

    async fn run(self: Box<Self>, tx: mpsc::Sender<SourceTick>) {
        let url = self.url.clone();
        let mut backoff_ms: u64 = BACKOFF_INITIAL_MS;
        loop {
            tracing::info!(%url, "bybit connecting");
            let (mut ws, _resp) = match tokio_tungstenite::connect_async(&url).await {
                Ok(pair) => pair,
                Err(e) => {
                    tracing::warn!(error = %e, "bybit connect failed");
                    sleep_jittered(&mut backoff_ms).await;
                    continue;
                }
            };

            // Subscribe message.
            let sub = serde_json::json!({
                "op": "subscribe",
                "args": ["publicTrade.ETHUSDT", "publicTrade.BTCUSDT", "publicTrade.SUIUSDT"]
            });
            if ws.send(Message::Text(sub.to_string())).await.is_err() {
                // Back off before retrying — otherwise a persistently failing
                // subscribe spins a tight reconnect loop.
                sleep_jittered(&mut backoff_ms).await;
                continue;
            }

            // Split so the keepalive sender and reader can borrow independently.
            let (mut write, mut read) = ws.split();
            // Bybit drops idle sockets after ~30 s; send its `{"op":"ping"}` under that.
            let mut keepalive = tokio::time::interval(Duration::from_secs(WS_KEEPALIVE_PERIOD_S));
            keepalive.tick().await; // consume the immediate first tick
            // Reset backoff only after real data arrives, not on TCP connect.
            let mut proven = false;
            loop {
                tokio::select! {
                    _ = keepalive.tick() => {
                        let ping = serde_json::json!({"op": "ping"}).to_string();
                        if write.send(Message::Text(ping)).await.is_err() {
                            break;
                        }
                    }
                    next = tokio::time::timeout(
                        Duration::from_millis(SOURCE_READ_IDLE_TIMEOUT_MS),
                        read.next(),
                    ) => {
                        let msg = match next {
                            Err(_) => {
                                tracing::warn!("bybit read idle; reconnecting");
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
                                if is_bybit_sub_failure(&text) {
                                    tracing::warn!(%text, "bybit subscribe failed; reconnecting");
                                    break;
                                }
                                if let Some(ticks) = parse_bybit_frame(&text) {
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

/// Bybit signals a rejected subscription with `{"op":"subscribe","success":false}`
/// (no `topic`), which `parse_bybit_frame` would silently drop — leaving the task
/// blocked on a socket it never joined. Detect it so the caller can reconnect.
fn is_bybit_sub_failure(raw: &str) -> bool {
    #[derive(Deserialize)]
    struct BybitAck {
        op: Option<String>,
        success: Option<bool>,
    }
    matches!(
        serde_json::from_str::<BybitAck>(raw),
        Ok(BybitAck {
            op: Some(op),
            success: Some(false),
        }) if op == "subscribe"
    )
}

#[derive(Deserialize)]
struct BybitFrame {
    topic: Option<String>,
    data: Option<Vec<BybitTrade>>,
}

#[derive(Deserialize)]
struct BybitTrade {
    p: String,
}

fn parse_bybit_frame(raw: &str) -> Option<Vec<SourceTick>> {
    let frame: BybitFrame = serde_json::from_str(raw).ok()?;
    let topic = frame.topic?;
    let symbol = topic.strip_prefix("publicTrade.")?;
    let asset = symbol_to_asset(symbol)?;
    let data = frame.data?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis() as i64;
    let ticks: Vec<SourceTick> = data
        .into_iter()
        .filter_map(|t| {
            let price: f64 = t.p.parse().ok()?;
            if !price.is_finite() || price <= 0.0 {
                return None;
            }
            Some(SourceTick {
                source: SourceId::Bybit,
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
    fn parse_publictrade_frame() {
        let raw = r#"{"topic":"publicTrade.ETHUSDT","data":[{"p":"3812.25","v":"0.01"}]}"#;
        let ticks = parse_bybit_frame(raw).unwrap();
        assert_eq!(ticks.len(), 1);
        assert_eq!(ticks[0].asset, AssetSymbol::Eth);
        assert!((ticks[0].price - 3812.25).abs() < 1e-9);
        assert_eq!(ticks[0].source, SourceId::Bybit);
    }

    #[test]
    fn parse_ignores_subscribe_ack() {
        // Bybit emits `{"op":"subscribe","success":true,"ret_msg":"subscribe"…}`;
        // no `topic` field → drop.
        let raw = r#"{"op":"subscribe","success":true}"#;
        assert!(parse_bybit_frame(raw).is_none());
    }

    #[test]
    fn parse_ignores_unknown_symbol() {
        let raw = r#"{"topic":"publicTrade.XRPUSDT","data":[{"p":"0.5"}]}"#;
        assert!(parse_bybit_frame(raw).is_none());
    }
}
