mod common;

use common::TestApp;
use futures_util::StreamExt;
use std::time::Duration;
use tokio::time::timeout;

#[tokio::test]
async fn client_receives_broadcasted_frames() {
    let app = TestApp::start().await;

    // Bind the router to a real TCP port so we can open a WS connection to it.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app.router.clone();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let url = format!("ws://{}/stream", addr);
    let (mut ws, _) = timeout(
        Duration::from_secs(2),
        tokio_tungstenite::connect_async(url),
    )
    .await
    .unwrap()
    .unwrap();

    // Push a synthetic frame through the broadcast.
    let synth = r#"{"type":"tick","asset":"BTC","run_id":1,"seq":1,"ts_ms":0,"mid":50000.0,"vol_annualized":0.8,"source_count":3}"#;
    let _ = app.state.broadcast.send(synth.to_string());

    let received = timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("WS frame did not arrive within 2s")
        .unwrap()
        .unwrap();
    assert!(received.is_text());
    assert_eq!(received.into_text().unwrap(), synth);

    let _ = ws.close(None).await;
}

#[tokio::test]
async fn slow_client_dropped_on_lag() {
    let app = TestApp::start().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app.router.clone();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let url = format!("ws://{}/stream", addr);
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    // Don't read from `ws` — let it lag.

    // Push 300 frames to overflow the 256 buffer.
    for i in 0..300 {
        let _ = app
            .state
            .broadcast
            .send(format!(r#"{{"type":"heartbeat","ts_ms":{i}}}"#));
    }
    // Allow the server to detect the lag and close.
    tokio::time::sleep(Duration::from_millis(200)).await;
    // Drain pending frames until close or none.
    let mut closed = false;
    while let Ok(Some(msg)) = timeout(Duration::from_millis(500), ws.next()).await {
        if let Ok(m) = msg {
            if m.is_close() {
                closed = true;
                break;
            }
        } else {
            break;
        }
    }
    assert!(closed, "expected server to close the connection on lag");
}
