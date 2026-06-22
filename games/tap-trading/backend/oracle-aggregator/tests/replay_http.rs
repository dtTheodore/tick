//! `GET /ring/:asset/:seq?run_id=N` response matrix + WS `/stream` smoke. ADR-0008 §5.
//!
//! Boots a real axum server on a kernel-assigned free port and exercises the
//! handler over HTTP — no axum::Router::oneshot shortcuts. The test must
//! observe what an external client observes.

#[path = "../src/api.rs"]
mod api;
#[path = "../src/broadcast.rs"]
mod broadcast;
#[path = "../src/constants.rs"]
mod constants;
#[path = "../src/ring_buffer.rs"]
mod ring_buffer;

use api::{router, AppState};
use broadcast::Broadcaster;
use ring_buffer::RingBuffers;
use std::sync::Arc;
use tap_trading_oracle_types::{AssetSymbol, OracleTick};

#[allow(dead_code)]
struct TestServer {
    base_url: String,
    port: u16,
    _join: tokio::task::JoinHandle<()>,
    rings: Arc<RingBuffers>,
    broadcaster: Broadcaster,
}

async fn spawn(run_id: u64) -> TestServer {
    spawn_with_broadcaster(run_id).await
}

async fn spawn_with_broadcaster(run_id: u64) -> TestServer {
    let rings = Arc::new(RingBuffers::new());
    let broadcaster = Broadcaster::new();
    let state = AppState {
        run_id,
        rings: rings.clone(),
        broadcaster: broadcaster.clone(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = router(state);
    let join = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    TestServer {
        base_url: format!("http://127.0.0.1:{port}"),
        port,
        _join: join,
        rings,
        broadcaster,
    }
}

fn tick(run_id: u64, seq: u64) -> OracleTick {
    OracleTick {
        asset: AssetSymbol::Eth,
        run_id,
        seq,
        ts_ms: 1_000_000 + (seq as i64) * 50,
        mid: 3812.0 + seq as f64,
        vol_annualized: 0.60,
        source_count: 4,
    }
}

#[tokio::test]
async fn missing_run_id_returns_409() {
    let s = spawn(42).await;
    let resp = reqwest::get(format!("{}/ring/ETH/0", s.base_url))
        .await
        .unwrap();
    assert_eq!(resp.status(), 409);
}

#[tokio::test]
async fn wrong_run_id_returns_409() {
    let s = spawn(42).await;
    s.rings.push(tick(42, 0));
    let resp = reqwest::get(format!("{}/ring/ETH/0?run_id=99", s.base_url))
        .await
        .unwrap();
    assert_eq!(resp.status(), 409);
}

#[tokio::test]
async fn unknown_asset_returns_404() {
    let s = spawn(42).await;
    let resp = reqwest::get(format!("{}/ring/DOGE/0?run_id=42", s.base_url))
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn rotated_past_seq_returns_410() {
    let s = spawn(42).await;
    // Overflow the ring so the earliest seqs rotate out of retention.
    let overflow = constants::RING_SIZE as u64 + 5;
    for seq in 0..overflow {
        s.rings.push(tick(42, seq));
    }
    let resp = reqwest::get(format!("{}/ring/ETH/4?run_id=42", s.base_url))
        .await
        .unwrap();
    assert_eq!(resp.status(), 410);
}

#[tokio::test]
async fn hit_returns_200_with_oracle_tick_body() {
    let s = spawn(42).await;
    s.rings.push(tick(42, 0));
    let resp = reqwest::get(format!("{}/ring/ETH/0?run_id=42", s.base_url))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: OracleTick = resp.json().await.unwrap();
    assert_eq!(body.seq, 0);
    assert_eq!(body.run_id, 42);
    assert_eq!(body.source_count, 4);
}

#[tokio::test]
async fn ws_stream_accepts_upgrade_and_delivers_heartbeat() {
    use futures::StreamExt;
    use tokio_tungstenite::{connect_async, tungstenite::Message};

    let s = spawn_with_broadcaster(42).await;

    // Connect first, then send; broadcast channel only delivers to active subscribers.
    let url = format!("ws://127.0.0.1:{}/stream", s.port);
    let (mut socket, _resp) = connect_async(&url).await.unwrap();

    // Small yield so the server-side ws_session task sets up its broadcast receiver.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    s.broadcaster
        .send(tap_trading_oracle_types::OracleMessage::Heartbeat { ts_ms: 12345 });

    let frame = tokio::time::timeout(std::time::Duration::from_secs(2), socket.next())
        .await
        .expect("ws heartbeat within 2s")
        .unwrap()
        .unwrap();
    match frame {
        Message::Text(json) => {
            assert!(json.contains(r#""type":"heartbeat""#), "got: {json}");
            assert!(json.contains(r#""ts_ms":12345"#), "got: {json}");
        }
        other => panic!("expected text frame, got {other:?}"),
    }
}
