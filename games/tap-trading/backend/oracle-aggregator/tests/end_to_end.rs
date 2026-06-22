//! End-to-end smoke: real axum server + real ring + real broadcast + real driver.
//!
//! `full_loop_*` feeds `SourceTick`s into the same mpsc the driver consumes
//! and asserts the output `OracleMessage::Tick` appears on `/stream`. The
//! remaining tests stay direct-write smoke for the ring rotation and status
//! frame paths — those are wire-surface assertions, not driver assertions.

#[path = "../src/aggregator.rs"]
mod aggregator;
#[path = "../src/api.rs"]
mod api;
#[path = "../src/broadcast.rs"]
mod broadcast;
#[path = "../src/constants.rs"]
mod constants;
#[path = "../src/driver.rs"]
mod driver;
#[path = "../src/ring_buffer.rs"]
mod ring_buffer;
#[path = "../src/runtime.rs"]
mod runtime;
#[path = "../src/sources/mod.rs"]
mod sources;
#[path = "../src/vol_state.rs"]
mod vol_state;

use broadcast::Broadcaster;
use futures::StreamExt;
use ring_buffer::RingBuffers;
use sources::{SourceId, SourceTick};
use std::sync::Arc;
use tap_trading_oracle_types::{AssetSymbol, OracleMessage, OracleTick};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};

struct Harness {
    base: String,
    port: u16,
    rings: Arc<RingBuffers>,
    broadcaster: Broadcaster,
    source_tx: mpsc::Sender<SourceTick>,
}

async fn spawn_with_driver(run_id: u64) -> Harness {
    // Drive the SAME wiring `main` uses (no real sources; the test injects
    // ticks via `source_tx`). This is what keeps `main`'s topology honest.
    let (app, handles) = runtime::wire(run_id, vec![]);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Harness {
        base: format!("http://127.0.0.1:{port}"),
        port,
        rings: handles.rings,
        broadcaster: handles.broadcaster,
        source_tx: handles.source_tx,
    }
}

fn synth_source(src: SourceId, asset: AssetSymbol, price: f64, ts_ms: i64) -> SourceTick {
    SourceTick {
        source: src,
        asset,
        price,
        ts_ms,
        pyth_conf_bps: None,
    }
}

fn synth_tick(seq: u64) -> OracleTick {
    OracleTick {
        asset: AssetSymbol::Eth,
        run_id: 42,
        seq,
        ts_ms: 1_000_000 + (seq as i64) * 50,
        mid: 3812.0 + seq as f64,
        vol_annualized: 0.60,
        source_count: 4,
    }
}

#[tokio::test]
async fn full_loop_source_tick_propagates_to_stream_and_ring() {
    let h = spawn_with_driver(42).await;

    let url = format!("ws://127.0.0.1:{}/stream", h.port);
    let (mut socket, _) = connect_async(&url).await.unwrap();

    // Feed >=2 sources for ETH so apply_sources emits.
    let now = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()) as i64;
    for src in [SourceId::Binance, SourceId::Bybit, SourceId::Okx] {
        h.source_tx
            .send(synth_source(src, AssetSymbol::Eth, 3812.0, now))
            .await
            .unwrap();
    }

    // Allow up to 5 s for the first 50 ms emit. Discard heartbeats and
    // non-tick frames.
    let deadline = std::time::Duration::from_secs(5);
    let first_tick = tokio::time::timeout(deadline, async {
        loop {
            let frame = socket.next().await.unwrap().unwrap();
            if let Message::Text(json) = frame {
                if let Ok(OracleMessage::Tick(t)) = serde_json::from_str::<OracleMessage>(&json) {
                    return t;
                }
            }
        }
    })
    .await
    .expect("a Tick within 5s");

    assert_eq!(first_tick.asset, AssetSymbol::Eth);
    assert_eq!(first_tick.run_id, 42);
    assert_eq!(first_tick.seq, 0, "first tick must have seq=0");
    assert!(first_tick.source_count >= 2);

    // Same tick should be in the ring.
    let resp = reqwest::get(format!("{}/ring/ETH/0?run_id=42", h.base))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: OracleTick = resp.json().await.unwrap();
    assert_eq!(body, first_tick);
}

#[tokio::test]
async fn ring_rotation_yields_410_once_seq_falls_out_of_retention() {
    let h = spawn_with_driver(42).await;
    // Push one full ring plus a few extra so the earliest seqs rotate out.
    let overflow = constants::RING_SIZE as u64 + 5;
    for seq in 0..overflow {
        h.rings.push(synth_tick(seq));
    }
    // seq 0 has aged out of the 120 s ring → 410.
    let gone = reqwest::get(format!("{}/ring/ETH/0?run_id=42", h.base))
        .await
        .unwrap();
    assert_eq!(gone.status(), 410);
    // The newest seq is still retained → 200.
    let newest = reqwest::get(format!("{}/ring/ETH/{}?run_id=42", h.base, overflow - 1))
        .await
        .unwrap();
    assert_eq!(newest.status(), 200);
}

#[tokio::test]
async fn ring_range_returns_contiguous_evidence_span() {
    let h = spawn_with_driver(42).await;
    for seq in 0..50 {
        h.rings.push(synth_tick(seq));
    }
    // Happy path: inclusive [10, 20] → 11 ticks in seq order.
    let resp = reqwest::get(format!("{}/ring/ETH/range?run_id=42&from_seq=10&to_seq=20", h.base))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let span: Vec<OracleTick> = resp.json().await.unwrap();
    assert_eq!(span.len(), 11);
    assert_eq!(span.first().unwrap().seq, 10);
    assert_eq!(span.last().unwrap().seq, 20);

    // Stale run_id → 409.
    let conflict = reqwest::get(format!("{}/ring/ETH/range?run_id=99&from_seq=10&to_seq=20", h.base))
        .await
        .unwrap();
    assert_eq!(conflict.status(), 409);

    // Inverted range → 400.
    let bad = reqwest::get(format!("{}/ring/ETH/range?run_id=42&from_seq=20&to_seq=10", h.base))
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);
}

#[tokio::test]
async fn status_frame_propagates_via_stream() {
    let h = spawn_with_driver(42).await;
    let url = format!("ws://127.0.0.1:{}/stream", h.port);
    let (mut socket, _) = connect_async(&url).await.unwrap();

    // Small sleep to let the WS session establish its broadcast subscriber
    // before we send — broadcast does not backfill.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    h.broadcaster.send(OracleMessage::Status(
        tap_trading_oracle_types::OracleStatus {
            asset: AssetSymbol::Eth,
            state: tap_trading_oracle_types::OracleStreamState::Degraded,
            reason: "all sources stale".to_string(),
            run_id: 42,
        },
    ));

    let deadline = std::time::Duration::from_secs(2);
    let status_seen = tokio::time::timeout(deadline, async {
        loop {
            let frame = socket.next().await.unwrap().unwrap();
            if let Message::Text(json) = frame {
                if json.contains(r#""type":"status""#) && json.contains(r#""state":"degraded""#) {
                    return true;
                }
            }
        }
    })
    .await
    .expect("status frame within 2s");
    assert!(status_seen);
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn fresh_tick(asset: AssetSymbol, source_count: u8) -> OracleTick {
    OracleTick {
        asset,
        run_id: 42,
        seq: 0,
        ts_ms: now_ms(),
        mid: 3000.0,
        vol_annualized: 0.60,
        source_count,
    }
}

async fn healthz_code(base: &str) -> u16 {
    reqwest::get(format!("{base}/healthz"))
        .await
        .unwrap()
        .status()
        .as_u16()
}

#[tokio::test]
async fn healthz_503_until_every_asset_has_fresh_two_source_ticks() {
    // ADR-0008 §5: ready only when every supported asset has a fresh tick with
    // source_count >= 2.
    let h = spawn_with_driver(42).await;
    assert_eq!(
        healthz_code(&h.base).await,
        503,
        "empty aggregator not ready"
    );

    h.rings.push(fresh_tick(AssetSymbol::Btc, 3));
    h.rings.push(fresh_tick(AssetSymbol::Eth, 3));
    assert_eq!(healthz_code(&h.base).await, 503, "missing SUI → not ready");

    h.rings.push(fresh_tick(AssetSymbol::Sui, 2));
    assert_eq!(healthz_code(&h.base).await, 200, "all assets fresh → ready");
}

#[tokio::test]
async fn healthz_503_when_an_asset_has_too_few_sources() {
    let h = spawn_with_driver(42).await;
    h.rings.push(fresh_tick(AssetSymbol::Btc, 3));
    h.rings.push(fresh_tick(AssetSymbol::Eth, 3));
    h.rings.push(fresh_tick(AssetSymbol::Sui, 1)); // single source
    assert_eq!(
        healthz_code(&h.base).await,
        503,
        "SUI < 2 sources → not ready"
    );
}

#[tokio::test]
async fn healthz_503_when_an_asset_tick_is_stale() {
    let h = spawn_with_driver(42).await;
    h.rings.push(fresh_tick(AssetSymbol::Btc, 3));
    h.rings.push(fresh_tick(AssetSymbol::Eth, 3));
    let stale = OracleTick {
        ts_ms: now_ms() - 10_000,
        ..fresh_tick(AssetSymbol::Sui, 3)
    };
    h.rings.push(stale);
    assert_eq!(
        healthz_code(&h.base).await,
        503,
        "stale SUI tick → not ready"
    );
}
