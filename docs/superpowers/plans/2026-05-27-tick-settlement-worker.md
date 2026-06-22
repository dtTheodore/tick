# Tick Settlement Worker — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up `tap-trading-settlement-worker`, a single-leader bin crate that subscribes to the aggregator's `WS /stream`, maintains an in-memory cache of `OPEN` positions kept current via Postgres `LISTEN`/`NOTIFY`, and writes idempotent settlements (`W` / `L` / `V`) in a single transaction per outcome. Single-leader semantics are enforced via Postgres advisory lock; failover is ≤ 2 s. After this plan lands, the settlement half of the Tick core loop is complete: every position written by the API (Plan E) reaches a terminal status via this worker, with payouts using the locked `multiplier_at_tap` per `MATH_SPEC §4.3`, voids using `last_known_mid` for `oracle_price`, and atomic balance + ledger updates that satisfy the schema's `CHECK (balance >= 0)`.

**Architecture:** The worker is a new bin crate at `games/tap-trading/backend/settlement-worker/`, joining the Tick sub-workspace alongside `pricing-engine` (Plan A). It is event-driven on three inputs: (a) aggregator `WS /stream` messages (`OracleMessage::{Tick, Status, Heartbeat}`), (b) Postgres `LISTEN tap_new_position` payloads, (c) a 30 s periodic sweep that re-hydrates from `positions WHERE status = 'OPEN'`. Single-leader: `pg_try_advisory_lock(0x_7174_5365_7474_6c72_i64)` at boot; standby spins at 1 Hz until the lock releases. Settlement writes use `ON CONFLICT (position_id) DO NOTHING RETURNING id` on `settlements` as the idempotency canary — if a row inserts, the rest of the transaction must commit; if not, early-return. Per ADR-0009 §5, the **immediate re-hydration on every LISTEN reconnect** is the durability guarantee; the 30 s sweep is the belt-and-suspenders safety net, not the primary recovery path.

**Tech Stack:** Rust 2021, `tokio` 1 (multi-thread runtime), `axum` 0.7 (`/healthz`, `/metrics`), `sqlx` 0.8 (`postgres`, `runtime-tokio`, `bigdecimal`, `chrono`, `macros`), `sqlx::postgres::PgListener` (LISTEN/NOTIFY), `tokio-tungstenite` 0.24 (WS client), `futures-util` 0.3 (stream combinators), `tracing` + `tracing-subscriber`, `thiserror` for typed errors, `serde_json` (already in workspace) for `OracleMessage`. Dev deps: `testcontainers` 0.23 + `testcontainers-modules` (`postgres` feature) for hermetic Postgres in integration tests.

**Spec:** ADR-0008 (`docs/decisions/0008-tick-oracle-wire-protocol.md`) — wire format and `/stream` endpoint. ADR-0009 (`docs/decisions/0009-tick-api-cross-service-contracts.md`) — table ownership (§2), NOTIFY contract (§5), `TAP_REFUND` taxonomy (§7), payout formula (§8). `SYSTEM_DESIGN.md §5.2` — the settle-win sketch (the worker mirrors this transaction shape verbatim). `SYSTEM_DESIGN.md §7.3` — leader election. `SYSTEM_DESIGN.md §9.1` — void policy: full-window gap → refund. `MATH_SPEC.md §4.3` — the lock-at-tap invariant (settle reads `positions.multiplier_at_tap`, never recomputes). `TESTING_STRATEGY.md §5` — idempotency, replay, race, lock-at-tap invariant, 85% line coverage. `games/tap-trading/backend/migrations/20260523120000_create_tick_schema.sql` — the actual Plan A schema (consolidated; see header note in ADR-0009 §3).

**Spec deviations / corrections (record before writing code):**

- **`oracle_price` on `V` rows.** The schema declares `CHECK (oracle_price > 0)` on `settlements` (line 94 of `migrations/20260523120000_create_tick_schema.sql`). The user's brief proposed `oracle_price = 0` for voids; that violates the constraint. We use `last_known_mid` per asset instead — the cache tracks `last_known_mid_per_asset: f64` updated on every `OracleMessage::Tick` for that asset. If the worker has never seen a `Tick` for the asset (boot-time void of a position that pre-dates the first tick), we void with `last_known_mid = strike_lo` (a guaranteed-positive value already on the position row) and log a `WARN`. This is rare-on-pathological-only and the value is purely informational on void rows.
- **WIN must credit `accounts.lifetime_points_won` AND `accounts.balance`.** `SYSTEM_DESIGN §5.2` settle_win sketch (lines 545–552) updates both. The brief mentioned only `balance`; the plan implements both. `TAP_REFUND` updates `balance` only (refunds are not "wins" — they don't move the lifetime counter).
- **`OracleStatus` has no `ts_ms` field** (ADR-0008 §6). The worker captures `gap_start_ms = now_ms()` on Normal→Degraded and `gap_end_ms = now_ms()` on Degraded→Normal — wall-clock at the receive boundary.
- **`settlements.streak_at_credit` and `streak_bonus` have NO DEFAULT** in the actual migration (the user's brief was correct on this; restating for emphasis). The worker always supplies `0` and `1.000` in v1. When streak logic ships, these values reflect the real bonus; old rows stay at `1.000` and remain correct.
- **Advisory-lock constant.** The brief suggested `0x71CC_5E77_1E_MENT_i64`, which is not valid hex (`M`, `N`, `T` are not hex digits). We use `0x_7174_5365_7474_6c72_i64` (the ASCII bytes of `"tSettlr"` packed big-endian into an `i64`). Documented in a constant at the top of `leader.rs` so future readers can find it.

**Verification baseline:** before starting, confirm the Tick sub-workspace is green:

```bash
cd games/tap-trading/backend && cargo check && cargo clippy --all-targets -- -D warnings && cargo test
```

and the root workspace is green:

```bash
cargo check --workspace && cargo test --workspace && cargo clippy --workspace -- -D warnings
```

After every commit in this plan, also run all three commands inside `games/tap-trading/backend/`.

---

## Commit map

| # | Subject | Scope |
|---|---------|-------|
| 1 | `chore(tick-worker): scaffold settlement-worker bin crate` | New `settlement-worker` member; workspace deps for tokio/axum/sqlx/tokio-tungstenite/tracing/thiserror; `main.rs` boots tokio runtime + tracing; `/healthz` (stub-only). |
| 2 | `feat(tick-worker): depend on tap-trading-oracle-types` | Add `tap-trading-oracle-types` path dependency in `Cargo.toml`; bring wire structs into scope via `use tap_trading_oracle_types::*;`. No local copies. |
| 3 | `feat(tick-worker): add advisory-lock leader election` | `leader.rs` acquires `pg_try_advisory_lock`, holds connection for life of process, releases on shutdown. Testcontainers integration test: two leaders → only one acquires. |
| 4 | `feat(tick-worker): add open-position cache with hydrate-on-boot` | `cache.rs` with `Arc<RwLock<HashMap<AssetSymbol, Vec<PositionRef>>>>` + per-asset `last_known_mid`. `hydrate(pool)` query. Testcontainers test: seed 3 OPEN + 1 WON, hydrate, assert only 3 in cache. |
| 5 | `feat(tick-worker): wire listen notify with reconnect rehydrate` | `PgListener` in `cache.rs`; on each LISTEN payload, fetch row → upsert. On reconnect, call `hydrate` immediately. Testcontainers test: NOTIFY 1 → cache insert; drop listener, NOTIFY 2, reconnect → rehydrate picks up 2. |
| 6 | `feat(tick-worker): add pure touch-detection function` | `touch.rs::evaluate_position(&PositionRef, &Tick) -> TouchOutcome::{Win, Expire, Hold}`. Table-driven unit tests for boundary equality, OTM, in-band, expiry. No IO. |
| 7 | `feat(tick-worker): subscribe to aggregator ws stream` | `loop_runner.rs` opens WS to `AGGREGATOR_WS_URL`, parses `OracleMessage`, dispatches. Mock-WS unit test confirms parse + dispatch. |
| 8 | `feat(tick-worker): implement win settlement transaction` | `settle.rs::settle_win` mirrors `SYSTEM_DESIGN §5.2` exactly: insert settlements (idempotent), update positions, insert points_ledger TAP_PAYOUT, update accounts balance + lifetime_points_won. Testcontainers integration test asserts all four writes happen atomically and second call is a no-op. |
| 9 | `feat(tick-worker): implement loss settlement transaction` | `settle.rs::settle_loss`: insert settlements outcome='L' points_delta=0, update positions status='LOST'. No ledger, no balance change. Testcontainers test. |
| 10 | `feat(tick-worker): implement void refund on oracle gap` | Status state-machine in `loop_runner.rs`: Normal→Degraded captures `gap_start_ms`; Degraded→Normal calls `settle.rs::settle_void_window` for every cached position whose `[t_open, t_close]` is contained in `[gap_start, gap_end]`. Writes TAP_REFUND ledger. Testcontainers test simulates the Status sequence end-to-end. |
| 11 | `feat(tick-worker): add periodic safety-net sweep` | 30 s tokio interval that re-runs `hydrate` + drains expired positions whose `t_close_ms < now`. Unit test for the expired-filter helper; integration test asserts a missed-NOTIFY position still settles. |
| 12 | `feat(tick-worker): expose prometheus metrics` | `health.rs::metrics` returns Prometheus text. Counters: `positions_settled_total{outcome}`, `positions_voided_total`, histogram `tick_processing_duration_seconds`. `/healthz` gates on leader=true AND `last_tick_received_ms < 2_000`. |
| 13 | `test(tick-worker): two-worker leader-race integration test` | Spawn two worker `tokio::task`s against one Testcontainers Postgres; assert only one credits the same position even when both ingest the same tick. |

Each commit must independently pass `cargo check && cargo test && cargo clippy --all-targets -- -D warnings` inside `games/tap-trading/backend/`.

---

## File map

### Created files

| Path | Responsibility |
|------|----------------|
| `games/tap-trading/backend/settlement-worker/Cargo.toml` | Crate metadata; depends on `tap-trading-pricing-engine` for `AssetSymbol`. |
| `games/tap-trading/backend/settlement-worker/src/main.rs` | Entrypoint: parse env, init tracing, build `PgPool`, run leader-acquire loop, spawn worker tasks, mount axum server. |
| `games/tap-trading/backend/settlement-worker/src/leader.rs` | `acquire_or_wait(pool) -> LeaderGuard`. Holds a dedicated `PgConnection` for life-of-process; releases on `Drop`. |
| `games/tap-trading/backend/settlement-worker/src/cache.rs` | `OpenPositionCache` with `PositionRef` struct, `hydrate`, `upsert`, `remove`, `active_for_asset`, `last_known_mid`. |
| `games/tap-trading/backend/settlement-worker/src/touch.rs` | `evaluate_position(pos, tick) -> TouchOutcome`. Pure. |
| `games/tap-trading/backend/settlement-worker/src/settle.rs` | `settle_win`, `settle_loss`, `settle_void_window`. Each is one `pool.begin()` → ON CONFLICT idempotency canary → mutations → commit. |
| `games/tap-trading/backend/settlement-worker/src/loop_runner.rs` | WS subscribe; dispatch `Tick` to touch+settle, `Status` to gap state machine, `Heartbeat` to health timer. |
| `games/tap-trading/backend/settlement-worker/src/health.rs` | axum router for `/healthz` and `/metrics`. Reads leader bool + last-tick-ms via `Arc<AtomicI64>`. |
| `games/tap-trading/backend/settlement-worker/src/error.rs` | `thiserror`-derived `WorkerError`. |
| `games/tap-trading/backend/settlement-worker/tests/touch_logic.rs` | Pure unit tests for `evaluate_position` (no IO; lives under `tests/` for module-boundary realism, not because it needs runtime). |
| `games/tap-trading/backend/settlement-worker/tests/idempotency.rs` | Testcontainers Postgres; tests `settle_win` invariants from `TESTING_STRATEGY §5.1` + lock-at-tap from `§5.4`. |
| `games/tap-trading/backend/settlement-worker/tests/listen_notify.rs` | Testcontainers; LISTEN payload → cache; reconnect → rehydrate. |
| `games/tap-trading/backend/settlement-worker/tests/void_path.rs` | Testcontainers; Status state-machine end-to-end. |
| `games/tap-trading/backend/settlement-worker/tests/concurrency.rs` | Testcontainers; two-worker race per `TESTING_STRATEGY §5.3`. |
| `games/tap-trading/backend/settlement-worker/tests/common/mod.rs` | Shared test helpers: `setup_test_postgres`, `insert_open_position`, `get_balance`, `count_settlements_for_position`. |

### Modified files

| Path | Reason |
|------|--------|
| `games/tap-trading/backend/Cargo.toml` | Add `settlement-worker` to `members`; add `tokio`, `axum`, `sqlx`, `tokio-tungstenite`, `futures-util`, `tracing`, `tracing-subscriber`, `thiserror`, `testcontainers`, `testcontainers-modules` to `[workspace.dependencies]`. |

---

## Pre-flight (one-time, not a commit)

- [ ] **Step P1: Verify Tick sub-workspace baseline is green**

Run from repo root:

```bash
cd games/tap-trading/backend && cargo check && cargo clippy --all-targets -- -D warnings && cargo test
```

Expected: all three succeed with no warnings. Plan A's 11 commits must be on `HEAD`. If anything is red, stop and report.

- [ ] **Step P2: Verify root workspace baseline**

```bash
cargo check --workspace && cargo test --workspace && cargo clippy --workspace -- -D warnings
```

Expected: green. We must not start work on top of a broken baseline.

- [ ] **Step P3: Confirm Docker is running (Testcontainers prereq)**

```bash
docker info >/dev/null 2>&1 && echo "OK" || echo "ERROR: docker not running"
```

Expected: `OK`. Testcontainers shells out to `docker` for hermetic Postgres in every integration test. If `ERROR`, start Docker Desktop and re-run.

- [ ] **Step P4: Confirm the actual schema has the columns/constraints we depend on**

```bash
grep -n "positions" games/tap-trading/backend/migrations/20260523120000_create_tick_schema.sql | head -5
grep -n "CHECK (oracle_price > 0)" games/tap-trading/backend/migrations/20260523120000_create_tick_schema.sql
grep -n "UNIQUE REFERENCES positions(id)" games/tap-trading/backend/migrations/20260523120000_create_tick_schema.sql
```

Expected: three matches confirming `positions` table, the `oracle_price > 0` constraint (settles VOID writes use `last_known_mid`, not `0`), and the `UNIQUE(position_id)` on `settlements` (the idempotency canary).

- [ ] **Step P5: Confirm `pricing-engine` exports `AssetSymbol`**

```bash
grep -n "pub use types::" games/tap-trading/backend/pricing-engine/src/lib.rs
```

Expected: `AssetSymbol` is in the re-export line. The worker depends on this re-export rather than defining its own enum.

- [ ] **Step P6: Confirm Plan C shipped `tap-trading-oracle-types`**

Plan C is a **hard** dependency of this plan. Its commits #1–#2 introduce the `tap-trading-oracle-types` crate exposing `OracleMessage`, `OracleTick`, `OracleStatus`, `OracleStreamState`. Verify the crate exists and is in the workspace `members` list:

```bash
test -f games/tap-trading/backend/oracle-types/Cargo.toml && \
  grep -n "oracle-types" games/tap-trading/backend/Cargo.toml
```

Expected: file exists and is listed under workspace `members`. If missing, stop and land Plan C first — this plan depends on the crate directly with no shadow copy.

---

## Task 1 — Scaffold the bin crate

Stand up `settlement-worker` with its minimal env-parse + tokio runtime + axum `/healthz`. No business logic yet.

**Files:**
- Modify: `games/tap-trading/backend/Cargo.toml` (add member; add workspace deps)
- Create: `games/tap-trading/backend/settlement-worker/Cargo.toml`
- Create: `games/tap-trading/backend/settlement-worker/src/main.rs`
- Create: `games/tap-trading/backend/settlement-worker/src/health.rs`
- Create: `games/tap-trading/backend/settlement-worker/src/error.rs`

- [ ] **Step 1.1: Add workspace dependencies**

Edit `games/tap-trading/backend/Cargo.toml`. Replace the `[workspace.dependencies]` block with:

```toml
[workspace.dependencies]
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
proptest = "1.5"
libm = "0.2"

# Settlement worker (Plan D) deps below.
tokio = { version = "1.40", features = ["macros", "rt-multi-thread", "signal", "sync", "time"] }
axum = { version = "0.7", default-features = false, features = ["http1", "tokio"] }
sqlx = { version = "0.8", default-features = false, features = ["runtime-tokio", "postgres", "bigdecimal", "chrono", "macros"] }
tokio-tungstenite = { version = "0.24", features = ["rustls-tls-native-roots"] }
futures-util = "0.3"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
thiserror = "1.0"

# Dev deps for integration tests.
testcontainers = "0.23"
testcontainers-modules = { version = "0.11", features = ["postgres"] }
```

Add the crate to `members`:

```toml
members = [
    "pricing-engine",
    "settlement-worker",
]
```

- [ ] **Step 1.2: Write the crate `Cargo.toml`**

Write `games/tap-trading/backend/settlement-worker/Cargo.toml`:

```toml
[package]
name = "tap-trading-settlement-worker"
version = "0.0.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true
publish.workspace = true

[[bin]]
name = "tap-trading-settlement-worker"
path = "src/main.rs"

[dependencies]
tap-trading-pricing-engine = { path = "../pricing-engine" }
serde = { workspace = true }
serde_json = { workspace = true }
tokio = { workspace = true }
axum = { workspace = true }
sqlx = { workspace = true }
tokio-tungstenite = { workspace = true }
futures-util = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
thiserror = { workspace = true }

[dev-dependencies]
testcontainers = { workspace = true }
testcontainers-modules = { workspace = true }
```

- [ ] **Step 1.3: Write the error module**

Write `games/tap-trading/backend/settlement-worker/src/error.rs`:

```rust
//! Typed errors. We keep one enum so call sites pattern-match instead of
//! string-matching, and so `tracing` events carry the variant name as a
//! structured field.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum WorkerError {
    #[error("database: {0}")]
    Db(#[from] sqlx::Error),

    #[error("websocket: {0}")]
    Ws(#[from] tokio_tungstenite::tungstenite::Error),

    #[error("decode oracle message: {0}")]
    Decode(#[from] serde_json::Error),

    #[error("missing env var: {0}")]
    MissingEnv(&'static str),

    #[error("invalid env var {name}: {reason}")]
    InvalidEnv { name: &'static str, reason: String },
}

pub type Result<T> = std::result::Result<T, WorkerError>;
```

- [ ] **Step 1.4: Write the health/metrics stub**

Write `games/tap-trading/backend/settlement-worker/src/health.rs`:

```rust
//! HTTP surface — `/healthz` and `/metrics`. v1 is a stub: `/healthz` returns
//! 200 unconditionally, `/metrics` returns an empty Prometheus payload. Real
//! gating on leader + last-tick lands in Task 12.

use axum::{routing::get, Router};

pub fn router() -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/metrics", get(|| async { "# tap-trading-settlement-worker metrics\n" }))
}
```

- [ ] **Step 1.5: Write the entrypoint**

Write `games/tap-trading/backend/settlement-worker/src/main.rs`:

```rust
//! `tap-trading-settlement-worker` — the worker that converts aggregator
//! ticks into final position outcomes.
//!
//! Architecture: ADR-0009 §2 (table ownership), SYSTEM_DESIGN.md §5.2
//! (settle-win transaction shape), §7.3 (advisory-lock leader election).

mod error;
mod health;

use std::net::SocketAddr;

use error::{Result, WorkerError};

fn env(name: &'static str) -> Result<String> {
    std::env::var(name).map_err(|_| WorkerError::MissingEnv(name))
}

fn env_port(name: &'static str) -> Result<u16> {
    env(name)?.parse().map_err(|e: std::num::ParseIntError| WorkerError::InvalidEnv {
        name,
        reason: e.to_string(),
    })
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Env-driven config — no hardcoded ports per repo CLAUDE.md.
    let _db_url = env("TAP_TRADING_DB_URL")?;
    let _aggregator_ws = env("TAP_TRADING_AGGREGATOR_WS_URL")?;
    let http_port = env_port("TAP_TRADING_SETTLEMENT_WORKER_PORT")?;

    let addr = SocketAddr::from(([0, 0, 0, 0], http_port));
    let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| {
        WorkerError::InvalidEnv { name: "TAP_TRADING_SETTLEMENT_WORKER_PORT", reason: e.to_string() }
    })?;
    tracing::info!(%addr, "settlement worker http listening");

    axum::serve(listener, health::router()).await.map_err(|e| WorkerError::InvalidEnv {
        name: "axum::serve",
        reason: e.to_string(),
    })?;
    Ok(())
}
```

The env vars `TAP_TRADING_DB_URL` and `TAP_TRADING_AGGREGATOR_WS_URL` are read but not yet used — they're declared here so the crate fails-loud at boot if missing. Per repo CLAUDE.md: no fallback string literals.

- [ ] **Step 1.6: Verify the crate compiles**

```bash
cd games/tap-trading/backend && cargo check -p tap-trading-settlement-worker
```

Expected: clean. If `unused import` warnings fire, that's OK in this commit — clippy gates them but only `-W` level by default. We tighten in step 1.8.

- [ ] **Step 1.7: Verify clippy is clean**

```bash
cd games/tap-trading/backend && cargo clippy -p tap-trading-settlement-worker --all-targets -- -D warnings
```

Expected: clean. Common failure: `unused_import` on `WorkerError` if you removed it from `main.rs`. The import is used by the `env` and `env_port` helpers, so it should be fine.

- [ ] **Step 1.8: Verify the workspace test suite still runs**

```bash
cd games/tap-trading/backend && cargo test
```

Expected: green. The worker crate has no tests yet; the pricing-engine tests should all still pass.

- [ ] **Step 1.9: Commit**

```bash
git add games/tap-trading/backend/Cargo.toml \
        games/tap-trading/backend/settlement-worker/
git commit -m "chore(tick-worker): scaffold settlement-worker bin crate"
```

---

## Task 2 — Depend on `tap-trading-oracle-types`

Plan C ships `tap-trading-oracle-types` as a shared library crate (its commits #1–#2) before this plan executes. We consume it directly — no shadow copies, no inlined structs. Per pre-flight P6, the crate is already in the workspace.

**Files:**
- Modify: `games/tap-trading/backend/settlement-worker/Cargo.toml`
- Modify: `games/tap-trading/backend/settlement-worker/src/main.rs`

- [ ] **Step 2.1: Add the path dependency**

Edit `games/tap-trading/backend/settlement-worker/Cargo.toml`. Under `[dependencies]`, add `tap-trading-oracle-types` alongside the existing `tap-trading-pricing-engine` entry:

```toml
tap-trading-pricing-engine = { path = "../pricing-engine" }
tap-trading-oracle-types = { path = "../oracle-types" }
```

- [ ] **Step 2.2: Bring the wire types into scope**

The types are used in later commits (`touch.rs` in Task 6, `loop_runner.rs` in Task 7, `settle.rs` in Task 8, integration tests across Tasks 8–13). To confirm the path dependency resolves end-to-end, add this line at the top of `games/tap-trading/backend/settlement-worker/src/main.rs`:

```rust
#[allow(unused_imports)]
use tap_trading_oracle_types::{OracleMessage, OracleStatus, OracleStreamState, OracleTick};
```

The `#[allow(unused_imports)]` keeps clippy quiet until Task 7's `loop_runner.rs` introduces the real call sites. When Task 7's `loop_runner.rs` lands, delete this scaffolding import from `main.rs` in the same commit.

- [ ] **Step 2.3: Smoke test the dependency wire-up**

```bash
cd games/tap-trading/backend && cargo check -p tap-trading-settlement-worker
```

Expected: clean. If `error[E0432]: unresolved import tap_trading_oracle_types` fires, Plan C is not on `HEAD` — go back to pre-flight P6 and re-verify before continuing.

- [ ] **Step 2.4: Verify clippy + check**

```bash
cd games/tap-trading/backend && cargo check && cargo clippy --all-targets -- -D warnings
```

Expected: clean.

- [ ] **Step 2.5: Commit**

```bash
git add games/tap-trading/backend/settlement-worker/Cargo.toml \
        games/tap-trading/backend/settlement-worker/src/main.rs
git commit -m "feat(tick-worker): depend on tap-trading-oracle-types"
```

---

## Task 3 — Advisory-lock leader election

Per `SYSTEM_DESIGN §7.3` and ADR-0009 §2, exactly one worker writes settlements at a time. We use Postgres `pg_try_advisory_lock` on a fixed `i64` key.

**Files:**
- Create: `games/tap-trading/backend/settlement-worker/src/leader.rs`
- Create: `games/tap-trading/backend/settlement-worker/tests/common/mod.rs`
- Create (initially with one test): `games/tap-trading/backend/settlement-worker/tests/idempotency.rs` (we'll grow this in Tasks 8–9; for now it hosts only the leader-acquire test as a placeholder. Actually we put the leader test in its own file to keep test fixtures focused.)

Re-do: write the leader test in its own integration test file `tests/leader_election.rs`. Cleaner module boundary.

- [ ] **Step 3.1: Define the leader-key constant and acquire logic**

Write `games/tap-trading/backend/settlement-worker/src/leader.rs`:

```rust
//! Postgres advisory-lock leader election. SYSTEM_DESIGN.md §7.3.
//!
//! At most one worker holds the lock; standby polls every 1 s. The lock is
//! held by a dedicated `PgConnection` for the life of the process. On normal
//! shutdown we explicitly release; on crash, Postgres releases the lock when
//! the connection drops (after its TCP keepalive window). Standby failover
//! target: ≤ 2 s.

use std::time::Duration;

use sqlx::{postgres::PgConnection, ConnectOptions, Connection, Executor};
use tracing::info;

use crate::error::Result;

/// Advisory-lock key. `0x_7174_5365_7474_6c72` is the ASCII bytes of
/// `"tSettlr"` packed big-endian; hex-readable and unlikely to collide with
/// any other lock the platform might claim later.
pub const LEADER_LOCK_KEY: i64 = 0x_7174_5365_7474_6c72_i64;

/// RAII handle. While alive, this process is the leader. Dropping releases.
pub struct LeaderGuard {
    conn: Option<PgConnection>,
}

impl LeaderGuard {
    /// Spin until the advisory lock is acquired. Polls every 1 s; per
    /// SYSTEM_DESIGN.md §7.3 the failover target is ≤ 2 s, and the poll
    /// interval bounds the worst-case acquisition delay.
    pub async fn acquire_or_wait(db_url: &str) -> Result<Self> {
        loop {
            let mut conn = sqlx::postgres::PgConnectOptions::new()
                .options(&[("application_name", "tap-trading-settlement-worker")])
                .connect_with_url_str(db_url)
                .await?;

            let row: (bool,) = sqlx::query_as("SELECT pg_try_advisory_lock($1)")
                .bind(LEADER_LOCK_KEY)
                .fetch_one(&mut conn)
                .await?;

            if row.0 {
                info!(key = LEADER_LOCK_KEY, "acquired leader lock");
                return Ok(Self { conn: Some(conn) });
            }

            // Release the per-attempt connection cleanly and wait.
            drop(conn);
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    /// Explicit release. Called on graceful shutdown so the standby promotes
    /// without waiting for TCP keepalive. Idempotent: drop after release is a
    /// no-op.
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
        // If the user didn't call `release` (e.g. panic), Postgres still
        // releases when the conn closes — but that can take seconds. Log so
        // operators notice missed graceful shutdowns.
        if self.conn.is_some() {
            tracing::warn!(key = LEADER_LOCK_KEY, "leader lock dropped without explicit release");
        }
    }
}

// sqlx 0.8 doesn't expose `connect_with_url_str` directly on PgConnectOptions;
// the helper below adapts.
trait ConnectWithUrlStr: Sized {
    async fn connect_with_url_str(self, url: &str) -> Result<PgConnection>;
}

impl ConnectWithUrlStr for sqlx::postgres::PgConnectOptions {
    async fn connect_with_url_str(self, url: &str) -> Result<PgConnection> {
        let opts: sqlx::postgres::PgConnectOptions = url.parse()?;
        Ok(opts.connect().await?)
    }
}
```

`sqlx::Error` already implements `From<sqlx::error::BoxDynError>` via `BoxDynError`, but parsing a URL returns `sqlx::Error::Configuration` which is in the same family. The `?` operator handles it.

Mapping to spec:
- `pg_try_advisory_lock` is non-blocking; `pg_advisory_lock` would block but burns connections.
- The lock is **session-scoped** (`pg_try_advisory_lock`, not `pg_try_advisory_xact_lock`) — releases only on explicit unlock or connection close, which is exactly the semantics SYSTEM_DESIGN §7.3 requires.

- [ ] **Step 3.2: Write the shared test helpers**

Write `games/tap-trading/backend/settlement-worker/tests/common/mod.rs`:

```rust
//! Shared integration-test helpers. Testcontainers spins a hermetic Postgres
//! per test; we run the Plan A migration file against it on startup.

use sqlx::{postgres::PgPoolOptions, PgPool};
use testcontainers::{runners::AsyncRunner, ContainerAsync};
use testcontainers_modules::postgres::Postgres;

pub struct TestDb {
    pub pool: PgPool,
    pub url: String,
    _container: ContainerAsync<Postgres>,
}

const MIGRATION_SQL: &str =
    include_str!("../../migrations/20260523120000_create_tick_schema.sql");

/// Spin Postgres in Docker, run the Plan A migration, return a pool.
///
/// The container is owned by the returned `TestDb` and dies when it goes out
/// of scope — no manual cleanup, no shared state across tests.
pub async fn setup_test_postgres() -> TestDb {
    let container = Postgres::default().start().await.expect("start postgres");
    let host_port = container.get_host_port_ipv4(5432).await.expect("port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{host_port}/postgres");

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await
        .expect("connect pool");

    sqlx::query(MIGRATION_SQL).execute(&pool).await.expect("apply migration");

    TestDb { pool, url, _container: container }
}

/// Inserts an OPEN position with the columns required by the schema. Returns
/// the generated `positions.id`.
#[allow(clippy::too_many_arguments)]
pub async fn insert_open_position(
    pool: &PgPool,
    account_id: i64,
    asset: &str,
    strike_lo: f64,
    strike_hi: f64,
    t_open_ms: i64,
    t_close_ms: i64,
    stake_points: i64,
    multiplier_at_tap: f64,
) -> i64 {
    let row: (i64,) = sqlx::query_as(
        r#"
        INSERT INTO positions
          (account_id, asset, strike_lo, strike_hi, t_open_ms, t_close_ms,
           stake_points, multiplier_at_tap, status, created_at_ms)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'OPEN', $9)
        RETURNING id
        "#,
    )
    .bind(account_id)
    .bind(asset)
    .bind(strike_lo)
    .bind(strike_hi)
    .bind(t_open_ms)
    .bind(t_close_ms)
    .bind(stake_points)
    .bind(multiplier_at_tap)
    .bind(t_open_ms)
    .fetch_one(pool)
    .await
    .expect("insert open position");
    row.0
}

/// Inserts an account with a known starting balance. Returns `accounts.id`.
pub async fn insert_account(pool: &PgPool, external_id: &str, starting_balance: i64) -> i64 {
    let row: (i64,) = sqlx::query_as(
        r#"
        INSERT INTO accounts
          (external_id, zklogin_sub, zklogin_iss, balance,
           lifetime_points_won, created_at_ms, last_active_ms)
        VALUES ($1, 'dev', 'dev', $2, 0, 0, 0)
        RETURNING id
        "#,
    )
    .bind(external_id)
    .bind(starting_balance)
    .fetch_one(pool)
    .await
    .expect("insert account");
    row.0
}

pub async fn get_balance(pool: &PgPool, account_id: i64) -> i64 {
    let row: (i64,) = sqlx::query_as("SELECT balance FROM accounts WHERE id = $1")
        .bind(account_id)
        .fetch_one(pool)
        .await
        .expect("get balance");
    row.0
}

pub async fn get_lifetime_points(pool: &PgPool, account_id: i64) -> i64 {
    let row: (i64,) = sqlx::query_as("SELECT lifetime_points_won FROM accounts WHERE id = $1")
        .bind(account_id)
        .fetch_one(pool)
        .await
        .expect("get lifetime");
    row.0
}

pub async fn count_settlements_for_position(pool: &PgPool, position_id: i64) -> i64 {
    let row: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM settlements WHERE position_id = $1")
            .bind(position_id)
            .fetch_one(pool)
            .await
            .expect("count settlements");
    row.0
}
```

- [ ] **Step 3.3: Register the module in `main.rs`**

Edit `main.rs` to add:

```rust
mod leader;
```

after `mod health;`.

- [ ] **Step 3.4: Write the leader-election integration test**

Write `games/tap-trading/backend/settlement-worker/tests/leader_election.rs`:

```rust
//! Single-leader semantics. SYSTEM_DESIGN.md §7.3.

mod common;

use std::time::Duration;

use tap_trading_settlement_worker::leader::LeaderGuard;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_one_worker_acquires_the_lock() {
    let db = common::setup_test_postgres().await;

    // First call acquires immediately.
    let first = LeaderGuard::acquire_or_wait(&db.url).await.expect("first acquires");

    // Second call would spin forever — wrap in a short timeout and assert
    // it does not resolve.
    let second = tokio::time::timeout(
        Duration::from_secs(3),
        LeaderGuard::acquire_or_wait(&db.url),
    )
    .await;

    assert!(second.is_err(), "second worker must not acquire while first holds");

    // Release first; the standby (re-attempted now) should succeed.
    let mut first = first;
    first.release().await.expect("release first");

    let third = tokio::time::timeout(
        Duration::from_secs(5),
        LeaderGuard::acquire_or_wait(&db.url),
    )
    .await
    .expect("third acquires within timeout")
    .expect("third returns Ok");

    drop(third);
    drop(first);
}
```

For this test (and all future integration tests) to compile, the bin crate must also expose its modules as a library. Add a `lib.rs` adjacent to `main.rs` — Cargo allows a crate to be both. Alternative: declare `#[path]` modules from the test itself. We choose the lib-and-bin pattern; it's cleaner.

- [ ] **Step 3.5: Convert the crate to bin + lib**

Edit `games/tap-trading/backend/settlement-worker/Cargo.toml`. Below the `[[bin]]` block, add:

```toml
[lib]
name = "tap_trading_settlement_worker"
path = "src/lib.rs"
```

Write `games/tap-trading/backend/settlement-worker/src/lib.rs`:

```rust
//! Library surface — re-exports every module so integration tests can call
//! them without the binary entry point. The `main` binary uses the same
//! modules via `crate::*` from `main.rs`.

pub mod error;
pub mod health;
pub mod leader;
```

Edit `main.rs` — replace the local `mod` declarations with `use` statements:

```rust
//! `tap-trading-settlement-worker` — the worker that converts aggregator
//! ticks into final position outcomes.
//!
//! Architecture: ADR-0009 §2 (table ownership), SYSTEM_DESIGN.md §5.2
//! (settle-win transaction shape), §7.3 (advisory-lock leader election).

use std::net::SocketAddr;

use tap_trading_settlement_worker::error::{Result, WorkerError};
use tap_trading_settlement_worker::health;

fn env(name: &'static str) -> Result<String> {
    std::env::var(name).map_err(|_| WorkerError::MissingEnv(name))
}

fn env_port(name: &'static str) -> Result<u16> {
    env(name)?.parse().map_err(|e: std::num::ParseIntError| WorkerError::InvalidEnv {
        name,
        reason: e.to_string(),
    })
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let _db_url = env("TAP_TRADING_DB_URL")?;
    let _aggregator_ws = env("TAP_TRADING_AGGREGATOR_WS_URL")?;
    let http_port = env_port("TAP_TRADING_SETTLEMENT_WORKER_PORT")?;

    let addr = SocketAddr::from(([0, 0, 0, 0], http_port));
    let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| {
        WorkerError::InvalidEnv {
            name: "TAP_TRADING_SETTLEMENT_WORKER_PORT",
            reason: e.to_string(),
        }
    })?;
    tracing::info!(%addr, "settlement worker http listening");

    axum::serve(listener, health::router()).await.map_err(|e| WorkerError::InvalidEnv {
        name: "axum::serve",
        reason: e.to_string(),
    })?;
    Ok(())
}
```

- [ ] **Step 3.6: Run the leader-election test**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-settlement-worker --test leader_election
```

Expected: green. First container spin takes 5–15 s; subsequent tests on the same Docker daemon should be much faster.

Likely failure: `sqlx::Error::Configuration("invalid URL")` if `connect_with_url_str` is wrong. Print the URL and confirm the format matches `postgres://user:pass@host:port/db`.

- [ ] **Step 3.7: Run the unit tests + clippy + check**

```bash
cd games/tap-trading/backend && cargo check && cargo clippy --all-targets -- -D warnings && cargo test -p tap-trading-settlement-worker
```

Expected: green. `cargo test -p` runs the new integration test.

- [ ] **Step 3.8: Commit**

```bash
git add games/tap-trading/backend/settlement-worker/Cargo.toml \
        games/tap-trading/backend/settlement-worker/src/lib.rs \
        games/tap-trading/backend/settlement-worker/src/leader.rs \
        games/tap-trading/backend/settlement-worker/src/main.rs \
        games/tap-trading/backend/settlement-worker/tests/common/ \
        games/tap-trading/backend/settlement-worker/tests/leader_election.rs
git commit -m "feat(tick-worker): add advisory-lock leader election"
```

---

## Task 4 — Open-position cache + hydrate on boot

Per `SYSTEM_DESIGN.md §5.2`, the worker scans an in-memory cache per tick — never Postgres. The cache stores compact `PositionRef`s plus a `last_known_mid` per asset (for the void path's `oracle_price`).

**Files:**
- Create: `games/tap-trading/backend/settlement-worker/src/cache.rs`
- Modify: `games/tap-trading/backend/settlement-worker/src/lib.rs`

- [ ] **Step 4.1: Write the cache module**

Write `games/tap-trading/backend/settlement-worker/src/cache.rs`:

```rust
//! In-memory open-position cache and per-asset last-known mid.
//!
//! The settlement loop is hot — per-tick lookups must never round-trip to
//! Postgres. We hydrate at boot, keep current via LISTEN/NOTIFY (Task 5),
//! and re-hydrate on every LISTEN reconnect (ADR-0009 §5).
//!
//! `last_known_mid` is recorded on every `OracleMessage::Tick`. It's the
//! `oracle_price` value written on void rows — required because the schema
//! declares `CHECK (oracle_price > 0)`.

use std::collections::HashMap;
use std::sync::Arc;

use sqlx::PgPool;
use tap_trading_pricing_engine::AssetSymbol;
use tokio::sync::RwLock;

use crate::error::Result;

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

    /// Replace the cache contents with every OPEN position from Postgres.
    /// Called on boot (after leader acquisition) and on every LISTEN
    /// reconnect (ADR-0009 §5).
    pub async fn hydrate(&self, pool: &PgPool) -> Result<usize> {
        let rows = sqlx::query_as::<_, PositionRow>(
            r#"
            SELECT id, account_id, asset, strike_lo, strike_hi,
                   t_open_ms, t_close_ms, stake_points, multiplier_at_tap
            FROM positions
            WHERE status = 'OPEN'
            "#,
        )
        .fetch_all(pool)
        .await?;

        let mut by_asset: HashMap<AssetSymbol, Vec<PositionRef>> = HashMap::new();
        for r in rows {
            by_asset.entry(r.asset_typed()).or_default().push(r.into_ref());
        }
        let total: usize = by_asset.values().map(|v| v.len()).sum();

        let mut g = self.inner.write().await;
        g.positions_by_asset = by_asset;
        // last_known_mid persists across rehydrates — ticks observed in the
        // meantime are still valid context.
        Ok(total)
    }

    /// Insert (or replace, on duplicate id) a position into the cache.
    /// Called from the LISTEN/NOTIFY path on `tap_new_position`.
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

    /// Snapshot of active positions for an asset whose monitoring window
    /// contains `ts_ms`. Returns owned `PositionRef`s so callers don't hold
    /// the read lock across `.await`.
    pub async fn active_for_asset(&self, asset: AssetSymbol, ts_ms: i64) -> Vec<PositionRef> {
        let g = self.inner.read().await;
        g.positions_by_asset
            .get(&asset)
            .map(|bucket| {
                bucket
                    .iter()
                    .filter(|p| p.t_open_ms <= ts_ms && ts_ms <= p.t_close_ms)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// All positions in the cache regardless of asset/window. Used by the
    /// void-on-gap path and the 30 s sweep.
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
    // sqlx::types::BigDecimal via the `bigdecimal` feature — we convert to
    // f64 for the cache because the pricing engine and touch logic are f64.
    // Loss-at-conversion is bounded by NUMERIC(20,8) precision, which is
    // ~12 decimal significant digits — well under f64's ~15.
    strike_lo: sqlx::types::BigDecimal,
    strike_hi: sqlx::types::BigDecimal,
    t_open_ms: i64,
    t_close_ms: i64,
    stake_points: i64,
    multiplier_at_tap: sqlx::types::BigDecimal,
}

impl PositionRow {
    fn asset_typed(&self) -> AssetSymbol {
        match self.asset.as_str() {
            "ETH" => AssetSymbol::Eth,
            "BTC" => AssetSymbol::Btc,
            "SOL" => AssetSymbol::Sol,
            other => panic!("unknown asset {other} — schema CHECK should reject this"),
        }
    }

    fn into_ref(self) -> PositionRef {
        PositionRef {
            id: self.id,
            account_id: self.account_id,
            asset: self.asset_typed(),
            strike_lo: bd_to_f64(&self.strike_lo),
            strike_hi: bd_to_f64(&self.strike_hi),
            t_open_ms: self.t_open_ms,
            t_close_ms: self.t_close_ms,
            stake_points: self.stake_points,
            multiplier_at_tap: bd_to_f64(&self.multiplier_at_tap),
        }
    }
}

fn bd_to_f64(b: &sqlx::types::BigDecimal) -> f64 {
    // BigDecimal exposes `to_string()` which round-trips through f64::parse;
    // direct `.to_f64()` requires the `num-traits` import which we avoid.
    b.to_string().parse::<f64>().unwrap_or_else(|_| {
        tracing::error!(value = %b, "bigdecimal -> f64 parse failed; defaulting to 0.0");
        0.0
    })
}
```

A note on the panic in `asset_typed`: the schema's `CHECK (asset IN ('ETH', 'BTC', 'SOL'))` guarantees this branch is unreachable in valid data. Panicking surfaces a corrupted-DB invariant violation immediately rather than silently dropping the position. Per CLAUDE.md Rule 12: "Fail loud."

- [ ] **Step 4.2: Register the module in `lib.rs`**

Edit `games/tap-trading/backend/settlement-worker/src/lib.rs`. Add:

```rust
pub mod cache;
```

- [ ] **Step 4.3: Write the hydrate integration test**

Write `games/tap-trading/backend/settlement-worker/tests/cache_hydrate.rs`:

```rust
//! Cache hydrate on boot. ADR-0009 §5 + SYSTEM_DESIGN.md §5.2.

mod common;

use tap_trading_pricing_engine::AssetSymbol;
use tap_trading_settlement_worker::cache::OpenPositionCache;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hydrate_loads_only_open_positions() {
    let db = common::setup_test_postgres().await;
    let acct = common::insert_account(&db.pool, "user-1", 10_000).await;

    // 3 OPEN positions across 2 assets + 1 already-WON position that must NOT
    // appear in the cache.
    let p1 = common::insert_open_position(&db.pool, acct, "BTC", 70_000.0, 70_010.0, 0, 5_000, 100, 2.5).await;
    let p2 = common::insert_open_position(&db.pool, acct, "BTC", 70_020.0, 70_030.0, 0, 5_000, 100, 2.5).await;
    let p3 = common::insert_open_position(&db.pool, acct, "ETH", 3_800.0, 3_801.0, 0, 5_000, 100, 2.5).await;
    let p_won = common::insert_open_position(&db.pool, acct, "BTC", 71_000.0, 71_001.0, 0, 5_000, 100, 2.5).await;
    sqlx::query("UPDATE positions SET status = 'WON' WHERE id = $1")
        .bind(p_won)
        .execute(&db.pool)
        .await
        .expect("flip to WON");

    let cache = OpenPositionCache::new();
    let count = cache.hydrate(&db.pool).await.expect("hydrate");

    assert_eq!(count, 3, "cache should have 3 OPEN positions");

    let btc = cache.active_for_asset(AssetSymbol::Btc, 1_000).await;
    assert_eq!(btc.len(), 2);
    let btc_ids: Vec<i64> = btc.iter().map(|p| p.id).collect();
    assert!(btc_ids.contains(&p1) && btc_ids.contains(&p2));
    assert!(!btc_ids.contains(&p_won), "WON position must not be in cache");

    let eth = cache.active_for_asset(AssetSymbol::Eth, 1_000).await;
    assert_eq!(eth.len(), 1);
    assert_eq!(eth[0].id, p3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn active_for_asset_respects_window() {
    let db = common::setup_test_postgres().await;
    let acct = common::insert_account(&db.pool, "user-2", 10_000).await;
    common::insert_open_position(&db.pool, acct, "BTC", 70_000.0, 70_010.0, 1_000, 6_000, 100, 2.5).await;

    let cache = OpenPositionCache::new();
    cache.hydrate(&db.pool).await.expect("hydrate");

    // Before t_open: not active.
    assert_eq!(cache.active_for_asset(AssetSymbol::Btc, 500).await.len(), 0);
    // Inside window: active.
    assert_eq!(cache.active_for_asset(AssetSymbol::Btc, 3_000).await.len(), 1);
    // After t_close: not active.
    assert_eq!(cache.active_for_asset(AssetSymbol::Btc, 7_000).await.len(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn last_known_mid_persists_across_hydrate() {
    let db = common::setup_test_postgres().await;

    let cache = OpenPositionCache::new();
    cache.record_last_mid(AssetSymbol::Btc, 70_000.0).await;
    cache.hydrate(&db.pool).await.expect("hydrate");

    assert_eq!(cache.last_mid(AssetSymbol::Btc).await, Some(70_000.0));
}
```

- [ ] **Step 4.4: Run the test**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-settlement-worker --test cache_hydrate
```

Expected: 3 tests pass.

- [ ] **Step 4.5: Full crate verify**

```bash
cd games/tap-trading/backend && cargo check && cargo clippy --all-targets -- -D warnings && cargo test
```

Expected: green.

- [ ] **Step 4.6: Commit**

```bash
git add games/tap-trading/backend/settlement-worker/src/cache.rs \
        games/tap-trading/backend/settlement-worker/src/lib.rs \
        games/tap-trading/backend/settlement-worker/tests/cache_hydrate.rs
git commit -m "feat(tick-worker): add open-position cache with hydrate-on-boot"
```

---

## Task 5 — LISTEN/NOTIFY with reconnect rehydrate

Per ADR-0009 §5, the worker `LISTEN tap_new_position`s. On payload `<position_id>`, it fetches the row and upserts the cache. On every LISTEN reconnect, it immediately re-hydrates — this is the durability guarantee, NOT the 30 s sweep.

**Files:**
- Modify: `games/tap-trading/backend/settlement-worker/src/cache.rs` (add `listen_loop`)

- [ ] **Step 5.1: Add the listen-loop method**

Append to `games/tap-trading/backend/settlement-worker/src/cache.rs`:

```rust
use sqlx::postgres::PgListener;

impl OpenPositionCache {
    /// Long-running LISTEN/NOTIFY loop. Returns only when the channel breaks
    /// in a way the listener can't recover from (caller restarts the task).
    ///
    /// Contract: on EVERY reconnect, before reading new payloads, do a full
    /// `hydrate` to catch positions inserted while the listener was down.
    /// ADR-0009 §5 — Postgres does NOT buffer NOTIFYs across a dropped LISTEN.
    pub async fn listen_loop(&self, pool: &PgPool, db_url: &str) -> Result<()> {
        loop {
            // (Re-)connect listener.
            let mut listener = match PgListener::connect(db_url).await {
                Ok(l) => l,
                Err(e) => {
                    tracing::warn!(error = %e, "PgListener::connect failed; retrying in 1s");
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    continue;
                }
            };

            if let Err(e) = listener.listen("tap_new_position").await {
                tracing::warn!(error = %e, "LISTEN failed; retrying");
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                continue;
            }

            // CRITICAL: re-hydrate immediately on every reconnect. NOTIFYs
            // emitted while we were disconnected are lost forever; this
            // catch-up SELECT is the durability mechanism.
            match self.hydrate(pool).await {
                Ok(n) => tracing::info!(positions = n, "rehydrated on listen reconnect"),
                Err(e) => tracing::error!(error = %e, "rehydrate failed; cache may lag briefly"),
            }

            // Drain notifications until the connection breaks.
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
                            Err(_) => {
                                tracing::warn!(payload, "non-integer NOTIFY payload");
                            }
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

    async fn fetch_and_upsert(&self, pool: &PgPool, position_id: i64) -> Result<()> {
        let row = sqlx::query_as::<_, PositionRow>(
            r#"
            SELECT id, account_id, asset, strike_lo, strike_hi,
                   t_open_ms, t_close_ms, stake_points, multiplier_at_tap
            FROM positions
            WHERE id = $1 AND status = 'OPEN'
            "#,
        )
        .bind(position_id)
        .fetch_optional(pool)
        .await?;

        if let Some(r) = row {
            self.upsert(r.into_ref()).await;
        }
        Ok(())
    }
}
```

- [ ] **Step 5.2: Write the LISTEN/NOTIFY integration test**

Write `games/tap-trading/backend/settlement-worker/tests/listen_notify.rs`:

```rust
//! NOTIFY → cache insert; reconnect → rehydrate. ADR-0009 §5.

mod common;

use std::time::Duration;

use tap_trading_pricing_engine::AssetSymbol;
use tap_trading_settlement_worker::cache::OpenPositionCache;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notify_inserts_into_cache() {
    let db = common::setup_test_postgres().await;
    let acct = common::insert_account(&db.pool, "user-N", 10_000).await;

    let cache = OpenPositionCache::new();
    let cache_clone = cache.clone();
    let url = db.url.clone();
    let pool = db.pool.clone();
    let listener_task = tokio::spawn(async move {
        let _ = cache_clone.listen_loop(&pool, &url).await;
    });

    // Wait for the listener to subscribe.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let pid = common::insert_open_position(&db.pool, acct, "BTC", 70_000.0, 70_010.0, 0, 5_000, 100, 2.5).await;
    sqlx::query(&format!("NOTIFY tap_new_position, '{pid}'"))
        .execute(&db.pool)
        .await
        .expect("notify");

    // Give the listener a moment to receive + fetch + upsert.
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let active = cache.active_for_asset(AssetSymbol::Btc, 0).await;
        if !active.is_empty() {
            assert_eq!(active[0].id, pid);
            listener_task.abort();
            return;
        }
    }
    panic!("NOTIFY did not propagate to cache within 2s");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rehydrate_picks_up_positions_inserted_while_disconnected() {
    let db = common::setup_test_postgres().await;
    let acct = common::insert_account(&db.pool, "user-R", 10_000).await;

    // Simulate "listener was down": insert directly, then call hydrate
    // explicitly (the same code path listen_loop runs on every reconnect).
    let pid = common::insert_open_position(&db.pool, acct, "ETH", 3_800.0, 3_801.0, 0, 5_000, 100, 2.5).await;

    let cache = OpenPositionCache::new();
    let count = cache.hydrate(&db.pool).await.expect("hydrate");
    assert_eq!(count, 1);

    let active = cache.active_for_asset(AssetSymbol::Eth, 1_000).await;
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].id, pid);
}
```

- [ ] **Step 5.3: Run the tests**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-settlement-worker --test listen_notify
```

Expected: 2 tests pass. `notify_inserts_into_cache` is the most informative — if it times out, the listener didn't subscribe in time; bump the initial sleep to 1 s and retry.

- [ ] **Step 5.4: Verify the full crate**

```bash
cd games/tap-trading/backend && cargo check && cargo clippy --all-targets -- -D warnings && cargo test
```

Expected: green.

- [ ] **Step 5.5: Commit**

```bash
git add games/tap-trading/backend/settlement-worker/src/cache.rs \
        games/tap-trading/backend/settlement-worker/tests/listen_notify.rs
git commit -m "feat(tick-worker): wire listen notify with reconnect rehydrate"
```

---

## Task 6 — Pure touch-detection function

`evaluate_position` is pure: given a `PositionRef` and a tick, return `Win | Expire | Hold`. No IO. Unit-tested without Postgres or any runtime.

**Files:**
- Create: `games/tap-trading/backend/settlement-worker/src/touch.rs`
- Modify: `games/tap-trading/backend/settlement-worker/src/lib.rs`

- [ ] **Step 6.1: Write the module with table-driven tests**

Write `games/tap-trading/backend/settlement-worker/src/touch.rs`:

```rust
//! Pure touch-detection. SYSTEM_DESIGN.md §5.2 lines 491–501.
//!
//! A tick "touches" a position iff `tick.mid` is at or beyond either barrier
//! AND `tick.ts_ms` is inside the monitoring window. Expiry: `tick.ts_ms` has
//! passed `t_close_ms` with no prior touch.
//!
//! Boundary policy: touch is `mid <= strike_lo` OR `mid >= strike_hi`. The
//! `=` cases match Pacifica/Euphoria's convention and `MATH_SPEC §2.1`'s
//! formulation (the no-touch probability is `P(S_t ∈ (L, H) for all t)` —
//! the open interval, so the boundary IS a touch).

use tap_trading_oracle_types::OracleTick;

use crate::cache::PositionRef;

#[derive(Debug, Clone, PartialEq)]
pub enum TouchOutcome {
    /// Tick is inside the window AND mid is at-or-beyond a barrier. Settle WIN.
    Win,
    /// Tick is past `t_close_ms` with no prior touch. Settle LOST.
    Expire,
    /// Mid is inside the band AND tick is in-window. Keep monitoring.
    Hold,
}

/// Pure decision: should this position be settled given this tick?
///
/// Precondition: caller has already confirmed `tick.asset == position.asset`.
/// We don't re-check here because the cache index is per-asset and re-checking
/// would mask a bug at the caller. CLAUDE.md Rule 12: fail loud, not silent.
pub fn evaluate_position(position: &PositionRef, tick: &OracleTick) -> TouchOutcome {
    debug_assert_eq!(position.asset, tick.asset, "evaluate_position called across assets");

    if tick.ts_ms < position.t_open_ms {
        // Tick predates the monitoring window — ignore.
        return TouchOutcome::Hold;
    }

    if tick.ts_ms > position.t_close_ms {
        return TouchOutcome::Expire;
    }

    if tick.mid <= position.strike_lo || tick.mid >= position.strike_hi {
        TouchOutcome::Win
    } else {
        TouchOutcome::Hold
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tap_trading_pricing_engine::AssetSymbol;

    fn pos(strike_lo: f64, strike_hi: f64, t_open_ms: i64, t_close_ms: i64) -> PositionRef {
        PositionRef {
            id: 1,
            account_id: 1,
            asset: AssetSymbol::Btc,
            strike_lo,
            strike_hi,
            t_open_ms,
            t_close_ms,
            stake_points: 100,
            multiplier_at_tap: 2.5,
        }
    }

    fn tick(mid: f64, ts_ms: i64) -> OracleTick {
        OracleTick {
            asset: AssetSymbol::Btc,
            run_id: 1,
            seq: 1,
            ts_ms,
            mid,
            vol_annualized: 0.80,
            source_count: 3,
        }
    }

    #[test]
    fn mid_inside_band_during_window_holds() {
        let p = pos(70_000.0, 70_010.0, 1_000, 6_000);
        let t = tick(70_005.0, 3_000);
        assert_eq!(evaluate_position(&p, &t), TouchOutcome::Hold);
    }

    #[test]
    fn mid_at_lower_barrier_is_win() {
        let p = pos(70_000.0, 70_010.0, 1_000, 6_000);
        let t = tick(70_000.0, 3_000);
        assert_eq!(evaluate_position(&p, &t), TouchOutcome::Win);
    }

    #[test]
    fn mid_at_upper_barrier_is_win() {
        let p = pos(70_000.0, 70_010.0, 1_000, 6_000);
        let t = tick(70_010.0, 3_000);
        assert_eq!(evaluate_position(&p, &t), TouchOutcome::Win);
    }

    #[test]
    fn mid_below_lower_barrier_is_win() {
        let p = pos(70_000.0, 70_010.0, 1_000, 6_000);
        let t = tick(69_999.99, 3_000);
        assert_eq!(evaluate_position(&p, &t), TouchOutcome::Win);
    }

    #[test]
    fn mid_above_upper_barrier_is_win() {
        let p = pos(70_000.0, 70_010.0, 1_000, 6_000);
        let t = tick(70_010.01, 3_000);
        assert_eq!(evaluate_position(&p, &t), TouchOutcome::Win);
    }

    #[test]
    fn tick_before_window_holds() {
        let p = pos(70_000.0, 70_010.0, 1_000, 6_000);
        let t = tick(70_005.0, 500);
        assert_eq!(evaluate_position(&p, &t), TouchOutcome::Hold);
    }

    #[test]
    fn tick_after_window_expires() {
        let p = pos(70_000.0, 70_010.0, 1_000, 6_000);
        let t = tick(70_005.0, 7_000);
        assert_eq!(evaluate_position(&p, &t), TouchOutcome::Expire);
    }

    #[test]
    fn tick_at_close_is_in_window() {
        // Boundary case: ts_ms == t_close_ms is still in-window.
        let p = pos(70_000.0, 70_010.0, 1_000, 6_000);
        let t_no_touch = tick(70_005.0, 6_000);
        assert_eq!(evaluate_position(&p, &t_no_touch), TouchOutcome::Hold);
        let t_touch = tick(70_000.0, 6_000);
        assert_eq!(evaluate_position(&p, &t_touch), TouchOutcome::Win);
    }

    #[test]
    fn tick_at_open_is_in_window() {
        let p = pos(70_000.0, 70_010.0, 1_000, 6_000);
        let t = tick(70_000.0, 1_000);
        assert_eq!(evaluate_position(&p, &t), TouchOutcome::Win);
    }
}
```

- [ ] **Step 6.2: Register the module**

Edit `games/tap-trading/backend/settlement-worker/src/lib.rs`. Add:

```rust
pub mod touch;
```

- [ ] **Step 6.3: Run the unit tests**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-settlement-worker touch::tests
```

Expected: 9 tests pass.

- [ ] **Step 6.4: Full crate verify**

```bash
cd games/tap-trading/backend && cargo check && cargo clippy --all-targets -- -D warnings && cargo test
```

Expected: green.

- [ ] **Step 6.5: Commit**

```bash
git add games/tap-trading/backend/settlement-worker/src/touch.rs \
        games/tap-trading/backend/settlement-worker/src/lib.rs
git commit -m "feat(tick-worker): add pure touch-detection function"
```

---

## Task 7 — Subscribe to aggregator WS stream

`loop_runner.rs` opens a WS to the aggregator and parses each frame into `OracleMessage`. Dispatch handlers are stubs — they land in Tasks 8/9/10.

**Files:**
- Create: `games/tap-trading/backend/settlement-worker/src/loop_runner.rs`
- Modify: `games/tap-trading/backend/settlement-worker/src/lib.rs`

- [ ] **Step 7.1: Write the loop runner module**

Write `games/tap-trading/backend/settlement-worker/src/loop_runner.rs`:

```rust
//! Aggregator WS subscriber + dispatch loop. ADR-0008 §5 (`/stream` endpoint).
//!
//! Each `OracleMessage` is dispatched by variant:
//!   - `Tick` → records last-known-mid, scans cache for touches/expiry (Task 8/9).
//!   - `Status` → flips per-asset gap state machine (Task 10).
//!   - `Heartbeat` → refreshes the last-tick health timer.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use futures_util::StreamExt;
use sqlx::PgPool;
use tap_trading_oracle_types::{OracleMessage, OracleStreamState};
use tap_trading_pricing_engine::AssetSymbol;
use tokio_tungstenite::tungstenite::Message;

use crate::cache::OpenPositionCache;
use crate::error::Result;

#[derive(Clone)]
pub struct LoopContext {
    pub pool: PgPool,
    pub cache: OpenPositionCache,
    pub last_tick_received_ms: Arc<AtomicI64>,
}

pub async fn run(ctx: LoopContext, ws_url: &str) -> Result<()> {
    loop {
        let (ws, _resp) = tokio_tungstenite::connect_async(ws_url).await?;
        tracing::info!(ws_url, "aggregator ws connected");
        let (_write, mut read) = ws.split();

        while let Some(frame) = read.next().await {
            let msg = match frame? {
                Message::Text(t) => t,
                Message::Binary(_) | Message::Ping(_) | Message::Pong(_) => continue,
                Message::Close(_) => break,
                Message::Frame(_) => continue,
            };

            let oracle_msg: OracleMessage = serde_json::from_str(&msg)?;
            handle_message(&ctx, oracle_msg).await;
        }

        tracing::warn!("aggregator ws stream ended; reconnecting in 1s");
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

pub async fn handle_message(ctx: &LoopContext, msg: OracleMessage) {
    match msg {
        OracleMessage::Tick(tick) => {
            ctx.last_tick_received_ms.store(tick.ts_ms, Ordering::Relaxed);
            ctx.cache.record_last_mid(tick.asset, tick.mid).await;
            // Touch/expire dispatch lands in Task 8/9.
            let _ = tick;
        }
        OracleMessage::Status(status) => {
            // Gap state-machine lands in Task 10.
            tracing::info!(
                asset = ?status.asset,
                state = ?status.state,
                reason = %status.reason,
                "oracle status",
            );
            let _ = OracleStreamState::Normal; // suppress unused-import until Task 10
            let _ = AssetSymbol::Btc;
        }
        OracleMessage::Heartbeat { ts_ms } => {
            ctx.last_tick_received_ms.store(ts_ms, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::postgres::PgPoolOptions;

    // We use a real PgPool because LoopContext requires it, but no SQL runs.
    // The pool URL points at a dead socket; constructing the pool (lazy) is fine.
    fn fake_ctx() -> LoopContext {
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy("postgres://nobody:nobody@127.0.0.1:1/none")
            .expect("lazy pool");
        LoopContext {
            pool,
            cache: OpenPositionCache::new(),
            last_tick_received_ms: Arc::new(AtomicI64::new(0)),
        }
    }

    #[tokio::test]
    async fn tick_updates_last_known_mid() {
        let ctx = fake_ctx();
        let msg: OracleMessage = serde_json::from_str(
            r#"{"type":"tick","asset":"BTC","run_id":1,"seq":1,"ts_ms":12345,"mid":70000.5,"vol_annualized":0.8,"source_count":3}"#,
        )
        .expect("parse");
        handle_message(&ctx, msg).await;
        assert_eq!(ctx.cache.last_mid(AssetSymbol::Btc).await, Some(70_000.5));
        assert_eq!(ctx.last_tick_received_ms.load(Ordering::Relaxed), 12345);
    }

    #[tokio::test]
    async fn heartbeat_updates_timer() {
        let ctx = fake_ctx();
        let msg: OracleMessage = serde_json::from_str(r#"{"type":"heartbeat","ts_ms":99999}"#)
            .expect("parse");
        handle_message(&ctx, msg).await;
        assert_eq!(ctx.last_tick_received_ms.load(Ordering::Relaxed), 99999);
    }
}
```

- [ ] **Step 7.2: Register the module**

Edit `games/tap-trading/backend/settlement-worker/src/lib.rs`. Add:

```rust
pub mod loop_runner;
```

Now that `loop_runner.rs` consumes `tap_trading_oracle_types::{OracleMessage, OracleStreamState}` directly, delete the scaffolding `#[allow(unused_imports)] use tap_trading_oracle_types::…;` line from `games/tap-trading/backend/settlement-worker/src/main.rs` that Task 2 added.

- [ ] **Step 7.3: Run the unit tests**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-settlement-worker loop_runner::tests
```

Expected: 2 tests pass.

- [ ] **Step 7.4: Verify the full crate**

```bash
cd games/tap-trading/backend && cargo check && cargo clippy --all-targets -- -D warnings && cargo test
```

Expected: green. Some `unused_variable` clippy warnings on `_ = tick` and `_ = OracleStreamState::Normal` would fail `-D warnings` — we use `let _ = …` (underscore binding) precisely to suppress, but `let _ = …` is a function call site so clippy treats it as deliberate. If it still fires, replace with `#[allow(unused_variables)]` on the match arm with a TODO-link comment pointing at the relevant task.

- [ ] **Step 7.5: Commit**

```bash
git add games/tap-trading/backend/settlement-worker/src/loop_runner.rs \
        games/tap-trading/backend/settlement-worker/src/lib.rs \
        games/tap-trading/backend/settlement-worker/src/main.rs
git commit -m "feat(tick-worker): subscribe to aggregator ws stream"
```

---

## Task 8 — WIN settlement transaction

`settle_win` mirrors `SYSTEM_DESIGN.md §5.2` lines 508–558 verbatim: insert settlements (idempotent canary), update positions, insert points_ledger TAP_PAYOUT, update accounts (both `balance` AND `lifetime_points_won`).

**Files:**
- Create: `games/tap-trading/backend/settlement-worker/src/settle.rs`
- Modify: `games/tap-trading/backend/settlement-worker/src/lib.rs`
- Modify: `games/tap-trading/backend/settlement-worker/src/loop_runner.rs` (wire dispatch)

- [ ] **Step 8.1: Write the settle module**

Write `games/tap-trading/backend/settlement-worker/src/settle.rs`:

```rust
//! Settlement transactions. SYSTEM_DESIGN.md §5.2; ADR-0009 §7-§8.
//!
//! Each public function is ONE atomic transaction. The `settlements` row is
//! the canary: `INSERT ... ON CONFLICT (position_id) DO NOTHING RETURNING id`.
//! If `RETURNING` yields nothing, the position was already settled — early-
//! return after rolling back the empty transaction.

use sqlx::types::BigDecimal;
use sqlx::PgPool;
use std::str::FromStr;
use tap_trading_oracle_types::OracleTick;

use crate::cache::PositionRef;
use crate::error::Result;

/// Win settlement. Pay out `floor(stake * multiplier_at_tap)` (ADR-0009 §8).
/// All four writes happen inside one transaction; the idempotency canary
/// short-circuits retries.
pub async fn settle_win(pool: &PgPool, position: &PositionRef, tick: &OracleTick) -> Result<bool> {
    let payout: i64 = ((position.stake_points as f64) * position.multiplier_at_tap).floor() as i64;

    let mut tx = pool.begin().await?;

    // Canary insert. NUMERIC(20,8) for oracle_price; NUMERIC(10,4) for
    // multiplier_used. f64::to_string then BigDecimal::from_str is exact for
    // typical magnitudes and within the column's precision.
    let inserted: Option<(i64,)> = sqlx::query_as(
        r#"
        INSERT INTO settlements
          (position_id, account_id, outcome, points_delta,
           oracle_price, settled_at_ms, multiplier_used,
           streak_at_credit, streak_bonus)
        VALUES ($1, $2, 'W', $3, $4, $5, $6, 0, 1.000)
        ON CONFLICT (position_id) DO NOTHING
        RETURNING id
        "#,
    )
    .bind(position.id)
    .bind(position.account_id)
    .bind(payout)
    .bind(f64_to_numeric(tick.mid))
    .bind(tick.ts_ms)
    .bind(f64_to_numeric(position.multiplier_at_tap))
    .fetch_optional(&mut *tx)
    .await?;

    if inserted.is_none() {
        // Already settled — committing an empty tx is fine; rolling back
        // is symmetric. We rollback to surface the no-op in pg stats.
        tx.rollback().await?;
        return Ok(false);
    }

    sqlx::query(
        "UPDATE positions SET status='WON', settled_at_ms=$2 WHERE id=$1 AND status='OPEN'",
    )
    .bind(position.id)
    .bind(tick.ts_ms)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"INSERT INTO points_ledger (account_id, kind, delta, ref_id, created_at_ms)
           VALUES ($1, 'TAP_PAYOUT', $2, $3, $4)"#,
    )
    .bind(position.account_id)
    .bind(payout)
    .bind(position.id)
    .bind(tick.ts_ms)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"UPDATE accounts
              SET balance = balance + $2,
                  lifetime_points_won = lifetime_points_won + $2
            WHERE id = $1"#,
    )
    .bind(position.account_id)
    .bind(payout)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(true)
}

fn f64_to_numeric(v: f64) -> BigDecimal {
    BigDecimal::from_str(&format!("{v:.8}")).expect("f64 -> BigDecimal must parse")
}
```

`Ok(true)` means "I credited this position"; `Ok(false)` means "already settled, no-op". Callers use this signal to decide whether to remove from cache (true) or to log a noisy retry (false).

- [ ] **Step 8.2: Register the module**

Edit `lib.rs`:

```rust
pub mod settle;
```

- [ ] **Step 8.3: Wire dispatch from loop_runner**

Edit `games/tap-trading/backend/settlement-worker/src/loop_runner.rs`. In `handle_message`, the `OracleMessage::Tick(tick)` arm becomes:

```rust
        OracleMessage::Tick(tick) => {
            ctx.last_tick_received_ms.store(tick.ts_ms, Ordering::Relaxed);
            ctx.cache.record_last_mid(tick.asset, tick.mid).await;

            let candidates = ctx.cache.active_for_asset(tick.asset, tick.ts_ms).await;
            for pos in candidates {
                use crate::touch::{evaluate_position, TouchOutcome};
                match evaluate_position(&pos, &tick) {
                    TouchOutcome::Win => {
                        match crate::settle::settle_win(&ctx.pool, &pos, &tick).await {
                            Ok(true) => ctx.cache.remove(pos.asset, pos.id).await,
                            Ok(false) => {
                                // Race-loser path: another worker (or earlier
                                // retry) already credited. Drop from cache to
                                // avoid retrying every tick.
                                ctx.cache.remove(pos.asset, pos.id).await;
                            }
                            Err(e) => tracing::error!(error = %e, position_id = pos.id, "settle_win failed"),
                        }
                    }
                    TouchOutcome::Expire | TouchOutcome::Hold => {
                        // Expire path lands in Task 9.
                    }
                }
            }
        }
```

Remove the now-unused stub binding `let _ = tick;` and the unused `OracleStreamState::Normal` placeholder.

- [ ] **Step 8.4: Write the WIN integration tests**

Write `games/tap-trading/backend/settlement-worker/tests/win_settlement.rs`:

```rust
//! WIN settlement integrity. TESTING_STRATEGY §5.1, §5.4.

mod common;

use tap_trading_oracle_types::OracleTick;
use tap_trading_pricing_engine::AssetSymbol;
use tap_trading_settlement_worker::cache::PositionRef;
use tap_trading_settlement_worker::settle::settle_win;

fn position_for(id: i64, account_id: i64, multiplier_at_tap: f64, stake: i64) -> PositionRef {
    PositionRef {
        id,
        account_id,
        asset: AssetSymbol::Btc,
        strike_lo: 70_000.0,
        strike_hi: 70_010.0,
        t_open_ms: 1_000,
        t_close_ms: 6_000,
        stake_points: stake,
        multiplier_at_tap,
    }
}

fn touching_tick(ts_ms: i64) -> OracleTick {
    OracleTick {
        asset: AssetSymbol::Btc,
        run_id: 1,
        seq: 1,
        ts_ms,
        mid: 70_010.0, // at upper barrier → win
        vol_annualized: 0.80,
        source_count: 3,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn win_credits_balance_ledger_and_lifetime_points() {
    let db = common::setup_test_postgres().await;
    let acct = common::insert_account(&db.pool, "winner", 1_000).await;
    let pid = common::insert_open_position(&db.pool, acct, "BTC", 70_000.0, 70_010.0, 1_000, 6_000, 100, 2.5).await;

    let credited = settle_win(&db.pool, &position_for(pid, acct, 2.5, 100), &touching_tick(3_000))
        .await
        .expect("settle_win");
    assert!(credited);

    // 100 * 2.5 = 250 payout, floored.
    assert_eq!(common::get_balance(&db.pool, acct).await, 1_000 + 250);
    assert_eq!(common::get_lifetime_points(&db.pool, acct).await, 250);
    assert_eq!(common::count_settlements_for_position(&db.pool, pid).await, 1);

    // Position flipped to WON.
    let status: (String,) = sqlx::query_as("SELECT status FROM positions WHERE id = $1")
        .bind(pid)
        .fetch_one(&db.pool)
        .await
        .expect("status");
    assert_eq!(status.0, "WON");

    // Ledger row exists with kind TAP_PAYOUT, delta = 250.
    let ledger: (String, i64) = sqlx::query_as(
        "SELECT kind, delta FROM points_ledger WHERE ref_id = $1 AND kind = 'TAP_PAYOUT'",
    )
    .bind(pid)
    .fetch_one(&db.pool)
    .await
    .expect("ledger");
    assert_eq!(ledger, ("TAP_PAYOUT".to_string(), 250));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_settle_is_noop_per_testing_strategy_5_1() {
    let db = common::setup_test_postgres().await;
    let acct = common::insert_account(&db.pool, "winner2", 1_000).await;
    let pid = common::insert_open_position(&db.pool, acct, "BTC", 70_000.0, 70_010.0, 1_000, 6_000, 100, 2.5).await;

    let first = settle_win(&db.pool, &position_for(pid, acct, 2.5, 100), &touching_tick(3_000))
        .await
        .expect("settle 1");
    assert!(first);
    let balance_1 = common::get_balance(&db.pool, acct).await;

    // Replay the same tick — worker-restart simulation.
    let second = settle_win(&db.pool, &position_for(pid, acct, 2.5, 100), &touching_tick(3_500))
        .await
        .expect("settle 2");
    assert!(!second, "second call must report 'already settled'");

    let balance_2 = common::get_balance(&db.pool, acct).await;
    assert_eq!(balance_1, balance_2, "balance unchanged on duplicate");
    assert_eq!(common::count_settlements_for_position(&db.pool, pid).await, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settle_uses_locked_multiplier_per_testing_strategy_5_4() {
    // The position is locked at 5.0. Even if we pass a tick whose mid is
    // wildly different from when the position was opened, payout is
    // 100 * 5.0 = 500 — NEVER recomputed from the tick.
    let db = common::setup_test_postgres().await;
    let acct = common::insert_account(&db.pool, "locked", 1_000).await;
    let pid = common::insert_open_position(&db.pool, acct, "BTC", 70_000.0, 70_010.0, 1_000, 6_000, 100, 5.0).await;

    settle_win(&db.pool, &position_for(pid, acct, 5.0, 100), &touching_tick(3_000))
        .await
        .expect("settle");

    assert_eq!(common::get_balance(&db.pool, acct).await, 1_500); // 1000 + 100*5.0
    let row: (sqlx::types::BigDecimal,) =
        sqlx::query_as("SELECT multiplier_used FROM settlements WHERE position_id = $1")
            .bind(pid)
            .fetch_one(&db.pool)
            .await
            .expect("mult");
    assert_eq!(row.0.to_string().parse::<f64>().unwrap(), 5.0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn payout_floors_per_adr_0009_section_8() {
    // 100 * 2.4999 = 249.99 → floor to 249.
    let db = common::setup_test_postgres().await;
    let acct = common::insert_account(&db.pool, "floored", 1_000).await;
    let pid = common::insert_open_position(&db.pool, acct, "BTC", 70_000.0, 70_010.0, 1_000, 6_000, 100, 2.4999).await;

    settle_win(&db.pool, &position_for(pid, acct, 2.4999, 100), &touching_tick(3_000))
        .await
        .expect("settle");

    assert_eq!(common::get_balance(&db.pool, acct).await, 1_249);
}
```

- [ ] **Step 8.5: Run the tests**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-settlement-worker --test win_settlement
```

Expected: 4 tests pass.

Likely failure: if `bigdecimal::BigDecimal` can't be cast back to `f64` via `.to_string().parse()`, the `multiplier_used` check fails. Both ends use the same conversion path, so it should round-trip cleanly.

- [ ] **Step 8.6: Full crate verify**

```bash
cd games/tap-trading/backend && cargo check && cargo clippy --all-targets -- -D warnings && cargo test
```

Expected: green.

- [ ] **Step 8.7: Commit**

```bash
git add games/tap-trading/backend/settlement-worker/src/settle.rs \
        games/tap-trading/backend/settlement-worker/src/lib.rs \
        games/tap-trading/backend/settlement-worker/src/loop_runner.rs \
        games/tap-trading/backend/settlement-worker/tests/win_settlement.rs
git commit -m "feat(tick-worker): implement win settlement transaction"
```

---

## Task 9 — LOSS expiry transaction

When a tick arrives past `t_close_ms` with no prior touch, write outcome `'L'` and `points_delta = 0`. No ledger row, no balance change — the stake was already debited at `POST /positions` (ADR-0009 §4).

**Files:**
- Modify: `games/tap-trading/backend/settlement-worker/src/settle.rs`
- Modify: `games/tap-trading/backend/settlement-worker/src/loop_runner.rs`

- [ ] **Step 9.1: Add `settle_loss` to settle.rs**

Append to `games/tap-trading/backend/settlement-worker/src/settle.rs`:

```rust
/// Loss settlement. A tick past `t_close_ms` arrived with no prior touch.
/// `points_delta = 0`; no ledger row, no balance change (stake was already
/// debited at tap-commit per ADR-0009 §4).
pub async fn settle_loss(pool: &PgPool, position: &PositionRef, tick: &OracleTick) -> Result<bool> {
    let mut tx = pool.begin().await?;

    let inserted: Option<(i64,)> = sqlx::query_as(
        r#"
        INSERT INTO settlements
          (position_id, account_id, outcome, points_delta,
           oracle_price, settled_at_ms, multiplier_used,
           streak_at_credit, streak_bonus)
        VALUES ($1, $2, 'L', 0, $3, $4, $5, 0, 1.000)
        ON CONFLICT (position_id) DO NOTHING
        RETURNING id
        "#,
    )
    .bind(position.id)
    .bind(position.account_id)
    .bind(f64_to_numeric(tick.mid))
    .bind(tick.ts_ms)
    .bind(f64_to_numeric(position.multiplier_at_tap))
    .fetch_optional(&mut *tx)
    .await?;

    if inserted.is_none() {
        tx.rollback().await?;
        return Ok(false);
    }

    sqlx::query(
        "UPDATE positions SET status='LOST', settled_at_ms=$2 WHERE id=$1 AND status='OPEN'",
    )
    .bind(position.id)
    .bind(tick.ts_ms)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(true)
}
```

- [ ] **Step 9.2: Wire LOSS dispatch into loop_runner**

Edit the `OracleMessage::Tick(tick)` arm in `loop_runner.rs`. Replace the `TouchOutcome::Expire | TouchOutcome::Hold` line with:

```rust
                    TouchOutcome::Expire => {
                        match crate::settle::settle_loss(&ctx.pool, &pos, &tick).await {
                            Ok(true) | Ok(false) => ctx.cache.remove(pos.asset, pos.id).await,
                            Err(e) => tracing::error!(error = %e, position_id = pos.id, "settle_loss failed"),
                        }
                    }
                    TouchOutcome::Hold => {
                        // Position remains in cache; awaiting future ticks.
                    }
```

- [ ] **Step 9.3: Write the LOSS integration test**

Write `games/tap-trading/backend/settlement-worker/tests/loss_settlement.rs`:

```rust
//! LOSS expiry path. SYSTEM_DESIGN.md §5.2 (settle_loss).

mod common;

use tap_trading_oracle_types::OracleTick;
use tap_trading_pricing_engine::AssetSymbol;
use tap_trading_settlement_worker::cache::PositionRef;
use tap_trading_settlement_worker::settle::settle_loss;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loss_writes_settlement_row_with_zero_delta_and_no_ledger() {
    let db = common::setup_test_postgres().await;
    let acct = common::insert_account(&db.pool, "loser", 1_000).await;
    let pid = common::insert_open_position(&db.pool, acct, "BTC", 70_000.0, 70_010.0, 1_000, 6_000, 100, 2.5).await;

    let pos = PositionRef {
        id: pid,
        account_id: acct,
        asset: AssetSymbol::Btc,
        strike_lo: 70_000.0,
        strike_hi: 70_010.0,
        t_open_ms: 1_000,
        t_close_ms: 6_000,
        stake_points: 100,
        multiplier_at_tap: 2.5,
    };
    let tick = OracleTick {
        asset: AssetSymbol::Btc,
        run_id: 1,
        seq: 1,
        ts_ms: 7_000, // past t_close
        mid: 70_005.0,
        vol_annualized: 0.80,
        source_count: 3,
    };
    let credited = settle_loss(&db.pool, &pos, &tick).await.expect("settle_loss");
    assert!(credited);

    // Balance unchanged — stake was debited at tap-commit, not by the worker.
    assert_eq!(common::get_balance(&db.pool, acct).await, 1_000);
    // No payout ledger row.
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM points_ledger WHERE ref_id = $1")
        .bind(pid)
        .fetch_one(&db.pool)
        .await
        .expect("ledger count");
    assert_eq!(count.0, 0);
    // Settlement row exists with outcome='L', delta=0.
    let row: (String, i64) =
        sqlx::query_as("SELECT outcome, points_delta FROM settlements WHERE position_id = $1")
            .bind(pid)
            .fetch_one(&db.pool)
            .await
            .expect("settlement row");
    assert_eq!(row, ("L".to_string(), 0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_loss_is_noop() {
    let db = common::setup_test_postgres().await;
    let acct = common::insert_account(&db.pool, "loser2", 1_000).await;
    let pid = common::insert_open_position(&db.pool, acct, "BTC", 70_000.0, 70_010.0, 1_000, 6_000, 100, 2.5).await;

    let pos = PositionRef {
        id: pid, account_id: acct, asset: AssetSymbol::Btc,
        strike_lo: 70_000.0, strike_hi: 70_010.0,
        t_open_ms: 1_000, t_close_ms: 6_000,
        stake_points: 100, multiplier_at_tap: 2.5,
    };
    let tick = OracleTick {
        asset: AssetSymbol::Btc, run_id: 1, seq: 1, ts_ms: 7_000,
        mid: 70_005.0, vol_annualized: 0.80, source_count: 3,
    };
    assert!(settle_loss(&db.pool, &pos, &tick).await.expect("loss 1"));
    assert!(!settle_loss(&db.pool, &pos, &tick).await.expect("loss 2"));
    assert_eq!(common::count_settlements_for_position(&db.pool, pid).await, 1);
}
```

- [ ] **Step 9.4: Run the tests**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-settlement-worker --test loss_settlement
```

Expected: 2 tests pass.

- [ ] **Step 9.5: Full crate verify**

```bash
cd games/tap-trading/backend && cargo check && cargo clippy --all-targets -- -D warnings && cargo test
```

Expected: green.

- [ ] **Step 9.6: Commit**

```bash
git add games/tap-trading/backend/settlement-worker/src/settle.rs \
        games/tap-trading/backend/settlement-worker/src/loop_runner.rs \
        games/tap-trading/backend/settlement-worker/tests/loss_settlement.rs
git commit -m "feat(tick-worker): implement loss settlement transaction"
```

---

## Task 10 — VOID refund on full-window oracle gap

Per `SYSTEM_DESIGN.md §9.1`, a position whose monitoring window was fully covered by an oracle gap is voided: status = 'VOIDED', stake refunded via `TAP_REFUND` ledger entry.

State machine: per-asset map `HashMap<AssetSymbol, GapState>` where `GapState = { gap_start_ms: Option<i64> }`. On `Status { Degraded }`: set `gap_start_ms = now_ms()` if `None`. On `Status { Normal }`: read `gap_start_ms`, set `gap_end_ms = now_ms()`, void every cached position whose window is contained in `[gap_start, gap_end]`, clear `gap_start_ms`.

**Files:**
- Modify: `games/tap-trading/backend/settlement-worker/src/settle.rs` (add `settle_void`)
- Modify: `games/tap-trading/backend/settlement-worker/src/loop_runner.rs` (add gap state)

- [ ] **Step 10.1: Add `settle_void` to settle.rs**

Append:

```rust
/// Void refund. The position's monitoring window was fully covered by an
/// oracle gap (DEGRADED status). Refund stake via TAP_REFUND ledger; outcome='V'.
///
/// `last_known_mid` is the `oracle_price` value — the schema requires it
/// `> 0` and a void inherently has no "real" oracle price. If the worker
/// has never seen a tick for this asset (extremely rare cold-start case),
/// fall back to `strike_lo` (guaranteed-positive by schema).
pub async fn settle_void(
    pool: &PgPool,
    position: &PositionRef,
    last_known_mid: Option<f64>,
    settled_at_ms: i64,
) -> Result<bool> {
    let oracle_price_f64 = match last_known_mid {
        Some(v) if v > 0.0 => v,
        _ => {
            tracing::warn!(position_id = position.id, "voiding with no last-known mid; using strike_lo");
            position.strike_lo
        }
    };

    let mut tx = pool.begin().await?;

    let inserted: Option<(i64,)> = sqlx::query_as(
        r#"
        INSERT INTO settlements
          (position_id, account_id, outcome, points_delta,
           oracle_price, settled_at_ms, multiplier_used,
           streak_at_credit, streak_bonus)
        VALUES ($1, $2, 'V', $3, $4, $5, $6, 0, 1.000)
        ON CONFLICT (position_id) DO NOTHING
        RETURNING id
        "#,
    )
    .bind(position.id)
    .bind(position.account_id)
    .bind(position.stake_points)
    .bind(f64_to_numeric(oracle_price_f64))
    .bind(settled_at_ms)
    .bind(f64_to_numeric(position.multiplier_at_tap))
    .fetch_optional(&mut *tx)
    .await?;

    if inserted.is_none() {
        tx.rollback().await?;
        return Ok(false);
    }

    sqlx::query(
        "UPDATE positions SET status='VOIDED', settled_at_ms=$2 WHERE id=$1 AND status='OPEN'",
    )
    .bind(position.id)
    .bind(settled_at_ms)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"INSERT INTO points_ledger (account_id, kind, delta, ref_id, created_at_ms)
           VALUES ($1, 'TAP_REFUND', $2, $3, $4)"#,
    )
    .bind(position.account_id)
    .bind(position.stake_points)
    .bind(position.id)
    .bind(settled_at_ms)
    .execute(&mut *tx)
    .await?;

    sqlx::query("UPDATE accounts SET balance = balance + $2 WHERE id = $1")
        .bind(position.account_id)
        .bind(position.stake_points)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(true)
}
```

Refunds touch `accounts.balance` but NOT `accounts.lifetime_points_won` — voids are not wins.

- [ ] **Step 10.2: Add gap state to loop_runner**

Edit `loop_runner.rs`. Modify `LoopContext` to include per-asset gap tracking:

```rust
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
pub struct GapTracker {
    /// `gap_start_ms` per asset; `None` means "currently Normal".
    inner: Mutex<HashMap<AssetSymbol, i64>>,
}

impl GapTracker {
    pub fn enter_degraded(&self, asset: AssetSymbol, now_ms: i64) {
        let mut g = self.inner.lock().expect("gap tracker poisoned");
        g.entry(asset).or_insert(now_ms);
    }

    /// Returns the gap_start_ms if a gap was in progress; clears the entry.
    pub fn exit_degraded(&self, asset: AssetSymbol) -> Option<i64> {
        let mut g = self.inner.lock().expect("gap tracker poisoned");
        g.remove(&asset)
    }
}

#[derive(Clone)]
pub struct LoopContext {
    pub pool: PgPool,
    pub cache: OpenPositionCache,
    pub last_tick_received_ms: Arc<AtomicI64>,
    pub gap_tracker: Arc<GapTracker>,
}
```

(`Mutex` not `tokio::Mutex` — these are single-state-update operations that don't span `.await`.)

Replace the `OracleMessage::Status(status)` arm in `handle_message` with:

```rust
        OracleMessage::Status(status) => {
            let now_ms = chrono::Utc::now().timestamp_millis();
            tracing::info!(
                asset = ?status.asset,
                state = ?status.state,
                reason = %status.reason,
                "oracle status",
            );
            match status.state {
                OracleStreamState::Degraded => {
                    ctx.gap_tracker.enter_degraded(status.asset, now_ms);
                }
                OracleStreamState::Normal => {
                    if let Some(gap_start_ms) = ctx.gap_tracker.exit_degraded(status.asset) {
                        process_gap_recovery(ctx, status.asset, gap_start_ms, now_ms).await;
                    }
                }
            }
        }
```

Add the `chrono` dep — sqlx already pulls it transitively via the `chrono` feature, but adding it explicitly to the crate's dependencies keeps the path obvious. Edit `settlement-worker/Cargo.toml` adding:

```toml
chrono = { version = "0.4", default-features = false, features = ["clock"] }
```

Add the recovery routine to `loop_runner.rs`:

```rust
async fn process_gap_recovery(ctx: &LoopContext, asset: AssetSymbol, gap_start_ms: i64, gap_end_ms: i64) {
    let positions = ctx.cache.all_positions().await;
    let last_mid = ctx.cache.last_mid(asset).await;
    for pos in positions {
        if pos.asset != asset { continue; }
        // Void only if monitoring window is fully contained in the gap.
        if pos.t_open_ms >= gap_start_ms && pos.t_close_ms <= gap_end_ms {
            match crate::settle::settle_void(&ctx.pool, &pos, last_mid, gap_end_ms).await {
                Ok(true) | Ok(false) => ctx.cache.remove(pos.asset, pos.id).await,
                Err(e) => tracing::error!(error = %e, position_id = pos.id, "settle_void failed"),
            }
        }
    }
}
```

- [ ] **Step 10.3: Update existing unit tests for the new field**

The `fake_ctx` helper in `loop_runner::tests` needs the new `gap_tracker` field. Add to the struct literal:

```rust
        gap_tracker: Arc::new(GapTracker::default()),
```

- [ ] **Step 10.4: Write the VOID integration test**

Write `games/tap-trading/backend/settlement-worker/tests/void_path.rs`:

```rust
//! VOID path. SYSTEM_DESIGN.md §9.1; ADR-0009 §7.

mod common;

use std::sync::Arc;
use std::sync::atomic::AtomicI64;

use tap_trading_oracle_types::{OracleMessage, OracleStatus, OracleStreamState, OracleTick};
use tap_trading_pricing_engine::AssetSymbol;
use tap_trading_settlement_worker::cache::OpenPositionCache;
use tap_trading_settlement_worker::loop_runner::{handle_message, GapTracker, LoopContext};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_window_gap_voids_and_refunds() {
    let db = common::setup_test_postgres().await;
    let acct = common::insert_account(&db.pool, "voided", 500).await;

    // A position whose entire 5s window will land inside the gap.
    // We use timestamps in the recent past so wall-clock now_ms() (used by
    // the Status handler) exceeds t_close_ms.
    let now_ms = chrono::Utc::now().timestamp_millis();
    let t_open = now_ms - 60_000;
    let t_close = now_ms - 55_000;
    let pid = common::insert_open_position(&db.pool, acct, "BTC", 70_000.0, 70_010.0, t_open, t_close, 100, 2.5).await;

    let cache = OpenPositionCache::new();
    cache.hydrate(&db.pool).await.expect("hydrate");
    // Pretend we saw a tick at 70_005 some time ago — gives the void path a
    // valid oracle_price (positive) per the schema's CHECK constraint.
    cache.record_last_mid(AssetSymbol::Btc, 70_005.0).await;

    let ctx = LoopContext {
        pool: db.pool.clone(),
        cache: cache.clone(),
        last_tick_received_ms: Arc::new(AtomicI64::new(0)),
        gap_tracker: Arc::new(GapTracker::default()),
    };

    // Seed a tick well BEFORE t_open so it doesn't trigger a touch.
    handle_message(&ctx, OracleMessage::Tick(OracleTick {
        asset: AssetSymbol::Btc, run_id: 1, seq: 1,
        ts_ms: t_open - 10_000,
        mid: 70_005.0, vol_annualized: 0.80, source_count: 3,
    })).await;

    // Enter Degraded *before* the position's t_open. We have to fake the
    // gap_start by inserting into the tracker directly because the Status
    // handler uses wall-clock now_ms(); we want a deterministic gap_start.
    ctx.gap_tracker.enter_degraded(AssetSymbol::Btc, t_open - 1_000);

    // Exit Degraded — uses wall-clock now, which is > t_close.
    handle_message(&ctx, OracleMessage::Status(OracleStatus {
        asset: AssetSymbol::Btc, state: OracleStreamState::Normal,
        reason: "sources recovered".into(), run_id: 1,
    })).await;

    // Assert: position is VOIDED, stake refunded, ledger TAP_REFUND row exists.
    let status: (String,) = sqlx::query_as("SELECT status FROM positions WHERE id = $1")
        .bind(pid).fetch_one(&db.pool).await.expect("status");
    assert_eq!(status.0, "VOIDED");

    assert_eq!(common::get_balance(&db.pool, acct).await, 500 + 100);

    let kind: (String, i64) = sqlx::query_as(
        "SELECT kind, delta FROM points_ledger WHERE ref_id = $1 AND kind = 'TAP_REFUND'",
    ).bind(pid).fetch_one(&db.pool).await.expect("refund row");
    assert_eq!(kind, ("TAP_REFUND".to_string(), 100));

    let outcome: (String,) = sqlx::query_as("SELECT outcome FROM settlements WHERE position_id = $1")
        .bind(pid).fetch_one(&db.pool).await.expect("settlement");
    assert_eq!(outcome.0, "V");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn partial_window_gap_does_not_void() {
    let db = common::setup_test_postgres().await;
    let acct = common::insert_account(&db.pool, "partial", 500).await;

    let now_ms = chrono::Utc::now().timestamp_millis();
    let t_open = now_ms - 5_000;
    let t_close = now_ms + 60_000; // FAR in the future — gap won't fully cover.
    let pid = common::insert_open_position(&db.pool, acct, "BTC", 70_000.0, 70_010.0, t_open, t_close, 100, 2.5).await;

    let cache = OpenPositionCache::new();
    cache.hydrate(&db.pool).await.expect("hydrate");
    cache.record_last_mid(AssetSymbol::Btc, 70_005.0).await;

    let ctx = LoopContext {
        pool: db.pool.clone(),
        cache: cache.clone(),
        last_tick_received_ms: Arc::new(AtomicI64::new(0)),
        gap_tracker: Arc::new(GapTracker::default()),
    };

    ctx.gap_tracker.enter_degraded(AssetSymbol::Btc, t_open - 1_000);
    handle_message(&ctx, OracleMessage::Status(OracleStatus {
        asset: AssetSymbol::Btc, state: OracleStreamState::Normal,
        reason: "sources recovered".into(), run_id: 1,
    })).await;

    // Position should still be OPEN — the gap ended before t_close.
    let status: (String,) = sqlx::query_as("SELECT status FROM positions WHERE id = $1")
        .bind(pid).fetch_one(&db.pool).await.expect("status");
    assert_eq!(status.0, "OPEN");
    assert_eq!(common::get_balance(&db.pool, acct).await, 500);
}
```

- [ ] **Step 10.5: Run the tests**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-settlement-worker --test void_path
```

Expected: 2 tests pass.

- [ ] **Step 10.6: Full crate verify**

```bash
cd games/tap-trading/backend && cargo check && cargo clippy --all-targets -- -D warnings && cargo test
```

Expected: green.

- [ ] **Step 10.7: Commit**

```bash
git add games/tap-trading/backend/settlement-worker/Cargo.toml \
        games/tap-trading/backend/settlement-worker/src/settle.rs \
        games/tap-trading/backend/settlement-worker/src/loop_runner.rs \
        games/tap-trading/backend/settlement-worker/tests/void_path.rs
git commit -m "feat(tick-worker): implement void refund on oracle gap"
```

---

## Task 11 — Periodic safety-net sweep

A 30 s tokio interval re-runs `hydrate` (catches any missed NOTIFYs that the immediate-rehydrate didn't, per ADR-0009 §5's belt-and-suspenders contract) and settles expired positions whose `t_close_ms < now`.

**Files:**
- Modify: `games/tap-trading/backend/settlement-worker/src/loop_runner.rs`

- [ ] **Step 11.1: Add the periodic sweep**

Append to `games/tap-trading/backend/settlement-worker/src/loop_runner.rs`:

```rust
/// Safety-net sweep: every 30 s, re-hydrate the cache and force-loss any
/// expired position the live tick loop missed. ADR-0009 §5 — this is the
/// belt-and-suspenders mechanism, not the primary recovery path.
pub async fn periodic_sweep(ctx: LoopContext) {
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(30));
    ticker.tick().await; // skip the immediate first tick.
    loop {
        ticker.tick().await;
        if let Err(e) = ctx.cache.hydrate(&ctx.pool).await {
            tracing::warn!(error = %e, "periodic hydrate failed");
            continue;
        }
        let now_ms = chrono::Utc::now().timestamp_millis();
        let expired = expired_positions(&ctx.cache, now_ms).await;
        for pos in expired {
            // Synthesize a tick from last-known mid so settle_loss has
            // valid (oracle_price > 0) data.
            let mid = ctx.cache.last_mid(pos.asset).await.unwrap_or(pos.strike_lo);
            let tick = tap_trading_oracle_types::OracleTick {
                asset: pos.asset, run_id: 0, seq: 0, ts_ms: now_ms,
                mid, vol_annualized: 0.60, source_count: 0,
            };
            match crate::settle::settle_loss(&ctx.pool, &pos, &tick).await {
                Ok(_) => ctx.cache.remove(pos.asset, pos.id).await,
                Err(e) => tracing::error!(error = %e, position_id = pos.id, "sweep settle_loss failed"),
            }
        }
    }
}

async fn expired_positions(cache: &OpenPositionCache, now_ms: i64) -> Vec<crate::cache::PositionRef> {
    cache
        .all_positions()
        .await
        .into_iter()
        .filter(|p| p.t_close_ms < now_ms)
        .collect()
}

#[cfg(test)]
mod sweep_tests {
    use super::*;
    use tap_trading_pricing_engine::AssetSymbol;

    fn pos(id: i64, t_close_ms: i64) -> crate::cache::PositionRef {
        crate::cache::PositionRef {
            id, account_id: 1, asset: AssetSymbol::Btc,
            strike_lo: 70_000.0, strike_hi: 70_010.0,
            t_open_ms: 0, t_close_ms,
            stake_points: 100, multiplier_at_tap: 2.5,
        }
    }

    #[tokio::test]
    async fn expired_filter_picks_only_past_close() {
        let cache = OpenPositionCache::new();
        cache.upsert(pos(1, 1_000)).await;  // expired
        cache.upsert(pos(2, 5_000)).await;  // not expired
        cache.upsert(pos(3, 999)).await;    // expired
        let out = expired_positions(&cache, 3_000).await;
        let mut ids: Vec<i64> = out.iter().map(|p| p.id).collect();
        ids.sort();
        assert_eq!(ids, vec![1, 3]);
    }
}
```

- [ ] **Step 11.2: Write the missed-NOTIFY integration test**

Write `games/tap-trading/backend/settlement-worker/tests/sweep.rs`:

```rust
//! Safety-net sweep catches positions the live loop missed.

mod common;

use std::sync::Arc;
use std::sync::atomic::AtomicI64;

use tap_trading_pricing_engine::AssetSymbol;
use tap_trading_settlement_worker::cache::OpenPositionCache;
use tap_trading_settlement_worker::loop_runner::{LoopContext, GapTracker};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sweep_force_loses_expired_positions_missed_by_tick_loop() {
    let db = common::setup_test_postgres().await;
    let acct = common::insert_account(&db.pool, "missed", 500).await;
    let now_ms = chrono::Utc::now().timestamp_millis();
    // Position expired 1 minute ago.
    let pid = common::insert_open_position(
        &db.pool, acct, "BTC", 70_000.0, 70_010.0,
        now_ms - 70_000, now_ms - 65_000, 100, 2.5,
    ).await;

    let cache = OpenPositionCache::new();
    cache.hydrate(&db.pool).await.expect("hydrate");
    cache.record_last_mid(AssetSymbol::Btc, 70_005.0).await;

    let ctx = LoopContext {
        pool: db.pool.clone(),
        cache: cache.clone(),
        last_tick_received_ms: Arc::new(AtomicI64::new(0)),
        gap_tracker: Arc::new(GapTracker::default()),
    };

    // Call the sweep's body directly (vs. the 30s interval) for determinism.
    cache.hydrate(&db.pool).await.expect("hydrate");
    let now_again = chrono::Utc::now().timestamp_millis();
    let expired: Vec<_> = cache.all_positions().await.into_iter()
        .filter(|p| p.t_close_ms < now_again)
        .collect();
    assert_eq!(expired.len(), 1);
    let pos = &expired[0];
    let mid = cache.last_mid(pos.asset).await.unwrap();
    let tick = tap_trading_oracle_types::OracleTick {
        asset: pos.asset, run_id: 0, seq: 0, ts_ms: now_again,
        mid, vol_annualized: 0.60, source_count: 0,
    };
    tap_trading_settlement_worker::settle::settle_loss(&ctx.pool, pos, &tick).await.expect("loss");

    let status: (String,) = sqlx::query_as("SELECT status FROM positions WHERE id = $1")
        .bind(pid).fetch_one(&db.pool).await.expect("status");
    assert_eq!(status.0, "LOST");
}
```

- [ ] **Step 11.3: Run the tests**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-settlement-worker --test sweep
cd games/tap-trading/backend && cargo test -p tap-trading-settlement-worker loop_runner::sweep_tests
```

Expected: 1 + 1 tests pass.

- [ ] **Step 11.4: Full crate verify**

```bash
cd games/tap-trading/backend && cargo check && cargo clippy --all-targets -- -D warnings && cargo test
```

Expected: green.

- [ ] **Step 11.5: Commit**

```bash
git add games/tap-trading/backend/settlement-worker/src/loop_runner.rs \
        games/tap-trading/backend/settlement-worker/tests/sweep.rs
git commit -m "feat(tick-worker): add periodic safety-net sweep"
```

---

## Task 12 — Prometheus metrics + real health gating

Replace the stub health/metrics with real ones. `/healthz` returns 200 only if `leader=true AND (now_ms - last_tick_received_ms) < 2000`. `/metrics` exposes counters and a histogram.

**Files:**
- Modify: `games/tap-trading/backend/settlement-worker/src/health.rs`
- Modify: `games/tap-trading/backend/settlement-worker/src/main.rs` (wire the state into the router)

- [ ] **Step 12.1: Redesign the health module**

Replace `games/tap-trading/backend/settlement-worker/src/health.rs` with:

```rust
//! HTTP surface — `/healthz` and `/metrics`.
//!
//! `/healthz`: 200 iff this process is the leader AND a tick has arrived in
//! the last 2 s. 503 otherwise. This is the alert-source for "settlement
//! worker silently broken" — Prometheus's blackbox prober pings this.
//!
//! `/metrics`: hand-rolled Prometheus text exposition. We avoid the
//! `prometheus` crate for now (one less dep) since v1 has three counters
//! and one histogram — sub-100-line surface.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};

use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Router,
};

#[derive(Default)]
pub struct Metrics {
    pub positions_settled_win: AtomicU64,
    pub positions_settled_loss: AtomicU64,
    pub positions_voided: AtomicU64,
    // crude histogram: bucketed at ms boundaries.
    pub tick_processing_ms_le_1: AtomicU64,
    pub tick_processing_ms_le_5: AtomicU64,
    pub tick_processing_ms_le_25: AtomicU64,
    pub tick_processing_ms_le_inf: AtomicU64,
}

impl Metrics {
    pub fn observe_tick_ms(&self, ms: u64) {
        if ms <= 1   { self.tick_processing_ms_le_1.fetch_add(1, Ordering::Relaxed); }
        if ms <= 5   { self.tick_processing_ms_le_5.fetch_add(1, Ordering::Relaxed); }
        if ms <= 25  { self.tick_processing_ms_le_25.fetch_add(1, Ordering::Relaxed); }
        self.tick_processing_ms_le_inf.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Clone)]
pub struct HealthState {
    pub is_leader: Arc<AtomicBool>,
    pub last_tick_received_ms: Arc<AtomicI64>,
    pub metrics: Arc<Metrics>,
}

const STALE_TICK_THRESHOLD_MS: i64 = 2_000;

pub fn router(state: HealthState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics))
        .with_state(state)
}

async fn healthz(State(state): State<HealthState>) -> impl IntoResponse {
    if !state.is_leader.load(Ordering::Relaxed) {
        return (StatusCode::SERVICE_UNAVAILABLE, "not leader").into_response();
    }
    let now_ms = chrono::Utc::now().timestamp_millis();
    let last = state.last_tick_received_ms.load(Ordering::Relaxed);
    if now_ms - last > STALE_TICK_THRESHOLD_MS {
        return (StatusCode::SERVICE_UNAVAILABLE, "tick stream stale").into_response();
    }
    (StatusCode::OK, "ok").into_response()
}

async fn metrics(State(state): State<HealthState>) -> impl IntoResponse {
    let m = &state.metrics;
    let win = m.positions_settled_win.load(Ordering::Relaxed);
    let loss = m.positions_settled_loss.load(Ordering::Relaxed);
    let void = m.positions_voided.load(Ordering::Relaxed);
    let b1 = m.tick_processing_ms_le_1.load(Ordering::Relaxed);
    let b5 = m.tick_processing_ms_le_5.load(Ordering::Relaxed);
    let b25 = m.tick_processing_ms_le_25.load(Ordering::Relaxed);
    let binf = m.tick_processing_ms_le_inf.load(Ordering::Relaxed);

    let body = format!(
        "# HELP positions_settled_total Position settlements by outcome.\n\
         # TYPE positions_settled_total counter\n\
         positions_settled_total{{outcome=\"W\"}} {win}\n\
         positions_settled_total{{outcome=\"L\"}} {loss}\n\
         positions_settled_total{{outcome=\"V\"}} {void}\n\
         # HELP positions_voided_total Total positions voided (alias of W/L/V V).\n\
         # TYPE positions_voided_total counter\n\
         positions_voided_total {void}\n\
         # HELP tick_processing_duration_ms Tick → settle latency, ms.\n\
         # TYPE tick_processing_duration_ms histogram\n\
         tick_processing_duration_ms_bucket{{le=\"1\"}} {b1}\n\
         tick_processing_duration_ms_bucket{{le=\"5\"}} {b5}\n\
         tick_processing_duration_ms_bucket{{le=\"25\"}} {b25}\n\
         tick_processing_duration_ms_bucket{{le=\"+Inf\"}} {binf}\n\
         tick_processing_duration_ms_count {binf}\n",
    );
    (StatusCode::OK, body).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn healthz_503_when_not_leader() {
        let state = HealthState {
            is_leader: Arc::new(AtomicBool::new(false)),
            last_tick_received_ms: Arc::new(AtomicI64::new(chrono::Utc::now().timestamp_millis())),
            metrics: Arc::new(Metrics::default()),
        };
        let resp = healthz(State(state)).await.into_response();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn healthz_503_when_tick_stale() {
        let state = HealthState {
            is_leader: Arc::new(AtomicBool::new(true)),
            last_tick_received_ms: Arc::new(AtomicI64::new(0)),
            metrics: Arc::new(Metrics::default()),
        };
        let resp = healthz(State(state)).await.into_response();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn healthz_200_when_leader_and_fresh() {
        let state = HealthState {
            is_leader: Arc::new(AtomicBool::new(true)),
            last_tick_received_ms: Arc::new(AtomicI64::new(chrono::Utc::now().timestamp_millis())),
            metrics: Arc::new(Metrics::default()),
        };
        let resp = healthz(State(state)).await.into_response();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
```

- [ ] **Step 12.2: Wire the health state through main.rs**

Edit `main.rs` to construct the `HealthState` and pass it to the router. The runtime now actually spawns the listen loop, the WS loop, and the periodic sweep — but per the surgical-changes rule we ship that wiring as part of THIS commit (it's required for `/healthz` to ever return 200).

Replace the body of `main()` with:

```rust
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let db_url = env("TAP_TRADING_DB_URL")?;
    let aggregator_ws = env("TAP_TRADING_AGGREGATOR_WS_URL")?;
    let http_port = env_port("TAP_TRADING_SETTLEMENT_WORKER_PORT")?;

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(10)
        .connect(&db_url)
        .await?;

    // Leader-acquire blocks until this process holds the lock.
    let _leader = tap_trading_settlement_worker::leader::LeaderGuard::acquire_or_wait(&db_url).await?;

    let cache = tap_trading_settlement_worker::cache::OpenPositionCache::new();
    cache.hydrate(&pool).await?;

    let is_leader = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let last_tick_received_ms = std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0));
    let metrics = std::sync::Arc::new(tap_trading_settlement_worker::health::Metrics::default());

    let ctx = tap_trading_settlement_worker::loop_runner::LoopContext {
        pool: pool.clone(),
        cache: cache.clone(),
        last_tick_received_ms: last_tick_received_ms.clone(),
        gap_tracker: std::sync::Arc::new(tap_trading_settlement_worker::loop_runner::GapTracker::default()),
    };

    // Background tasks. `tokio::spawn` returns a handle but we don't join —
    // the axum server below holds the process open; if any task panics
    // tokio surfaces it via the default panic handler.
    let listen_cache = cache.clone();
    let listen_pool = pool.clone();
    let listen_url = db_url.clone();
    tokio::spawn(async move {
        if let Err(e) = listen_cache.listen_loop(&listen_pool, &listen_url).await {
            tracing::error!(error = %e, "listen_loop exited");
        }
    });

    let ws_ctx = ctx.clone();
    tokio::spawn(async move {
        if let Err(e) = tap_trading_settlement_worker::loop_runner::run(ws_ctx, &aggregator_ws).await {
            tracing::error!(error = %e, "ws loop exited");
        }
    });

    let sweep_ctx = ctx.clone();
    tokio::spawn(async move {
        tap_trading_settlement_worker::loop_runner::periodic_sweep(sweep_ctx).await;
    });

    let health_state = tap_trading_settlement_worker::health::HealthState {
        is_leader,
        last_tick_received_ms,
        metrics,
    };

    let addr = SocketAddr::from(([0, 0, 0, 0], http_port));
    let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| {
        WorkerError::InvalidEnv {
            name: "TAP_TRADING_SETTLEMENT_WORKER_PORT",
            reason: e.to_string(),
        }
    })?;
    tracing::info!(%addr, "settlement worker http listening");

    axum::serve(listener, tap_trading_settlement_worker::health::router(health_state))
        .await
        .map_err(|e| WorkerError::InvalidEnv {
            name: "axum::serve",
            reason: e.to_string(),
        })?;
    Ok(())
```

Add `use std::sync::Arc;` and `use std::sync::atomic::AtomicI64;` etc. as needed at the top.

- [ ] **Step 12.3: Add `From<sqlx::Error>` paths**

`sqlx::postgres::PgPoolOptions::connect` returns `sqlx::Error`, which the `?` operator handles via the existing `From<sqlx::Error>` in `WorkerError::Db`.

- [ ] **Step 12.4: Run the unit tests**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-settlement-worker health::tests
```

Expected: 3 tests pass.

- [ ] **Step 12.5: Full crate verify**

```bash
cd games/tap-trading/backend && cargo check && cargo clippy --all-targets -- -D warnings && cargo test
```

Expected: green.

- [ ] **Step 12.6: Commit**

```bash
git add games/tap-trading/backend/settlement-worker/src/health.rs \
        games/tap-trading/backend/settlement-worker/src/main.rs
git commit -m "feat(tick-worker): expose prometheus metrics"
```

---

## Task 13 — Two-worker race integration test

The canonical test from `TESTING_STRATEGY.md §5.3`: two workers running, only one credits.

**Files:**
- Create: `games/tap-trading/backend/settlement-worker/tests/concurrency.rs`

- [ ] **Step 13.1: Write the race test**

Write `games/tap-trading/backend/settlement-worker/tests/concurrency.rs`:

```rust
//! Two-worker race: only one credits the same position. TESTING_STRATEGY §5.3.
//!
//! We don't spin two full binaries; we exercise the contract that protects
//! against double-credit:
//!   1. `pg_try_advisory_lock` is single-leader (only one acquires).
//!   2. `UNIQUE(position_id)` on `settlements` is the defense-in-depth.
//!
//! Together: even if leader election degenerates (a bug splits the brain),
//! the UNIQUE constraint prevents double-credit. This test pins both.

mod common;

use tap_trading_oracle_types::OracleTick;
use tap_trading_pricing_engine::AssetSymbol;
use tap_trading_settlement_worker::cache::PositionRef;
use tap_trading_settlement_worker::settle::settle_win;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_concurrent_settle_win_calls_yield_one_credit() {
    let db = common::setup_test_postgres().await;
    let acct = common::insert_account(&db.pool, "racer", 1_000).await;
    let pid = common::insert_open_position(&db.pool, acct, "BTC", 70_000.0, 70_010.0, 1_000, 6_000, 100, 2.5).await;

    let pos = PositionRef {
        id: pid, account_id: acct, asset: AssetSymbol::Btc,
        strike_lo: 70_000.0, strike_hi: 70_010.0,
        t_open_ms: 1_000, t_close_ms: 6_000,
        stake_points: 100, multiplier_at_tap: 2.5,
    };
    let tick = OracleTick {
        asset: AssetSymbol::Btc, run_id: 1, seq: 1, ts_ms: 3_000,
        mid: 70_010.0, vol_annualized: 0.80, source_count: 3,
    };

    // Fire two concurrent calls; only one should report `true` (credited).
    let pool_a = db.pool.clone();
    let pool_b = db.pool.clone();
    let pos_a = pos.clone();
    let pos_b = pos.clone();
    let tick_a = tick.clone();
    let tick_b = tick.clone();
    let (a, b) = tokio::join!(
        tokio::spawn(async move { settle_win(&pool_a, &pos_a, &tick_a).await }),
        tokio::spawn(async move { settle_win(&pool_b, &pos_b, &tick_b).await })
    );
    let a = a.expect("task a panicked").expect("a result");
    let b = b.expect("task b panicked").expect("b result");

    // Exactly one is true.
    assert!(a ^ b, "exactly one task credits: a={a} b={b}");

    // Exactly one ledger row + one settlement row, balance credited once.
    assert_eq!(common::count_settlements_for_position(&db.pool, pid).await, 1);
    assert_eq!(common::get_balance(&db.pool, acct).await, 1_000 + 250);
    let ledger_count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM points_ledger WHERE ref_id = $1 AND kind = 'TAP_PAYOUT'",
    )
    .bind(pid)
    .fetch_one(&db.pool)
    .await
    .expect("ledger count");
    assert_eq!(ledger_count.0, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn second_leader_blocked_while_first_holds() {
    // This re-states the leader_election test from a different angle —
    // proves the lock is the OUTER guard while UNIQUE is the inner one.
    let db = common::setup_test_postgres().await;
    let first = tap_trading_settlement_worker::leader::LeaderGuard::acquire_or_wait(&db.url)
        .await
        .expect("first acquires");
    let second = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        tap_trading_settlement_worker::leader::LeaderGuard::acquire_or_wait(&db.url),
    )
    .await;
    assert!(second.is_err(), "second leader must NOT acquire");
    drop(first);
}
```

- [ ] **Step 13.2: Run the test**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-settlement-worker --test concurrency
```

Expected: 2 tests pass.

Likely failure: `assert!(a ^ b)` flickers — both report `true` if the SQL race somehow yields two inserts. If that fires, inspect: the schema's `UNIQUE(position_id)` constraint is the load-bearing invariant; if both insert, the constraint is missing or the test setup didn't apply the migration.

- [ ] **Step 13.3: Full crate verify**

```bash
cd games/tap-trading/backend && cargo check && cargo clippy --all-targets -- -D warnings && cargo test
```

Expected: green.

- [ ] **Step 13.4: Commit**

```bash
git add games/tap-trading/backend/settlement-worker/tests/concurrency.rs
git commit -m "test(tick-worker): two-worker leader-race integration test"
```

---

## Final verification

- [ ] **Step F1: All 13 commits land on the current branch**

```bash
git log --oneline -13
```

Expected: 13 commits, newest at top, matching the Commit map subjects.

- [ ] **Step F2: Tick workspace clean across check, clippy, tests**

```bash
cd games/tap-trading/backend && cargo check && cargo clippy --all-targets -- -D warnings && cargo test
```

Expected: green. Test summary: ~25+ tests across `touch`, `loop_runner`, `health`, `sweep_tests` units plus the integration tests (`leader_election`, `cache_hydrate`, `listen_notify`, `win_settlement`, `loss_settlement`, `void_path`, `sweep`, `concurrency`). Wire-type round-trip coverage lives in Plan C's `tap-trading-oracle-types` crate.

- [ ] **Step F3: Root workspace still builds**

```bash
cd "$(git rev-parse --show-toplevel)" && cargo check --workspace && cargo test --workspace && cargo clippy --workspace -- -D warnings
```

Expected: same green baseline as before this plan started — root workspace is unchanged.

- [ ] **Step F4: Schema invariant — no OPEN row points at a settlement row**

Spin a hermetic Postgres, apply migrations, run the full suite once, then assert:

```bash
docker run --rm -d --name tt-verify -p 55432:5432 -e POSTGRES_PASSWORD=postgres postgres:16-alpine
sleep 5
PGPASSWORD=postgres psql -h 127.0.0.1 -p 55432 -U postgres -v ON_ERROR_STOP=1 \
  -f games/tap-trading/backend/migrations/20260523120000_create_tick_schema.sql
PGPASSWORD=postgres psql -h 127.0.0.1 -p 55432 -U postgres -c \
  "SELECT COUNT(*) FROM positions p JOIN settlements s ON s.position_id = p.id WHERE p.status = 'OPEN';"
docker rm -f tt-verify
```

Expected: count is `0`. We never write a settlement row for an OPEN position — every settle path transitions status away from OPEN in the same transaction.

- [ ] **Step F5: Coverage probe (no enforcement; informational only)**

`TESTING_STRATEGY.md §5.5` targets 85% line coverage. Run `cargo llvm-cov` if available:

```bash
which cargo-llvm-cov || cargo install cargo-llvm-cov --locked
cd games/tap-trading/backend && cargo llvm-cov --workspace --html
```

Expected: settlement-worker crate ≥ 85% lines covered. If under, add tests for whatever path the report flags. The plan's tests should comfortably exceed the target — pure logic (touch, sweep filter, health) is exhaustively unit-tested and every settle path has its own integration test.

- [ ] **Step F6: PR description — note the deviations and cross-plan deps**

For the PR description (no commit body per CLAUDE.md):

> **Plan D (`tap-trading-settlement-worker`) deviates from the user's brief in three places, each justified inline in the plan:**
>
> 1. VOID `oracle_price` uses `last_known_mid` (or `strike_lo` as cold-start fallback), not `0` — the schema declares `CHECK (oracle_price > 0)`. The cache tracks `last_known_mid` per asset.
> 2. WIN updates both `accounts.balance` AND `accounts.lifetime_points_won` per `SYSTEM_DESIGN.md §5.2`. The brief mentioned only `balance`. Refunds touch only `balance` — they're not wins.
> 3. `OracleStatus` has no `ts_ms` field per ADR-0008 §6 — the worker uses wall-clock `now_ms()` for `gap_start_ms` / `gap_end_ms`.
>
> **Advisory-lock constant:** `0x_7174_5365_7474_6c72_i64` (ASCII `"tSettlr"`). The brief's `0x71CC_5E77_1E_MENT_i64` was not valid hex.
>
> **Cross-plan deps:**
> - Plan A (pricing-engine `AssetSymbol`) — hard. Already shipped.
> - Plan B (`tap-trading-migrate`) — soft. The integration tests apply the migration SQL directly via `include_str!`; a future `tap-trading-migrate` library can swap in.
> - Plan C (`tap-trading-oracle-types`) — hard. This crate consumes the wire structs directly via `tap_trading_oracle_types::{OracleMessage, OracleTick, OracleStatus, OracleStreamState}`. Plan C must land before this plan executes.
> - Plan E (API) — hard for end-to-end. The API's `NOTIFY tap_new_position` is what populates the cache; without it, the worker only sees positions inserted via the test helpers.

---

## Plan E preview (not in scope here)

Plan E picks up where this leaves off:
- Build `tap-trading-api` (axum) with the tap-commit pipeline from ADR-0009 §4: idempotency check, cell validation, replay quote from aggregator's `GET /ring`, drift check (3% per ADR-0009 §4 step 6), atomic INSERT positions + ledger + balance debit + `NOTIFY tap_new_position`.
- Lazy-create accounts via the `X-Account-Id` middleware (ADR-0009 §1).
- Schema amendment for `positions.oracle_seq_at_tap`, `positions.oracle_run_id_at_tap`, `positions.client_request_id` (ADR-0009 §3 — Plan E's first task; lands on top of Plan A's chain).
- Read-only endpoints: `/me/state`, leaderboards, quest progress.
- Worktree dev-env wiring (`scripts/worktree-env.sh`, `sync-service-envs.sh`, `ensure-worktree-coherence.sh`, `mprocs.yaml`, `start-headless.sh`) for `TAP_TRADING_API_PORT`, `TAP_TRADING_SETTLEMENT_WORKER_PORT`, `TAP_TRADING_AGGREGATOR_WS_URL`. Per CLAUDE.md the five files move together.
