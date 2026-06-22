//! WS /stream — re-broadcast aggregator frames to clients.
//!
//! The handler upgrades an HTTP connection to WebSocket and forwards every
//! frame received from `state.broadcast` (populated by the aggregator subscriber
//! task). A lagged receiver gets a Close frame and the connection drops.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;

use crate::state::AppState;

pub async fn ws_stream(State(state): State<AppState>, ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle(socket, state))
}

async fn handle(mut socket: WebSocket, state: AppState) {
    // Subscribe BEFORE snapshotting so no live tick is lost in the gap between
    // the two; a tick present in both the snapshot and the live stream is a
    // harmless 1-point duplicate the client de-dups by ts.
    let mut rx = state.broadcast.subscribe();

    // Backfill: send the recent-tick history so the chart paints its real shape
    // immediately instead of a flat seed line. Skipped when the buffer is empty
    // (cold start / no upstream yet) — the client then seeds from live frames.
    if let Some(frame) = state.history.snapshot_frame() {
        if socket.send(Message::Text(frame)).await.is_err() {
            return;
        }
    }

    let mut ping = tokio::time::interval(Duration::from_secs(5));
    ping.tick().await; // consume the immediate tick so the first ping fires at t+5s
    loop {
        tokio::select! {
            biased;
            msg = rx.recv() => match msg {
                Ok(text) => {
                    if socket.send(Message::Text(text)).await.is_err() { return; }
                }
                Err(RecvError::Lagged(_)) => {
                    let _ = socket.send(Message::Close(None)).await;
                    return;
                }
                Err(RecvError::Closed) => return,
            },
            _ = ping.tick() => {
                if socket.send(Message::Ping(Vec::new())).await.is_err() { return; }
            }
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Close(_))) | None => return,
                _ => {}
            }
        }
    }
}
