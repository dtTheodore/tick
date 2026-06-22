//! Tick oracle wire types — single source of truth.
//!
//! Spec: `docs/decisions/0008-tick-oracle-wire-protocol.md`.
//! Companion: `games/tap-trading/docs/ORACLE_SPEC.md` (semantics).
//!
//! ADR-0008 is authoritative when it disagrees with `ORACLE_SPEC §5` on
//! field names (`ts_ms` not `timestamp_ms`, `source_count` not
//! `sources_used`). See plan deviation notes for context.

pub use tap_trading_pricing_engine::AssetSymbol;

use serde::{Deserialize, Serialize};

/// One aggregated price tick for one asset at one server timestamp. ADR-0008 §3.
///
/// `(asset, run_id, seq)` is the unique key across the aggregator's lifetime.
/// `mid` and `vol_annualized` are `f64` to match `tap-trading-pricing-engine`'s
/// input signature — see ADR-0008 §3 for the precision-vs-NUMERIC reasoning.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct OracleTick {
    pub asset: AssetSymbol,
    pub run_id: u64,
    pub seq: u64,
    pub ts_ms: i64,
    pub mid: f64,
    pub vol_annualized: f64,
    pub source_count: u8,
}

/// Per-asset stream-state change. ADR-0008 §6.
///
/// `reason` is a human-readable string for operator dashboards; it is NOT
/// machine-parsed and may change format without a wire-version bump.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OracleStatus {
    pub asset: AssetSymbol,
    pub state: OracleStreamState,
    pub reason: String,
    pub run_id: u64,
}

/// Top-level WS envelope. ADR-0008 §2.
///
/// Encoding is JSON with the discriminator key `type`. Binary swap-out
/// happens at the codec layer; the enum shape is stable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OracleMessage {
    Tick(OracleTick),
    Status(OracleStatus),
    Heartbeat { ts_ms: i64 },
}

/// Per-asset stream state. ADR-0008 §6.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OracleStreamState {
    Normal,
    Degraded,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tick JSON roundtrips exactly. Field names: ts_ms (not timestamp_ms),
    /// source_count (not sources_used) — ADR-0008 §3.
    #[test]
    fn tick_roundtrips_with_expected_field_names() {
        let tick = OracleTick {
            asset: AssetSymbol::Eth,
            run_id: 1_700_000_000_000,
            seq: 9_847_234,
            ts_ms: 1_747_526_400_123,
            mid: 3812.45,
            vol_annualized: 0.78,
            source_count: 4,
        };
        let json = serde_json::to_string(&tick).unwrap();
        assert!(
            json.contains(r#""asset":"ETH""#),
            "asset uppercased: {json}"
        );
        assert!(
            json.contains(r#""ts_ms":1747526400123"#),
            "ts_ms not timestamp_ms: {json}"
        );
        assert!(
            json.contains(r#""source_count":4"#),
            "source_count not sources_used: {json}"
        );
        let back: OracleTick = serde_json::from_str(&json).unwrap();
        assert_eq!(back, tick);
    }

    /// OracleMessage uses internally-tagged enum `type: tick|status|heartbeat`.
    #[test]
    fn message_tick_serializes_with_type_tag() {
        let msg = OracleMessage::Tick(OracleTick {
            asset: AssetSymbol::Btc,
            run_id: 1,
            seq: 0,
            ts_ms: 0,
            mid: 70_000.0,
            vol_annualized: 0.60,
            source_count: 3,
        });
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.starts_with(r#"{"type":"tick""#), "tag missing: {json}");
        let back: OracleMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(back, msg);
    }

    #[test]
    fn message_status_serializes_with_snake_case_state() {
        let msg = OracleMessage::Status(OracleStatus {
            asset: AssetSymbol::Sui,
            state: OracleStreamState::Degraded,
            reason: "Pyth excluded (conf 145 bps); Bybit stale 1.2s".to_string(),
            run_id: 42,
        });
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"status""#), "type tag: {json}");
        assert!(
            json.contains(r#""state":"degraded""#),
            "snake_case state: {json}"
        );
        let back: OracleMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(back, msg);
    }

    /// Heartbeat is `{ "type": "heartbeat", "ts_ms": N }` per ADR-0008 §6.
    #[test]
    fn message_heartbeat_serializes_with_ts_ms() {
        let msg = OracleMessage::Heartbeat {
            ts_ms: 1_747_526_400_000,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(
            json, r#"{"type":"heartbeat","ts_ms":1747526400000}"#,
            "ADR-0008 §6 heartbeat shape: {json}"
        );
        let back: OracleMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(back, msg);
    }

    #[test]
    fn stream_state_serializes_snake_case() {
        let n = serde_json::to_string(&OracleStreamState::Normal).unwrap();
        let d = serde_json::to_string(&OracleStreamState::Degraded).unwrap();
        assert_eq!(n, r#""normal""#);
        assert_eq!(d, r#""degraded""#);
    }

    /// Pricing-engine and oracle-types must share ONE `AssetSymbol` — a
    /// re-export, not a structurally-identical shadow copy (ADR-0008 §1). The
    /// identity assignment compiles for free even against a look-alike enum, so
    /// assert on `TypeId`: only a genuine re-export makes the two types equal.
    #[test]
    fn asset_symbol_is_the_pricing_engine_type_not_a_shadow_copy() {
        use std::any::TypeId;
        assert_eq!(
            TypeId::of::<AssetSymbol>(),
            TypeId::of::<tap_trading_pricing_engine::AssetSymbol>(),
            "oracle-types AssetSymbol must BE pricing-engine's, not a copy"
        );
    }
}
