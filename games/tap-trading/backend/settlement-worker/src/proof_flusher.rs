//! Batched Walrus proof publishing for off-chain Tick settlements (ADR-0011).
//!
//! Settlement correctness NEVER depends on this path. `settle_win`/`settle_loss`
//! credit balances synchronously and stamp `proof_status='pending'`. This
//! flusher runs out-of-band: every ~60s it gathers pending settlements,
//! reassembles each one's proof from the oracle ring, serializes ONE
//! `BatchProofBlob`, stores it on Walrus once, then stamps each row's
//! `walrus_blob_id` + `proof_index`. A Walrus failure leaves rows 'pending' for
//! the next flush; a single bad row is marked 'failed' and skipped — the batch
//! still publishes. It touches only proof columns, never balances.

use anyhow::{anyhow, Context};
use sqlx::PgPool;
use tap_trading_oracle_types::{AssetSymbol, OracleTick};
use tap_trading_proof_types::{
    multiplier_f64_to_bps, Band, BatchProofBlob, EvidenceTick, Outcome, ProofBlob, QuoteAtTap,
    Settlement, Window, PROOF_SCHEMA_VERSION,
};
use tap_trading_touch::path_touches_band;

use crate::cache::{OpenPositionCache, PositionRef};
use crate::onchain::{fetch_window, EvidenceError, ProofConfig, ProofPublisher};
use crate::settle;

const ORACLE_PRICE_SCALE: f64 = 1e9;
const FORMULA_VERSION: &str = "tick_v2_window_aware";

/// Build the self-contained proof for one settled position. `multiplier_bps` is
/// recovered from the locked `multiplier_at_tap` via the SAME float→bps function
/// the verifier uses (`multiplier_f64_to_bps`), so a correct recompute matches
/// exactly. `quote_at_tap` is sourced from the tap tick (`seq == oracle_seq_at_tap`)
/// so the verifier's recompute is reproducible.
fn build_proof(
    pos: &PositionRef,
    owner: &str,
    outcome: Outcome,
    ticks: &[OracleTick],
    settled_at_ms: i64,
) -> anyhow::Result<ProofBlob> {
    let from_seq = pos.oracle_seq_at_tap as u64;
    let quote = ticks
        .iter()
        .find(|t| t.seq == from_seq)
        .ok_or_else(|| anyhow!("tap tick seq {from_seq} missing from evidence"))?;

    let (lo, hi) = (pos.strike_lo, pos.strike_hi);
    let touch = derive_touch(ticks, pos.t_open_ms, pos.t_close_ms, lo, hi);

    let evidence_ticks = ticks
        .iter()
        .map(|t| EvidenceTick { seq: t.seq, ts_ms: t.ts_ms, mid: t.mid })
        .collect();

    Ok(ProofBlob {
        v: PROOF_SCHEMA_VERSION,
        position_id: pos.id.to_string(),
        vault_id: String::new(),
        owner: owner.to_string(),
        asset: ticker(pos.asset).to_string(),
        band: Band {
            lo: (pos.strike_lo * ORACLE_PRICE_SCALE) as u64,
            hi: (pos.strike_hi * ORACLE_PRICE_SCALE) as u64,
        },
        window: Window { t_open_ms: pos.t_open_ms as u64, t_close_ms: pos.t_close_ms as u64 },
        stake: pos.stake_points as u64,
        multiplier_bps: multiplier_f64_to_bps(pos.multiplier_at_tap),
        quote_at_tap: QuoteAtTap {
            oracle_run_id: pos.oracle_run_id_at_tap as u64,
            oracle_seq: from_seq,
            tap_ms: pos.created_at_ms as u64,
            mid: quote.mid,
            vol_annualized: quote.vol_annualized,
            formula_version: FORMULA_VERSION.into(),
            floor_curve: "1.00+0.0000*tau".into(),
        },
        settlement: Settlement {
            outcome,
            touch_seq: touch.map(|(seq, _)| seq),
            touch_mid: touch.map(|(_, mid)| mid),
            evidence_ticks,
            settled_at_ms: settled_at_ms as u64,
            sui_tx_digest: String::new(),
        },
    })
}

/// First in-window tick whose arriving segment crosses the band — the same
/// `tap-trading-touch` logic the verifier replays, so `touch_seq` agrees with an
/// independent verify.
fn derive_touch(ticks: &[OracleTick], t_open: i64, t_close: i64, lo: f64, hi: f64) -> Option<(u64, f64)> {
    let mut prev: Option<f64> = None;
    for t in ticks {
        if t.ts_ms > t_close {
            break;
        }
        if t.ts_ms >= t_open && path_touches_band(prev, t.mid, lo, hi) {
            return Some((t.seq, t.mid));
        }
        prev = Some(t.mid);
    }
    None
}

fn ticker(asset: AssetSymbol) -> &'static str {
    match asset {
        AssetSymbol::Eth => "ETH",
        AssetSymbol::Btc => "BTC",
        AssetSymbol::Sui => "SUI",
    }
}

/// Map the DB `outcome` char to a proof `Outcome`. Only 'W'/'L' reach the
/// flusher (the SELECT filters to those); 'V' has no replayable evidence path.
fn outcome_from_char(c: &str) -> Option<Outcome> {
    match c {
        "W" => Some(Outcome::Won),
        "L" => Some(Outcome::Lost),
        "V" => Some(Outcome::Void),
        _ => None,
    }
}

/// One pending-settlement row the flush SELECT returns: position id, outcome
/// char, evidence upper-bound seq, settled-at ms, and the account's bound Sui
/// wallet (the proof `owner`).
type PendingProofRow = (i64, String, Option<i64>, i64, Option<String>);

pub struct ProofFlusher {
    publisher: Box<dyn ProofPublisher>,
    config: ProofConfig,
    http: reqwest::Client,
}

impl ProofFlusher {
    pub fn new(publisher: Box<dyn ProofPublisher>, config: ProofConfig) -> Self {
        Self { publisher, config, http: reqwest::Client::new() }
    }

    /// One flush pass. Gathers up to `limit` pending win/loss settlements that
    /// carry an `evidence_to_seq`, builds each proof from the oracle ring, then
    /// stores ONE `BatchProofBlob` on Walrus and stamps every row's blob id +
    /// index. Returns the number of settlements published.
    ///
    /// Robustness contract:
    ///   - A row whose evidence window is empty/incomplete is marked 'failed'
    ///     and skipped — it never aborts the batch.
    ///   - A Walrus store failure returns Err with all rows left 'pending' (the
    ///     next flush retries the whole batch); no row is stamped.
    ///   - Only `proof_status`/`walrus_blob_id`/`proof_index` are written.
    pub async fn flush_once(&self, pool: &PgPool, limit: i64) -> anyhow::Result<usize> {
        // `evidence_to_seq IS NOT NULL` excludes legacy rows predating this
        // column: their evidence has long aged out of the ring, so we leave them
        // 'pending' harmlessly rather than hammering the ring for 410s.
        let rows: Vec<PendingProofRow> = sqlx::query_as(
            r#"
            SELECT s.position_id, s.outcome, s.evidence_to_seq, s.settled_at_ms, a.sui_address
            FROM settlements s
            JOIN positions p ON p.id = s.position_id
            JOIN accounts  a ON a.id = s.account_id
            WHERE s.proof_status = 'pending'
              AND s.evidence_to_seq IS NOT NULL
              AND s.outcome IN ('W', 'L')
            ORDER BY s.position_id
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(pool)
        .await
        .context("selecting pending settlements for proof")?;

        if rows.is_empty() {
            return Ok(0);
        }

        let mut proofs: Vec<ProofBlob> = Vec::with_capacity(rows.len());
        let mut stamped: Vec<i64> = Vec::with_capacity(rows.len());

        for (position_id, outcome_char, evidence_to_seq, settled_at_ms, sui_address) in rows {
            let (Some(outcome), Some(to_seq)) =
                (outcome_from_char(&outcome_char), evidence_to_seq)
            else {
                continue;
            };
            let pos = match OpenPositionCache::fetch_by_id(pool, position_id).await {
                Ok(Some(p)) => p,
                Ok(None) => {
                    tracing::warn!(position_id, "proof flush: position row gone; marking failed");
                    let _ = settle::mark_proof_failed(pool, position_id).await;
                    continue;
                }
                Err(e) => {
                    tracing::warn!(error = %e, position_id, "proof flush: fetch_by_id failed; skipping");
                    continue;
                }
            };
            let from_seq = pos.oracle_seq_at_tap as u64;
            let to_seq = to_seq as u64;
            // Window must be contiguous and forward; a sweep-expired straggler
            // carries to_seq=0 (synthetic tick) → empty/backwards → mark failed.
            let ticks = match fetch_window(
                &self.http,
                &self.config.aggregator_http_url,
                pos.asset,
                pos.oracle_run_id_at_tap as u64,
                from_seq,
                to_seq,
            )
            .await
            {
                Ok(t) if !t.is_empty() => t,
                Ok(_) => {
                    tracing::warn!(position_id, "proof flush: empty evidence window; marking failed");
                    let _ = settle::mark_proof_failed(pool, position_id).await;
                    continue;
                }
                // Ring couldn't serve a complete window (409/410) → evidence aged
                // out, permanently unrecoverable: mark failed so we stop hammering.
                Err(EvidenceError::Incomplete { status }) => {
                    tracing::warn!(position_id, status, "proof flush: evidence incomplete; marking failed");
                    let _ = settle::mark_proof_failed(pool, position_id).await;
                    continue;
                }
                // Transport blip (aggregator down) is transient — leave the row
                // 'pending' so the next flush retries it once the ring is back.
                Err(EvidenceError::Transport(e)) => {
                    tracing::warn!(error = %e, position_id, "proof flush: evidence fetch transport error; leaving pending");
                    continue;
                }
            };
            match build_proof(&pos, &sui_address.unwrap_or_default(), outcome, &ticks, settled_at_ms) {
                Ok(blob) => {
                    proofs.push(blob);
                    stamped.push(position_id);
                }
                Err(e) => {
                    tracing::warn!(error = %e, position_id, "proof flush: build_proof failed; marking failed");
                    let _ = settle::mark_proof_failed(pool, position_id).await;
                }
            }
        }

        if proofs.is_empty() {
            return Ok(0);
        }

        let batch = BatchProofBlob { v: PROOF_SCHEMA_VERSION, proofs };
        let bytes = serde_json::to_vec(&batch).context("serialize batch proof blob")?;
        // A failure here leaves every row 'pending' (nothing stamped yet) — the
        // next flush retries the whole batch.
        let blob_id = self.publisher.store(&bytes).await.context("walrus store batch")?;

        for (index, position_id) in stamped.iter().enumerate() {
            if let Err(e) =
                settle::mark_proof_published(pool, *position_id, &blob_id, index as i32).await
            {
                tracing::error!(error = %e, position_id, "proof flush: mark_proof_published failed");
            }
        }
        tracing::info!(count = stamped.len(), %blob_id, "proof batch published to walrus");
        Ok(stamped.len())
    }
}

/// Time-based flush loop: every `interval_secs`, publish up to 500 pending
/// proofs in one Walrus blob. 60s is ample (proofs are not latency-sensitive;
/// the payout already happened synchronously).
pub async fn run_flusher(pool: PgPool, flusher: ProofFlusher, interval_secs: u64) {
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
    ticker.tick().await; // skip the immediate first tick
    loop {
        ticker.tick().await;
        if let Err(e) = flusher.flush_once(&pool, 500).await {
            tracing::warn!(error = %e, "proof flush failed; rows stay pending for next flush");
        }
    }
}
