//! Postgres advisory-lock leader election.
//!
//! At most one worker holds the lock; standby polls every 1 s. The lock is
//! held by a dedicated `PgConnection` for the life of the process. On normal
//! shutdown we explicitly release; on crash, Postgres releases the lock when
//! the connection drops. Standby failover target: ≤ 2 s.

use std::time::Duration;

use sqlx::{Connection, PgConnection};
use tracing::info;

use crate::error::Result;

/// Advisory-lock key. `0x_7174_5365_7474_6c72` is the ASCII bytes of
/// `"tSettlr"` packed big-endian; hex-readable and unlikely to collide.
pub const LEADER_LOCK_KEY: i64 = 0x_7174_5365_7474_6c72_i64;

/// RAII handle. While alive, this process is the leader. Dropping releases.
pub struct LeaderGuard {
    conn: Option<PgConnection>,
}

impl LeaderGuard {
    /// Spin until the advisory lock is acquired. Polls every 1 s.
    ///
    /// All transient errors — connect refused during a Postgres restart, query
    /// errors, dropped sockets — are logged and retried; only an unrecoverable
    /// programmer error would escape. A standby must keep polling through a
    /// brief Postgres blip; previously the connect `?` exited the process and
    /// took both worker instances down on the same outage.
    pub async fn acquire_or_wait(db_url: &str) -> Result<Self> {
        loop {
            let mut conn = match PgConnection::connect(db_url).await {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(error = %e, "leader: connect failed; retrying in 1s");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            };

            let row: std::result::Result<(bool,), sqlx::Error> =
                sqlx::query_as("SELECT pg_try_advisory_lock($1)")
                    .bind(LEADER_LOCK_KEY)
                    .fetch_one(&mut conn)
                    .await;
            let acquired = match row {
                Ok((b,)) => b,
                Err(e) => {
                    tracing::warn!(error = %e, "leader: try_advisory_lock query failed; retrying in 1s");
                    drop(conn);
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            };

            if acquired {
                info!(key = LEADER_LOCK_KEY, "acquired leader lock");
                return Ok(Self { conn: Some(conn) });
            }

            drop(conn);
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    /// Explicit release for graceful shutdown. Idempotent.
    pub async fn release(&mut self) -> Result<()> {
        if let Some(mut conn) = self.conn.take() {
            sqlx::query("SELECT pg_advisory_unlock($1)")
                .bind(LEADER_LOCK_KEY)
                .execute(&mut conn)
                .await?;
            info!(key = LEADER_LOCK_KEY, "released leader lock");
        }
        Ok(())
    }
}

impl Drop for LeaderGuard {
    fn drop(&mut self) {
        if self.conn.is_some() {
            tracing::warn!(key = LEADER_LOCK_KEY, "leader lock dropped without explicit release");
        }
    }
}
