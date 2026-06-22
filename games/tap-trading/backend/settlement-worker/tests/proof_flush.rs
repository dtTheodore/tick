//! Batched-proof flusher (ADR-0011): a settled win records `evidence_to_seq`,
//! then the flusher reassembles the proof from the oracle ring, stores ONE
//! BatchProofBlob on Walrus, and stamps the row's blob id + `proof_index`. The
//! captured proof must pass the independent verifier — this is the contract the
//! frontend "Verify this tap" button relies on.

mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tap_trading_oracle_types::{AssetSymbol, OracleTick};
use tap_trading_pricing_engine::{compute_multiplier, Cell, OracleState, PricingConfig};
use tap_trading_proof_types::BatchProofBlob;
use tap_trading_proof_verifier::{verify, VerifyResult};
use tap_trading_settlement_worker::cache::PositionRef;
use tap_trading_settlement_worker::onchain::{ProofConfig, ProofPublisher};
use tap_trading_settlement_worker::proof_flusher::ProofFlusher;
use tap_trading_settlement_worker::settle::{settle_loss, settle_win};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// Band/window/quote chosen once; the multiplier is derived from these by the
// SAME engine the verifier uses, so the recompute matches (see `locked_mult`).
const BAND_LO: f64 = 70_000.0;
const BAND_HI: f64 = 70_010.0;
const T_OPEN: i64 = 1_000;
const T_CLOSE: i64 = 6_000;
const TAP_MID: f64 = 69_990.0; // tap tick below band
const TAP_VOL: f64 = 0.61;
const TAP_SEQ: u64 = 100;
const RUN_ID: u64 = 7;
const TOUCH_SEQ: u64 = 101;
const TOUCH_MID: f64 = 70_005.0; // in-band, settles the win

/// The multiplier the worker would lock at tap, recomputed with the SAME engine
/// the verifier replays. `now_ms` = tap_ms = T_OPEN (what `insert_open_position`
/// records as `created_at_ms`). The verifier re-derives this from `quote_at_tap`
/// and compares within BPS_EPSILON, so the proof only verifies if the stored
/// `multiplier_at_tap` is THIS value (not an arbitrary number).
fn locked_mult() -> f64 {
    let cell = Cell {
        asset: AssetSymbol::Btc,
        strike_lo: BAND_LO,
        strike_hi: BAND_HI,
        t_open_ms: T_OPEN as u64,
        t_close_ms: T_CLOSE as u64,
    };
    let oracle = OracleState {
        asset: AssetSymbol::Btc,
        spot: TAP_MID,
        sigma_annualized: TAP_VOL,
        timestamp_ms: T_OPEN as u64,
    };
    compute_multiplier(&cell, &oracle, &PricingConfig::default(), T_OPEN as u64).expect("multiplier")
}

/// Stub publisher: captures the stored bytes (so the test can assert exactly one
/// `store` and inspect the batch) and returns a fixed blob id. The capture
/// buffer is a shared `Arc` so the test retains a handle after the publisher is
/// boxed into the flusher.
struct CapturePublisher {
    stored: Arc<Mutex<Vec<Vec<u8>>>>,
}

#[async_trait]
impl ProofPublisher for CapturePublisher {
    async fn store(&self, bytes: &[u8]) -> anyhow::Result<String> {
        self.stored.lock().unwrap().push(bytes.to_vec());
        Ok("blobBATCH123".into())
    }
    async fn read(&self, _blob_id: &str) -> anyhow::Result<Vec<u8>> {
        Ok(vec![])
    }
}

fn position(id: i64, account_id: i64, multiplier: f64) -> PositionRef {
    PositionRef {
        id,
        account_id,
        asset: AssetSymbol::Btc,
        strike_lo: BAND_LO,
        strike_hi: BAND_HI,
        t_open_ms: T_OPEN,
        t_close_ms: T_CLOSE,
        stake_points: 100,
        multiplier_at_tap: multiplier,
        oracle_seq_at_tap: TAP_SEQ as i64,
        oracle_run_id_at_tap: RUN_ID as i64,
        created_at_ms: T_OPEN,
    }
}

/// Insert an OPEN position carrying the tap seq/run the flusher will fetch
/// evidence for. `common::insert_open_position` hard-codes seq/run to 0, so we
/// insert directly to set them to (TAP_SEQ, RUN_ID).
async fn insert_position_row(pool: &sqlx::PgPool, account_id: i64, multiplier: f64) -> i64 {
    let row: (i64,) = sqlx::query_as(
        r#"
        INSERT INTO positions
          (account_id, asset, strike_lo, strike_hi, t_open_ms, t_close_ms,
           stake_points, multiplier_at_tap, status, created_at_ms,
           oracle_seq_at_tap, oracle_run_id_at_tap, client_request_id)
        VALUES ($1,'BTC',$2,$3,$4,$5,100,$6,'OPEN',$4,$7,$8,$9)
        RETURNING id
        "#,
    )
    .bind(account_id)
    .bind(BAND_LO)
    .bind(BAND_HI)
    .bind(T_OPEN)
    .bind(T_CLOSE)
    .bind(multiplier)
    .bind(TAP_SEQ as i64)
    .bind(RUN_ID as i64)
    .bind(uuid::Uuid::new_v4())
    .fetch_one(pool)
    .await
    .expect("insert position");
    row.0
}

/// Evidence path the mock ring serves: the tap tick at TAP_SEQ (below band, the
/// window-head the verifier requires) and the touching tick at TOUCH_SEQ.
fn evidence_json() -> String {
    let ticks = vec![
        OracleTick { asset: AssetSymbol::Btc, run_id: RUN_ID, seq: TAP_SEQ, ts_ms: T_OPEN, mid: TAP_MID, vol_annualized: TAP_VOL, source_count: 3 },
        OracleTick { asset: AssetSymbol::Btc, run_id: RUN_ID, seq: TOUCH_SEQ, ts_ms: 3_000, mid: TOUCH_MID, vol_annualized: TAP_VOL, source_count: 3 },
    ];
    serde_json::to_string(&ticks).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn flush_publishes_batch_and_stamps_verifiable_proof() {
    let db = common::setup_test_postgres().await;
    let acct = common::insert_account(&db.pool, "proof-player", 1_000).await;
    // Bind a wallet so the proof carries an owner (the flusher reads sui_address).
    sqlx::query("UPDATE accounts SET sui_address='0xWALLET' WHERE id=$1")
        .bind(acct).execute(&db.pool).await.unwrap();

    let mult = locked_mult();
    let pid = insert_position_row(&db.pool, acct, mult).await;

    // Settle the win at the touching tick — records evidence_to_seq = TOUCH_SEQ,
    // proof_status defaults to 'pending'.
    let touch_tick = OracleTick {
        asset: AssetSymbol::Btc, run_id: RUN_ID, seq: TOUCH_SEQ, ts_ms: 3_000,
        mid: TOUCH_MID, vol_annualized: TAP_VOL, source_count: 3,
    };
    let credited = settle_win(&db.pool, &position(pid, acct, mult), &touch_tick)
        .await
        .expect("settle_win");
    assert!(credited);

    // Mock oracle ring serves the evidence window.
    let agg = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ring/BTC/range"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(evidence_json(), "application/json"))
        .mount(&agg)
        .await;

    let captured = Arc::new(Mutex::new(Vec::new()));
    let publisher = Box::new(CapturePublisher { stored: captured.clone() });
    let cfg = ProofConfig { aggregator_http_url: agg.uri(), walrus_store_epochs: 1 };
    let flusher = ProofFlusher::new(publisher, cfg);

    let published = flusher.flush_once(&db.pool, 500).await.expect("flush");
    assert_eq!(published, 1, "exactly one proof published");

    // Exactly one Walrus store call for the whole batch.
    assert_eq!(captured.lock().unwrap().len(), 1, "one store() for the batch");

    // The settlement row is stamped published with blob id + proof_index = 0.
    let row: (String, Option<String>, Option<i32>) = sqlx::query_as(
        "SELECT proof_status, walrus_blob_id, proof_index FROM settlements WHERE position_id=$1",
    )
    .bind(pid)
    .fetch_one(&db.pool)
    .await
    .expect("settlement row");
    assert_eq!(row.0, "published");
    assert_eq!(row.1.as_deref(), Some("blobBATCH123"));
    assert_eq!(row.2, Some(0));

    // The captured BatchProofBlob's proofs[0] verifies Valid independently.
    let bytes = captured.lock().unwrap()[0].clone();
    let batch: BatchProofBlob = serde_json::from_slice(&bytes).expect("deserialize batch");
    assert_eq!(batch.proofs.len(), 1);
    assert_eq!(batch.proofs[0].owner, "0xWALLET");
    assert_eq!(verify(&batch.proofs[0]), VerifyResult::Valid);

    // Money path untouched by the flusher: the win credit stands.
    assert_eq!(common::get_balance(&db.pool, acct).await, 1_000 + (100.0 * mult).floor() as i64);
}

// Expiry seq/evidence for the loss case: the path never enters the band and the
// tail tick reaches t_close, which the verifier's Lost branch requires.
const EXPIRE_SEQ: u64 = 102;

fn loss_evidence_json() -> String {
    let ticks = vec![
        OracleTick { asset: AssetSymbol::Btc, run_id: RUN_ID, seq: TAP_SEQ, ts_ms: T_OPEN, mid: TAP_MID, vol_annualized: TAP_VOL, source_count: 3 },
        OracleTick { asset: AssetSymbol::Btc, run_id: RUN_ID, seq: EXPIRE_SEQ, ts_ms: T_CLOSE, mid: 69_995.0, vol_annualized: TAP_VOL, source_count: 3 },
    ];
    serde_json::to_string(&ticks).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn flush_loss_proof_verifies_valid() {
    let db = common::setup_test_postgres().await;
    let acct = common::insert_account(&db.pool, "loss-proof-player", 1_000).await;
    sqlx::query("UPDATE accounts SET sui_address='0xWALLET' WHERE id=$1")
        .bind(acct).execute(&db.pool).await.unwrap();

    let mult = locked_mult();
    let pid = insert_position_row(&db.pool, acct, mult).await;

    // Expire as a Loss at the close tick — records evidence_to_seq = EXPIRE_SEQ.
    let expire_tick = OracleTick {
        asset: AssetSymbol::Btc, run_id: RUN_ID, seq: EXPIRE_SEQ, ts_ms: T_CLOSE,
        mid: 69_995.0, vol_annualized: TAP_VOL, source_count: 3,
    };
    assert!(settle_loss(&db.pool, &position(pid, acct, mult), &expire_tick).await.expect("settle_loss"));

    let agg = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ring/BTC/range"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(loss_evidence_json(), "application/json"))
        .mount(&agg)
        .await;

    let captured = Arc::new(Mutex::new(Vec::new()));
    let publisher = Box::new(CapturePublisher { stored: captured.clone() });
    let cfg = ProofConfig { aggregator_http_url: agg.uri(), walrus_store_epochs: 1 };
    let flusher = ProofFlusher::new(publisher, cfg);

    assert_eq!(flusher.flush_once(&db.pool, 500).await.expect("flush"), 1);

    let bytes = captured.lock().unwrap()[0].clone();
    let batch: BatchProofBlob = serde_json::from_slice(&bytes).expect("deserialize batch");
    assert_eq!(verify(&batch.proofs[0]), VerifyResult::Valid);

    // Loss credits nothing — balance unchanged.
    assert_eq!(common::get_balance(&db.pool, acct).await, 1_000);
}
