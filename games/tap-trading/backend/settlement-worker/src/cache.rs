//! In-memory open-position cache and per-asset last-known mid.
//!
//! The settlement loop never round-trips to Postgres per tick. We hydrate at
//! boot, keep current via LISTEN/NOTIFY (Task 5), and re-hydrate on every
//! LISTEN reconnect (ADR-0009 §5).
//!
//! `last_known_mid` is recorded on every `OracleMessage::Tick` and used as
//! `oracle_price` on void rows — required because schema has `CHECK (oracle_price > 0)`.

use std::collections::HashMap;
use std::sync::Arc;

use sqlx::postgres::PgListener;
use sqlx::PgPool;
use tap_trading_pricing_engine::AssetSymbol;
use tokio::sync::RwLock;

use crate::error::Result;

/// The position SELECT, as a compile-time literal so every call site stays in
/// sync AND satisfies sqlx's `&'static str` requirement (it rejects runtime-built
/// query strings). `$where` is appended verbatim. Defined before first use.
macro_rules! position_select {
    ($where:literal) => {
        concat!(
            "SELECT id, account_id, asset, strike_lo, strike_hi, ",
            "t_open_ms, t_close_ms, stake_points, multiplier_at_tap, ",
            "oracle_seq_at_tap, oracle_run_id_at_tap, created_at_ms ",
            "FROM positions ",
            $where
        )
    };
}

#[derive(Clone, Debug, PartialEq)]
pub struct PositionRef {
    pub id: i64,
    pub account_id: i64,
    pub asset: AssetSymbol,
    pub strike_lo: f64,
    pub strike_hi: f64,
    pub t_open_ms: i64,
    pub t_close_ms: i64,
    pub stake_points: i64,
    pub multiplier_at_tap: f64,
    /// Tap-time inputs needed to assemble the Walrus proof (ADR-0011): the tap
    /// tick (mid/vol pulled from the ring at this seq) and the tap timestamp
    /// (`created_at_ms`), which the multiplier is computed against.
    pub oracle_seq_at_tap: i64,
    pub oracle_run_id_at_tap: i64,
    pub created_at_ms: i64,
}

#[derive(Default)]
struct CacheInner {
    positions_by_asset: HashMap<AssetSymbol, Vec<PositionRef>>,
    last_known_mid: HashMap<AssetSymbol, f64>,
}

#[derive(Clone, Default)]
pub struct OpenPositionCache {
    inner: Arc<RwLock<CacheInner>>,
}

impl OpenPositionCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace cache contents with every OPEN position from Postgres.
    /// Called on boot (after leader acquisition) and on every LISTEN reconnect.
    pub async fn hydrate(&self, pool: &PgPool) -> Result<usize> {
        let rows = sqlx::query_as::<_, PositionRow>(position_select!("WHERE status = 'OPEN'"))
            .fetch_all(pool)
            .await?;

        let mut by_asset: HashMap<AssetSymbol, Vec<PositionRef>> = HashMap::new();
        for r in rows {
            let raw_asset = r.asset.clone();
            let id = r.id;
            match r.into_ref() {
                Some(pos) => by_asset.entry(pos.asset).or_default().push(pos),
                None => tracing::warn!(asset = %raw_asset, position_id = id, "hydrate: skipping unknown asset"),
            }
        }
        let total: usize = by_asset.values().map(|v| v.len()).sum();

        let mut g = self.inner.write().await;
        g.positions_by_asset = by_asset;
        // last_known_mid persists across rehydrates — observed ticks remain valid.
        Ok(total)
    }

    /// Insert or replace a position in the cache (used by LISTEN/NOTIFY path).
    pub async fn upsert(&self, p: PositionRef) {
        let mut g = self.inner.write().await;
        let bucket = g.positions_by_asset.entry(p.asset).or_default();
        if let Some(slot) = bucket.iter_mut().find(|x| x.id == p.id) {
            *slot = p;
        } else {
            bucket.push(p);
        }
    }

    /// Remove a position from the cache (post-settlement).
    pub async fn remove(&self, asset: AssetSymbol, position_id: i64) {
        let mut g = self.inner.write().await;
        if let Some(bucket) = g.positions_by_asset.get_mut(&asset) {
            bucket.retain(|p| p.id != position_id);
        }
    }

    /// Positions whose monitoring window has opened at or before `ts_ms`.
    ///
    /// Intentionally NOT upper-bounded by `t_close_ms`: SYSTEM_DESIGN §5.2's
    /// hot loop must consider both in-window touches (Win) AND post-close
    /// untouched positions (Expire→Loss) on every tick. The caller's
    /// `evaluate_position` makes the final Win/Expire/Hold call; this method
    /// just delivers candidates. Returns owned values so callers don't hold
    /// the read lock across `.await`.
    pub async fn settleable_for_asset(&self, asset: AssetSymbol, ts_ms: i64) -> Vec<PositionRef> {
        let g = self.inner.read().await;
        g.positions_by_asset
            .get(&asset)
            .map(|bucket| {
                bucket
                    .iter()
                    .filter(|p| p.t_open_ms <= ts_ms)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// All positions in the cache regardless of asset/window.
    /// Used by the void-on-gap path and the 30 s sweep.
    pub async fn all_positions(&self) -> Vec<PositionRef> {
        let g = self.inner.read().await;
        g.positions_by_asset.values().flatten().cloned().collect()
    }

    pub async fn record_last_mid(&self, asset: AssetSymbol, mid: f64) {
        let mut g = self.inner.write().await;
        g.last_known_mid.insert(asset, mid);
    }

    pub async fn last_mid(&self, asset: AssetSymbol) -> Option<f64> {
        let g = self.inner.read().await;
        g.last_known_mid.get(&asset).copied()
    }
}

#[derive(sqlx::FromRow)]
struct PositionRow {
    id: i64,
    account_id: i64,
    asset: String,
    strike_lo: sqlx::types::BigDecimal,
    strike_hi: sqlx::types::BigDecimal,
    t_open_ms: i64,
    t_close_ms: i64,
    stake_points: i64,
    multiplier_at_tap: sqlx::types::BigDecimal,
    oracle_seq_at_tap: i64,
    oracle_run_id_at_tap: i64,
    created_at_ms: i64,
}

impl PositionRow {
    /// Map the schema's `VARCHAR(16)` asset to the typed enum. Returns `None`
    /// for unknown strings — schema drift (e.g. a new asset added ahead of this
    /// worker) must NOT data-panic the long-running hydrate/listen/sweep tasks.
    fn asset_typed(&self) -> Option<AssetSymbol> {
        match self.asset.as_str() {
            "ETH" => Some(AssetSymbol::Eth),
            "BTC" => Some(AssetSymbol::Btc),
            "SUI" => Some(AssetSymbol::Sui),
            _ => None,
        }
    }

    fn into_ref(self) -> Option<PositionRef> {
        let asset = self.asset_typed()?;
        Some(PositionRef {
            id: self.id,
            account_id: self.account_id,
            asset,
            strike_lo: bd_to_f64(&self.strike_lo),
            strike_hi: bd_to_f64(&self.strike_hi),
            t_open_ms: self.t_open_ms,
            t_close_ms: self.t_close_ms,
            stake_points: self.stake_points,
            multiplier_at_tap: bd_to_f64(&self.multiplier_at_tap),
            oracle_seq_at_tap: self.oracle_seq_at_tap,
            oracle_run_id_at_tap: self.oracle_run_id_at_tap,
            created_at_ms: self.created_at_ms,
        })
    }
}

fn bd_to_f64(b: &sqlx::types::BigDecimal) -> f64 {
    b.to_string().parse::<f64>().unwrap_or_else(|_| {
        tracing::error!(value = %b, "bigdecimal -> f64 parse failed; defaulting to 0.0");
        0.0
    })
}

impl OpenPositionCache {
    /// Long-running LISTEN/NOTIFY loop. Returns only on unrecoverable error.
    ///
    /// Contract: on EVERY reconnect, do a full `hydrate` before reading new
    /// payloads. Postgres does NOT buffer NOTIFYs across a dropped connection.
    /// ADR-0009 §5.
    pub async fn listen_loop(&self, pool: &PgPool, db_url: &str) -> Result<()> {
        loop {
            let mut listener = match PgListener::connect(db_url).await {
                Ok(l) => l,
                Err(e) => {
                    tracing::warn!(error = %e, "PgListener connect failed; retrying in 1s");
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    continue;
                }
            };

            if let Err(e) = listener.listen("tap_new_position").await {
                tracing::warn!(error = %e, "LISTEN failed; retrying");
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                continue;
            }

            // CRITICAL: re-hydrate on every reconnect — NOTIFYs while down are lost.
            match self.hydrate(pool).await {
                Ok(n) => tracing::info!(positions = n, "rehydrated on listen reconnect"),
                Err(e) => tracing::error!(error = %e, "rehydrate failed on reconnect"),
            }

            loop {
                match listener.recv().await {
                    Ok(notif) => {
                        let payload = notif.payload();
                        match payload.parse::<i64>() {
                            Ok(position_id) => {
                                if let Err(e) = self.fetch_and_upsert(pool, position_id).await {
                                    tracing::warn!(error = %e, %position_id, "fetch-on-notify failed");
                                }
                            }
                            Err(_) => tracing::warn!(payload, "non-integer NOTIFY payload"),
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "PgListener recv error; reconnecting");
                        break;
                    }
                }
            }
        }
    }

    /// Fetch a single position by id regardless of status — used by the proof
    /// flusher, which reassembles proofs for already-settled (non-OPEN)
    /// positions that are no longer in the live cache.
    pub async fn fetch_by_id(pool: &PgPool, position_id: i64) -> Result<Option<PositionRef>> {
        let row = sqlx::query_as::<_, PositionRow>(position_select!("WHERE id = $1"))
            .bind(position_id)
            .fetch_optional(pool)
            .await?;
        Ok(row.and_then(|r| r.into_ref()))
    }

    async fn fetch_and_upsert(&self, pool: &PgPool, position_id: i64) -> Result<()> {
        let row = sqlx::query_as::<_, PositionRow>(position_select!("WHERE id = $1 AND status = 'OPEN'"))
            .bind(position_id)
            .fetch_optional(pool)
            .await?;

        if let Some(r) = row {
            let raw_asset = r.asset.clone();
            match r.into_ref() {
                Some(pos) => self.upsert(pos).await,
                None => tracing::warn!(asset = %raw_asset, %position_id, "fetch_and_upsert: skipping unknown asset"),
            }
        }
        Ok(())
    }
}
