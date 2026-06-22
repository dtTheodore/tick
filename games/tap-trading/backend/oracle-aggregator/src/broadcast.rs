//! WS broadcast plumbing.
//!
//! `tokio::sync::broadcast` is the right primitive here: one producer (the
//! aggregator loop) and many consumers (WS subscribers + worker + api). At
//! 20 Hz × 3 assets ≈ 60 msg/s, a capacity of 512 absorbs ~8.5 s of burst —
//! deliberately longer than the 5 s heartbeat, so a transiently slow client
//! is not dropped before the heartbeat that proves its liveness arrives.
//! Beyond that the WS handler treats `Lagged` as fatal and closes the socket,
//! forcing the client to reconnect — better than silently desynchronising.

// Methods are added incrementally; not all are called from every task.
#![allow(dead_code)]

use crate::constants::HEARTBEAT_PERIOD_S;
use std::time::Duration;
use tap_trading_oracle_types::OracleMessage;
use tokio::sync::broadcast::{self, Sender};
use tokio::time::interval;

pub const CHANNEL_CAPACITY: usize = 512;

#[derive(Clone)]
pub struct Broadcaster {
    tx: Sender<OracleMessage>,
}

impl Broadcaster {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(CHANNEL_CAPACITY);
        Self { tx }
    }

    pub fn sender(&self) -> Sender<OracleMessage> {
        self.tx.clone()
    }

    pub fn send(&self, msg: OracleMessage) {
        // Dropped if no receivers — that's fine; no consumers yet.
        let _ = self.tx.send(msg);
    }

    /// Spawn a task that emits a `Heartbeat` every `HEARTBEAT_PERIOD_S`.
    pub fn spawn_heartbeat(&self) -> tokio::task::JoinHandle<()> {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(HEARTBEAT_PERIOD_S));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                let ts_ms = chrono_like_now_ms();
                let _ = tx.send(OracleMessage::Heartbeat { ts_ms });
            }
        })
    }
}

impl Default for Broadcaster {
    fn default() -> Self {
        Self::new()
    }
}

/// `chrono` is overkill for one timestamp; use `SystemTime` directly.
fn chrono_like_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
