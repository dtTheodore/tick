//! Walrus proof publishing + oracle-ring evidence fetch for off-chain Tick
//! settlement (ADR-0011). The shipped publisher shells out to the installed
//! `walrus` CLI (the CLI has a funded keystore + working config today); the
//! trait boundary makes programmatic signing a later swap, not a rewrite.
//!
//! Per-tap on-chain settlement (the old `usdc` settle_mode) is gone: Tick is
//! off-chain USDC, and proofs are published in BATCHES (one Walrus blob per
//! flush) by the proof flusher, so per-tap Walrus cost ≈ 0.

use std::process::Stdio;

use anyhow::{anyhow, bail, Context};
use tap_trading_oracle_types::{AssetSymbol, OracleTick};
use tokio::process::Command;

/// Config for the (always-on) proof flusher, from env. Proof publishing can be
/// disabled for local dev without a `walrus` CLI by setting
/// `TICK_PROOFS_ENABLED=false` (then `from_env` returns `None`).
#[derive(Clone, Debug)]
pub struct ProofConfig {
    pub aggregator_http_url: String,
    pub walrus_store_epochs: u32,
}

impl ProofConfig {
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        let enabled = std::env::var("TICK_PROOFS_ENABLED")
            .map(|v| v != "false")
            .unwrap_or(true);
        if !enabled {
            return Ok(None);
        }
        let ws = std::env::var("TAP_AGGREGATOR_WS_URL")
            .map_err(|_| anyhow!("TAP_AGGREGATOR_WS_URL missing (needed for proof evidence fetch)"))?;
        Ok(Some(Self {
            aggregator_http_url: ws_to_http_base(&ws),
            walrus_store_epochs: std::env::var("WALRUS_STORE_EPOCHS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(5),
        }))
    }
}

fn ws_to_http_base(ws: &str) -> String {
    let base = ws
        .strip_suffix("/stream")
        .unwrap_or(ws);
    base.replacen("wss://", "https://", 1)
        .replacen("ws://", "http://", 1)
}

// ===== Proof publishing (Walrus) =====

#[async_trait::async_trait]
pub trait ProofPublisher: Send + Sync {
    /// Store `bytes` on Walrus, returning the blob id.
    async fn store(&self, bytes: &[u8]) -> anyhow::Result<String>;
    /// Read a blob back (used by the e2e/verifier path).
    async fn read(&self, blob_id: &str) -> anyhow::Result<Vec<u8>>;
}

pub struct WalrusCliPublisher {
    epochs: u32,
    // Walrus client context (the `default_context` in the CLI config selects the
    // Sui+Walrus network). We pin it explicitly so proofs land on the same
    // network the game runs on regardless of the operator's CLI default —
    // `WALRUS_CONTEXT`, defaulting to `testnet`.
    context: String,
}

impl WalrusCliPublisher {
    pub fn new(epochs: u32) -> Self {
        let context = std::env::var("WALRUS_CONTEXT").unwrap_or_else(|_| "testnet".to_string());
        Self { epochs, context }
    }
}

#[async_trait::async_trait]
impl ProofPublisher for WalrusCliPublisher {
    async fn store(&self, bytes: &[u8]) -> anyhow::Result<String> {
        // walrus stores files; write the blob to a unique temp path.
        let path = std::env::temp_dir().join(format!("tick-proof-{}.json", unique_suffix(bytes)));
        tokio::fs::write(&path, bytes).await.context("writing proof to temp file")?;
        let out = Command::new("walrus")
            .arg("--context")
            .arg(&self.context)
            .args(["store", "--epochs"])
            .arg(self.epochs.to_string())
            .arg("--json")
            .arg(&path)
            .stdin(Stdio::null())
            .output()
            .await
            .context("spawning `walrus store`")?;
        let _ = tokio::fs::remove_file(&path).await;
        if !out.status.success() {
            bail!("walrus store failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        let v: serde_json::Value = serde_json::from_slice(&out.stdout)
            .context("parsing walrus store --json output")?;
        find_blob_id(&v).ok_or_else(|| anyhow!("no blobId in walrus store output"))
    }

    async fn read(&self, blob_id: &str) -> anyhow::Result<Vec<u8>> {
        let out = Command::new("walrus")
            .arg("--context")
            .arg(&self.context)
            .args(["read", blob_id])
            .stdin(Stdio::null())
            .output()
            .await
            .context("spawning `walrus read`")?;
        if !out.status.success() {
            bail!("walrus read failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(out.stdout)
    }
}

/// Recursively find the first `blobId` string in a Walrus `store --json` value.
/// Tolerates both `newlyCreated.blobObject.blobId` and `alreadyCertified.blobId`
/// shapes (and any array wrapping) without hard-coding the path.
fn find_blob_id(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(s)) = map.get("blobId") {
                return Some(s.clone());
            }
            map.values().find_map(find_blob_id)
        }
        serde_json::Value::Array(arr) => arr.iter().find_map(find_blob_id),
        _ => None,
    }
}

fn unique_suffix(bytes: &[u8]) -> String {
    // Avoid Date/rand (unavailable / non-deterministic concerns): a cheap hash of
    // the content + pid is plenty to dodge temp-file collisions across settles.
    let mut h: u64 = std::process::id() as u64;
    for &b in bytes {
        h = h.wrapping_mul(1099511628211).wrapping_add(b as u64);
    }
    format!("{h:016x}")
}

// ===== Evidence (aggregator ring range) =====

#[derive(Debug, thiserror::Error)]
pub enum EvidenceError {
    #[error("evidence incomplete (ring returned {status})")]
    Incomplete { status: u16 },
    #[error("evidence fetch transport: {0}")]
    Transport(#[from] reqwest::Error),
}

/// Pull the contiguous tick path `[from_seq, to_seq]` for proof evidence in one
/// call (ADR-0011 §6). 409/410 mean the ring couldn't serve a complete window →
/// `Incomplete`, so the caller marks the proof failed without blocking payout.
pub async fn fetch_window(
    http: &reqwest::Client,
    aggregator_http_url: &str,
    asset: AssetSymbol,
    run_id: u64,
    from_seq: u64,
    to_seq: u64,
) -> Result<Vec<OracleTick>, EvidenceError> {
    let url = format!(
        "{}/ring/{}/range?run_id={}&from_seq={}&to_seq={}",
        aggregator_http_url, asset.ticker(), run_id, from_seq, to_seq
    );
    let resp = http.get(&url).send().await?;
    let status = resp.status().as_u16();
    if status == 200 {
        Ok(resp.json::<Vec<OracleTick>>().await?)
    } else {
        Err(EvidenceError::Incomplete { status })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_url_maps_to_http_base() {
        assert_eq!(ws_to_http_base("ws://localhost:3321/stream"), "http://localhost:3321");
        assert_eq!(ws_to_http_base("wss://agg.example/stream"), "https://agg.example");
        assert_eq!(ws_to_http_base("ws://h:1"), "http://h:1");
    }

    #[test]
    fn find_blob_id_walks_walrus_store_shape() {
        let v: serde_json::Value = serde_json::from_str(
            r#"[{"blobStoreResult":{"newlyCreated":{"blobObject":{"blobId":"YdbQ..","size":22}}}}]"#,
        )
        .unwrap();
        assert_eq!(find_blob_id(&v).as_deref(), Some("YdbQ.."));
        let already: serde_json::Value =
            serde_json::from_str(r#"{"blobStoreResult":{"alreadyCertified":{"blobId":"ZZZ"}}}"#).unwrap();
        assert_eq!(find_blob_id(&already).as_deref(), Some("ZZZ"));
    }
}
