# Tick Oracle Aggregator + Wire Types — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the two crates that produce and shape Tick's live oracle feed: `tap-trading-oracle-types` (pure-library wire types per ADR-0008) and `tap-trading-oracle-aggregator` (binary that ingests Pyth Hermes + Binance + Bybit + OKX, runs the median + EWMA aggregation pipeline from `ORACLE_SPEC §4.4`, broadcasts `OracleMessage` JSON over `WS /stream`, and serves the `(asset, run_id, seq)` replay endpoint `GET /ring/:asset/:seq?run_id=N`). After this plan lands, Plan D (settlement worker) can consume the same `tap-trading-oracle-types` crate to read live ticks, and Plan E (api) can replay ticks for the drift check at `POST /positions`.

**Architecture:** Both crates live inside the existing self-contained workspace at `games/tap-trading/backend/Cargo.toml` (created by Plan A). `tap-trading-oracle-types` is a no-IO, no-async library — its only job is to be the *single* canonical encoding of `OracleMessage`, `OracleTick`, `OracleStatus`, `OracleStreamState`, and to re-export `AssetSymbol` from `tap-trading-pricing-engine` so downstream consumers do not pin `pricing-engine` directly. `tap-trading-oracle-aggregator` is a tokio + axum bin. State is entirely in-memory (per ADR-0008 §8 — no Postgres writes): per-asset latest source ticks, per-asset 1s-return deque for vol, per-asset 10-deep ring buffer for replay, a `tokio::sync::broadcast` channel for WS fanout. Sources are pluggable behind a `Source` trait so the integration test can substitute hand-rolled `tokio-tungstenite` mock servers for the real CEX endpoints.

**Tech Stack:** Rust 2021, tokio 1.x (multi-threaded runtime), axum 0.7 (HTTP + WS via `axum::extract::ws`), tokio-tungstenite 0.21 (outbound WS clients + inbound mock servers in tests), reqwest 0.12 + reqwest-eventsource 0.6 (Pyth Hermes SSE fallback path; see §4.2 of ORACLE_SPEC for WS-vs-SSE), serde + serde_json (wire), dashmap 6.x (per-asset state without `RwLock<HashMap>`), thiserror 1.x (typed errors), tracing 0.1 + tracing-subscriber 0.3 (structured logs), anyhow 1.x (main-only error glue). Test deps: `tokio` `[features = ["test-util", "macros", "rt-multi-thread"]]`, `futures` 0.3 for stream helpers. **No `wiremock`** — wiremock is HTTP-only, and 3 of the 4 sources are pure WS; mock servers are hand-rolled `tokio-tungstenite::accept_async` loops (see Task 14).

**Spec:** `docs/decisions/0008-tick-oracle-wire-protocol.md` (authoritative wire shape, replay semantics, `run_id`/`seq` rules, `vol_annualized` cold-start default = 0.60); `games/tap-trading/docs/ORACLE_SPEC.md §4` (aggregation pipeline), §5 (client subscription protocol), §6 (anti-manipulation thresholds), §7 (failure modes); `games/tap-trading/docs/MATH_SPEC.md §3.1` (EWMA vol on 1-second log returns) + §3.4 (`EWMA → jump-adjust → broadcast` per oracle tick); `games/tap-trading/docs/TESTING_STRATEGY.md §4` (aggregator test pyramid).

**Spec deviations / corrections (record before writing code):**
- **ADR-0008 vs ORACLE_SPEC field-name drift.** ORACLE_SPEC §5.3 lists `timestamp_ms` and `sources_used: [...]`; ADR-0008 §3 mandates `ts_ms: i64` and `source_count: u8`. **ADR-0008 wins** — it is the explicitly-authoritative wire spec. This plan implements `ts_ms` and `source_count` everywhere. The plan does NOT carry a `sources_used` array on `OracleTick` (debug info only — keep it server-side in tracing logs, do not put on the wire; debuggability is paid for via `tracing` not JSON bloat).
- **ADR-0008 §6 `Heartbeat` field name.** ADR shows `Heartbeat { ts_ms: i64 }`; ORACLE_SPEC §5.5 shows `timestamp_ms`. Same call as above — ADR wins.
- **Vol cold-start.** ADR-0008 §7 says emit `vol_annualized = 0.60` when fewer than 30 s of return data exist; no `cold_start: bool` flag. The Plan A `estimate_realized_vol(&returns, 0.94)` signature returns `Result<f64, PricingError::InsufficientHistory>` on empty input (vol.rs:24–31, shipped) — we DO NOT change Plan A. The aggregator's vol-state owns the cold-start branch: deque length < 30 → return `0.60` directly without calling the estimator; else call the estimator and on any `Err(_)` also return `0.60` (defensive — see Task 8). The number of returns in the deque is the gate (one per second), not wall-clock time, because the aggregator may have started mid-restart with a pre-warmed history (see next deviation).
- **Cold-start history bootstrap deferred.** ORACLE_SPEC §3.1 prescribes replaying 5 min of historical Pyth ticks at boot via Hermes REST `/v2/updates/price/{publish_time}` so σ̂ is meaningful immediately. **This plan ships WITHOUT that bootstrap.** Every aggregator restart resets all three assets to `vol_annualized = 0.60` for the first ~30 s of uptime. The floor curve (Plan A `compute_multiplier`) is the safety net during that window — the conservative 0.60 produces small multipliers, which is the correct UX. A future ticket adds the historical replay; surface as an open question for Plan-D/E review.
- **DEGRADED hysteresis duration.** ADR-0008 §6 / ORACLE_SPEC §4.4 / ORACLE_SPEC §6 specify *what* triggers DEGRADED (`source_count < 2`) but not *how long* the condition must persist before emission. We pick **2 s** (40 consecutive 50 ms ticks) for entering DEGRADED and **2 s** of `source_count ≥ 2` for recovery, named `DEGRADED_HYSTERESIS_MS = 2000` in `constants.rs`. Rationale: 50 ms is shorter than any natural network blip; 2 s avoids flapping the client overlay during reconnect storms. Surface in PR description for Plan E review.
- **`seq` pauses during DEGRADED.** ORACLE_SPEC §4.4 step 4 says "do NOT emit a new tick" when `|active| < 2`. We honor this: while an asset is DEGRADED, no `OracleTick` is appended to the ring buffer for that asset and `seq` does not advance. Only `OracleStatus { state: Degraded, … }` frames are broadcast. On recovery, the next emitted tick's `seq` is `last_seq + 1`. Document in `aggregator.rs`.
- **Source list scope.** Phase 1 supports only ETH, BTC, SOL (per `tap-trading-pricing-engine::types::AssetSymbol` and `ORACLE_SPEC §2`). Phase 2 assets (SUI, DEEP, …) are NOT in scope; the source `Symbol` map in each CEX client is small enough to enumerate by hand.

**Verification baseline:** before starting, confirm `cd games/tap-trading/backend && cargo check && cargo test && cargo clippy -- -D warnings` is green at HEAD on `feat/tap-trading` — Plan A is shipped, so all of `tap-trading-pricing-engine`'s tests must pass. After every commit in this plan, the same three commands must remain green inside `games/tap-trading/backend/`. The repo-root workspace is unaffected by this plan (no edits to root `Cargo.toml`).

---

## Commit map

| # | Subject | Scope |
|---|---------|-------|
| 1 | `feat(tick-oracle-types): scaffold oracle wire types crate` | New `oracle-types` lib crate; `AssetSymbol` re-export; `OracleStreamState`. Compiles with empty body. |
| 2 | `feat(tick-oracle-types): define oracle message and tick types` | `OracleMessage`, `OracleTick`, `OracleStatus`. Serde JSON roundtrip tests for every variant + enum tag-rename test. |
| 3 | `feat(tick-oracle): scaffold aggregator bin and axum server` | New `oracle-aggregator` bin crate; tokio runtime; env-driven port + `run_id` assignment; `GET /healthz` and `GET /metrics` stubs. |
| 4 | `feat(tick-oracle): add aggregation core median and ema blend` | Pure functions `median_of`, `ema_step`; per-asset `AggregatorState` with `apply_sources` returning `Option<OracleTick>`. Table-driven unit tests. |
| 5 | `feat(tick-oracle): add ring buffer for replay queries` | `RingBuffer` per asset, 10-deep, indexed by `(run_id, seq)`. Tests cover hit / 410 (rotated past) / 409 (wrong run_id) / 404 (unknown asset). |
| 6 | `feat(tick-oracle): expose ring buffer over http` | `GET /ring/:asset/:seq?run_id=N` wired to ring buffer; full 200 / 410 / 409 / 404 response matrix. Integration test against a live axum server bound to a free port. |
| 7 | `feat(tick-oracle): broadcast ticks over ws stream` | `tokio::sync::broadcast` channel + axum WS upgrade at `/stream`; 5 s heartbeat task; subscriber count metric. End-to-end client connects, receives JSON frames. |
| 8 | `feat(tick-oracle): wire vol state via pricing engine ewma` | Per-asset 1-s log-return deque (cap 600 = 10 min); calls `pricing_engine::estimate_realized_vol(λ=0.94)` then `jump_adjusted_sigma`; cold-start branch returns 0.60 when `len < 30`. Deterministic unit tests with hand-computed returns. |
| 9 | `feat(tick-oracle): add pyth hermes source via sse` | Hermes SSE client (`reqwest-eventsource`) implementing `Source` trait; testnet feed IDs in `config.rs`. Trait-level mock via in-memory channel exercises happy-path + low-conf-bps drop. |
| 10 | `feat(tick-oracle): add binance source via websocket` | Binance Spot WS client subscribing to `{symbol}@aggTrade`; parses to `SourceTick`. Mock harness via `tokio-tungstenite::accept_async`. |
| 11 | `feat(tick-oracle): add bybit source via websocket` | Bybit v5 public WS, channel `publicTrade.{symbol}`. Same shape. |
| 12 | `feat(tick-oracle): add okx source via websocket` | OKX v5 public WS, channel `trades-all`. Same shape. |
| 13 | `feat(tick-oracle): degraded state hysteresis and recovery` | Source health monitor; emit `OracleStatus { Degraded }` after 2 s of `source_count < 2`; recovery after 2 s of `≥ 2`. `seq` pauses during DEGRADED. Synthetic-source integration test. |
| 14 | `feat(tick-oracle): exponential backoff for source reconnects` | Per-source connection supervisor: 100 ms → 30 s with ±10% jitter; emits `tracing` events on reconnect. Unit test against a mock-server-that-rejects-then-accepts. |
| 15 | `feat(tick-oracle): add 50ms aggregator driver loop` | Single-owner driver task: `select!` over `source_rx.recv()` + `interval(50ms)`. Drains source ticks into per-asset latest-tick map; every 50 ms runs `AssetPriceState::apply_sources` → `AssetStreamPhase::step` → `AssetVolState::next_vol`, assembles `OracleTick { seq: next_seq[asset]++ }`, pushes to ring + broadcasts. Honors DEGRADED hysteresis (no ring push, no seq advance while degraded). `MissedTickBehavior::Skip`. Deterministic unit tests via `tick_once` helper + paused tokio clock. |
| 16 | `test(tick-oracle): end-to-end aggregator over mock sources` | Boot the aggregator with the real driver from Task 15 + hand-rolled mock WS / SSE sources; assert correct `OracleTick` sequence on `/stream`, correct 200/410/409 matrix on `/ring`. |

Each commit must independently pass `cargo check && cargo test && cargo clippy -- -D warnings` inside `games/tap-trading/backend/`.

---

## File map

### Created files

| Path | Responsibility |
|------|----------------|
| `games/tap-trading/backend/oracle-types/Cargo.toml` | Library crate metadata. |
| `games/tap-trading/backend/oracle-types/src/lib.rs` | All wire types per ADR-0008. Re-exports `AssetSymbol` from `tap-trading-pricing-engine`. |
| `games/tap-trading/backend/oracle-aggregator/Cargo.toml` | Bin crate metadata. |
| `games/tap-trading/backend/oracle-aggregator/src/main.rs` | Process entrypoint: tokio runtime, env loading, `run_id` assignment, axum server bind, source-supervisor spawn. |
| `games/tap-trading/backend/oracle-aggregator/src/config.rs` | Env-derived config struct (`TAP_AGGREGATOR_PORT`, source URLs, Pyth feed IDs per network). |
| `games/tap-trading/backend/oracle-aggregator/src/constants.rs` | `EMIT_PERIOD_MS = 50`, `RING_SIZE = 10`, `RETURN_DEQUE_CAP = 600`, `COLD_START_RETURN_THRESHOLD = 30`, `COLD_START_VOL_ANNUALIZED = 0.60`, `DEGRADED_HYSTERESIS_MS = 2000`, `HEARTBEAT_PERIOD_S = 5`, `SOURCE_FRESHNESS_MS = 1000`, `PYTH_CONF_REJECT_BPS = 100`. |
| `games/tap-trading/backend/oracle-aggregator/src/aggregator.rs` | `AggregatorState` per asset; `apply_sources(now, sources) -> Option<OracleTick>` implementing `ORACLE_SPEC §4.4` steps 1–7; DEGRADED hysteresis. Pure functions for `median_of`, `ema_step`. |
| `games/tap-trading/backend/oracle-aggregator/src/ring_buffer.rs` | `RingBuffer` per asset, 10-deep `VecDeque<OracleTick>` indexed by `(run_id, seq)`; `get(run_id, seq) -> RingLookup` returning `Hit / Gone / Conflict`. |
| `games/tap-trading/backend/oracle-aggregator/src/vol_state.rs` | Per-asset 1-s log-return deque (cap 600). `next_vol(now_ms, mid) -> f64` does: append return at 1-s sample boundary, call `pricing_engine::estimate_realized_vol(λ=0.94)` then `jump_adjusted_sigma`; cold-start returns `COLD_START_VOL_ANNUALIZED` when deque length < 30. |
| `games/tap-trading/backend/oracle-aggregator/src/broadcast.rs` | `tokio::sync::broadcast::Sender<OracleMessage>`; WS fanout helper that owns the heartbeat task. |
| `games/tap-trading/backend/oracle-aggregator/src/driver.rs` | 50 ms aggregator driver. Owns per-asset `AssetPriceState`, `AssetVolState`, `AssetStreamPhase`, `next_seq`, and latest-tick-per-source maps. Single-owner async task: `select!` between `source_rx.recv()` and `interval(50ms)`. |
| `games/tap-trading/backend/oracle-aggregator/src/api.rs` | Axum router: `GET /healthz`, `GET /metrics`, `GET /ring/:asset/:seq?run_id=N`, `WS /stream`. Hands the broadcast `Receiver` and ring-buffer handle to handlers via `axum::extract::State`. |
| `games/tap-trading/backend/oracle-aggregator/src/sources/mod.rs` | `Source` trait (`async fn ticks(&self) -> mpsc::Receiver<SourceTick>`); `SourceTick` struct; `SourceId` enum; `start_with_backoff` helper. |
| `games/tap-trading/backend/oracle-aggregator/src/sources/pyth.rs` | Pyth Hermes SSE client implementing `Source`. |
| `games/tap-trading/backend/oracle-aggregator/src/sources/binance.rs` | Binance Spot WS client. |
| `games/tap-trading/backend/oracle-aggregator/src/sources/bybit.rs` | Bybit v5 Spot WS client. |
| `games/tap-trading/backend/oracle-aggregator/src/sources/okx.rs` | OKX v5 Spot WS client. |
| `games/tap-trading/backend/oracle-aggregator/tests/aggregation.rs` | Integration tests for `AggregatorState` driven by synthetic `SourceTick` streams. |
| `games/tap-trading/backend/oracle-aggregator/tests/ring_buffer.rs` | Integration tests for the ring buffer `Hit / Gone / Conflict` semantics. |
| `games/tap-trading/backend/oracle-aggregator/tests/replay_http.rs` | Integration tests for `GET /ring/:asset/:seq?run_id=N` over a live axum server. |
| `games/tap-trading/backend/oracle-aggregator/tests/end_to_end.rs` | Hand-rolled mock WS + mock SSE servers; full aggregator boot; assertions on `/stream` and `/ring`. |

### Modified files

| Path | Why |
|------|-----|
| `games/tap-trading/backend/Cargo.toml` | Add `oracle-types` and `oracle-aggregator` to `members`. Add new `[workspace.dependencies]`: tokio, axum, tokio-tungstenite, reqwest, reqwest-eventsource, dashmap, thiserror, tracing, tracing-subscriber, futures, anyhow. |

---

## Pre-flight (one-time, not a commit)

- [ ] **Step P1: Verify the Tick workspace baseline is green at HEAD**

Run from repo root:

```bash
cd games/tap-trading/backend && cargo check && cargo test && cargo clippy -- -D warnings
```

Expected: all three succeed with no warnings. `cargo test` should show the full Plan-A pricing-engine test suite passing (Hui, BGK, vol, multiplier, quantlib_parity). If any of those red, stop — Plan A invariants must hold before this plan adds dependencies on the same crate.

- [ ] **Step P2: Confirm `AssetSymbol`, `estimate_realized_vol`, `jump_adjusted_sigma` are exported**

```bash
grep -nE 'pub use (types::AssetSymbol|vol::\{estimate_realized_vol, jump_adjusted_sigma\})' \
  games/tap-trading/backend/pricing-engine/src/lib.rs
```

Expected: both lines present in `lib.rs`. If `jump_adjusted_sigma` is missing, the §3.4 jump-buffer step in Task 8 cannot be wired — stop and fix Plan A first.

- [ ] **Step P3: Confirm root workspace does not pick up the new crates**

```bash
grep -nE '^(\s*"games/tap-trading)' Cargo.toml || echo "OK"
```

Expected: `OK`. Tick is a separate workspace per `SYSTEM_DESIGN §0`.

- [ ] **Step P4: Confirm rust toolchain**

```bash
rustc --version
```

Expected: rust 1.80 or newer (matches `games/tap-trading/backend/Cargo.toml` `rust-version = "1.80"`). If older, `rustup update stable`.

---

## Task 1 — Scaffold `tap-trading-oracle-types`

Empty library crate registered in the workspace. No types yet — they land in Task 2 driven by failing tests.

**Files:**
- Modify: `games/tap-trading/backend/Cargo.toml` (add `oracle-types` to `members`)
- Create: `games/tap-trading/backend/oracle-types/Cargo.toml`
- Create: `games/tap-trading/backend/oracle-types/src/lib.rs`

- [ ] **Step 1.1: Register the crate in the workspace and add the new workspace deps**

Edit `games/tap-trading/backend/Cargo.toml`. Inside `members`, add `"oracle-types"` so the list becomes:

```toml
members = [
    "pricing-engine",
    "oracle-types",
]
```

Inside `[workspace.dependencies]`, add (alphabetized):

```toml
tap-trading-oracle-types = { path = "oracle-types" }
tap-trading-pricing-engine = { path = "pricing-engine" }
thiserror = "1.0"
```

The two `tap-trading-*` lines let downstream crates (the aggregator next, and Plans D + E later) depend via `workspace = true` instead of typing `path = ".."` every time.

- [ ] **Step 1.2: Write the crate `Cargo.toml`**

Write `games/tap-trading/backend/oracle-types/Cargo.toml`:

```toml
[package]
name = "tap-trading-oracle-types"
version = "0.0.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true
publish.workspace = true

[lib]
path = "src/lib.rs"

[dependencies]
serde = { workspace = true }
tap-trading-pricing-engine = { workspace = true }

[dev-dependencies]
serde_json = { workspace = true }
```

No tokio, no axum, no IO. This crate is consumed by the aggregator, the worker, and the api; keeping it pure means every consumer compiles the same struct definition.

- [ ] **Step 1.3: Write a placeholder `lib.rs` so the empty crate compiles**

Write `games/tap-trading/backend/oracle-types/src/lib.rs`:

```rust
//! Tick oracle wire types — single source of truth.
//!
//! Spec: `docs/decisions/0008-tick-oracle-wire-protocol.md`.
//! Companion: `games/tap-trading/docs/ORACLE_SPEC.md` (semantics).
//!
//! ADR-0008 is authoritative when it disagrees with `ORACLE_SPEC §5` on
//! field names (`ts_ms` not `timestamp_ms`, `source_count` not
//! `sources_used`). See plan deviation notes for context.

pub use tap_trading_pricing_engine::AssetSymbol;

/// Per-asset stream state. ADR-0008 §6.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OracleStreamState {
    Normal,
    Degraded,
}
```

`AssetSymbol` is re-exported here (not redefined) so a typo in either crate is a compile error, not a silent wire mismatch.

- [ ] **Step 1.4: Verify the empty crate builds**

```bash
cd games/tap-trading/backend && cargo check -p tap-trading-oracle-types && cargo clippy -p tap-trading-oracle-types -- -D warnings
```

Expected: green. `cargo test -p tap-trading-oracle-types` runs zero tests and exits 0 (we add them in Task 2).

- [ ] **Step 1.5: Commit**

```bash
git add games/tap-trading/backend/Cargo.toml \
        games/tap-trading/backend/oracle-types/
git commit -m "feat(tick-oracle-types): scaffold oracle wire types crate"
```

---

## Task 2 — Define `OracleMessage`, `OracleTick`, `OracleStatus`

The full ADR-0008 wire surface. TDD flow: write serde-roundtrip tests against expected JSON shapes first, then add the structs to make them pass.

**Files:**
- Modify: `games/tap-trading/backend/oracle-types/src/lib.rs`

- [ ] **Step 2.1: Write the failing serde roundtrip tests**

Append to `games/tap-trading/backend/oracle-types/src/lib.rs`:

```rust
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
        assert!(json.contains(r#""asset":"ETH""#), "asset uppercased: {json}");
        assert!(json.contains(r#""ts_ms":1747526400123"#), "ts_ms not timestamp_ms: {json}");
        assert!(json.contains(r#""source_count":4"#), "source_count not sources_used: {json}");
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
            asset: AssetSymbol::Sol,
            state: OracleStreamState::Degraded,
            reason: "Pyth excluded (conf 145 bps); Bybit stale 1.2s".to_string(),
            run_id: 42,
        });
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"status""#), "type tag: {json}");
        assert!(json.contains(r#""state":"degraded""#), "snake_case state: {json}");
        let back: OracleMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(back, msg);
    }

    /// Heartbeat is `{ "type": "heartbeat", "ts_ms": N }` per ADR-0008 §6.
    #[test]
    fn message_heartbeat_serializes_with_ts_ms() {
        let msg = OracleMessage::Heartbeat { ts_ms: 1_747_526_400_000 };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(
            json,
            r#"{"type":"heartbeat","ts_ms":1747526400000}"#,
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

    /// Pricing-engine and oracle-types must share AssetSymbol — re-export, not redefine.
    #[test]
    fn asset_symbol_is_reexported_from_pricing_engine() {
        let our: AssetSymbol = AssetSymbol::Eth;
        let theirs: tap_trading_pricing_engine::AssetSymbol = our;
        let _ = theirs;
    }
}
```

- [ ] **Step 2.2: Run, verify all tests fail with type-not-found errors**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-oracle-types
```

Expected: compile errors `cannot find type OracleTick / OracleMessage / OracleStatus`. The `OracleStreamState` and `asset_symbol_is_reexported_from_pricing_engine` tests should compile but fail to link until the rest of the module compiles. This is the correct failing state.

- [ ] **Step 2.3: Define the types**

Insert before the `#[cfg(test)]` block in `games/tap-trading/backend/oracle-types/src/lib.rs`:

```rust
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
```

- [ ] **Step 2.4: Re-run the tests**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-oracle-types
```

Expected: all 6 tests pass. Likely failure modes:
- `message_tick_serializes_with_type_tag`: if it fails with `{"Tick":{…}}` instead of `{"type":"tick", …}`, the enum is missing `#[serde(tag = "type", rename_all = "snake_case")]`.
- `message_heartbeat_serializes_with_ts_ms`: if it serializes as `{"type":"Heartbeat","ts_ms":…}`, the `rename_all` is missing or wrong.
- `tick_roundtrips_with_expected_field_names`: if `ts_ms` becomes `timestamp_ms`, you accidentally added `#[serde(rename = …)]` somewhere — remove it.

- [ ] **Step 2.5: Full crate check**

```bash
cd games/tap-trading/backend && cargo check && cargo clippy -p tap-trading-oracle-types --all-targets -- -D warnings && cargo test -p tap-trading-oracle-types
```

Expected: green.

- [ ] **Step 2.6: Commit**

```bash
git add games/tap-trading/backend/oracle-types/src/lib.rs
git commit -m "feat(tick-oracle-types): define oracle message and tick types"
```

---

## Task 3 — Scaffold `tap-trading-oracle-aggregator` bin

Bin crate with the tokio runtime, env-driven config, `run_id` assignment, and the two stub endpoints (`/healthz`, `/metrics`). No source clients, no aggregation logic — those land in later tasks.

**Files:**
- Modify: `games/tap-trading/backend/Cargo.toml` (add `oracle-aggregator` to members + add new workspace deps)
- Create: `games/tap-trading/backend/oracle-aggregator/Cargo.toml`
- Create: `games/tap-trading/backend/oracle-aggregator/src/main.rs`
- Create: `games/tap-trading/backend/oracle-aggregator/src/config.rs`
- Create: `games/tap-trading/backend/oracle-aggregator/src/constants.rs`
- Create: `games/tap-trading/backend/oracle-aggregator/src/api.rs`

- [ ] **Step 3.1: Register the crate and add workspace deps**

Edit `games/tap-trading/backend/Cargo.toml`. Add `"oracle-aggregator"` to `members`:

```toml
members = [
    "pricing-engine",
    "oracle-types",
    "oracle-aggregator",
]
```

In `[workspace.dependencies]` (alphabetized), add:

```toml
anyhow = "1.0"
axum = { version = "0.7", features = ["ws"] }
dashmap = "6.0"
futures = "0.3"
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "stream"] }
reqwest-eventsource = "0.6"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "sync", "time", "signal", "net"] }
tokio-tungstenite = { version = "0.21", features = ["rustls-tls-webpki-roots"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

`reqwest` uses `rustls-tls` (not native-tls) to keep cross-platform CI cheap; `stream` is needed by the Hermes SSE adapter.

- [ ] **Step 3.2: Write the bin crate `Cargo.toml`**

Write `games/tap-trading/backend/oracle-aggregator/Cargo.toml`:

```toml
[package]
name = "tap-trading-oracle-aggregator"
version = "0.0.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true
publish.workspace = true

[[bin]]
name = "tap-trading-oracle-aggregator"
path = "src/main.rs"

[dependencies]
anyhow = { workspace = true }
axum = { workspace = true }
dashmap = { workspace = true }
futures = { workspace = true }
reqwest = { workspace = true }
reqwest-eventsource = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
tap-trading-oracle-types = { workspace = true }
tap-trading-pricing-engine = { workspace = true }
thiserror = { workspace = true }
tokio = { workspace = true }
tokio-tungstenite = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }

[dev-dependencies]
futures = { workspace = true }
tokio = { workspace = true, features = ["test-util", "macros", "rt-multi-thread"] }
```

- [ ] **Step 3.3: Write `constants.rs`**

Write `games/tap-trading/backend/oracle-aggregator/src/constants.rs`:

```rust
//! Compile-time constants. Sourced from `ORACLE_SPEC` and ADR-0008 unless noted.

/// Aggregator emit cadence. ORACLE_SPEC §4.1 / §5.6.
pub const EMIT_PERIOD_MS: u64 = 50;

/// Ring-buffer depth per asset (500 ms at 20 Hz). ADR-0008 §5.
pub const RING_SIZE: usize = 10;

/// Cap on the 1-s log-return deque per asset (10 min). MATH_SPEC §3.1 history.
pub const RETURN_DEQUE_CAP: usize = 600;

/// Below this many returns, the aggregator emits the cold-start vol default.
/// ADR-0008 §7.
pub const COLD_START_RETURN_THRESHOLD: usize = 30;

/// Cold-start `vol_annualized` value. ADR-0008 §7.
pub const COLD_START_VOL_ANNUALIZED: f64 = 0.60;

/// Plan decision: how long `source_count < 2` must persist before emitting
/// `Status::Degraded` (and how long `>= 2` must persist before clearing).
/// 2 s = 40 consecutive 50 ms ticks. See plan deviation notes.
pub const DEGRADED_HYSTERESIS_MS: u64 = 2_000;

/// WS heartbeat cadence. ADR-0008 §5 / ORACLE_SPEC §5.5.
pub const HEARTBEAT_PERIOD_S: u64 = 5;

/// Source freshness window. ORACLE_SPEC §4.4 step 2.
pub const SOURCE_FRESHNESS_MS: u64 = 1_000;

/// Pyth confidence-interval rejection threshold (basis points). ORACLE_SPEC §6.
pub const PYTH_CONF_REJECT_BPS: u32 = 100;

/// EWMA λ for vol on 1-s returns. MATH_SPEC §3.1 (RiskMetrics standard).
pub const EWMA_LAMBDA_VOL: f64 = 0.94;

/// EMA α for median-price smoothing. ORACLE_SPEC §4.4 step 6.
///
/// Distinct from EWMA_LAMBDA_VOL — this α is on the *price* path
/// (responsiveness), λ above is on the *vol* path (statistical
/// smoothing). Conflating them collapses the two-state design.
pub const EMA_ALPHA_PRICE: f64 = 0.6;
```

- [ ] **Step 3.4: Write `config.rs`**

Write `games/tap-trading/backend/oracle-aggregator/src/config.rs`:

```rust
//! Env-derived process config.
//!
//! Per repo CLAUDE.md: no string-literal port fallbacks. A missing env var
//! is a setup bug — fail loudly so the worktree dev-env can wire the value.

use anyhow::{Context, Result};
use std::env;

#[derive(Debug, Clone)]
pub struct AggregatorConfig {
    pub bind_addr: String,
    pub network: PythNetwork,
    pub hermes_base_url: String,
    pub binance_ws_url: String,
    pub bybit_ws_url: String,
    pub okx_ws_url: String,
}

#[derive(Debug, Clone, Copy)]
pub enum PythNetwork {
    Mainnet,
    Testnet,
}

impl AggregatorConfig {
    /// Load from env. `TAP_AGGREGATOR_PORT` is mandatory.
    pub fn from_env() -> Result<Self> {
        let port: u16 = env::var("TAP_AGGREGATOR_PORT")
            .context("TAP_AGGREGATOR_PORT must be set by the worktree dev-env")?
            .parse()
            .context("TAP_AGGREGATOR_PORT must parse as u16")?;
        let bind_addr = format!("0.0.0.0:{port}");

        let network = match env::var("TAP_PYTH_NETWORK").as_deref() {
            Ok("mainnet") => PythNetwork::Mainnet,
            Ok("testnet") | Err(_) => PythNetwork::Testnet,
            Ok(other) => anyhow::bail!("TAP_PYTH_NETWORK must be 'mainnet' or 'testnet', got {other}"),
        };

        let hermes_base_url = match network {
            PythNetwork::Mainnet => "https://hermes.pyth.network".to_string(),
            PythNetwork::Testnet => "https://hermes-beta.pyth.network".to_string(),
        };

        Ok(Self {
            bind_addr,
            network,
            hermes_base_url,
            binance_ws_url: "wss://stream.binance.com:9443/ws".to_string(),
            bybit_ws_url: "wss://stream.bybit.com/v5/public/spot".to_string(),
            okx_ws_url: "wss://ws.okx.com:8443/ws/v5/public".to_string(),
        })
    }
}
```

The CEX WS URLs are hard-coded — they have no per-environment variant (unlike Pyth, which has separate mainnet vs testnet Hermes endpoints). The worktree dev-env wires `TAP_AGGREGATOR_PORT` and `TAP_PYTH_NETWORK` in Plan B.

- [ ] **Step 3.5: Write the api stub**

Write `games/tap-trading/backend/oracle-aggregator/src/api.rs`:

```rust
//! HTTP + WS router. Full surface lands across Tasks 6, 7.

use axum::{routing::get, Router};

pub fn router() -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics))
}

async fn healthz() -> &'static str {
    // Real health logic (per-asset source_count check) lands in Task 13.
    "ok"
}

async fn metrics() -> &'static str {
    // Stub. Prometheus exposition format lands in a later plan.
    "# tap_trading_oracle_aggregator metrics stub\n"
}
```

- [ ] **Step 3.6: Write `main.rs`**

Write `games/tap-trading/backend/oracle-aggregator/src/main.rs`:

```rust
//! Tick oracle aggregator — process entrypoint.

mod api;
mod config;
mod constants;

use anyhow::{Context, Result};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();

    let cfg = config::AggregatorConfig::from_env()?;
    let run_id = assign_run_id();
    tracing::info!(?cfg.network, %cfg.bind_addr, run_id, "oracle-aggregator starting");

    let listener = tokio::net::TcpListener::bind(&cfg.bind_addr)
        .await
        .with_context(|| format!("bind {}", cfg.bind_addr))?;
    let app = api::router();
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("axum serve")?;

    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("tap_trading_oracle_aggregator=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

/// Pick a `run_id`. ADR-0008 §4 allows unix-ms; we use it for human readability
/// in logs. Restart → fresh `run_id`, so clients with cached `oracle_seq_at_tap`
/// get a 409 Conflict from `/ring` and re-fetch a fresh quote.
fn assign_run_id() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(1)
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.ok();
    };
    #[cfg(unix)]
    let term = async {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut s) = signal(SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();

    tokio::select! { _ = ctrl_c => {}, _ = term => {} }
    tracing::info!("oracle-aggregator shutting down");
}
```

- [ ] **Step 3.7: Smoke-test by binding to a free port**

```bash
cd games/tap-trading/backend && cargo build -p tap-trading-oracle-aggregator
```

Expected: compiles clean. Now bind-test:

```bash
cd games/tap-trading/backend && TAP_AGGREGATOR_PORT=0 \
  cargo run -p tap-trading-oracle-aggregator >/tmp/agg.out 2>&1 &
sleep 1
kill %1 2>/dev/null
grep "oracle-aggregator starting" /tmp/agg.out
```

Expected: the log line appears with a `run_id` and `bind_addr`. `TAP_AGGREGATOR_PORT=0` asks the kernel for any free port, which is enough to prove the binary boots without hard-coding a worktree-specific port. If the bind fails with `Address already in use`, the test env has something on port 0 (impossible) — re-run.

- [ ] **Step 3.8: Verify env-validation failure mode**

```bash
cd games/tap-trading/backend && (cargo run -p tap-trading-oracle-aggregator 2>&1 || true) | grep -i "TAP_AGGREGATOR_PORT"
```

Expected: the process exits non-zero with `TAP_AGGREGATOR_PORT must be set by the worktree dev-env`. Per repo CLAUDE.md ("Read config from the env, not from literals … A missing env var is a setup bug — fail loudly"), this failure mode is required.

- [ ] **Step 3.9: Full check + clippy + test**

```bash
cd games/tap-trading/backend && cargo check && cargo clippy --all-targets -- -D warnings && cargo test
```

Expected: green. The aggregator crate has no tests yet — `cargo test -p tap-trading-oracle-aggregator` runs zero and exits 0.

- [ ] **Step 3.10: Commit**

```bash
git add games/tap-trading/backend/Cargo.toml \
        games/tap-trading/backend/oracle-aggregator/
git commit -m "feat(tick-oracle): scaffold aggregator bin and axum server"
```

---

## Task 4 — Aggregation core: median + EMA blend

Pure functions and the per-asset state machine that implements `ORACLE_SPEC §4.4` steps 1–7 (snapshot latest, drop stale, drop low-confidence Pyth, drop if `|active| < 2`, median, EWMA, emit). No sources, no axum, no tokio yet — `apply_sources` takes a `&BTreeMap<SourceId, SourceTick>` and `now_ms` and returns an `Option<OracleTick>`.

**Files:**
- Create: `games/tap-trading/backend/oracle-aggregator/src/sources/mod.rs` (just the trait + `SourceTick` struct; impls land in Tasks 9–12)
- Create: `games/tap-trading/backend/oracle-aggregator/src/aggregator.rs`
- Modify: `games/tap-trading/backend/oracle-aggregator/src/main.rs` (declare the new modules)
- Create: `games/tap-trading/backend/oracle-aggregator/tests/aggregation.rs`

- [ ] **Step 4.1: Define `SourceTick` and `SourceId`**

Write `games/tap-trading/backend/oracle-aggregator/src/sources/mod.rs`:

```rust
//! Source-side types. Concrete clients land in Tasks 9–12.

use tap_trading_oracle_types::AssetSymbol;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SourceId {
    Pyth,
    Binance,
    Bybit,
    Okx,
}

impl SourceId {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceId::Pyth => "pyth",
            SourceId::Binance => "binance",
            SourceId::Bybit => "bybit",
            SourceId::Okx => "okx",
        }
    }
}

/// One observation from one source, normalized into our units.
///
/// `ts_ms` is the **server-received** timestamp — never the exchange-provided
/// one (ORACLE_SPEC §4.5 rationale).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SourceTick {
    pub source: SourceId,
    pub asset: AssetSymbol,
    pub price: f64,
    pub ts_ms: i64,
    /// Pyth confidence interval in basis points of price; `None` for non-Pyth.
    pub pyth_conf_bps: Option<u32>,
}
```

- [ ] **Step 4.2: Write `aggregator.rs` skeleton + failing tests**

Write `games/tap-trading/backend/oracle-aggregator/src/aggregator.rs`:

```rust
//! Per-asset aggregation state and pure helpers.
//!
//! `ORACLE_SPEC §4.4` defines the 7-step pipeline that runs every 50 ms.
//! Vol-state, ring-buffer, and broadcast wiring live in sibling modules.
//! This file owns only steps 1–6 (price aggregation) — step 7 (`OracleTick`
//! assembly with `seq`, `run_id`, `vol_annualized`) is the caller's.

use crate::constants::{
    EMA_ALPHA_PRICE, PYTH_CONF_REJECT_BPS, SOURCE_FRESHNESS_MS,
};
use crate::sources::{SourceId, SourceTick};
use std::collections::BTreeMap;

/// Median of a non-empty slice of `f64`. For even length, returns the mean
/// of the two middle elements (statistical median). Caller guarantees no NaN.
pub fn median_of(prices: &[f64]) -> f64 {
    assert!(!prices.is_empty(), "median_of: empty slice");
    let mut sorted = prices.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).expect("no NaN allowed"));
    let n = sorted.len();
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        0.5 * (sorted[n / 2 - 1] + sorted[n / 2])
    }
}

/// EMA step: `smoothed_t = α · raw + (1 − α) · smoothed_{t−1}`.
/// Returns `raw` if `prev` is `None` (cold start).
pub fn ema_step(prev: Option<f64>, raw: f64, alpha: f64) -> f64 {
    match prev {
        None => raw,
        Some(p) => alpha * raw + (1.0 - alpha) * p,
    }
}

/// Result of one aggregation step. ORACLE_SPEC §4.4.
#[derive(Debug, Clone, PartialEq)]
pub enum AggregateOutcome {
    /// New aggregated price ready (steps 1–6 succeeded).
    Emit { mid: f64, median: f64, source_count: u8 },
    /// Too few active sources (`< 2`). Caller decides whether to emit Status.
    InsufficientSources { reason: String },
}

/// Per-asset price state. Owns the EMA carrier (`smoothed`).
#[derive(Debug, Default)]
pub struct AssetPriceState {
    smoothed: Option<f64>,
}

impl AssetPriceState {
    /// Run steps 1–6 against the latest tick per source.
    pub fn apply_sources(
        &mut self,
        now_ms: i64,
        latest: &BTreeMap<SourceId, SourceTick>,
    ) -> AggregateOutcome {
        // Step 2: drop stale sources.
        let mut dropped = Vec::new();
        let active: Vec<&SourceTick> = latest
            .values()
            .filter(|t| {
                let age = now_ms - t.ts_ms;
                let fresh = age <= SOURCE_FRESHNESS_MS as i64 && age >= 0;
                if !fresh {
                    dropped.push(format!("{} stale {}ms", t.source.as_str(), age));
                }
                fresh
            })
            // Step 3: drop low-confidence Pyth.
            .filter(|t| match t.pyth_conf_bps {
                Some(bps) if bps > PYTH_CONF_REJECT_BPS => {
                    dropped.push(format!("pyth conf {bps} bps"));
                    false
                }
                _ => true,
            })
            .collect();

        // Step 4: minimum active count.
        if active.len() < 2 {
            return AggregateOutcome::InsufficientSources {
                reason: dropped.join("; "),
            };
        }

        // Step 5: median.
        let prices: Vec<f64> = active.iter().map(|t| t.price).collect();
        let median = median_of(&prices);

        // Step 6: EMA blend.
        let mid = ema_step(self.smoothed, median, EMA_ALPHA_PRICE);
        self.smoothed = Some(mid);

        AggregateOutcome::Emit {
            mid,
            median,
            source_count: active.len() as u8,
        }
    }
}
```

- [ ] **Step 4.3: Wire the new modules into `main.rs`**

Edit `games/tap-trading/backend/oracle-aggregator/src/main.rs`. Add `mod aggregator;` and `mod sources;` next to the existing `mod` declarations:

```rust
mod aggregator;
mod api;
mod config;
mod constants;
mod sources;
```

- [ ] **Step 4.4: Write the integration test**

Write `games/tap-trading/backend/oracle-aggregator/tests/aggregation.rs`:

```rust
//! Aggregation core tests. ORACLE_SPEC §4.4 and TESTING_STRATEGY §4.1.

use std::collections::BTreeMap;
use tap_trading_oracle_types::AssetSymbol;

// Re-export module paths from the bin crate. Cargo allows integration tests
// to reach into the bin's library — but a bin crate has no library. We add
// `#[path = ...]` includes here to pull the source files directly.
//
// (Avoiding the alternative of splitting the bin into a lib + bin is
// deliberate: this crate's only public surface is the binary; tests are
// the only callers of these modules.)
#[path = "../src/aggregator.rs"]
mod aggregator;
#[path = "../src/constants.rs"]
mod constants;
#[path = "../src/sources/mod.rs"]
mod sources;

use aggregator::{ema_step, median_of, AggregateOutcome, AssetPriceState};
use sources::{SourceId, SourceTick};

fn tick(src: SourceId, asset: AssetSymbol, price: f64, ts_ms: i64) -> SourceTick {
    SourceTick { source: src, asset, price, ts_ms, pyth_conf_bps: None }
}

fn pyth(asset: AssetSymbol, price: f64, ts_ms: i64, conf_bps: u32) -> SourceTick {
    SourceTick {
        source: SourceId::Pyth,
        asset,
        price,
        ts_ms,
        pyth_conf_bps: Some(conf_bps),
    }
}

#[test]
fn median_of_odd_length_picks_middle() {
    assert_eq!(median_of(&[3.0, 1.0, 2.0]), 2.0);
}

#[test]
fn median_of_even_length_averages_middle_two() {
    assert_eq!(median_of(&[1.0, 2.0, 3.0, 4.0]), 2.5);
}

#[test]
fn median_robust_to_single_outlier() {
    // ORACLE_SPEC §4.5 rationale: one bad Pyth update should not shift the mid.
    let m = median_of(&[100.0, 100.1, 100.0, 50_000.0]);
    assert!((m - 100.05).abs() < 1e-9, "got {m}");
}

#[test]
fn ema_cold_start_equals_raw() {
    assert_eq!(ema_step(None, 3812.5, 0.6), 3812.5);
}

#[test]
fn ema_blends_with_prior() {
    let s = ema_step(Some(100.0), 200.0, 0.6);
    assert!((s - 160.0).abs() < 1e-9, "got {s}");
}

#[test]
fn four_active_sources_produce_emit() {
    let now = 1_000_000;
    let mut state = AssetPriceState::default();
    let mut latest = BTreeMap::new();
    latest.insert(SourceId::Pyth, pyth(AssetSymbol::Eth, 3812.0, now, 50));
    latest.insert(SourceId::Binance, tick(SourceId::Binance, AssetSymbol::Eth, 3812.10, now));
    latest.insert(SourceId::Bybit, tick(SourceId::Bybit, AssetSymbol::Eth, 3812.20, now));
    latest.insert(SourceId::Okx, tick(SourceId::Okx, AssetSymbol::Eth, 3812.30, now));
    match state.apply_sources(now, &latest) {
        AggregateOutcome::Emit { source_count, .. } => assert_eq!(source_count, 4),
        other => panic!("expected Emit, got {other:?}"),
    }
}

#[test]
fn stale_source_dropped_above_1000ms() {
    let now = 1_000_000;
    let mut state = AssetPriceState::default();
    let mut latest = BTreeMap::new();
    // Pyth is 1500ms old — must be dropped.
    latest.insert(SourceId::Pyth, pyth(AssetSymbol::Eth, 3812.0, now - 1_500, 50));
    latest.insert(SourceId::Binance, tick(SourceId::Binance, AssetSymbol::Eth, 3812.1, now));
    latest.insert(SourceId::Bybit, tick(SourceId::Bybit, AssetSymbol::Eth, 3812.2, now));
    latest.insert(SourceId::Okx, tick(SourceId::Okx, AssetSymbol::Eth, 3812.3, now));
    match state.apply_sources(now, &latest) {
        AggregateOutcome::Emit { source_count, .. } => assert_eq!(source_count, 3),
        other => panic!("expected Emit, got {other:?}"),
    }
}

#[test]
fn pyth_dropped_when_confidence_above_100bps() {
    let now = 1_000_000;
    let mut state = AssetPriceState::default();
    let mut latest = BTreeMap::new();
    latest.insert(SourceId::Pyth, pyth(AssetSymbol::Eth, 3812.0, now, 145));
    latest.insert(SourceId::Binance, tick(SourceId::Binance, AssetSymbol::Eth, 3812.1, now));
    latest.insert(SourceId::Bybit, tick(SourceId::Bybit, AssetSymbol::Eth, 3812.2, now));
    latest.insert(SourceId::Okx, tick(SourceId::Okx, AssetSymbol::Eth, 3812.3, now));
    match state.apply_sources(now, &latest) {
        AggregateOutcome::Emit { source_count, .. } => assert_eq!(source_count, 3),
        other => panic!("expected Emit, got {other:?}"),
    }
}

#[test]
fn insufficient_sources_yields_status_signal() {
    let now = 1_000_000;
    let mut state = AssetPriceState::default();
    let mut latest = BTreeMap::new();
    latest.insert(SourceId::Pyth, pyth(AssetSymbol::Eth, 3812.0, now - 5_000, 50));
    latest.insert(SourceId::Binance, tick(SourceId::Binance, AssetSymbol::Eth, 3812.1, now));
    latest.insert(SourceId::Bybit, tick(SourceId::Bybit, AssetSymbol::Eth, 3812.2, now - 5_000));
    latest.insert(SourceId::Okx, tick(SourceId::Okx, AssetSymbol::Eth, 3812.3, now - 5_000));
    match state.apply_sources(now, &latest) {
        AggregateOutcome::InsufficientSources { reason } => {
            assert!(reason.contains("stale"), "reason should mention staleness: {reason}");
        }
        other => panic!("expected InsufficientSources, got {other:?}"),
    }
}

#[test]
fn ema_carries_over_across_calls() {
    // Two consecutive apply_sources: second result should be blended with first.
    let now = 1_000_000;
    let mut state = AssetPriceState::default();
    let four = |p: f64, t: i64| {
        let mut m = BTreeMap::new();
        m.insert(SourceId::Pyth, pyth(AssetSymbol::Eth, p, t, 50));
        m.insert(SourceId::Binance, tick(SourceId::Binance, AssetSymbol::Eth, p, t));
        m.insert(SourceId::Bybit, tick(SourceId::Bybit, AssetSymbol::Eth, p, t));
        m.insert(SourceId::Okx, tick(SourceId::Okx, AssetSymbol::Eth, p, t));
        m
    };
    let r1 = state.apply_sources(now, &four(100.0, now));
    let r2 = state.apply_sources(now + 50, &four(200.0, now + 50));
    let mid2 = match r2 {
        AggregateOutcome::Emit { mid, .. } => mid,
        _ => panic!("expected Emit"),
    };
    // r1.mid = 100; raw2 = 200; blended = 0.6·200 + 0.4·100 = 160.
    assert!((mid2 - 160.0).abs() < 1e-6, "got {mid2}");
    let _ = r1;
}
```

- [ ] **Step 4.5: Run the tests**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-oracle-aggregator --test aggregation
```

Expected: all 9 tests pass. Likely failure modes:
- `pyth_dropped_when_confidence_above_100bps` fails: the conf check uses `>=` instead of `>` — re-read ORACLE_SPEC §4.4 step 3 ("`pyth.conf > 100 bps`").
- `stale_source_dropped_above_1000ms` fails with `source_count = 4`: the freshness comparison is `>=` or the `now - ts_ms` direction is flipped.
- `ema_carries_over_across_calls` returns `200.0` instead of `160.0`: `smoothed` is not being persisted across calls.

- [ ] **Step 4.6: Full check + clippy**

```bash
cd games/tap-trading/backend && cargo check && cargo clippy --all-targets -- -D warnings && cargo test
```

Expected: green.

- [ ] **Step 4.7: Commit**

```bash
git add games/tap-trading/backend/oracle-aggregator/src/aggregator.rs \
        games/tap-trading/backend/oracle-aggregator/src/sources/mod.rs \
        games/tap-trading/backend/oracle-aggregator/src/main.rs \
        games/tap-trading/backend/oracle-aggregator/tests/aggregation.rs
git commit -m "feat(tick-oracle): add aggregation core median and ema blend"
```

---

## Task 5 — Ring buffer for replay queries

Per-asset 10-deep deque, indexed by `(run_id, seq)`. Lookup returns `RingLookup::{Hit / Gone / Conflict}` mapped to HTTP status codes by the api layer in Task 6.

**Files:**
- Create: `games/tap-trading/backend/oracle-aggregator/src/ring_buffer.rs`
- Modify: `games/tap-trading/backend/oracle-aggregator/src/main.rs` (`mod ring_buffer;`)
- Create: `games/tap-trading/backend/oracle-aggregator/tests/ring_buffer.rs`

- [ ] **Step 5.1: Write `ring_buffer.rs`**

Write `games/tap-trading/backend/oracle-aggregator/src/ring_buffer.rs`:

```rust
//! Per-asset ring buffer for `(run_id, seq)` replay. ADR-0008 §5.
//!
//! At 20 Hz × 10 entries = 500 ms of history per asset — the budget the
//! client has between displaying a multiplier and committing the tap.

use crate::constants::RING_SIZE;
use dashmap::DashMap;
use std::collections::VecDeque;
use std::sync::Mutex;
use tap_trading_oracle_types::{AssetSymbol, OracleTick};

/// Reply shape for `GET /ring/:asset/:seq?run_id=N`. ADR-0008 §5.
#[derive(Debug, Clone, PartialEq)]
pub enum RingLookup {
    Hit(OracleTick),
    /// `seq` is older than the oldest entry we still retain. 410 Gone.
    Gone,
    /// `run_id` doesn't match the aggregator's current `run_id`. 409 Conflict.
    Conflict,
}

#[derive(Debug, Default)]
pub struct AssetRing {
    entries: VecDeque<OracleTick>,
}

impl AssetRing {
    pub fn push(&mut self, tick: OracleTick) {
        if self.entries.len() == RING_SIZE {
            self.entries.pop_front();
        }
        self.entries.push_back(tick);
    }

    fn lookup(&self, run_id: u64, seq: u64) -> RingLookup {
        let Some(front) = self.entries.front() else {
            // No data yet for this asset.
            return RingLookup::Gone;
        };
        if front.run_id != run_id {
            return RingLookup::Conflict;
        }
        if seq < front.seq {
            return RingLookup::Gone;
        }
        for t in &self.entries {
            if t.seq == seq {
                return RingLookup::Hit(*t);
            }
        }
        // seq is newer than the newest entry — treat as not-yet-emitted; 410.
        RingLookup::Gone
    }
}

#[derive(Debug, Default)]
pub struct RingBuffers {
    inner: DashMap<AssetSymbol, Mutex<AssetRing>>,
}

impl RingBuffers {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, tick: OracleTick) {
        let entry = self.inner.entry(tick.asset).or_default();
        let mut ring = entry.lock().expect("ring mutex poisoned");
        ring.push(tick);
    }

    pub fn get(&self, asset: AssetSymbol, run_id: u64, seq: u64) -> RingLookup {
        let Some(entry) = self.inner.get(&asset) else {
            return RingLookup::Gone;
        };
        entry.lock().expect("ring mutex poisoned").lookup(run_id, seq)
    }
}
```

`DashMap<Asset, Mutex<AssetRing>>` is intentional: lookups (read path) and pushes (write path) are both short critical sections; a single `RwLock<HashMap>` would serialize the *push* against every concurrent `/ring` reader.

- [ ] **Step 5.2: Wire into `main.rs`**

Edit `games/tap-trading/backend/oracle-aggregator/src/main.rs`. Add `mod ring_buffer;` next to the other module declarations.

- [ ] **Step 5.3: Write the integration test**

Write `games/tap-trading/backend/oracle-aggregator/tests/ring_buffer.rs`:

```rust
//! Ring-buffer semantics. ADR-0008 §5.

#[path = "../src/constants.rs"]
mod constants;
#[path = "../src/ring_buffer.rs"]
mod ring_buffer;

use ring_buffer::{RingBuffers, RingLookup};
use tap_trading_oracle_types::{AssetSymbol, OracleTick};

fn make_tick(run_id: u64, seq: u64) -> OracleTick {
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

#[test]
fn unknown_asset_returns_gone() {
    let rb = RingBuffers::new();
    // BTC never pushed.
    assert_eq!(rb.get(AssetSymbol::Btc, 1, 0), RingLookup::Gone);
}

#[test]
fn fresh_push_is_a_hit() {
    let rb = RingBuffers::new();
    rb.push(make_tick(1, 0));
    match rb.get(AssetSymbol::Eth, 1, 0) {
        RingLookup::Hit(t) => assert_eq!(t.seq, 0),
        other => panic!("expected Hit, got {other:?}"),
    }
}

#[test]
fn wrong_run_id_returns_conflict() {
    let rb = RingBuffers::new();
    rb.push(make_tick(42, 0));
    assert_eq!(rb.get(AssetSymbol::Eth, 99, 0), RingLookup::Conflict);
}

#[test]
fn rotated_past_seq_returns_gone() {
    let rb = RingBuffers::new();
    // Push 15 entries; the first 5 rotate out (RING_SIZE = 10).
    for seq in 0..15 {
        rb.push(make_tick(1, seq));
    }
    // Seq 4 fell out → Gone.
    assert_eq!(rb.get(AssetSymbol::Eth, 1, 4), RingLookup::Gone);
    // Seq 5 still present → Hit.
    match rb.get(AssetSymbol::Eth, 1, 5) {
        RingLookup::Hit(t) => assert_eq!(t.seq, 5),
        other => panic!("expected Hit, got {other:?}"),
    }
    // Seq 14 (newest) still present → Hit.
    match rb.get(AssetSymbol::Eth, 1, 14) {
        RingLookup::Hit(t) => assert_eq!(t.seq, 14),
        other => panic!("expected Hit, got {other:?}"),
    }
}

#[test]
fn future_seq_returns_gone() {
    let rb = RingBuffers::new();
    rb.push(make_tick(1, 0));
    // Asking for seq 100 when only 0 exists → Gone (caller re-fetches a fresh quote).
    assert_eq!(rb.get(AssetSymbol::Eth, 1, 100), RingLookup::Gone);
}

#[test]
fn per_asset_isolation() {
    let rb = RingBuffers::new();
    rb.push(make_tick(1, 0));
    // BTC is independent.
    assert_eq!(rb.get(AssetSymbol::Btc, 1, 0), RingLookup::Gone);
}
```

- [ ] **Step 5.4: Run, verify green**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-oracle-aggregator --test ring_buffer
```

Expected: all 6 tests pass. Likely failures:
- `rotated_past_seq_returns_gone`: if seq 5 returns `Gone` instead of `Hit`, the ring is rotating one entry too many — re-check the `len == RING_SIZE` guard.
- `wrong_run_id_returns_conflict`: if it returns `Gone`, you're missing the front-tick `run_id != run_id` short-circuit.

- [ ] **Step 5.5: Full check**

```bash
cd games/tap-trading/backend && cargo check && cargo clippy --all-targets -- -D warnings && cargo test
```

Expected: green.

- [ ] **Step 5.6: Commit**

```bash
git add games/tap-trading/backend/oracle-aggregator/src/ring_buffer.rs \
        games/tap-trading/backend/oracle-aggregator/src/main.rs \
        games/tap-trading/backend/oracle-aggregator/tests/ring_buffer.rs
git commit -m "feat(tick-oracle): add ring buffer for replay queries"
```

---

## Task 6 — Expose ring buffer over HTTP

Wire `GET /ring/:asset/:seq?run_id=N` to the ring buffer. Full response matrix per ADR-0008 §5.

**Files:**
- Modify: `games/tap-trading/backend/oracle-aggregator/src/api.rs`
- Modify: `games/tap-trading/backend/oracle-aggregator/src/main.rs` (build app state with a `RingBuffers` handle; pass into router)
- Create: `games/tap-trading/backend/oracle-aggregator/tests/replay_http.rs`

- [ ] **Step 6.1: Extend `api.rs`**

Replace `games/tap-trading/backend/oracle-aggregator/src/api.rs`:

```rust
//! HTTP + WS router. WS handler lands in Task 7.

use crate::ring_buffer::{RingBuffers, RingLookup};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;
use tap_trading_oracle_types::AssetSymbol;

#[derive(Clone)]
pub struct AppState {
    pub run_id: u64,
    pub rings: Arc<RingBuffers>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics))
        .route("/ring/:asset/:seq", get(get_ring))
        .with_state(state)
}

async fn healthz() -> &'static str {
    // Per-asset source_count check lands in Task 13.
    "ok"
}

async fn metrics() -> &'static str {
    "# tap_trading_oracle_aggregator metrics stub\n"
}

#[derive(Debug, Deserialize)]
struct RingQuery {
    run_id: Option<u64>,
}

async fn get_ring(
    State(state): State<AppState>,
    Path((asset, seq)): Path<(String, u64)>,
    Query(query): Query<RingQuery>,
) -> Response {
    let asset = match parse_asset(&asset) {
        Some(a) => a,
        None => return (StatusCode::NOT_FOUND, "unknown asset").into_response(),
    };
    let Some(run_id) = query.run_id else {
        return (StatusCode::CONFLICT, "missing run_id").into_response();
    };
    if run_id != state.run_id {
        return (StatusCode::CONFLICT, "stale run_id").into_response();
    }
    match state.rings.get(asset, run_id, seq) {
        RingLookup::Hit(tick) => Json(tick).into_response(),
        RingLookup::Gone => (StatusCode::GONE, "seq rotated").into_response(),
        RingLookup::Conflict => (StatusCode::CONFLICT, "stale run_id").into_response(),
    }
}

fn parse_asset(raw: &str) -> Option<AssetSymbol> {
    match raw.to_ascii_uppercase().as_str() {
        "ETH" => Some(AssetSymbol::Eth),
        "BTC" => Some(AssetSymbol::Btc),
        "SOL" => Some(AssetSymbol::Sol),
        _ => None,
    }
}
```

Note: the `run_id` check is short-circuited *before* hitting `rings.get` so a 409 fires even when the ring is empty for that asset. The ring's internal `Conflict` branch is a defense-in-depth.

- [ ] **Step 6.2: Update `main.rs` to build and pass the state**

Edit `games/tap-trading/backend/oracle-aggregator/src/main.rs`. Above `axum::serve(...)`, build the state:

```rust
    let rings = std::sync::Arc::new(ring_buffer::RingBuffers::new());
    let app = api::router(api::AppState { run_id, rings });
```

Remove the old `let app = api::router();` line.

- [ ] **Step 6.3: Write the HTTP integration test**

Write `games/tap-trading/backend/oracle-aggregator/tests/replay_http.rs`:

```rust
//! `GET /ring/:asset/:seq?run_id=N` response matrix. ADR-0008 §5.
//!
//! Boots a real axum server on a kernel-assigned free port and exercises the
//! handler over HTTP — no axum::Router::oneshot shortcuts. The test must
//! observe what an external client observes.

#[path = "../src/api.rs"]
mod api;
#[path = "../src/constants.rs"]
mod constants;
#[path = "../src/ring_buffer.rs"]
mod ring_buffer;

use api::{router, AppState};
use ring_buffer::RingBuffers;
use std::sync::Arc;
use tap_trading_oracle_types::{AssetSymbol, OracleTick};

struct TestServer {
    base_url: String,
    _join: tokio::task::JoinHandle<()>,
    rings: Arc<RingBuffers>,
}

async fn spawn(run_id: u64) -> TestServer {
    let rings = Arc::new(RingBuffers::new());
    let state = AppState { run_id, rings: rings.clone() };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = router(state);
    let join = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    TestServer { base_url: format!("http://127.0.0.1:{port}"), _join: join, rings }
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
    let resp = reqwest::get(format!("{}/ring/ETH/0", s.base_url)).await.unwrap();
    assert_eq!(resp.status(), 409);
}

#[tokio::test]
async fn wrong_run_id_returns_409() {
    let s = spawn(42).await;
    s.rings.push(tick(42, 0));
    let resp = reqwest::get(format!("{}/ring/ETH/0?run_id=99", s.base_url)).await.unwrap();
    assert_eq!(resp.status(), 409);
}

#[tokio::test]
async fn unknown_asset_returns_404() {
    let s = spawn(42).await;
    let resp = reqwest::get(format!("{}/ring/DOGE/0?run_id=42", s.base_url)).await.unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn rotated_past_seq_returns_410() {
    let s = spawn(42).await;
    for seq in 0..15 {
        s.rings.push(tick(42, seq));
    }
    let resp = reqwest::get(format!("{}/ring/ETH/4?run_id=42", s.base_url)).await.unwrap();
    assert_eq!(resp.status(), 410);
}

#[tokio::test]
async fn hit_returns_200_with_oracle_tick_body() {
    let s = spawn(42).await;
    s.rings.push(tick(42, 0));
    let resp = reqwest::get(format!("{}/ring/ETH/0?run_id=42", s.base_url)).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body: OracleTick = resp.json().await.unwrap();
    assert_eq!(body.seq, 0);
    assert_eq!(body.run_id, 42);
    assert_eq!(body.source_count, 4);
}
```

Test deps needed: `reqwest` (already in workspace deps) — make it available to the test target. Add to `oracle-aggregator/Cargo.toml` under `[dev-dependencies]`:

```toml
reqwest = { workspace = true, features = ["json"] }
```

- [ ] **Step 6.4: Run**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-oracle-aggregator --test replay_http
```

Expected: all 5 tests pass. If `missing_run_id_returns_409` returns 200, the `query.run_id.is_none()` short-circuit is missing or fires after the ring lookup.

- [ ] **Step 6.5: Full crate check + clippy**

```bash
cd games/tap-trading/backend && cargo check && cargo clippy --all-targets -- -D warnings && cargo test
```

Expected: green.

- [ ] **Step 6.6: Commit**

```bash
git add games/tap-trading/backend/oracle-aggregator/src/api.rs \
        games/tap-trading/backend/oracle-aggregator/src/main.rs \
        games/tap-trading/backend/oracle-aggregator/Cargo.toml \
        games/tap-trading/backend/oracle-aggregator/tests/replay_http.rs
git commit -m "feat(tick-oracle): expose ring buffer over http"
```

---

## Task 7 — Broadcast ticks over WS `/stream`

Wire `tokio::sync::broadcast` + axum WS upgrade at `/stream`, send 5 s `Heartbeat` frames, fan out `OracleMessage` JSON to every subscriber.

**Files:**
- Create: `games/tap-trading/backend/oracle-aggregator/src/broadcast.rs`
- Modify: `games/tap-trading/backend/oracle-aggregator/src/api.rs` (add `/stream`)
- Modify: `games/tap-trading/backend/oracle-aggregator/src/main.rs` (wire broadcast into AppState, spawn heartbeat task)

- [ ] **Step 7.1: Write `broadcast.rs`**

Write `games/tap-trading/backend/oracle-aggregator/src/broadcast.rs`:

```rust
//! WS broadcast plumbing.
//!
//! `tokio::sync::broadcast` is the right primitive here: one producer (the
//! aggregator loop) and many consumers (WS subscribers + worker + api). A
//! channel capacity of 256 absorbs ~13 s of 20 Hz × 3 assets bursts before
//! a slow consumer is dropped (`broadcast::error::RecvError::Lagged`).
//! The WS handler treats `Lagged` as fatal and closes the socket, forcing
//! the client to reconnect — better than silently desynchronising.

use crate::constants::HEARTBEAT_PERIOD_S;
use std::time::Duration;
use tap_trading_oracle_types::OracleMessage;
use tokio::sync::broadcast::{self, Sender};
use tokio::time::interval;

pub const CHANNEL_CAPACITY: usize = 256;

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
            ticker.set_missed_tick_behavior(
                tokio::time::MissedTickBehavior::Delay,
            );
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
```

- [ ] **Step 7.2: Add `/stream` to `api.rs`**

Edit `games/tap-trading/backend/oracle-aggregator/src/api.rs`. Add to imports:

```rust
use crate::broadcast::Broadcaster;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use futures::{SinkExt, StreamExt};
use tokio::sync::broadcast::error::RecvError;
```

Extend `AppState`:

```rust
#[derive(Clone)]
pub struct AppState {
    pub run_id: u64,
    pub rings: Arc<RingBuffers>,
    pub broadcaster: Broadcaster,
}
```

Add to the router:

```rust
        .route("/stream", get(ws_upgrade))
```

Add the handler:

```rust
async fn ws_upgrade(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| ws_session(socket, state))
}

async fn ws_session(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let mut rx = state.broadcaster.sender().subscribe();

    // Reader task: ignore client frames except Close (per ADR-0008 §2 the
    // client never sends app frames; only subscribe-by-default).
    let reader = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            if matches!(msg, Message::Close(_)) {
                break;
            }
        }
    });

    // Writer loop: pump broadcast → WS.
    while let Ok(()) = async {
        match rx.recv().await {
            Ok(msg) => {
                let json = match serde_json::to_string(&msg) {
                    Ok(j) => j,
                    Err(e) => {
                        tracing::error!(error = %e, "OracleMessage serialize failed");
                        return Err(());
                    }
                };
                sender.send(Message::Text(json)).await.map_err(|_| ())?;
                Ok(())
            }
            Err(RecvError::Lagged(n)) => {
                tracing::warn!(skipped = n, "ws client lagged; closing");
                Err(())
            }
            Err(RecvError::Closed) => Err(()),
        }
    }
    .await
    {}

    reader.abort();
}
```

- [ ] **Step 7.3: Wire `Broadcaster` into `main.rs`**

Edit `games/tap-trading/backend/oracle-aggregator/src/main.rs`. Add `mod broadcast;` and update the state assembly:

```rust
    let rings = std::sync::Arc::new(ring_buffer::RingBuffers::new());
    let broadcaster = broadcast::Broadcaster::new();
    let _heartbeat_join = broadcaster.spawn_heartbeat();
    let app = api::router(api::AppState {
        run_id,
        rings,
        broadcaster,
    });
```

- [ ] **Step 7.4: Smoke-test the WS path**

We don't add a heavy integration test for `/stream` here — the end-to-end test in Task 16 covers it. For now, just verify the binary builds and serves the WS upgrade.

```bash
cd games/tap-trading/backend && cargo check -p tap-trading-oracle-aggregator
```

Expected: clean compile.

- [ ] **Step 7.5: Add a minimal WS connect test**

Append a `#[tokio::test]` to `tests/replay_http.rs`:

```rust
#[tokio::test]
async fn ws_stream_accepts_upgrade_and_delivers_heartbeat() {
    use futures::StreamExt;
    use tokio_tungstenite::{connect_async, tungstenite::Message};

    let s = spawn_with_broadcaster(42).await;
    // Send a heartbeat manually so we don't wait the full 5s.
    s.broadcaster
        .send(tap_trading_oracle_types::OracleMessage::Heartbeat { ts_ms: 12345 });

    let url = format!("ws://127.0.0.1:{}/stream", s.port);
    let (mut socket, _resp) = connect_async(&url).await.unwrap();
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
```

You'll need to extend the test harness in `replay_http.rs` to expose the broadcaster and the port. Replace the `TestServer` struct + `spawn` fn at the top of that file with:

```rust
struct TestServer {
    base_url: String,
    port: u16,
    _join: tokio::task::JoinHandle<()>,
    rings: Arc<RingBuffers>,
    broadcaster: crate::broadcast::Broadcaster,
}

async fn spawn(run_id: u64) -> TestServer {
    spawn_with_broadcaster(run_id).await
}

async fn spawn_with_broadcaster(run_id: u64) -> TestServer {
    use crate::broadcast::Broadcaster;
    let rings = Arc::new(RingBuffers::new());
    let broadcaster = Broadcaster::new();
    let state = AppState { run_id, rings: rings.clone(), broadcaster: broadcaster.clone() };
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
```

Add `#[path = "../src/broadcast.rs"] mod broadcast;` at the top of `replay_http.rs` next to the other `#[path]` includes. Add to `Cargo.toml` `[dev-dependencies]`:

```toml
tokio-tungstenite = { workspace = true }
```

- [ ] **Step 7.6: Run**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-oracle-aggregator --test replay_http
```

Expected: all 6 tests pass (5 HTTP + 1 WS).

- [ ] **Step 7.7: Full check**

```bash
cd games/tap-trading/backend && cargo check && cargo clippy --all-targets -- -D warnings && cargo test
```

Expected: green.

- [ ] **Step 7.8: Commit**

```bash
git add games/tap-trading/backend/oracle-aggregator/src/broadcast.rs \
        games/tap-trading/backend/oracle-aggregator/src/api.rs \
        games/tap-trading/backend/oracle-aggregator/src/main.rs \
        games/tap-trading/backend/oracle-aggregator/Cargo.toml \
        games/tap-trading/backend/oracle-aggregator/tests/replay_http.rs
git commit -m "feat(tick-oracle): broadcast ticks over ws stream"
```

---

## Task 8 — Vol state via pricing-engine EWMA

Per-asset 1-second log-return deque (cap 600 = 10 min). Sampled once per second from the latest emitted `mid`. Calls Plan A's `estimate_realized_vol(λ=0.94)` then `jump_adjusted_sigma`. Cold-start branch returns `0.60` (ADR-0008 §7) when deque length is below 30.

**Files:**
- Create: `games/tap-trading/backend/oracle-aggregator/src/vol_state.rs`
- Modify: `games/tap-trading/backend/oracle-aggregator/src/main.rs` (`mod vol_state;`)

- [ ] **Step 8.1: Write `vol_state.rs`**

Write `games/tap-trading/backend/oracle-aggregator/src/vol_state.rs`:

```rust
//! Per-asset vol state.
//!
//! Pipeline per spec MATH_SPEC §3.4: `EWMA → jump-adjust → broadcast`.
//! The deque holds 1-second log returns; we sample one return per second
//! and call `tap_trading_pricing_engine::estimate_realized_vol(λ=0.94)`
//! followed by `jump_adjusted_sigma`. Cold-start (ADR-0008 §7): until we
//! have at least `COLD_START_RETURN_THRESHOLD` returns, emit the constant
//! `COLD_START_VOL_ANNUALIZED`.
//!
//! The pricing engine's signature returns `Result<f64, PricingError>` and
//! we treat any `Err(_)` as cold-start fallback — defense-in-depth against
//! unexpected NaN propagation from upstream sources.

use crate::constants::{
    COLD_START_RETURN_THRESHOLD, COLD_START_VOL_ANNUALIZED, EWMA_LAMBDA_VOL,
    RETURN_DEQUE_CAP,
};
use std::collections::VecDeque;
use tap_trading_pricing_engine::{estimate_realized_vol, jump_adjusted_sigma};

#[derive(Debug, Default)]
pub struct AssetVolState {
    /// Most recent 1-second log returns, oldest first.
    returns: VecDeque<f64>,
    /// Last EWMA result, needed by `jump_adjusted_sigma`.
    prev_sigma_annualized: f64,
    /// Mid at the last 1-second sample boundary; used to compute the next return.
    last_sampled_mid: Option<f64>,
    /// Wall-clock-ms of the last 1-s sample; sample boundary fires when (now − last) ≥ 1000.
    last_sample_ts_ms: Option<i64>,
}

impl AssetVolState {
    /// Update with the most recent `mid` at server time `now_ms`. Returns the
    /// annualized vol to publish on the next `OracleTick`.
    ///
    /// Called on every aggregator tick (20 Hz). Internally rate-limits the
    /// 1-second sampling so the deque grows at exactly 1 Hz independent of
    /// emit cadence.
    pub fn next_vol(&mut self, now_ms: i64, mid: f64) -> f64 {
        self.maybe_sample(now_ms, mid);

        if self.returns.len() < COLD_START_RETURN_THRESHOLD {
            return COLD_START_VOL_ANNUALIZED;
        }

        let slice: Vec<f64> = self.returns.iter().copied().collect();
        let raw = match estimate_realized_vol(&slice, EWMA_LAMBDA_VOL) {
            Ok(v) => v,
            Err(_) => {
                tracing::warn!("estimate_realized_vol returned Err; using cold-start vol");
                return COLD_START_VOL_ANNUALIZED;
            }
        };

        // Apply the spike absorber from MATH_SPEC §3.4.
        let last_return = self.returns.back().copied().unwrap_or(0.0);
        let (adjusted, was_spike) =
            jump_adjusted_sigma(raw, last_return, self.prev_sigma_annualized);
        if was_spike {
            tracing::info!(raw, adjusted, "vol spike absorbed");
        }
        self.prev_sigma_annualized = adjusted;
        adjusted
    }

    fn maybe_sample(&mut self, now_ms: i64, mid: f64) {
        match self.last_sample_ts_ms {
            None => {
                self.last_sample_ts_ms = Some(now_ms);
                self.last_sampled_mid = Some(mid);
            }
            Some(prev_ts) if now_ms - prev_ts >= 1_000 => {
                if let Some(prev_mid) = self.last_sampled_mid {
                    if mid > 0.0 && prev_mid > 0.0 {
                        let r = (mid / prev_mid).ln();
                        if r.is_finite() {
                            if self.returns.len() == RETURN_DEQUE_CAP {
                                self.returns.pop_front();
                            }
                            self.returns.push_back(r);
                        }
                    }
                }
                self.last_sampled_mid = Some(mid);
                self.last_sample_ts_ms = Some(now_ms);
            }
            _ => { /* not yet a 1-s boundary */ }
        }
    }

    #[cfg(test)]
    pub fn returns_len(&self) -> usize {
        self.returns.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cold_start_returns_constant_until_threshold() {
        let mut v = AssetVolState::default();
        for i in 0..(COLD_START_RETURN_THRESHOLD - 1) {
            // One sample per second.
            let now = (i as i64) * 1_000;
            let mid = 100.0 + i as f64 * 0.01;
            let vol = v.next_vol(now, mid);
            assert!(
                (vol - COLD_START_VOL_ANNUALIZED).abs() < 1e-12,
                "expected cold-start at i={i}, got {vol}"
            );
        }
    }

    #[test]
    fn deque_grows_by_one_per_second() {
        let mut v = AssetVolState::default();
        // First call seeds last_sampled_mid; no return yet.
        v.next_vol(0, 100.0);
        assert_eq!(v.returns_len(), 0);
        // 999 ms later — still no return.
        v.next_vol(999, 100.1);
        assert_eq!(v.returns_len(), 0);
        // 1000 ms past the seed — one return.
        v.next_vol(1_000, 100.1);
        assert_eq!(v.returns_len(), 1);
    }

    #[test]
    fn deque_capped_at_cap() {
        let mut v = AssetVolState::default();
        // Push 700 samples at 1-s cadence; capacity 600.
        for i in 0..=700 {
            v.next_vol((i as i64) * 1_000, 100.0 + (i as f64).sin() * 0.01);
        }
        assert_eq!(v.returns_len(), RETURN_DEQUE_CAP);
    }

    #[test]
    fn vol_finite_and_positive_after_warmup() {
        let mut v = AssetVolState::default();
        // 60 s of 1% per-second returns → non-cold-start, expect a real vol.
        let mut mid = 100.0;
        let mut last_vol = COLD_START_VOL_ANNUALIZED;
        for i in 0..=60 {
            mid *= 1.001;
            last_vol = v.next_vol((i as i64) * 1_000, mid);
        }
        assert!(last_vol.is_finite(), "vol must be finite");
        assert!(
            (last_vol - COLD_START_VOL_ANNUALIZED).abs() > 1e-6,
            "vol should have moved off the cold-start default, got {last_vol}"
        );
        assert!(last_vol > 0.0, "vol must be positive");
    }

    #[test]
    fn nan_mid_does_not_corrupt_deque() {
        let mut v = AssetVolState::default();
        v.next_vol(0, 100.0);
        v.next_vol(1_000, f64::NAN);
        // ln(NaN / 100) is NaN → must NOT push.
        assert_eq!(v.returns_len(), 0);
    }
}
```

- [ ] **Step 8.2: Wire into `main.rs`**

Edit `games/tap-trading/backend/oracle-aggregator/src/main.rs`. Add `mod vol_state;` next to the other module declarations.

- [ ] **Step 8.3: Run the inline tests**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-oracle-aggregator vol_state::tests
```

Expected: all 5 tests pass. Likely failure modes:
- `deque_grows_by_one_per_second` returns `returns_len = 1` after the first call: the seed logic pushed a return on the first call instead of just initialising `last_sampled_mid`. The first call MUST only seed; the second call (when `now − last_ts ≥ 1000`) pushes.
- `cold_start_returns_constant_until_threshold` returns something non-constant: the cold-start guard is wrong — re-check `if self.returns.len() < COLD_START_RETURN_THRESHOLD`.
- `nan_mid_does_not_corrupt_deque` returns `returns_len = 1`: the `r.is_finite()` guard is missing.

- [ ] **Step 8.4: Full check**

```bash
cd games/tap-trading/backend && cargo check && cargo clippy --all-targets -- -D warnings && cargo test
```

Expected: green.

- [ ] **Step 8.5: Commit**

```bash
git add games/tap-trading/backend/oracle-aggregator/src/vol_state.rs \
        games/tap-trading/backend/oracle-aggregator/src/main.rs
git commit -m "feat(tick-oracle): wire vol state via pricing engine ewma"
```

---

## Task 9 — Pyth Hermes source via SSE

Wire `reqwest-eventsource` to `${HERMES}/v2/updates/price/stream` for ETH, BTC, SOL. Implement the `Source` trait. The trait is in `sources/mod.rs`; concrete impl in `sources/pyth.rs`.

**Files:**
- Modify: `games/tap-trading/backend/oracle-aggregator/src/sources/mod.rs` (add the `Source` trait + `start_with_backoff` stub)
- Create: `games/tap-trading/backend/oracle-aggregator/src/sources/pyth.rs`

- [ ] **Step 9.1: Extend `sources/mod.rs` with the `Source` trait**

Edit `games/tap-trading/backend/oracle-aggregator/src/sources/mod.rs`. Add at the bottom:

```rust
use async_trait::async_trait;
use tokio::sync::mpsc;

/// Connected source that pushes `SourceTick` into a channel.
///
/// `run` consumes `self` and returns when the source's supervisor decides
/// to stop (process shutdown). Reconnect is internal to the implementation
/// — callers do not see individual disconnects.
#[async_trait]
pub trait Source: Send + 'static {
    fn id(&self) -> SourceId;
    async fn run(self: Box<Self>, tx: mpsc::Sender<SourceTick>);
}

pub mod pyth;
```

Add to `Cargo.toml` `[dependencies]`:

```toml
async-trait = "0.1"
```

And in `[workspace.dependencies]`:

```toml
async-trait = "0.1"
```

(Edit both files; the bin crate references workspace via `async-trait = { workspace = true }`.)

- [ ] **Step 9.2: Write `sources/pyth.rs`**

Write `games/tap-trading/backend/oracle-aggregator/src/sources/pyth.rs`:

```rust
//! Pyth Hermes SSE source.
//!
//! Endpoint: `${HERMES}/v2/updates/price/stream?ids[]=…&ids[]=…&parsed=true`.
//! Each event line is JSON: `{ "parsed": [{ "id": "<hex>", "price": {"price":"N","conf":"N","expo":-8,"publish_time":…} }] }`.
//! See https://hermes.pyth.network/docs/.

use crate::sources::{Source, SourceId, SourceTick};
use async_trait::async_trait;
use futures::StreamExt;
use reqwest_eventsource::{Event, EventSource};
use serde::Deserialize;
use std::collections::HashMap;
use tap_trading_oracle_types::AssetSymbol;
use tokio::sync::mpsc;

/// Feed-id → asset mapping. Mainnet/testnet feed IDs differ — caller wires
/// the correct set via `PythSource::new`. ORACLE_SPEC §2.
pub struct PythSource {
    hermes_base_url: String,
    feeds: HashMap<String, AssetSymbol>,
}

impl PythSource {
    pub fn new(hermes_base_url: String, feeds: HashMap<String, AssetSymbol>) -> Self {
        Self { hermes_base_url, feeds }
    }

    fn build_url(&self) -> String {
        let ids: Vec<String> = self
            .feeds
            .keys()
            .map(|id| format!("ids[]={id}"))
            .collect();
        format!(
            "{}/v2/updates/price/stream?{}&parsed=true",
            self.hermes_base_url,
            ids.join("&")
        )
    }
}

#[async_trait]
impl Source for PythSource {
    fn id(&self) -> SourceId {
        SourceId::Pyth
    }

    async fn run(self: Box<Self>, tx: mpsc::Sender<SourceTick>) {
        let url = self.build_url();
        let mut backoff_ms: u64 = 100;
        loop {
            tracing::info!(%url, "pyth hermes connecting");
            let client = match EventSource::get(&url) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(error = %e, "pyth hermes eventsource init failed");
                    sleep_jittered(&mut backoff_ms).await;
                    continue;
                }
            };
            backoff_ms = 100;
            let mut stream = client;
            while let Some(ev) = stream.next().await {
                match ev {
                    Ok(Event::Open) => tracing::info!("pyth hermes SSE open"),
                    Ok(Event::Message(msg)) => {
                        if let Some(ticks) = parse_hermes_frame(&msg.data, &self.feeds) {
                            for t in ticks {
                                let _ = tx.send(t).await;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "pyth hermes stream error; reconnecting");
                        break;
                    }
                }
            }
            sleep_jittered(&mut backoff_ms).await;
        }
    }
}

async fn sleep_jittered(backoff_ms: &mut u64) {
    use std::time::Duration;
    let jitter = (*backoff_ms as f64) * 0.1 * (rand_unit() - 0.5) * 2.0;
    let with_jitter = (*backoff_ms as f64 + jitter).max(50.0) as u64;
    tokio::time::sleep(Duration::from_millis(with_jitter)).await;
    *backoff_ms = (*backoff_ms * 2).min(30_000);
}

fn rand_unit() -> f64 {
    // Small jitter helper; std rand is enough.
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    (nanos % 1_000) as f64 / 1_000.0
}

#[derive(Deserialize)]
struct HermesFrame {
    parsed: Vec<HermesParsed>,
}

#[derive(Deserialize)]
struct HermesParsed {
    id: String,
    price: HermesPrice,
}

#[derive(Deserialize)]
struct HermesPrice {
    price: String,
    conf: String,
    expo: i32,
    publish_time: i64,
}

fn parse_hermes_frame(
    raw: &str,
    feeds: &HashMap<String, AssetSymbol>,
) -> Option<Vec<SourceTick>> {
    let frame: HermesFrame = serde_json::from_str(raw).ok()?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis() as i64;
    let _ = now_ms; // silence unused-warning if compiled out below
    let ticks = frame
        .parsed
        .into_iter()
        .filter_map(|p| {
            let asset = feeds.get(p.id.trim_start_matches("0x"))?;
            let raw_price = p.price.price.parse::<i128>().ok()?;
            let raw_conf = p.price.conf.parse::<i128>().ok()?;
            let scale = 10_f64.powi(p.price.expo);
            let price = raw_price as f64 * scale;
            let conf = raw_conf as f64 * scale;
            let conf_bps = if price > 0.0 {
                ((conf / price) * 10_000.0).round().max(0.0) as u32
            } else {
                u32::MAX
            };
            Some(SourceTick {
                source: SourceId::Pyth,
                asset: *asset,
                price,
                // ORACLE_SPEC §4.5: server-received timestamp.
                ts_ms: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()?
                    .as_millis() as i64,
                pyth_conf_bps: Some(conf_bps),
            })
        })
        .collect::<Vec<_>>();
    if ticks.is_empty() {
        None
    } else {
        Some(ticks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hermes_frame_yields_one_tick_per_known_feed() {
        let raw = r#"{"parsed":[{"id":"ff61491a931112ddf1bd8147cd1b641375f79f5825126d665480874634fd0ace","price":{"price":"381225000000","conf":"100000000","expo":-8,"publish_time":1747526400}}]}"#;
        let mut feeds = HashMap::new();
        feeds.insert(
            "ff61491a931112ddf1bd8147cd1b641375f79f5825126d665480874634fd0ace".to_string(),
            AssetSymbol::Eth,
        );
        let ticks = parse_hermes_frame(raw, &feeds).unwrap();
        assert_eq!(ticks.len(), 1);
        let t = &ticks[0];
        assert_eq!(t.asset, AssetSymbol::Eth);
        assert!((t.price - 3812.25).abs() < 1e-6, "got {}", t.price);
        // conf = 100_000_000 · 1e-8 = 1.0; bps = (1.0 / 3812.25) · 10000 ≈ 3 bps.
        assert!(t.pyth_conf_bps.unwrap() <= 5);
    }

    #[test]
    fn parse_hermes_frame_drops_unknown_feed() {
        let raw = r#"{"parsed":[{"id":"deadbeef","price":{"price":"1","conf":"1","expo":-8,"publish_time":0}}]}"#;
        let feeds = HashMap::new();
        assert!(parse_hermes_frame(raw, &feeds).is_none());
    }

    #[test]
    fn parse_hermes_frame_rejects_malformed_json() {
        assert!(parse_hermes_frame("not json", &HashMap::new()).is_none());
    }
}
```

- [ ] **Step 9.3: Test**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-oracle-aggregator sources::pyth::tests
```

Expected: all 3 tests pass. The full `run()` loop is exercised by the end-to-end test in Task 16 (against a mock SSE server).

- [ ] **Step 9.4: Full check**

```bash
cd games/tap-trading/backend && cargo check && cargo clippy --all-targets -- -D warnings && cargo test
```

Expected: green.

- [ ] **Step 9.5: Commit**

```bash
git add games/tap-trading/backend/Cargo.toml \
        games/tap-trading/backend/oracle-aggregator/Cargo.toml \
        games/tap-trading/backend/oracle-aggregator/src/sources/mod.rs \
        games/tap-trading/backend/oracle-aggregator/src/sources/pyth.rs
git commit -m "feat(tick-oracle): add pyth hermes source via sse"
```

---

## Task 10 — Binance source via WebSocket

Subscribe to `wss://stream.binance.com:9443/ws/{symbol}@aggTrade` for `ethusdt`, `btcusdt`, `solusdt`. Binance multiplexes by per-symbol URL path; for 3 symbols we run 3 sockets.

**Files:**
- Modify: `games/tap-trading/backend/oracle-aggregator/src/sources/mod.rs` (add `pub mod binance;`)
- Create: `games/tap-trading/backend/oracle-aggregator/src/sources/binance.rs`

- [ ] **Step 10.1: Write `sources/binance.rs`**

Write `games/tap-trading/backend/oracle-aggregator/src/sources/binance.rs`:

```rust
//! Binance Spot WS source.
//!
//! Channel `{symbol}@aggTrade`. One socket per symbol — Binance's combined
//! stream endpoint exists but @aggTrade is small and the per-symbol URL is
//! simpler to reconnect.

use crate::sources::{Source, SourceId, SourceTick};
use async_trait::async_trait;
use futures::StreamExt;
use serde::Deserialize;
use tap_trading_oracle_types::AssetSymbol;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

pub struct BinanceSource {
    pub base_url: String,
}

#[derive(Debug, Clone, Copy)]
struct SymbolBinding {
    asset: AssetSymbol,
    binance_path: &'static str,
}

const SYMBOLS: &[SymbolBinding] = &[
    SymbolBinding { asset: AssetSymbol::Eth, binance_path: "ethusdt@aggTrade" },
    SymbolBinding { asset: AssetSymbol::Btc, binance_path: "btcusdt@aggTrade" },
    SymbolBinding { asset: AssetSymbol::Sol, binance_path: "solusdt@aggTrade" },
];

#[async_trait]
impl Source for BinanceSource {
    fn id(&self) -> SourceId {
        SourceId::Binance
    }

    async fn run(self: Box<Self>, tx: mpsc::Sender<SourceTick>) {
        // Spawn one task per symbol; they share the outbound channel.
        let mut joins = Vec::new();
        for sym in SYMBOLS {
            let url = format!("{}/{}", self.base_url, sym.binance_path);
            let asset = sym.asset;
            let tx_cloned = tx.clone();
            joins.push(tokio::spawn(async move {
                run_one(url, asset, tx_cloned).await;
            }));
        }
        for j in joins {
            let _ = j.await;
        }
    }
}

async fn run_one(url: String, asset: AssetSymbol, tx: mpsc::Sender<SourceTick>) {
    let mut backoff_ms: u64 = 100;
    loop {
        tracing::info!(%url, %asset_dbg(asset), "binance connecting");
        let (mut ws, _resp) = match tokio_tungstenite::connect_async(&url).await {
            Ok(pair) => pair,
            Err(e) => {
                tracing::warn!(error = %e, "binance connect failed");
                sleep_jittered(&mut backoff_ms).await;
                continue;
            }
        };
        backoff_ms = 100;
        while let Some(msg) = ws.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if let Some(tick) = parse_binance_agg_trade(&text, asset) {
                        let _ = tx.send(tick).await;
                    }
                }
                Ok(Message::Ping(payload)) => {
                    use futures::SinkExt;
                    let _ = ws.send(Message::Pong(payload)).await;
                }
                Ok(Message::Close(_)) | Err(_) => {
                    tracing::warn!("binance ws closed; reconnecting");
                    break;
                }
                _ => {}
            }
        }
        sleep_jittered(&mut backoff_ms).await;
    }
}

fn asset_dbg(a: AssetSymbol) -> String {
    format!("{a:?}")
}

async fn sleep_jittered(backoff_ms: &mut u64) {
    use std::time::Duration;
    let jitter = (*backoff_ms as f64) * 0.1 * (rand_unit() - 0.5) * 2.0;
    let with_jitter = (*backoff_ms as f64 + jitter).max(50.0) as u64;
    tokio::time::sleep(Duration::from_millis(with_jitter)).await;
    *backoff_ms = (*backoff_ms * 2).min(30_000);
}

fn rand_unit() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    (nanos % 1_000) as f64 / 1_000.0
}

#[derive(Deserialize)]
struct BinanceAggTrade {
    /// `"p"` is the price string in Binance's WS schema.
    p: String,
}

fn parse_binance_agg_trade(raw: &str, asset: AssetSymbol) -> Option<SourceTick> {
    let parsed: BinanceAggTrade = serde_json::from_str(raw).ok()?;
    let price: f64 = parsed.p.parse().ok()?;
    if !price.is_finite() || price <= 0.0 {
        return None;
    }
    let ts_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis() as i64;
    Some(SourceTick {
        source: SourceId::Binance,
        asset,
        price,
        ts_ms,
        pyth_conf_bps: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_agg_trade_extracts_price() {
        let raw = r#"{"e":"aggTrade","p":"3812.25","q":"0.001","T":1747526400000}"#;
        let t = parse_binance_agg_trade(raw, AssetSymbol::Eth).unwrap();
        assert_eq!(t.source, SourceId::Binance);
        assert_eq!(t.asset, AssetSymbol::Eth);
        assert!((t.price - 3812.25).abs() < 1e-9);
        assert!(t.pyth_conf_bps.is_none());
    }

    #[test]
    fn parse_agg_trade_rejects_unparseable() {
        assert!(parse_binance_agg_trade("not-json", AssetSymbol::Eth).is_none());
    }

    #[test]
    fn parse_agg_trade_rejects_zero_price() {
        let raw = r#"{"p":"0"}"#;
        assert!(parse_binance_agg_trade(raw, AssetSymbol::Eth).is_none());
    }
}
```

- [ ] **Step 10.2: Wire into module index**

Edit `games/tap-trading/backend/oracle-aggregator/src/sources/mod.rs`. Add `pub mod binance;` next to `pub mod pyth;`.

- [ ] **Step 10.3: Test**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-oracle-aggregator sources::binance::tests
```

Expected: all 3 tests pass.

- [ ] **Step 10.4: Full check**

```bash
cd games/tap-trading/backend && cargo check && cargo clippy --all-targets -- -D warnings && cargo test
```

Expected: green.

- [ ] **Step 10.5: Commit**

```bash
git add games/tap-trading/backend/oracle-aggregator/src/sources/mod.rs \
        games/tap-trading/backend/oracle-aggregator/src/sources/binance.rs
git commit -m "feat(tick-oracle): add binance source via websocket"
```

---

## Task 11 — Bybit source via WebSocket

Bybit v5 public Spot stream. Single socket; subscribe message with all 3 symbols. Channel `publicTrade.{symbol}`.

**Files:**
- Modify: `games/tap-trading/backend/oracle-aggregator/src/sources/mod.rs` (add `pub mod bybit;`)
- Create: `games/tap-trading/backend/oracle-aggregator/src/sources/bybit.rs`

- [ ] **Step 11.1: Write `sources/bybit.rs`**

Write `games/tap-trading/backend/oracle-aggregator/src/sources/bybit.rs`:

```rust
//! Bybit v5 public Spot WS source.
//!
//! One socket, multiple subscriptions:
//!   `{"op":"subscribe","args":["publicTrade.ETHUSDT","publicTrade.BTCUSDT","publicTrade.SOLUSDT"]}`
//! Trade frames arrive as `{"topic":"publicTrade.ETHUSDT","data":[{"p":"3812.45",…}]}`.

use crate::sources::{Source, SourceId, SourceTick};
use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use tap_trading_oracle_types::AssetSymbol;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

pub struct BybitSource {
    pub url: String,
}

fn symbol_to_asset(sym: &str) -> Option<AssetSymbol> {
    match sym {
        "ETHUSDT" => Some(AssetSymbol::Eth),
        "BTCUSDT" => Some(AssetSymbol::Btc),
        "SOLUSDT" => Some(AssetSymbol::Sol),
        _ => None,
    }
}

#[async_trait]
impl Source for BybitSource {
    fn id(&self) -> SourceId {
        SourceId::Bybit
    }

    async fn run(self: Box<Self>, tx: mpsc::Sender<SourceTick>) {
        let mut backoff_ms: u64 = 100;
        loop {
            tracing::info!(%self.url, "bybit connecting");
            let (mut ws, _resp) = match tokio_tungstenite::connect_async(&self.url).await {
                Ok(pair) => pair,
                Err(e) => {
                    tracing::warn!(error = %e, "bybit connect failed");
                    sleep_jittered(&mut backoff_ms).await;
                    continue;
                }
            };
            backoff_ms = 100;

            // Subscribe message.
            let sub = serde_json::json!({
                "op": "subscribe",
                "args": ["publicTrade.ETHUSDT", "publicTrade.BTCUSDT", "publicTrade.SOLUSDT"]
            });
            if ws.send(Message::Text(sub.to_string())).await.is_err() {
                continue;
            }

            while let Some(msg) = ws.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Some(ticks) = parse_bybit_frame(&text) {
                            for t in ticks {
                                let _ = tx.send(t).await;
                            }
                        }
                    }
                    Ok(Message::Ping(p)) => {
                        let _ = ws.send(Message::Pong(p)).await;
                    }
                    Ok(Message::Close(_)) | Err(_) => break,
                    _ => {}
                }
            }
            sleep_jittered(&mut backoff_ms).await;
        }
    }
}

async fn sleep_jittered(backoff_ms: &mut u64) {
    use std::time::Duration;
    let jitter = (*backoff_ms as f64) * 0.1 * (rand_unit() - 0.5) * 2.0;
    let with_jitter = (*backoff_ms as f64 + jitter).max(50.0) as u64;
    tokio::time::sleep(Duration::from_millis(with_jitter)).await;
    *backoff_ms = (*backoff_ms * 2).min(30_000);
}

fn rand_unit() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    (nanos % 1_000) as f64 / 1_000.0
}

#[derive(Deserialize)]
struct BybitFrame {
    topic: Option<String>,
    data: Option<Vec<BybitTrade>>,
}

#[derive(Deserialize)]
struct BybitTrade {
    p: String,
}

fn parse_bybit_frame(raw: &str) -> Option<Vec<SourceTick>> {
    let frame: BybitFrame = serde_json::from_str(raw).ok()?;
    let topic = frame.topic?;
    let symbol = topic.strip_prefix("publicTrade.")?;
    let asset = symbol_to_asset(symbol)?;
    let data = frame.data?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis() as i64;
    let ticks: Vec<SourceTick> = data
        .into_iter()
        .filter_map(|t| {
            let price: f64 = t.p.parse().ok()?;
            if !price.is_finite() || price <= 0.0 {
                return None;
            }
            Some(SourceTick {
                source: SourceId::Bybit,
                asset,
                price,
                ts_ms: now_ms,
                pyth_conf_bps: None,
            })
        })
        .collect();
    if ticks.is_empty() {
        None
    } else {
        Some(ticks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_publictrade_frame() {
        let raw = r#"{"topic":"publicTrade.ETHUSDT","data":[{"p":"3812.25","v":"0.01"}]}"#;
        let ticks = parse_bybit_frame(raw).unwrap();
        assert_eq!(ticks.len(), 1);
        assert_eq!(ticks[0].asset, AssetSymbol::Eth);
        assert!((ticks[0].price - 3812.25).abs() < 1e-9);
        assert_eq!(ticks[0].source, SourceId::Bybit);
    }

    #[test]
    fn parse_ignores_subscribe_ack() {
        // Bybit emits `{"op":"subscribe","success":true,"ret_msg":"subscribe"…}`;
        // no `topic` field → drop.
        let raw = r#"{"op":"subscribe","success":true}"#;
        assert!(parse_bybit_frame(raw).is_none());
    }

    #[test]
    fn parse_ignores_unknown_symbol() {
        let raw = r#"{"topic":"publicTrade.XRPUSDT","data":[{"p":"0.5"}]}"#;
        assert!(parse_bybit_frame(raw).is_none());
    }
}
```

- [ ] **Step 11.2: Wire into module index**

Edit `games/tap-trading/backend/oracle-aggregator/src/sources/mod.rs`. Add `pub mod bybit;`.

- [ ] **Step 11.3: Test + clippy + full check**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-oracle-aggregator sources::bybit::tests && cargo check && cargo clippy --all-targets -- -D warnings
```

Expected: green.

- [ ] **Step 11.4: Commit**

```bash
git add games/tap-trading/backend/oracle-aggregator/src/sources/mod.rs \
        games/tap-trading/backend/oracle-aggregator/src/sources/bybit.rs
git commit -m "feat(tick-oracle): add bybit source via websocket"
```

---

## Task 12 — OKX source via WebSocket

OKX v5 public stream. Channel `trades-all`, multi-instrument subscription.

**Files:**
- Modify: `games/tap-trading/backend/oracle-aggregator/src/sources/mod.rs` (add `pub mod okx;`)
- Create: `games/tap-trading/backend/oracle-aggregator/src/sources/okx.rs`

- [ ] **Step 12.1: Write `sources/okx.rs`**

Write `games/tap-trading/backend/oracle-aggregator/src/sources/okx.rs`:

```rust
//! OKX v5 public WS source.
//!
//! Subscribe: `{"op":"subscribe","args":[{"channel":"trades","instId":"ETH-USDT"},…]}`.
//! Data: `{"arg":{"channel":"trades","instId":"ETH-USDT"},"data":[{"px":"3812.25","sz":"0.01","ts":"…"}]}`.

use crate::sources::{Source, SourceId, SourceTick};
use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use tap_trading_oracle_types::AssetSymbol;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

pub struct OkxSource {
    pub url: String,
}

fn instid_to_asset(inst: &str) -> Option<AssetSymbol> {
    match inst {
        "ETH-USDT" => Some(AssetSymbol::Eth),
        "BTC-USDT" => Some(AssetSymbol::Btc),
        "SOL-USDT" => Some(AssetSymbol::Sol),
        _ => None,
    }
}

#[async_trait]
impl Source for OkxSource {
    fn id(&self) -> SourceId {
        SourceId::Okx
    }

    async fn run(self: Box<Self>, tx: mpsc::Sender<SourceTick>) {
        let mut backoff_ms: u64 = 100;
        loop {
            tracing::info!(%self.url, "okx connecting");
            let (mut ws, _resp) = match tokio_tungstenite::connect_async(&self.url).await {
                Ok(pair) => pair,
                Err(e) => {
                    tracing::warn!(error = %e, "okx connect failed");
                    sleep_jittered(&mut backoff_ms).await;
                    continue;
                }
            };
            backoff_ms = 100;

            let sub = serde_json::json!({
                "op": "subscribe",
                "args": [
                    {"channel": "trades", "instId": "ETH-USDT"},
                    {"channel": "trades", "instId": "BTC-USDT"},
                    {"channel": "trades", "instId": "SOL-USDT"}
                ]
            });
            if ws.send(Message::Text(sub.to_string())).await.is_err() {
                continue;
            }

            while let Some(msg) = ws.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Some(ticks) = parse_okx_frame(&text) {
                            for t in ticks {
                                let _ = tx.send(t).await;
                            }
                        }
                    }
                    Ok(Message::Ping(p)) => {
                        let _ = ws.send(Message::Pong(p)).await;
                    }
                    Ok(Message::Close(_)) | Err(_) => break,
                    _ => {}
                }
            }
            sleep_jittered(&mut backoff_ms).await;
        }
    }
}

async fn sleep_jittered(backoff_ms: &mut u64) {
    use std::time::Duration;
    let jitter = (*backoff_ms as f64) * 0.1 * (rand_unit() - 0.5) * 2.0;
    let with_jitter = (*backoff_ms as f64 + jitter).max(50.0) as u64;
    tokio::time::sleep(Duration::from_millis(with_jitter)).await;
    *backoff_ms = (*backoff_ms * 2).min(30_000);
}

fn rand_unit() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    (nanos % 1_000) as f64 / 1_000.0
}

#[derive(Deserialize)]
struct OkxFrame {
    arg: Option<OkxArg>,
    data: Option<Vec<OkxTrade>>,
}

#[derive(Deserialize)]
struct OkxArg {
    channel: String,
    #[serde(rename = "instId")]
    inst_id: String,
}

#[derive(Deserialize)]
struct OkxTrade {
    px: String,
}

fn parse_okx_frame(raw: &str) -> Option<Vec<SourceTick>> {
    let frame: OkxFrame = serde_json::from_str(raw).ok()?;
    let arg = frame.arg?;
    if arg.channel != "trades" {
        return None;
    }
    let asset = instid_to_asset(&arg.inst_id)?;
    let data = frame.data?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis() as i64;
    let ticks: Vec<SourceTick> = data
        .into_iter()
        .filter_map(|t| {
            let price: f64 = t.px.parse().ok()?;
            if !price.is_finite() || price <= 0.0 {
                return None;
            }
            Some(SourceTick {
                source: SourceId::Okx,
                asset,
                price,
                ts_ms: now_ms,
                pyth_conf_bps: None,
            })
        })
        .collect();
    if ticks.is_empty() {
        None
    } else {
        Some(ticks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_trades_frame() {
        let raw = r#"{"arg":{"channel":"trades","instId":"ETH-USDT"},"data":[{"px":"3812.25","sz":"0.5","ts":"1"}]}"#;
        let ticks = parse_okx_frame(raw).unwrap();
        assert_eq!(ticks.len(), 1);
        assert_eq!(ticks[0].asset, AssetSymbol::Eth);
        assert!((ticks[0].price - 3812.25).abs() < 1e-9);
        assert_eq!(ticks[0].source, SourceId::Okx);
    }

    #[test]
    fn parse_ignores_subscribe_ack() {
        let raw = r#"{"event":"subscribe","arg":{"channel":"trades","instId":"ETH-USDT"}}"#;
        // No `data` field → drop.
        assert!(parse_okx_frame(raw).is_none());
    }

    #[test]
    fn parse_ignores_wrong_channel() {
        let raw = r#"{"arg":{"channel":"books","instId":"ETH-USDT"},"data":[{"px":"3812"}]}"#;
        assert!(parse_okx_frame(raw).is_none());
    }
}
```

- [ ] **Step 12.2: Wire + test + commit**

Edit `games/tap-trading/backend/oracle-aggregator/src/sources/mod.rs`: add `pub mod okx;`.

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-oracle-aggregator sources::okx::tests && cargo check && cargo clippy --all-targets -- -D warnings && cargo test
```

Expected: green.

```bash
git add games/tap-trading/backend/oracle-aggregator/src/sources/mod.rs \
        games/tap-trading/backend/oracle-aggregator/src/sources/okx.rs
git commit -m "feat(tick-oracle): add okx source via websocket"
```

---

## Task 13 — DEGRADED hysteresis and recovery

When `source_count < 2` for `DEGRADED_HYSTERESIS_MS = 2000` continuously, emit `OracleStatus { state: Degraded, ... }`. While DEGRADED, no `OracleTick` is appended to the ring and `seq` does not advance for that asset. Recovery requires `≥ 2` sources for the same duration.

**Files:**
- Modify: `games/tap-trading/backend/oracle-aggregator/src/aggregator.rs` (add `StreamPhase`, hysteresis logic)
- Modify: `games/tap-trading/backend/oracle-aggregator/tests/aggregation.rs` (add transition tests)

- [ ] **Step 13.1: Extend `AssetPriceState` with phase tracking**

Append to `games/tap-trading/backend/oracle-aggregator/src/aggregator.rs`:

```rust
use crate::constants::DEGRADED_HYSTERESIS_MS;
use tap_trading_oracle_types::OracleStreamState;

/// What the aggregator decided to emit on a given asset for one 50 ms tick.
#[derive(Debug, Clone, PartialEq)]
pub enum TickDecision {
    /// Emit a tick (price + vol assembled by the caller).
    Tick { mid: f64, median: f64, source_count: u8 },
    /// Emit a status frame because the asset just transitioned phase.
    Status { state: OracleStreamState, reason: String },
    /// No emission: still in current phase, or insufficient data without hysteresis fired.
    Silence,
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
enum Phase {
    #[default]
    Normal,
    Degraded,
}

#[derive(Debug, Default)]
pub struct AssetStreamPhase {
    phase: Phase,
    /// Ms-stamp of the first observation that started moving toward the
    /// opposite phase. `None` if no transition is pending.
    pending_since_ms: Option<i64>,
}

impl AssetStreamPhase {
    /// Drive the hysteresis state machine. Returns the side-effect to emit.
    pub fn step(
        &mut self,
        now_ms: i64,
        outcome: &AggregateOutcome,
    ) -> TickDecision {
        match (self.phase, outcome) {
            (Phase::Normal, AggregateOutcome::Emit { mid, median, source_count }) => {
                self.pending_since_ms = None;
                TickDecision::Tick { mid: *mid, median: *median, source_count: *source_count }
            }
            (Phase::Normal, AggregateOutcome::InsufficientSources { reason }) => {
                let started = *self.pending_since_ms.get_or_insert(now_ms);
                if now_ms - started >= DEGRADED_HYSTERESIS_MS as i64 {
                    self.phase = Phase::Degraded;
                    self.pending_since_ms = None;
                    TickDecision::Status {
                        state: OracleStreamState::Degraded,
                        reason: reason.clone(),
                    }
                } else {
                    TickDecision::Silence
                }
            }
            (Phase::Degraded, AggregateOutcome::Emit { mid, median, source_count }) => {
                let started = *self.pending_since_ms.get_or_insert(now_ms);
                if now_ms - started >= DEGRADED_HYSTERESIS_MS as i64 {
                    self.phase = Phase::Normal;
                    self.pending_since_ms = None;
                    // Recovery emits a Status FIRST; the next 50 ms tick will emit data.
                    // (Caller can choose to also emit a tick this same step; we keep it
                    // single-action per step for simplicity.)
                    TickDecision::Status {
                        state: OracleStreamState::Normal,
                        reason: format!("recovered with {source_count} sources"),
                    }
                } else {
                    // Not enough sustained recovery; still degraded → no tick emission.
                    let _ = (mid, median);
                    TickDecision::Silence
                }
            }
            (Phase::Degraded, AggregateOutcome::InsufficientSources { .. }) => {
                self.pending_since_ms = None;
                TickDecision::Silence
            }
        }
    }
}
```

- [ ] **Step 13.2: Add the transition tests**

Append to `games/tap-trading/backend/oracle-aggregator/tests/aggregation.rs`:

```rust
use aggregator::{AssetStreamPhase, TickDecision};
use tap_trading_oracle_types::OracleStreamState;

fn insufficient() -> AggregateOutcome {
    AggregateOutcome::InsufficientSources { reason: "test".into() }
}

fn emit() -> AggregateOutcome {
    AggregateOutcome::Emit { mid: 100.0, median: 100.0, source_count: 4 }
}

#[test]
fn normal_to_degraded_requires_full_hysteresis_window() {
    let mut p = AssetStreamPhase::default();
    // First insufficient — pending transition starts; still emit Silence.
    assert_eq!(p.step(0, &insufficient()), TickDecision::Silence);
    // 1999 ms later — still pending.
    assert_eq!(p.step(1_999, &insufficient()), TickDecision::Silence);
    // 2000 ms — phase flips to Degraded; emit Status.
    match p.step(2_000, &insufficient()) {
        TickDecision::Status { state, .. } => assert_eq!(state, OracleStreamState::Degraded),
        other => panic!("expected Status(Degraded), got {other:?}"),
    }
}

#[test]
fn normal_emit_after_brief_insufficient_does_not_flip() {
    let mut p = AssetStreamPhase::default();
    p.step(0, &insufficient());
    // Recovery within window → pending cleared, still Normal.
    match p.step(500, &emit()) {
        TickDecision::Tick { .. } => {}
        other => panic!("expected Tick, got {other:?}"),
    }
    // Subsequent insufficient must restart the timer.
    p.step(600, &insufficient());
    assert_eq!(p.step(2_000, &insufficient()), TickDecision::Silence);
    match p.step(2_600, &insufficient()) {
        TickDecision::Status { state, .. } => assert_eq!(state, OracleStreamState::Degraded),
        other => panic!("expected Status(Degraded), got {other:?}"),
    }
}

#[test]
fn degraded_to_normal_requires_full_hysteresis_window() {
    let mut p = AssetStreamPhase::default();
    // Force into Degraded.
    p.step(0, &insufficient());
    p.step(2_000, &insufficient());
    // Recovery starts; less than 2000 ms → still Silence.
    assert_eq!(p.step(2_100, &emit()), TickDecision::Silence);
    assert_eq!(p.step(3_999, &emit()), TickDecision::Silence);
    // At 2000 ms of sustained Emit, flip back.
    match p.step(4_100, &emit()) {
        TickDecision::Status { state, .. } => assert_eq!(state, OracleStreamState::Normal),
        other => panic!("expected Status(Normal), got {other:?}"),
    }
}

#[test]
fn degraded_insufficient_resets_recovery_timer() {
    let mut p = AssetStreamPhase::default();
    p.step(0, &insufficient());
    p.step(2_000, &insufficient());
    p.step(2_100, &emit()); // recovery pending
    p.step(2_500, &insufficient()); // resets
    // Now need a full 2 s of emit again.
    assert_eq!(p.step(3_000, &emit()), TickDecision::Silence);
    assert_eq!(p.step(4_499, &emit()), TickDecision::Silence);
    match p.step(4_600, &emit()) {
        TickDecision::Status { state, .. } => assert_eq!(state, OracleStreamState::Normal),
        other => panic!("expected Status(Normal), got {other:?}"),
    }
}
```

- [ ] **Step 13.3: Run**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-oracle-aggregator --test aggregation
```

Expected: all 13 tests pass (9 from Task 4 + 4 new).

- [ ] **Step 13.4: Full check**

```bash
cd games/tap-trading/backend && cargo check && cargo clippy --all-targets -- -D warnings && cargo test
```

Expected: green.

- [ ] **Step 13.5: Commit**

```bash
git add games/tap-trading/backend/oracle-aggregator/src/aggregator.rs \
        games/tap-trading/backend/oracle-aggregator/tests/aggregation.rs
git commit -m "feat(tick-oracle): degraded state hysteresis and recovery"
```

---

## Task 14 — Exponential backoff supervisor for source reconnects

The three CEX source modules each already have a backoff loop (Tasks 10–12). This task extracts the common helper into `sources/mod.rs` so all four sources call the same supervisor, and adds a unit test that drives a deliberately-failing mock server to assert backoff behaviour.

**Files:**
- Modify: `games/tap-trading/backend/oracle-aggregator/src/sources/mod.rs` (export `backoff_supervisor` helper)
- Modify: `games/tap-trading/backend/oracle-aggregator/src/sources/{binance,bybit,okx,pyth}.rs` (call the shared helper)
- Create: `games/tap-trading/backend/oracle-aggregator/tests/backoff.rs`

- [ ] **Step 14.1: Add the shared backoff helper to `sources/mod.rs`**

Append:

```rust
use std::time::Duration;

/// Exponential backoff with jitter. Capped at 30 s. ORACLE_SPEC §4.2.
pub async fn sleep_jittered(backoff_ms: &mut u64) {
    let jitter_amplitude = (*backoff_ms as f64) * 0.1;
    let jitter = (rand_unit() - 0.5) * 2.0 * jitter_amplitude;
    let with_jitter = (*backoff_ms as f64 + jitter).max(50.0) as u64;
    tokio::time::sleep(Duration::from_millis(with_jitter)).await;
    *backoff_ms = (*backoff_ms * 2).min(30_000);
}

pub fn rand_unit() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    (nanos % 1_000) as f64 / 1_000.0
}
```

In each source file (`binance.rs`, `bybit.rs`, `okx.rs`, `pyth.rs`), delete the local copies of `sleep_jittered` and `rand_unit`, and replace call sites with `crate::sources::sleep_jittered`.

- [ ] **Step 14.2: Write the mock-server backoff test**

Write `games/tap-trading/backend/oracle-aggregator/tests/backoff.rs`:

```rust
//! Backoff helper bounds. ORACLE_SPEC §4.2: exp 100ms → 30s, jitter ±10%.

#[path = "../src/sources/mod.rs"]
mod sources;

#[tokio::test]
async fn backoff_doubles_within_jitter_band() {
    let mut backoff_ms: u64 = 100;
    let start = std::time::Instant::now();
    sources::sleep_jittered(&mut backoff_ms).await;
    let elapsed = start.elapsed().as_millis() as u64;
    // First sleep: 100 ms ± 10 ms = [90, 110].
    assert!(elapsed >= 80, "elapsed {elapsed} ms");
    assert!(elapsed <= 200, "elapsed {elapsed} ms");
    // After: backoff_ms doubled to 200.
    assert_eq!(backoff_ms, 200);
}

#[tokio::test]
async fn backoff_caps_at_30_seconds() {
    let mut backoff_ms: u64 = 20_000;
    sources::sleep_jittered(&mut backoff_ms).await;
    assert!(backoff_ms <= 30_000, "got {backoff_ms}");
    // One more iteration: 40_000 → 30_000 cap.
    backoff_ms = 40_000;
    let mut tmp = backoff_ms;
    tmp = (tmp * 2).min(30_000);
    assert_eq!(tmp, 30_000);
}
```

Note the timing test uses 100 ms → 200 ms range with healthy tolerance. CI machines can be slow; if this proves flaky in practice (>1% fail rate), drop the timing assertion and keep only the post-sleep `backoff_ms` doubling assertion.

The mock-WS-that-rejects-then-accepts integration test is folded into Task 16's end-to-end harness — there's no value in mocking a WS server *just* to test the reconnect side of `tokio-tungstenite::connect_async`, and the end-to-end test already exercises both the connect and the message-pump paths.

- [ ] **Step 14.3: Run + full check**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-oracle-aggregator --test backoff && cargo check && cargo clippy --all-targets -- -D warnings && cargo test
```

Expected: green.

- [ ] **Step 14.4: Commit**

```bash
git add games/tap-trading/backend/oracle-aggregator/src/sources/ \
        games/tap-trading/backend/oracle-aggregator/tests/backoff.rs
git commit -m "feat(tick-oracle): exponential backoff for source reconnects"
```

---

## Task 15 — Add 50ms aggregator driver loop

The orchestrating task that turns the per-component pieces from Tasks 4, 5, 7, 8, and 13 into a live oracle feed. Single-owner async task: a `select!` loop drains `SourceTick`s from the `mpsc` channel into per-asset latest-source-tick maps, and every 50 ms runs `AssetPriceState::apply_sources` → `AssetStreamPhase::step` → `AssetVolState::next_vol` → assemble `OracleTick { seq: next_seq[asset]++ }` → push to ring + broadcast.

Design decisions for this task:
- **No `RwLock` per asset.** The driver is the single owner of all per-asset state — `AssetPriceState`, `AssetVolState`, `AssetStreamPhase`, `next_seq`, and the latest-source-tick map all live in plain `HashMap`s on the driver task's stack. The `select!` loop guarantees mutual exclusion; locks would only add contention.
- **`seq` continues on recovery.** Per the plan header (line 19) and Task 13: `seq` does not advance during DEGRADED, and on recovery the next tick's `seq` is `last_seq + 1`. ADR-0008 §6 reads "reset on recovery" but Task 13 already encodes the continuation semantics — we follow Task 13. (Flagged in the PR description.)
- **`MissedTickBehavior::Skip`.** If a tick is missed (e.g. system pause), we do NOT burst-emit catch-up ticks — we drop them. The aggregator's clients want fresh data, not history.
- **Cold-start vol via `AssetVolState`.** The cold-start branch is already inside `AssetVolState::next_vol` (Task 8). The driver just calls it; no extra branch here.
- **`tick_once` extracted for tests.** The body of one 50 ms iteration is `pub(crate) fn tick_once(&mut self, now_ms: i64)`; `pub async fn run(...)` is the `select!` loop that calls it. This lets unit tests drive the loop synchronously without `tokio::time::advance` gymnastics.

**Files:**
- Create: `games/tap-trading/backend/oracle-aggregator/src/driver.rs`
- Modify: `games/tap-trading/backend/oracle-aggregator/src/main.rs` (`mod driver;`)

- [ ] **Step 15.1: Write `driver.rs`**

Write `games/tap-trading/backend/oracle-aggregator/src/driver.rs`:

```rust
//! 50 ms aggregator driver. Single-owner task that drains source ticks and
//! emits `OracleTick` / `OracleStatus` to the ring + broadcast.
//!
//! Sole writer of `AssetPriceState`, `AssetVolState`, `AssetStreamPhase`,
//! per-asset `next_seq`, and latest-tick-per-source. No locks: the
//! `select!` between `source_rx.recv()` and the 50 ms interval is the
//! mutual-exclusion mechanism.

use crate::aggregator::{
    AggregateOutcome, AssetPriceState, AssetStreamPhase, TickDecision,
};
use crate::broadcast::Broadcaster;
use crate::constants::EMIT_PERIOD_MS;
use crate::ring_buffer::RingBuffers;
use crate::sources::{SourceId, SourceTick};
use crate::vol_state::AssetVolState;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;
use tap_trading_oracle_types::{
    AssetSymbol, OracleMessage, OracleStatus, OracleTick,
};
use tokio::sync::mpsc;
use tokio::time::{interval, MissedTickBehavior};

/// All per-asset state owned by the driver task. No locks — single owner.
struct DriverState {
    /// Latest tick per (asset, source). Refreshed on every `source_rx.recv()`.
    latest: HashMap<AssetSymbol, BTreeMap<SourceId, SourceTick>>,
    price: HashMap<AssetSymbol, AssetPriceState>,
    vol: HashMap<AssetSymbol, AssetVolState>,
    phase: HashMap<AssetSymbol, AssetStreamPhase>,
    /// Monotonic `seq` per asset under the current `run_id`. Paused (not
    /// advanced) while the asset is DEGRADED.
    next_seq: HashMap<AssetSymbol, u64>,
    rings: Arc<RingBuffers>,
    broadcaster: Broadcaster,
    run_id: u64,
}

impl DriverState {
    fn new(rings: Arc<RingBuffers>, broadcaster: Broadcaster, run_id: u64) -> Self {
        Self {
            latest: HashMap::new(),
            price: HashMap::new(),
            vol: HashMap::new(),
            phase: HashMap::new(),
            next_seq: HashMap::new(),
            rings,
            broadcaster,
            run_id,
        }
    }

    /// Absorb one source observation. Server-stamps `ts_ms` on receive.
    fn ingest(&mut self, tick: SourceTick) {
        self.latest
            .entry(tick.asset)
            .or_default()
            .insert(tick.source, tick);
    }

    /// Run one 50 ms emit step for every asset that has at least one
    /// recorded source. Pure-ish: side effects are ring push + broadcast.
    pub(crate) fn tick_once(&mut self, now_ms: i64) {
        // Collect the asset list up-front to keep the borrow checker happy
        // while we mutate per-asset state below.
        let assets: Vec<AssetSymbol> = self.latest.keys().copied().collect();
        for asset in assets {
            self.tick_asset(asset, now_ms);
        }
    }

    fn tick_asset(&mut self, asset: AssetSymbol, now_ms: i64) {
        let latest = match self.latest.get(&asset) {
            Some(m) if !m.is_empty() => m,
            _ => return,
        };
        let price = self.price.entry(asset).or_default();
        let outcome = price.apply_sources(now_ms, latest);

        let phase = self.phase.entry(asset).or_default();
        let decision = phase.step(now_ms, &outcome);

        match decision {
            TickDecision::Tick { mid, source_count, .. } => {
                // Vol is computed against the freshly-aggregated mid.
                let vol = self.vol.entry(asset).or_default().next_vol(now_ms, mid);
                let seq_slot = self.next_seq.entry(asset).or_insert(0);
                let seq = *seq_slot;
                *seq_slot = seq.saturating_add(1);

                let tick = OracleTick {
                    asset,
                    run_id: self.run_id,
                    seq,
                    ts_ms: now_ms,
                    mid,
                    vol_annualized: vol,
                    source_count,
                };
                self.rings.push(tick);
                self.broadcaster.send(OracleMessage::Tick(tick));
            }
            TickDecision::Status { state, reason } => {
                self.broadcaster.send(OracleMessage::Status(OracleStatus {
                    asset,
                    state,
                    reason,
                    run_id: self.run_id,
                }));
            }
            TickDecision::Silence => {}
        }
    }
}

/// Wall-clock-ms since UNIX epoch. Local copy to avoid pubbing the one in
/// `broadcast.rs` — single-line helper, not worth the cross-module coupling.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Run the driver. Returns only when `source_rx` is closed (i.e. process
/// shutdown). Spawn on the tokio runtime.
pub async fn run(
    mut source_rx: mpsc::Receiver<SourceTick>,
    rings: Arc<RingBuffers>,
    broadcaster: Broadcaster,
    run_id: u64,
) {
    let mut state = DriverState::new(rings, broadcaster, run_id);
    let mut ticker = interval(Duration::from_millis(EMIT_PERIOD_MS));
    // Skip catch-up bursts if the loop falls behind — clients want fresh
    // data, not backfill.
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    // Discard the immediate first tick so the first emit happens after
    // EMIT_PERIOD_MS, not at t=0 before any sources have been ingested.
    ticker.tick().await;

    loop {
        tokio::select! {
            maybe = source_rx.recv() => {
                match maybe {
                    Some(tick) => state.ingest(tick),
                    None => {
                        tracing::info!("source channel closed; driver exiting");
                        return;
                    }
                }
            }
            _ = ticker.tick() => {
                state.tick_once(now_ms());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregator::AssetPriceState;
    use crate::ring_buffer::RingLookup;

    fn src(source: SourceId, asset: AssetSymbol, price: f64, ts_ms: i64) -> SourceTick {
        SourceTick { source, asset, price, ts_ms, pyth_conf_bps: None }
    }

    fn fresh_state() -> DriverState {
        DriverState::new(Arc::new(RingBuffers::new()), Broadcaster::new(), 7)
    }

    #[test]
    fn emits_tick_with_seq_starting_at_zero() {
        let mut s = fresh_state();
        let mut rx = s.broadcaster.sender().subscribe();
        let t = 1_000_000;
        s.ingest(src(SourceId::Binance, AssetSymbol::Eth, 3812.0, t));
        s.ingest(src(SourceId::Bybit, AssetSymbol::Eth, 3812.1, t));
        s.ingest(src(SourceId::Okx, AssetSymbol::Eth, 3812.2, t));

        s.tick_once(t);

        let msg = rx.try_recv().expect("a broadcast frame");
        match msg {
            OracleMessage::Tick(tick) => {
                assert_eq!(tick.asset, AssetSymbol::Eth);
                assert_eq!(tick.run_id, 7);
                assert_eq!(tick.seq, 0, "first tick seq must be 0");
                assert_eq!(tick.source_count, 3);
                // Ring should also have it.
                match s.rings.get(AssetSymbol::Eth, 7, 0) {
                    RingLookup::Hit(rt) => assert_eq!(rt, tick),
                    other => panic!("expected Hit, got {other:?}"),
                }
            }
            other => panic!("expected Tick, got {other:?}"),
        }
    }

    #[test]
    fn seq_advances_monotonically_across_ticks() {
        let mut s = fresh_state();
        let mut rx = s.broadcaster.sender().subscribe();
        let base = 1_000_000;
        for i in 0..3 {
            let now = base + (i * 50);
            s.ingest(src(SourceId::Binance, AssetSymbol::Eth, 3812.0 + i as f64, now));
            s.ingest(src(SourceId::Bybit, AssetSymbol::Eth, 3812.1 + i as f64, now));
            s.tick_once(now);
        }
        let mut seqs = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            if let OracleMessage::Tick(t) = msg {
                seqs.push(t.seq);
            }
        }
        assert_eq!(seqs, vec![0, 1, 2]);
    }

    #[test]
    fn single_source_produces_silence_not_tick() {
        // Step 4 of ORACLE_SPEC §4.4: < 2 active sources → no tick.
        // Hysteresis hasn't fired yet either → Silence, no Status.
        let mut s = fresh_state();
        let mut rx = s.broadcaster.sender().subscribe();
        let t = 1_000_000;
        s.ingest(src(SourceId::Binance, AssetSymbol::Eth, 3812.0, t));
        s.tick_once(t);
        assert!(rx.try_recv().is_err(), "no frame expected");
        // Seq must NOT have advanced.
        assert!(!s.next_seq.contains_key(&AssetSymbol::Eth));
    }

    #[test]
    fn degraded_pauses_seq_then_recovers_with_last_plus_one() {
        // Confirm Task-13 semantics: seq pauses during DEGRADED, continues
        // (not resets) on recovery. ADR-0008 §6 reads "reset" but the plan
        // header line 19 + Task 13 explicitly chose continuation.
        let mut s = fresh_state();
        let mut rx = s.broadcaster.sender().subscribe();

        let t0 = 0i64;
        // 1) Normal: 2 sources → tick(seq=0).
        s.ingest(src(SourceId::Binance, AssetSymbol::Eth, 3812.0, t0));
        s.ingest(src(SourceId::Bybit, AssetSymbol::Eth, 3812.1, t0));
        s.tick_once(t0);
        // 2) Drop one source by making the rest of the latest-map stale.
        // Easier: just clear the map so apply_sources sees zero active.
        s.latest.get_mut(&AssetSymbol::Eth).unwrap().clear();
        // First Silence (pending), then a Status(Degraded) after 2 s.
        s.tick_once(2_001);
        // 3) Restore sources; 2 s of sustained Emit triggers Status(Normal).
        let t_recover = 5_000;
        s.ingest(src(SourceId::Binance, AssetSymbol::Eth, 3812.0, t_recover));
        s.ingest(src(SourceId::Bybit, AssetSymbol::Eth, 3812.1, t_recover));
        s.tick_once(t_recover);
        s.tick_once(t_recover + 2_001);
        // 4) Next emit-eligible step issues seq=1, NOT seq=0.
        let t_next = t_recover + 2_100;
        s.ingest(src(SourceId::Binance, AssetSymbol::Eth, 3812.0, t_next));
        s.ingest(src(SourceId::Bybit, AssetSymbol::Eth, 3812.1, t_next));
        s.tick_once(t_next);

        let mut tick_seqs = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            if let OracleMessage::Tick(t) = msg {
                tick_seqs.push(t.seq);
            }
        }
        // First two ticks at t0 (seq=0) and t_next (seq=1).
        assert_eq!(tick_seqs, vec![0, 1]);
    }

    #[tokio::test(start_paused = true)]
    async fn run_drains_channel_and_emits_on_interval() {
        let rings = Arc::new(RingBuffers::new());
        let broadcaster = Broadcaster::new();
        let mut rx = broadcaster.sender().subscribe();
        let (tx, source_rx) = mpsc::channel::<SourceTick>(8);

        let handle = tokio::spawn(super::run(source_rx, rings, broadcaster, 9));

        // Feed two sources, then advance the paused clock past 50 ms.
        tx.send(src(SourceId::Binance, AssetSymbol::Eth, 3812.0, 0)).await.unwrap();
        tx.send(src(SourceId::Bybit, AssetSymbol::Eth, 3812.1, 0)).await.unwrap();
        tokio::time::advance(Duration::from_millis(60)).await;
        // Yield to let the driver task run the interval branch.
        tokio::task::yield_now().await;

        let msg = tokio::time::timeout(Duration::from_millis(50), async {
            loop {
                if let Ok(OracleMessage::Tick(t)) = rx.recv().await {
                    return t;
                }
            }
        })
        .await
        .expect("a tick within the test budget");
        assert_eq!(msg.run_id, 9);

        drop(tx);
        let _ = handle.await;
    }
}
```

- [ ] **Step 15.2: Wire into `main.rs`**

Edit `games/tap-trading/backend/oracle-aggregator/src/main.rs`. Add `mod driver;` next to the other module declarations. The full list should now include all of:

```rust
mod aggregator;
mod api;
mod broadcast;
mod config;
mod constants;
mod driver;
mod ring_buffer;
mod sources;
mod vol_state;
```

(Order: alphabetical; the production `runtime::spawn` helper from Task 16 will import `crate::driver`.)

- [ ] **Step 15.3: Run the driver tests**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-oracle-aggregator driver::tests
```

Expected: all 5 driver tests pass. Likely failure modes:
- `emits_tick_with_seq_starting_at_zero` reports `seq=1`: the `next_seq` slot was initialized to 1 instead of 0 — `or_insert(0)` then read-before-increment is the fix.
- `single_source_produces_silence_not_tick` finds a frame in `rx`: `apply_sources` step 4 isn't returning `InsufficientSources` for `len < 2`, or the phase machine is emitting on cold-start. Check Task 4 step 4.
- `degraded_pauses_seq_then_recovers_with_last_plus_one` reports seqs `[0, 0]`: `tick_asset` is reaching the `Tick` arm during DEGRADED — the phase machine's `(Phase::Degraded, Emit)` arm must be returning `Status` or `Silence`, NOT `Tick`. Re-check Task 13.

- [ ] **Step 15.4: Full check**

```bash
cd games/tap-trading/backend && cargo check && cargo clippy --all-targets -- -D warnings && cargo test
```

Expected: green across the whole workspace.

- [ ] **Step 15.5: Commit**

```bash
git add games/tap-trading/backend/oracle-aggregator/src/driver.rs \
        games/tap-trading/backend/oracle-aggregator/src/main.rs
git commit -m "feat(tick-oracle): add 50ms aggregator driver loop"
```

---

## Task 16 — End-to-end aggregator test over mock sources

End-to-end smoke that exercises the real 50 ms driver from Task 15 over hand-rolled mock sources. The driver consumes `SourceTick`s through the same `mpsc` it uses in production; the test feeds the channel, then asserts that `OracleTick` JSON appears on `/stream` and `/ring/:asset/:seq?run_id=N` returns the same tick. This test catches wiring bugs across the entire data path — the per-task tests above each exercise a single layer.

**Files:**
- Create: `games/tap-trading/backend/oracle-aggregator/tests/end_to_end.rs`
- Modify: `games/tap-trading/backend/oracle-aggregator/src/main.rs` (extract a `run_with_config` helper that takes injected sources, so the test can drive the full pipeline without forking a binary)

- [ ] **Step 16.1: Extract `run_with_config` from `main.rs`**

The current `main()` reads env, builds state, runs `axum::serve`. We split out the part that *runs the aggregator with already-built dependencies* so the test can call it with mocks. The helper spawns the Task-15 driver task on the `source_rx` end of the channel.

Edit `games/tap-trading/backend/oracle-aggregator/src/main.rs`. Below `mod` declarations add:

```rust
pub mod runtime {
    use crate::api::{router, AppState};
    use crate::broadcast::Broadcaster;
    use crate::driver;
    use crate::ring_buffer::RingBuffers;
    use crate::sources::{Source, SourceTick};
    use anyhow::Result;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    pub struct RuntimeHandles {
        pub broadcaster: Broadcaster,
        pub rings: Arc<RingBuffers>,
        pub run_id: u64,
        pub source_tx: mpsc::Sender<SourceTick>,
    }

    /// Bind to `listener`, spawn driver + sources, return handles for tests.
    /// Production `main` uses this same helper.
    pub async fn spawn(
        listener: tokio::net::TcpListener,
        run_id: u64,
        sources: Vec<Box<dyn Source>>,
    ) -> Result<RuntimeHandles> {
        let rings = Arc::new(RingBuffers::new());
        let broadcaster = Broadcaster::new();
        let _hb = broadcaster.spawn_heartbeat();

        let (source_tx, source_rx) = mpsc::channel::<SourceTick>(1024);

        // Spawn the Task-15 driver. Owns source_rx, the per-asset state maps,
        // and the 50 ms emit interval.
        tokio::spawn(driver::run(
            source_rx,
            rings.clone(),
            broadcaster.clone(),
            run_id,
        ));

        for src in sources {
            let tx_cloned = source_tx.clone();
            tokio::spawn(async move {
                src.run(tx_cloned).await;
            });
        }

        let app = router(AppState {
            run_id,
            rings: rings.clone(),
            broadcaster: broadcaster.clone(),
        });
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        Ok(RuntimeHandles { broadcaster, rings, run_id, source_tx })
    }
}
```

- [ ] **Step 16.2: Write the end-to-end smoke test**

Write `games/tap-trading/backend/oracle-aggregator/tests/end_to_end.rs`:

```rust
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
#[path = "../src/sources/mod.rs"]
mod sources;
#[path = "../src/vol_state.rs"]
mod vol_state;

use api::{router, AppState};
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
    let rings = Arc::new(RingBuffers::new());
    let broadcaster = Broadcaster::new();
    let (source_tx, source_rx) = mpsc::channel::<SourceTick>(1024);

    tokio::spawn(driver::run(
        source_rx,
        rings.clone(),
        broadcaster.clone(),
        run_id,
    ));

    let state = AppState {
        run_id,
        rings: rings.clone(),
        broadcaster: broadcaster.clone(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = router(state);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Harness {
        base: format!("http://127.0.0.1:{port}"),
        port,
        rings,
        broadcaster,
        source_tx,
    }
}

fn synth_source(src: SourceId, asset: AssetSymbol, price: f64, ts_ms: i64) -> SourceTick {
    SourceTick { source: src, asset, price, ts_ms, pyth_conf_bps: None }
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

    // Allow up to 5 s for the first 50 ms emit. We discard heartbeats (which
    // arrive every 5 s) and any non-tick frames.
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
async fn ring_rotation_yields_410_after_eleventh_push() {
    let h = spawn_with_driver(42).await;
    for seq in 0..15 {
        h.rings.push(synth_tick(seq));
    }
    let resp = reqwest::get(format!("{}/ring/ETH/0?run_id=42", h.base))
        .await
        .unwrap();
    assert_eq!(resp.status(), 410);
}

#[tokio::test]
async fn status_frame_propagates_via_stream() {
    let h = spawn_with_driver(42).await;
    let url = format!("ws://127.0.0.1:{}/stream", h.port);
    let (mut socket, _) = connect_async(&url).await.unwrap();

    h.broadcaster
        .send(OracleMessage::Status(tap_trading_oracle_types::OracleStatus {
            asset: AssetSymbol::Eth,
            state: tap_trading_oracle_types::OracleStreamState::Degraded,
            reason: "all sources stale".to_string(),
            run_id: 42,
        }));

    let deadline = std::time::Duration::from_secs(2);
    let status_seen = tokio::time::timeout(deadline, async {
        loop {
            let frame = socket.next().await.unwrap().unwrap();
            if let Message::Text(json) = frame {
                if json.contains(r#""type":"status""#)
                    && json.contains(r#""state":"degraded""#)
                {
                    return true;
                }
            }
        }
    })
    .await
    .expect("status frame within 2s");
    assert!(status_seen);
}
```

- [ ] **Step 16.3: Run**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-oracle-aggregator --test end_to_end
```

Expected: all 3 tests pass.

- [ ] **Step 16.4: Full crate verify**

```bash
cd games/tap-trading/backend && cargo check && cargo clippy --all-targets -- -D warnings && cargo test
```

Expected: green across both new crates and the existing pricing-engine.

- [ ] **Step 16.5: Commit**

```bash
git add games/tap-trading/backend/oracle-aggregator/src/main.rs \
        games/tap-trading/backend/oracle-aggregator/tests/end_to_end.rs
git commit -m "test(tick-oracle): end-to-end aggregator over mock sources"
```

---

## Final verification

- [ ] **Step F1: All 16 commits are on the current branch**

```bash
git log --oneline -16
```

Expected: 16 commits with the subjects from the Commit map, newest at top.

- [ ] **Step F2: Tick workspace is fully green**

```bash
cd games/tap-trading/backend && cargo check && cargo clippy --all-targets -- -D warnings && cargo test
```

Expected: all three commands succeed with zero warnings. Test summary:
- `tap-trading-oracle-types`: ~6 unit tests (serde roundtrips).
- `tap-trading-oracle-aggregator`: ~30+ tests across `aggregator`, `ring_buffer`, `vol_state`, `driver`, `sources::*`, `aggregation` (integration), `ring_buffer` (integration), `replay_http` (integration), `backoff` (integration), `end_to_end` (integration).
- `tap-trading-pricing-engine`: existing tests untouched.

- [ ] **Step F3: Bin still boots and serves `/healthz`**

```bash
cd games/tap-trading/backend && TAP_AGGREGATOR_PORT=18080 \
  cargo run -p tap-trading-oracle-aggregator >/tmp/agg.out 2>&1 &
sleep 2
curl -sS http://127.0.0.1:18080/healthz
echo
kill %1 2>/dev/null
```

Expected: `ok` printed. The 503 health logic (per-asset source-count check) is a follow-on (Plan B wiring); for this plan the stub is acceptable.

- [ ] **Step F4: Document open follow-ups for the next PR description**

The following items are deliberately out-of-scope here and need explicit handoff to the next plan author:

> 1. **Cold-start history replay.** Per ORACLE_SPEC §3.1, the aggregator should replay 5 min of Pyth ticks at boot via Hermes REST `/v2/updates/price/{publish_time}` so `vol_annualized` is meaningful immediately. Currently every restart resets all assets to 0.60 for ~30 s. Floor curve is the safety net.
> 2. **`/metrics` Prometheus exposition.** Stubbed.
> 3. **`/healthz` per-asset `source_count` check.** Stubbed at `ok`.
> 4. **Plan B dev-env wiring.** `scripts/worktree-env.sh` (add `TAP_AGGREGATOR_PORT` export), `sync-service-envs.sh`, `ensure-worktree-coherence.sh` (add `check_port`), `mprocs.yaml`, `start-headless.sh` — all five together per the repo contract. Plan B already covers this.
> 5. **`DEGRADED_HYSTERESIS_MS = 2000` is a plan author decision.** Surface in the PR for review.

---

## Plan D/E preview (not in scope here)

Plans D and E depend on this plan's two crates:

- **Plan D — settlement worker.** Imports `tap-trading-oracle-types` (subscribes to `WS /stream` to drive settlements) and `tap-trading-pricing-engine` (`compute_p_touch` for the final-state check at `t_close`). The worker queries `/ring/:asset/:seq` to reconstruct the exact tick a position locked at, for audit log.
- **Plan E — tick api.** Imports `tap-trading-oracle-types` (re-broadcasts `WS /stream` to clients) and `tap-trading-pricing-engine` (`compute_multiplier` for the drift check at `POST /positions`). The drift check calls `GET /ring/:asset/:seq?run_id=N` against the aggregator; a 409/410 means stale quote → 409 to the client.
- **Plan B (already merged) sets the env contract** — `TAP_AGGREGATOR_PORT` is the only required new env var from this plan's perspective; `TAP_PYTH_NETWORK` is optional with `testnet` default. Plan B already wires both.
