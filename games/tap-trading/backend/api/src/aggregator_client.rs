//! Aggregator HTTP + WS client.
//!
//! Wire types (`OracleTick`, `OracleStatus`, `OracleStreamState`,
//! `OracleMessage`, `AssetSymbol`) are imported from `tap-trading-oracle-types`
//! (Plan C). WS subscriber task lands in Task 13.

use tap_trading_oracle_types::{AssetSymbol, OracleTick};

#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    #[error("stale")]
    Stale,
    #[error("unknown_asset")]
    UnknownAsset,
    #[error("transport")]
    Transport(#[from] reqwest::Error),
    #[error("decode")]
    Decode(#[from] serde_json::Error),
}

pub struct AggregatorClient {
    base_url: String,
    http: reqwest::Client,
}

impl AggregatorClient {
    pub fn new(base_url: String) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_millis(500))
                .build()
                .expect("reqwest client builder"),
            base_url,
        }
    }

    /// Fetch a historical tick from the aggregator ring buffer. ADR-0008 §5.
    ///
    /// Returns `Stale` on 409/410 (seq evicted or run-id mismatch) and on any
    /// other non-200 status, to fail closed on undefined responses.
    pub async fn replay(
        &self,
        asset: AssetSymbol,
        seq: u64,
        run_id: u64,
    ) -> Result<OracleTick, ReplayError> {
        let asset_str = match asset {
            AssetSymbol::Eth => "ETH",
            AssetSymbol::Btc => "BTC",
            AssetSymbol::Sui => "SUI",
        };
        let url = format!(
            "{}/ring/{}/{}?run_id={}",
            self.base_url, asset_str, seq, run_id
        );
        let resp = self.http.get(&url).send().await?;
        match resp.status().as_u16() {
            200 => Ok(resp.json::<OracleTick>().await?),
            404 => Err(ReplayError::UnknownAsset),
            _ => {
                tracing::warn!(
                    status = resp.status().as_u16(),
                    "unexpected aggregator status; treating as stale"
                );
                Err(ReplayError::Stale)
            }
        }
    }
}

use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message;

use crate::history::TickHistory;

/// Spawn a long-lived task that connects to the aggregator's `WS /stream`,
/// records every Text frame into `history` (for chart backfill on new client
/// connects), and re-broadcasts it to `tx`. On disconnect, sleeps 500ms and
/// reconnects. Reconnect loop is intentional — the aggregator is expected to
/// restart periodically (new run_id on each start).
pub fn spawn_aggregator_subscriber(
    base_url: String,
    tx: broadcast::Sender<String>,
    history: Arc<TickHistory>,
) {
    tokio::spawn(async move {
        loop {
            let ws_url = base_url
                .replacen("http://", "ws://", 1)
                .replacen("https://", "wss://", 1)
                + "/stream";
            tracing::info!(%ws_url, "connecting to aggregator WS");
            let conn = tokio_tungstenite::connect_async(&ws_url).await;
            match conn {
                Ok((ws, _)) => {
                    let (mut sink, mut stream) = ws.split();
                    while let Some(msg) = stream.next().await {
                        match msg {
                            Ok(Message::Text(t)) => {
                                history.record(&t);
                                let _ = tx.send(t);
                            }
                            Ok(Message::Ping(p)) => {
                                let _ = sink.send(Message::Pong(p)).await;
                            }
                            Ok(Message::Close(_)) | Err(_) => break,
                            _ => {}
                        }
                    }
                }
                Err(e) => tracing::warn!(error = %e, "aggregator WS connect failed"),
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path_regex, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn sample_tick(seq: u64, run_id: u64) -> serde_json::Value {
        serde_json::json!({
            "asset": "BTC", "run_id": run_id, "seq": seq,
            "ts_ms": 1_700_000_000_000_i64, "mid": 50_000.0,
            "vol_annualized": 0.80, "source_count": 3
        })
    }

    #[tokio::test]
    async fn replay_200_returns_tick() {
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path_regex(r"^/ring/BTC/12345$"))
            .and(query_param("run_id", "999"))
            .respond_with(ResponseTemplate::new(200).set_body_json(sample_tick(12345, 999)))
            .mount(&mock)
            .await;
        let client = AggregatorClient::new(mock.uri());
        let tick = client.replay(AssetSymbol::Btc, 12345, 999).await.unwrap();
        assert_eq!(tick.seq, 12345);
        assert_eq!(tick.run_id, 999);
        assert_eq!(tick.mid, 50_000.0);
    }

    #[tokio::test]
    async fn replay_410_is_stale() {
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(410))
            .mount(&mock)
            .await;
        let client = AggregatorClient::new(mock.uri());
        assert!(matches!(
            client.replay(AssetSymbol::Eth, 1, 1).await,
            Err(ReplayError::Stale)
        ));
    }

    #[tokio::test]
    async fn replay_409_is_stale() {
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(409))
            .mount(&mock)
            .await;
        let client = AggregatorClient::new(mock.uri());
        assert!(matches!(
            client.replay(AssetSymbol::Eth, 1, 1).await,
            Err(ReplayError::Stale)
        ));
    }

    #[tokio::test]
    async fn replay_404_is_unknown_asset() {
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock)
            .await;
        let client = AggregatorClient::new(mock.uri());
        assert!(matches!(
            client.replay(AssetSymbol::Eth, 1, 1).await,
            Err(ReplayError::UnknownAsset)
        ));
    }

    #[tokio::test]
    async fn replay_500_is_stale() {
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&mock)
            .await;
        let client = AggregatorClient::new(mock.uri());
        assert!(matches!(
            client.replay(AssetSymbol::Eth, 1, 1).await,
            Err(ReplayError::Stale)
        ));
    }
}
