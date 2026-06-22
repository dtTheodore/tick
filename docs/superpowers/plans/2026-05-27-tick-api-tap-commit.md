# Tick API + Tap-Commit Pipeline — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `tap-trading-api` — the axum HTTP + WS service that owns account identity, the tap-commit pipeline, history reads, and re-broadcast of the oracle stream — plus the genesis-migration amendment that adds three columns `positions` needs (`oracle_seq_at_tap`, `oracle_run_id_at_tap`, `client_request_id`). After this plan lands, a client can POST a tap, the server idempotently validates + replays the quote + recomputes the multiplier + debits balance + writes a `positions` row + emits `NOTIFY tap_new_position`. Positions stay `OPEN` forever in tests (Plan D's settlement worker is not a prerequisite). The lock-at-tap invariant (`MATH_SPEC §4.3`) is enforced and tested.

**Architecture:** New member crate `games/tap-trading/backend/api/` (binary crate exposing `pub fn router() -> Router` so integration tests can mount the handlers without spawning a process). Axum 0.7 + tower middleware: outer `X-Account-Id` layer (lazy-creates the row), inner Redis token-bucket rate limiter scoped to authenticated routes. The aggregator client is a single struct holding a reqwest pool (HTTP `/ring`) and a long-lived tokio task (WS `/stream`) that re-broadcasts received `OracleMessage` frames over a `tokio::sync::broadcast` channel; each connected client gets a `Receiver` and forwards JSON frames to its own WS. Postgres I/O is `sqlx::PgPool`; idempotency is enforced by `UNIQUE (account_id, client_request_id)` + `INSERT … ON CONFLICT DO NOTHING RETURNING id`. Balance debit + position insert + ledger write + `NOTIFY` happen in one `BEGIN…COMMIT` with `SELECT … FOR UPDATE` on `accounts` to serialize concurrent taps for the same account.

**Tech Stack:** Rust 2021. `axum` 0.7, `tower` 0.4, `tower-http` 0.5 (trace, cors), `tokio` 1.x multi-thread, `sqlx` 0.8 (postgres, runtime-tokio, tls-rustls, uuid, chrono, macros), `redis` 0.25 (async tokio + connection-manager), `reqwest` 0.12 (json), `tokio-tungstenite` 0.23 (WS client to aggregator + tests), `uuid` 1 (v4 + serde), `serde` + `serde_json`, `thiserror` 1, `anyhow` 1 (CLI boundary only), `tracing` + `tracing-subscriber`, `prometheus` 0.13 (metrics text exposition), `clap` 4 (derive). Tests: `testcontainers` 0.20 (Postgres + Redis containers; CLAUDE.md mandate — never mock DB / Redis), `wiremock` 0.6 (mock aggregator HTTP), `tokio-tungstenite` server-side for the WS aggregator mock.

**Spec:**
- `docs/decisions/0008-tick-oracle-wire-protocol.md` — wire protocol consumed by the aggregator client (HTTP `/ring/:asset/:seq?run_id=N`, WS `/stream`).
- `docs/decisions/0009-tick-api-cross-service-contracts.md` — this service's binding contract: §1 `X-Account-Id` middleware, §2 table ownership, §3 schema delta (owned by this plan), §4 nine-step pipeline, §5 NOTIFY contract, §6 rate limit, §7 refund kind (worker-side), §8 payout formula.
- `games/tap-trading/docs/SYSTEM_DESIGN.md` — §2.1/§2.2/§2.4 schema shapes; §3 API surface; §3.3 tap-commit; §3.6 oracle read-through; §5.1 first-tap flow.
- `games/tap-trading/docs/MATH_SPEC.md §4.3` — lock-at-tap invariant.
- `games/tap-trading/docs/PRD.md` — stake tiers `{50, 100, 500, 1000}` (line 23); MVP-08 multi-tap-per-column + lock window.
- `games/tap-trading/docs/TESTING_STRATEGY.md §6` — API integration test contract (Testcontainers, concurrency, rate-limit, drift, balance).

**Spec deviations / corrections (record before writing code):**
- **Rate-limit burst.** `SYSTEM_DESIGN.md §3.3` step 2 says "10 taps/sec, burst 20". `ADR-0009 §6` says "10 taps/sec, burst 10". ADR-0009 is more recent and is the binding contract for cross-service surfaces — **implement burst = 10**. Open a doc PR against SYSTEM_DESIGN to align.
- **Tap-commit response body.** `SYSTEM_DESIGN.md §3.3` step 6 returns `{ position_id, multiplier_locked, expected_payout, t_open_ms, t_close_ms }`. `ADR-0009 §4` step 9 returns `{ position_id, multiplier_at_tap, status, t_close_ms }`. ADR-0009 is canonical — **implement ADR-0009's shape**. Clients compute `expected_payout = stake_points * multiplier_at_tap` themselves; `status` lets the idempotent-replay path return the (possibly already-settled) terminal state without a second round trip.
- **Schema delta in the genesis migration.** ADR-0009 §3 directs Plan E to amend the staged `migrations/20260523120000_create_tick_schema.sql` in place rather than ship a `_add_columns` follow-up, because Plan A's migration is staged on this same branch (`feat/tap-trading`) and not yet merged. The plan A docs (`docs/superpowers/plans/2026-05-23-…-pricing-engine.md`) describe an "eight separate migration files" structure that no longer matches the landed state — that's Plan A's debt, not ours. Task 1 below is that amendment.
- **Stake-tier validation lives in the app layer.** The DB keeps `stake_points > 0` (current CHECK) loose so Tier 2+ (adds `5000`) doesn't require a migration. The API rejects values outside `{50, 100, 500, 1000}` with `400 invalid_stake` (Tier 1, v1 — `PRD.md` line 23).
- **Strike-grid alignment validation is out of scope.** `SYSTEM_DESIGN.md §3.3` step 3 mentions per-asset grid spacing (Δ$0.5 ETH, Δ$10 BTC, Δ$0.1 SOL) but ADR-0009 §4 step 3 does not require it. Schema CHECKs (`strike_hi > strike_lo`, `strike_lo > 0`) are the only constraints. Grid alignment is a frontend/UX concern in v1; revisit if abuse surfaces.

**Post-execution deviations (from code review, recorded after the work landed):**
- **History cursor is `position_id`, not `created_at_ms`.** Task 12's row in the commit map below describes `cursor=<created_at_ms>`. Code review showed that strict-`<` keyset on `created_at_ms` silently drops rows when a tap burst lands several positions in the same millisecond (`now_ms()` is ms-resolution). The shipped implementation keyset-paginates by `id` (unique, monotonic with insertion); the cursor is opaque to clients and well-behaved clients echo back `next_cursor` unchanged.

**Verification baseline:** before starting, confirm
1. `cargo check && cargo test && cargo clippy --all-targets -- -D warnings` is green inside `games/tap-trading/backend/` (Plans A + B + C complete — Plan E hard-depends on Plan C's `tap-trading-oracle-types` crate. Plan E does **not** depend on Plan D, so positions stay OPEN in this plan's tests. Compile order is A → B → C, then D and E in either order — but to run the *closed* tap→settle loop end-to-end, Plan E's schema delta + `NOTIFY tap_new_position` must land before Plan D's end-to-end settlement test, because the worker settles positions the API creates).
2. `cargo check --workspace && cargo test --workspace && cargo clippy --workspace -- -D warnings` is green at repo root.
3. The migration file `games/tap-trading/backend/migrations/20260523120000_create_tick_schema.sql` exists and matches the layout summarized in ADR-0009 §3 (currently in `git status` as `A`, on branch `feat/tap-trading`).

After every commit in this plan, the Tick-workspace triple (check / test / clippy `--all-targets`) must stay green.

---

## Commit map

| # | Subject | Scope |
|---|---------|-------|
| 1 | `feat(tick-db): add positions oracle and idempotency cols` | Amend the genesis migration: add `positions.oracle_seq_at_tap`, `oracle_run_id_at_tap`, `client_request_id`, and `UNIQUE (account_id, client_request_id)` constraint `positions_dedup_request`. |
| 2 | `feat(tick-api): scaffold api bin crate` | New `api/` member; axum 0.7 skeleton; `/healthz` returns `{ "status": "ok" }`; `/metrics` stub; sqlx pool + redis pool from env; oracle wire types imported from `tap-trading-oracle-types` (Plan C). |
| 3 | `feat(tick-api): add x-account-id middleware` | Tower layer that validates header, lazy-creates account + SIGNUP ledger, attaches `AccountCtx`. Integration test: unknown header → exactly one new account row + one `SIGNUP` row; second call reuses. |
| 4 | `feat(tick-api): add get /v1/me endpoint` | Account summary handler. Integration test asserts shape including default 10_000 balance and streak fallback 0. |
| 5 | `feat(tick-api): add redis token-bucket rate limiter` | Lua EVAL token-bucket per `tap:rl:{account_id}` (capacity 10, refill 10/sec). Tower layer attaching `Retry-After` on 429. Integration test: 11th request in <1s gets 429. |
| 6 | `feat(tick-api): add aggregator http client` | `AggregatorClient::replay(asset, seq, run_id) -> Result<OracleTick, ReplayError>`. Wiremock-backed tests cover 200 / 410 / 409 / 404 / timeout. |
| 7 | `feat(tick-api): add cell validation and drift calc` | Pure functions: `validate_cell`, `validate_stake_tier`, `drift_exceeded`. Table-driven unit tests cover every error branch + boundary at exactly 3.0% / 3.000001%. |
| 8 | `feat(tick-api): add post /v1/positions happy path` | Wires steps 3 → 9 of ADR-0009 §4 (validate → replay → recompute → drift → balance FOR UPDATE → INSERT positions/ledger/balance → NOTIFY → 201). NO idempotency layer yet (added in commit 10) and NO rate limit (added by chaining commit 5's layer). Integration test: full flow against mocked aggregator + real Postgres. |
| 9 | `feat(tick-api): add post /v1/positions error paths` | `stale_quote` (410/409), `drift_exceeded`, `insufficient_balance`, `lock_window`, `invalid_stake`, `unknown_asset`, malformed JSON. Each gets one integration test. |
| 10 | `feat(tick-api): add idempotency layer to tap commit` | Pre-flight `SELECT … WHERE client_request_id = …` + `INSERT … ON CONFLICT (account_id, client_request_id) DO NOTHING RETURNING id` retry loop. Integration test: same `client_request_id` twice → identical response, ledger has one `TAP_STAKE`. |
| 11 | `feat(tick-api): emit notify on new position` | `NOTIFY tap_new_position, '<position_id>'` as the last statement of the commit transaction. Integration test: a `LISTEN`-side connection asserts the payload arrives within 1 second. |
| 12 | `feat(tick-api): add me history and position by id` | `GET /v1/me/history?limit=N&cursor=<created_at_ms>` with settlement LEFT JOIN; `GET /v1/positions/:id` with ownership check (403 not-owned). Integration tests for both, including the 403 path. |
| 13 | `feat(tick-api): add ws /stream re-broadcast` | Connect once to aggregator `WS /stream`; fanout to clients via `tokio::sync::broadcast::channel(256)`. Independent 5s ping to clients. Tests use a `tokio-tungstenite` server mock for the aggregator side. |
| 14 | `feat(tick-api): expose prometheus metrics` | `taps_committed_total`, `taps_rejected_total{reason}`, `tap_handler_duration_seconds`. `/metrics` returns text exposition. One integration test parses the scrape and asserts counters move. |
| 15 | `test(tick-api): concurrent taps balance invariant` | 100 simultaneous taps for the same account, balance = N·stake → exactly N succeed (FOR UPDATE serializes), N − 100 return 422 `insufficient_balance`, final balance = 0, ledger has exactly N `TAP_STAKE` rows. |
| 16 | `test(tick-api): lock-at-tap invariant` | Submit `client_multiplier` ≠ server's recompute but inside the 3% gate; tap succeeds; assert `positions.multiplier_at_tap` matches the test's independent recompute, not the client's claim (`MATH_SPEC §4.3`). |

Each commit must independently pass `cargo check && cargo test && cargo clippy --all-targets -- -D warnings` from inside `games/tap-trading/backend/`.

---

## File map

### Created files

| Path | Responsibility |
|------|----------------|
| `games/tap-trading/backend/api/Cargo.toml` | Crate metadata; declares `[lib]` (router exposed for tests) and `[[bin]]` (`tap-trading-api`). |
| `games/tap-trading/backend/api/src/main.rs` | Binary entrypoint: read env, build `AppState`, mount `router()`, `tokio::main`. |
| `games/tap-trading/backend/api/src/lib.rs` | `pub fn router(state: AppState) -> Router` so integration tests mount handlers without spawning a process. |
| `games/tap-trading/backend/api/src/state.rs` | `AppState { pg: PgPool, redis: ConnectionManager, aggregator: Arc<AggregatorClient>, broadcast: broadcast::Sender<String>, metrics: Arc<Metrics> }`. |
| `games/tap-trading/backend/api/src/error.rs` | `ApiError` enum + `IntoResponse` impl; canonical error JSON `{ error: "code", message: "..." }`. |
| `games/tap-trading/backend/api/src/account_ctx.rs` | `AccountCtx { id: i64, external_id: String }` request extension. |
| `games/tap-trading/backend/api/src/middleware/account_id.rs` | Tower layer validating `X-Account-Id`, lazy-creating account + SIGNUP ledger, attaching `AccountCtx`. |
| `games/tap-trading/backend/api/src/middleware/rate_limit.rs` | Redis token-bucket layer via Lua EVAL. Lua script embedded as a `&str` constant. |
| `games/tap-trading/backend/api/src/middleware/mod.rs` | Re-exports. |
| `games/tap-trading/backend/api/src/aggregator_client.rs` | HTTP `/ring` client + WS `/stream` subscriber task. Oracle wire types are imported from `tap-trading-oracle-types` (Plan C). |
| `games/tap-trading/backend/api/src/handlers/positions.rs` | `POST /v1/positions`, `GET /v1/positions/:id`. |
| `games/tap-trading/backend/api/src/handlers/me.rs` | `GET /v1/me`, `GET /v1/me/history`. |
| `games/tap-trading/backend/api/src/handlers/stream.rs` | `WS /stream` upgrade handler, broadcast `Receiver` per client. |
| `games/tap-trading/backend/api/src/handlers/health.rs` | `GET /healthz`, `GET /metrics`. |
| `games/tap-trading/backend/api/src/handlers/mod.rs` | Re-exports. |
| `games/tap-trading/backend/api/src/validation.rs` | `validate_cell`, `validate_stake_tier`, `drift_exceeded` — pure, table-driven tested. |
| `games/tap-trading/backend/api/src/db.rs` | sqlx queries: `find_position_by_request_id`, `insert_position_ledger_debit`, `select_balance_for_update`, `list_history`, `find_position_by_id`. |
| `games/tap-trading/backend/api/src/metrics.rs` | Prometheus counters/histograms registry. |
| `games/tap-trading/backend/api/src/now.rs` | `pub fn now_ms() -> i64`; tests can override via env (`TAP_TEST_NOW_MS`). |
| `games/tap-trading/backend/api/tests/common/mod.rs` | `TestApp` harness: starts Postgres + Redis containers, runs migrations via `tap_trading_migrate::run_migrations`, builds `AppState`, returns an in-process `axum::Router` exercised through `tower::ServiceExt::oneshot`. |
| `games/tap-trading/backend/api/tests/middleware_account_id.rs` | Lazy-create + reuse + invalid-header tests. |
| `games/tap-trading/backend/api/tests/rate_limit.rs` | 11th-in-a-second → 429; bucket isolated per account. |
| `games/tap-trading/backend/api/tests/post_positions_happy.rs` | One green-path integration test against mocked aggregator + real Postgres. |
| `games/tap-trading/backend/api/tests/post_positions_errors.rs` | Stale quote, drift, insufficient balance, lock window, invalid stake, unknown asset. |
| `games/tap-trading/backend/api/tests/idempotency.rs` | Same `client_request_id` twice → one ledger row, identical responses. |
| `games/tap-trading/backend/api/tests/notify.rs` | LISTEN-side connection asserts `tap_new_position` payload. |
| `games/tap-trading/backend/api/tests/get_me.rs` | `/v1/me` shape; default balance 10_000; streak default 0. |
| `games/tap-trading/backend/api/tests/get_history.rs` | History pagination + 403 not-owned position-by-id. |
| `games/tap-trading/backend/api/tests/ws_stream.rs` | Mock aggregator WS pushes 3 ticks + 1 heartbeat; client receives same. |
| `games/tap-trading/backend/api/tests/metrics.rs` | `/metrics` scrape: counters move on success / on drift rejection. |
| `games/tap-trading/backend/api/tests/concurrency.rs` | 100 concurrent taps, balance = N·stake, exactly N succeed; remaining return 422. |
| `games/tap-trading/backend/api/tests/lock_at_tap.rs` | `MATH_SPEC §4.3`: committed `multiplier_at_tap` matches server recompute. |

### Modified files

| Path | Change |
|------|--------|
| `games/tap-trading/backend/migrations/20260523120000_create_tick_schema.sql` | Add three `positions` columns + named UNIQUE constraint per ADR-0009 §3. **Edited in place** because Plan A's migration is staged on this same branch, not yet merged. |
| `games/tap-trading/backend/Cargo.toml` | Add `api` to `members`; add `axum`, `tower`, `tower-http`, `tokio-tungstenite`, `reqwest`, `redis`, `prometheus`, `uuid`, `tracing`, `tracing-subscriber`, `testcontainers`, `wiremock` to `[workspace.dependencies]`. |

---

## Pre-flight (one-time, not a commit)

- [ ] **Step P1: Verify Tick workspace baseline is green**

Run from repo root:

```bash
cd games/tap-trading/backend && cargo check && cargo test && cargo clippy --all-targets -- -D warnings
```

Expected: all three succeed with no warnings. Plan A (pricing engine) and Plan B (migrate runner) must be landed first.

- [ ] **Step P2: Verify root workspace baseline is green**

Run from repo root:

```bash
cargo check --workspace && cargo test --workspace && cargo clippy --workspace -- -D warnings
```

Expected: green.

- [ ] **Step P3: Verify the genesis migration file is present**

```bash
ls -la games/tap-trading/backend/migrations/
```

Expected: `20260523120000_create_tick_schema.sql` exists. If missing, Plan A is incomplete — stop.

- [ ] **Step P4: Confirm Postgres + Redis are reachable for Testcontainers**

```bash
docker info >/dev/null && echo "docker ok"
```

Expected: `docker ok`. Testcontainers spawns its own ephemeral Postgres + Redis per test process; no manual `docker compose up` is required for the test suite.

- [ ] **Step P5: Document Plan D non-dependency**

This plan does NOT require Plan D (settlement worker). In integration tests, positions stay `status='OPEN'` forever; settlement-side assertions (status flip, payout) are out of scope here and will be added by Plan D's tests (against this plan's committed positions). The history endpoint LEFT JOINs `settlements`; the absence of rows means tests assert `settlement: null`.

---

## Task 1 — Schema delta: positions oracle and idempotency cols

ADR-0009 §3 amendment. Plan A's `20260523120000_create_tick_schema.sql` is staged on this same branch and not yet merged, so this task **edits the file in place** rather than shipping a `_add_columns` follow-up. The amendment adds three columns to `positions` plus one UNIQUE constraint.

**Files:**
- Modify: `games/tap-trading/backend/migrations/20260523120000_create_tick_schema.sql`

- [ ] **Step 1.1: Edit the `positions` table definition**

Open `games/tap-trading/backend/migrations/20260523120000_create_tick_schema.sql`. Locate the `CREATE TABLE positions (...)` block. Insert three columns just before the `CHECK (...)` clauses (i.e. immediately after `created_at_ms BIGINT NOT NULL,`):

```sql
  oracle_seq_at_tap   BIGINT NOT NULL,
  oracle_run_id_at_tap BIGINT NOT NULL,
  client_request_id   UUID NOT NULL,
```

Then, immediately after the closing `)` and before the `;` that ends the statement, add a named UNIQUE constraint inside the table definition:

```sql
  CHECK (oracle_seq_at_tap >= 0),
  CONSTRAINT positions_dedup_request UNIQUE (account_id, client_request_id)
```

(Slot these alongside the existing `CHECK` clauses; the table-level constraint follows the same indentation as the surrounding `CHECK`s.)

The final `positions` block must list, in order: column defs (including the three new ones), CHECK clauses (including the new `oracle_seq_at_tap >= 0`), and the named UNIQUE.

Rationale: ADR-0009 §3 mandates `oracle_seq_at_tap`, `oracle_run_id_at_tap`, `client_request_id`. The `UNIQUE (account_id, client_request_id)` constraint is the *only* line of defense against duplicate taps from a retrying client — naming it (`positions_dedup_request`) makes the constraint-violation error code trappable from sqlx (`error.constraint() == Some("positions_dedup_request")`).

- [ ] **Step 1.2: Dry-run apply the amended migration**

```bash
source ./scripts/worktree-env.sh
docker compose up -d postgres
sleep 2
docker compose exec -T postgres psql -U dopamint -d dopamint -c "CREATE SCHEMA tick_dryrun; SET search_path TO tick_dryrun;"
docker compose exec -T postgres psql -U dopamint -d dopamint -v ON_ERROR_STOP=1 \
    -c "SET search_path TO tick_dryrun;" \
    -f /dev/stdin < games/tap-trading/backend/migrations/20260523120000_create_tick_schema.sql
docker compose exec -T postgres psql -U dopamint -d dopamint -c "SET search_path TO tick_dryrun; \d positions"
docker compose exec -T postgres psql -U dopamint -d dopamint -c "DROP SCHEMA tick_dryrun CASCADE;"
```

Expected: migration applies clean; `\d positions` shows the three new columns and the `positions_dedup_request` UNIQUE constraint.

- [ ] **Step 1.3: Verify duplicate rejection (manual SQL probe)**

Repeat the same throwaway-schema dance but actually exercise the constraint:

```bash
docker compose exec -T postgres psql -U dopamint -d dopamint -v ON_ERROR_STOP=1 <<'SQL'
CREATE SCHEMA tick_dryrun2;
SET search_path TO tick_dryrun2;
\i games/tap-trading/backend/migrations/20260523120000_create_tick_schema.sql

INSERT INTO accounts (external_id, zklogin_sub, zklogin_iss, balance, created_at_ms, last_active_ms)
  VALUES ('probe-1', 'dev', 'dev', 10000, 0, 0) RETURNING id;
-- First insert: succeeds.
INSERT INTO positions (account_id, asset, strike_lo, strike_hi, t_open_ms, t_close_ms,
  stake_points, multiplier_at_tap, oracle_seq_at_tap, oracle_run_id_at_tap,
  client_request_id, created_at_ms)
  VALUES (1, 'BTC', 1, 2, 0, 5000, 100, 1.5, 1, 1,
          '00000000-0000-0000-0000-000000000001'::uuid, 0);
-- Second insert with same (account_id, client_request_id): MUST fail.
INSERT INTO positions (account_id, asset, strike_lo, strike_hi, t_open_ms, t_close_ms,
  stake_points, multiplier_at_tap, oracle_seq_at_tap, oracle_run_id_at_tap,
  client_request_id, created_at_ms)
  VALUES (1, 'BTC', 1, 2, 0, 5000, 100, 1.5, 1, 1,
          '00000000-0000-0000-0000-000000000001'::uuid, 0);
SQL
```

Expected: the script exits non-zero with `duplicate key value violates unique constraint "positions_dedup_request"`. Clean up:

```bash
docker compose exec -T postgres psql -U dopamint -d dopamint -c "DROP SCHEMA tick_dryrun2 CASCADE;"
```

- [ ] **Step 1.4: Run the workspace test suite (Plan B already applies this migration via Testcontainers)**

```bash
cd games/tap-trading/backend && cargo test
```

Expected: green. Plan B's `tap-trading-migrate::tests` re-applies the migration inside a fresh Postgres container per test; the new schema must round-trip through `sqlx::migrate!`. If `tap-trading-migrate` tests fail, the column definitions are malformed — fix the SQL.

- [ ] **Step 1.5: Commit**

```bash
git add games/tap-trading/backend/migrations/20260523120000_create_tick_schema.sql
git commit -m "feat(tick-db): add positions oracle and idempotency cols"
```

---

## Task 2 — Scaffold api bin crate

New member `api/`. Library + binary so integration tests mount the router in-process.

**Files:**
- Modify: `games/tap-trading/backend/Cargo.toml`
- Create: `games/tap-trading/backend/api/Cargo.toml`
- Create: `games/tap-trading/backend/api/src/main.rs`
- Create: `games/tap-trading/backend/api/src/lib.rs`
- Create: `games/tap-trading/backend/api/src/state.rs`
- Create: `games/tap-trading/backend/api/src/error.rs`
- Create: `games/tap-trading/backend/api/src/handlers/mod.rs`
- Create: `games/tap-trading/backend/api/src/handlers/health.rs`
- Create: `games/tap-trading/backend/api/src/now.rs`
- Create: `games/tap-trading/backend/api/src/aggregator_client.rs`

- [ ] **Step 2.1: Register `api` in the workspace + add deps**

Edit `games/tap-trading/backend/Cargo.toml`. Change `members` to:

```toml
members = [
    "pricing-engine",
    "migrate",
    "api",
]
```

Add to `[workspace.dependencies]`:

```toml
axum = { version = "0.7", features = ["ws", "macros", "tokio"] }
tower = { version = "0.4", features = ["util"] }
tower-http = { version = "0.5", features = ["trace", "cors"] }
tokio-tungstenite = { version = "0.23", features = ["rustls-tls-webpki-roots"] }
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
redis = { version = "0.25", features = ["tokio-comp", "connection-manager"] }
prometheus = { version = "0.13", default-features = false }
uuid = { version = "1", features = ["v4", "serde"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
testcontainers = "0.20"
wiremock = "0.6"
futures-util = "0.3"
```

- [ ] **Step 2.2: Write the crate manifest**

Write `games/tap-trading/backend/api/Cargo.toml`:

```toml
[package]
name = "tap-trading-api"
version = "0.0.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true
publish.workspace = true

[lib]
path = "src/lib.rs"

[[bin]]
name = "tap-trading-api"
path = "src/main.rs"

[dependencies]
tap-trading-pricing-engine = { path = "../pricing-engine" }
tap-trading-migrate = { path = "../migrate" }
tap-trading-oracle-types = { path = "../oracle-types" }

axum = { workspace = true }
tower = { workspace = true }
tower-http = { workspace = true }
tokio = { workspace = true, features = ["full"] }
tokio-tungstenite = { workspace = true }
reqwest = { workspace = true }
redis = { workspace = true }
sqlx = { workspace = true, features = ["postgres", "runtime-tokio", "tls-rustls", "uuid", "chrono", "macros"] }
serde = { workspace = true }
serde_json = { workspace = true }
uuid = { workspace = true }
thiserror = { workspace = true }
anyhow = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
prometheus = { workspace = true }
clap = { workspace = true }
futures-util = { workspace = true }

[dev-dependencies]
testcontainers = { workspace = true }
wiremock = { workspace = true }
```

- [ ] **Step 2.3: Write `now.rs`**

Write `games/tap-trading/backend/api/src/now.rs`:

```rust
//! Wall-clock helper. Test override via `TAP_TEST_NOW_MS`.

use std::time::{SystemTime, UNIX_EPOCH};

/// Epoch milliseconds. Tests can pin time via `TAP_TEST_NOW_MS=<ms>`.
pub fn now_ms() -> i64 {
    if let Ok(v) = std::env::var("TAP_TEST_NOW_MS") {
        if let Ok(n) = v.parse::<i64>() {
            return n;
        }
    }
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
```

- [ ] **Step 2.4: Write `error.rs`**

Write `games/tap-trading/backend/api/src/error.rs`:

```rust
//! Canonical API error type. All handlers return `Result<T, ApiError>`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("missing_account_id")]
    MissingAccountId,
    #[error("invalid_account_id")]
    InvalidAccountId,
    #[error("invalid_stake")]
    InvalidStake,
    #[error("unknown_asset")]
    UnknownAsset,
    #[error("lock_window")]
    LockWindow,
    #[error("invalid_cell")]
    InvalidCell,
    #[error("stale_quote")]
    StaleQuote,
    #[error("drift_exceeded")]
    DriftExceeded { server_multiplier: f64 },
    #[error("insufficient_balance")]
    InsufficientBalance,
    #[error("rate_limited")]
    RateLimited { retry_after_secs: u64 },
    #[error("not_found")]
    NotFound,
    #[error("forbidden")]
    Forbidden,
    #[error("internal")]
    Internal(#[from] anyhow::Error),
    #[error("db")]
    Db(#[from] sqlx::Error),
    #[error("redis")]
    Redis(#[from] redis::RedisError),
}

#[derive(Serialize)]
struct ErrBody<'a> {
    error: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    server_multiplier: Option<f64>,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, body, retry_after) = match &self {
            ApiError::MissingAccountId => (StatusCode::UNAUTHORIZED, "missing_account_id", None),
            ApiError::InvalidAccountId => (StatusCode::BAD_REQUEST, "invalid_account_id", None),
            ApiError::InvalidStake => (StatusCode::BAD_REQUEST, "invalid_stake", None),
            ApiError::UnknownAsset => (StatusCode::BAD_REQUEST, "unknown_asset", None),
            ApiError::LockWindow => (StatusCode::BAD_REQUEST, "lock_window", None),
            ApiError::InvalidCell => (StatusCode::BAD_REQUEST, "invalid_cell", None),
            ApiError::StaleQuote => (StatusCode::UNPROCESSABLE_ENTITY, "stale_quote", None),
            ApiError::DriftExceeded { .. } => (StatusCode::UNPROCESSABLE_ENTITY, "drift_exceeded", None),
            ApiError::InsufficientBalance => (StatusCode::UNPROCESSABLE_ENTITY, "insufficient_balance", None),
            ApiError::RateLimited { retry_after_secs } => (StatusCode::TOO_MANY_REQUESTS, "rate_limited", Some(*retry_after_secs)),
            ApiError::NotFound => (StatusCode::NOT_FOUND, "not_found", None),
            ApiError::Forbidden => (StatusCode::FORBIDDEN, "forbidden", None),
            ApiError::Internal(_) | ApiError::Db(_) | ApiError::Redis(_) => {
                tracing::error!(error = ?self, "internal error");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal", None)
            }
        };
        let server_multiplier = match &self {
            ApiError::DriftExceeded { server_multiplier } => Some(*server_multiplier),
            _ => None,
        };
        let mut resp = (status, Json(ErrBody { error: body, server_multiplier })).into_response();
        if let Some(secs) = retry_after {
            resp.headers_mut().insert("retry-after", secs.to_string().parse().unwrap());
        }
        resp
    }
}
```

- [ ] **Step 2.5: Write `aggregator_client.rs`**

Wire types come from `tap-trading-oracle-types` (Plan C). This file owns only client logic — the reqwest pool, the `ReplayError` taxonomy, and the `replay` stub. Real bodies land in Task 6 (HTTP) and Task 13 (WS).

Write `games/tap-trading/backend/api/src/aggregator_client.rs`:

```rust
//! Aggregator HTTP + WS client.
//!
//! Wire types (`OracleTick`, `OracleStatus`, `OracleStreamState`,
//! `OracleMessage`, `AssetSymbol`) are imported from `tap-trading-oracle-types`
//! (Plan C). Real bodies land in Task 6 (HTTP) and Task 13 (WS); this file
//! ships the client skeleton so the crate builds.

use tap_trading_oracle_types::{AssetSymbol, OracleTick};

#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    #[error("stale")]    // 410 or 409
    Stale,
    #[error("unknown_asset")] // 404
    UnknownAsset,
    #[error("transport")]
    Transport(#[from] reqwest::Error),
    #[error("decode")]
    Decode(#[from] serde_json::Error),
}

pub struct AggregatorClient {
    base_url: String,
    http: reqwest::Client,
}

impl AggregatorClient {
    pub fn new(base_url: String) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_millis(500))
                .build()
                .expect("reqwest client builder"),
            base_url,
        }
    }

    /// Real body lands in Task 6.
    pub async fn replay(
        &self,
        _asset: AssetSymbol,
        _seq: u64,
        _run_id: u64,
    ) -> Result<OracleTick, ReplayError> {
        unimplemented!("Task 6.3")
    }
}
```

- [ ] **Step 2.6: Write `state.rs`**

Write `games/tap-trading/backend/api/src/state.rs`:

```rust
//! Shared handler state. Cloneable via `Arc`s inside; tests construct directly.

use std::sync::Arc;
use sqlx::PgPool;
use tokio::sync::broadcast;

use crate::aggregator_client::AggregatorClient;

#[derive(Clone)]
pub struct AppState {
    pub pg: PgPool,
    pub redis: redis::aio::ConnectionManager,
    pub aggregator: Arc<AggregatorClient>,
    pub broadcast: broadcast::Sender<String>,
}
```

- [ ] **Step 2.7: Write `handlers/health.rs`**

Write `games/tap-trading/backend/api/src/handlers/health.rs`:

```rust
//! Liveness + metrics. `/metrics` is wired to a real registry in Task 14.

use axum::Json;
use serde_json::json;

pub async fn healthz() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok" }))
}

pub async fn metrics() -> String {
    // Stub. Real exposition lands in Task 14.
    String::new()
}
```

Write `games/tap-trading/backend/api/src/handlers/mod.rs`:

```rust
pub mod health;
```

- [ ] **Step 2.8: Write `lib.rs` (router)**

Write `games/tap-trading/backend/api/src/lib.rs`:

```rust
//! Tick API service — library entry point.
//!
//! Spec: `docs/decisions/0009-tick-api-cross-service-contracts.md`,
//! `games/tap-trading/docs/SYSTEM_DESIGN.md §3`.

pub mod aggregator_client;
pub mod error;
pub mod handlers;
pub mod now;
pub mod state;

use axum::{routing::get, Router};

pub use state::AppState;

/// Build the router. Middleware and most routes are added by later tasks.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(handlers::health::healthz))
        .route("/metrics", get(handlers::health::metrics))
        .with_state(state)
}
```

- [ ] **Step 2.9: Write `main.rs`**

Write `games/tap-trading/backend/api/src/main.rs`:

```rust
//! `tap-trading-api` binary.

use std::sync::Arc;
use anyhow::{anyhow, Result};
use sqlx::postgres::PgPoolOptions;
use tap_trading_api::{router, AppState};
use tap_trading_api::aggregator_client::AggregatorClient;
use tokio::sync::broadcast;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .json()
        .init();

    let database_url = std::env::var("DATABASE_URL")
        .map_err(|_| anyhow!("DATABASE_URL is required"))?;
    let redis_url = std::env::var("REDIS_URL")
        .map_err(|_| anyhow!("REDIS_URL is required"))?;
    let aggregator_url = std::env::var("TAP_AGGREGATOR_URL")
        .map_err(|_| anyhow!("TAP_AGGREGATOR_URL is required"))?;
    let port = std::env::var("TAP_API_PORT")
        .map_err(|_| anyhow!("TAP_API_PORT is required"))?
        .parse::<u16>()
        .map_err(|e| anyhow!("TAP_API_PORT invalid: {e}"))?;

    let pg = PgPoolOptions::new()
        .max_connections(20)
        .connect(&database_url)
        .await?;
    let redis_client = redis::Client::open(redis_url)?;
    let redis = redis::aio::ConnectionManager::new(redis_client).await?;
    let (broadcast_tx, _) = broadcast::channel(256);

    let state = AppState {
        pg,
        redis,
        aggregator: Arc::new(AggregatorClient::new(aggregator_url)),
        broadcast: broadcast_tx,
    };

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!(port, "tap-trading-api listening");
    axum::serve(listener, router(state)).await?;
    Ok(())
}
```

- [ ] **Step 2.10: Verify it builds + healthz returns ok**

```bash
cd games/tap-trading/backend && cargo check && cargo clippy --all-targets -- -D warnings && cargo test -p tap-trading-api
```

Expected: clean compile (no tests yet); clippy passes. `cargo test -p tap-trading-api` runs zero tests and exits 0.

- [ ] **Step 2.11: Commit**

```bash
git add games/tap-trading/backend/Cargo.toml \
        games/tap-trading/backend/api/
git commit -m "feat(tick-api): scaffold api bin crate"
```

---

## Task 3 — X-Account-Id middleware + lazy-create

ADR-0009 §1. A tower layer validates the header, looks up `accounts.external_id`, lazy-creates the row + SIGNUP ledger entry if unknown, and attaches `AccountCtx { id, external_id }` as a request extension.

**Files:**
- Create: `games/tap-trading/backend/api/src/account_ctx.rs`
- Create: `games/tap-trading/backend/api/src/middleware/mod.rs`
- Create: `games/tap-trading/backend/api/src/middleware/account_id.rs`
- Create: `games/tap-trading/backend/api/tests/common/mod.rs`
- Create: `games/tap-trading/backend/api/tests/middleware_account_id.rs`
- Modify: `games/tap-trading/backend/api/src/lib.rs`

- [ ] **Step 3.1: Write `account_ctx.rs`**

```rust
//! Authenticated request context. Attached by `account_id` middleware.

#[derive(Debug, Clone)]
pub struct AccountCtx {
    pub id: i64,
    pub external_id: String,
}
```

- [ ] **Step 3.2: Write `middleware/account_id.rs`**

Write `games/tap-trading/backend/api/src/middleware/account_id.rs`:

```rust
//! `X-Account-Id` middleware. ADR-0009 §1.
//!
//! Validates the header, lazy-creates the account row (with a one-time SIGNUP
//! ledger entry of +10_000 points), attaches `AccountCtx` to request extensions.

use axum::extract::{Request, State};
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::response::Response;

use crate::account_ctx::AccountCtx;
use crate::error::ApiError;
use crate::now::now_ms;
use crate::state::AppState;

const HEADER: &str = "x-account-id";
const MAX_LEN: usize = 128;
const SIGNUP_BONUS: i64 = 10_000;

pub async fn account_id_middleware(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let value = headers
        .get(HEADER)
        .ok_or(ApiError::MissingAccountId)?
        .to_str()
        .map_err(|_| ApiError::InvalidAccountId)?;
    if value.is_empty() || value.len() > MAX_LEN {
        return Err(ApiError::InvalidAccountId);
    }
    let ctx = lookup_or_create(&state, value).await?;
    req.extensions_mut().insert(ctx);
    Ok(next.run(req).await)
}

async fn lookup_or_create(state: &AppState, external_id: &str) -> Result<AccountCtx, ApiError> {
    let now = now_ms();
    let mut tx = state.pg.begin().await?;

    let inserted: Option<(i64,)> = sqlx::query_as(
        r#"
        INSERT INTO accounts
          (external_id, zklogin_sub, zklogin_iss, balance, lifetime_points_won,
           signup_bonus_at_ms, created_at_ms, last_active_ms)
        VALUES ($1, 'dev', 'dev', $2, 0, $3, $3, $3)
        ON CONFLICT (external_id) DO NOTHING
        RETURNING id
        "#,
    )
    .bind(external_id)
    .bind(SIGNUP_BONUS)
    .bind(now)
    .fetch_optional(&mut *tx)
    .await?;

    let id = if let Some((id,)) = inserted {
        sqlx::query(
            r#"INSERT INTO points_ledger (account_id, kind, delta, ref_id, created_at_ms)
               VALUES ($1, 'SIGNUP', $2, NULL, $3)"#,
        )
        .bind(id)
        .bind(SIGNUP_BONUS)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        id
    } else {
        let (id,): (i64,) = sqlx::query_as(
            "SELECT id FROM accounts WHERE external_id = $1",
        )
        .bind(external_id)
        .fetch_one(&mut *tx)
        .await?;
        id
    };
    tx.commit().await?;
    Ok(AccountCtx { id, external_id: external_id.to_string() })
}
```

The transaction is short — `INSERT … ON CONFLICT DO NOTHING RETURNING id` is one round-trip; if it returns a row we know we created it, so the SIGNUP ledger row is safe to insert. The fallback `SELECT id` runs only when the account already existed. Two concurrent requests for the same unknown header both attempt the INSERT; exactly one wins and writes the SIGNUP row; the loser falls through to the SELECT and reads the winner's id.

- [ ] **Step 3.3: Write `middleware/mod.rs`**

```rust
pub mod account_id;
```

- [ ] **Step 3.4: Wire the middleware into `lib.rs`**

Edit `games/tap-trading/backend/api/src/lib.rs`. Add `pub mod account_ctx;` and `pub mod middleware;` to the module list. Replace the body of `router` with two `Router`s — `public` (healthz, metrics — no middleware) and `authenticated` (everything else, wrapped in the middleware):

```rust
pub fn router(state: AppState) -> Router {
    use axum::middleware::from_fn_with_state;
    let public = Router::new()
        .route("/healthz", get(handlers::health::healthz))
        .route("/metrics", get(handlers::health::metrics));
    let authenticated = Router::new()
        // routes land here in Tasks 4, 8, 12, 13
        .layer(from_fn_with_state(state.clone(), middleware::account_id::account_id_middleware));
    public.merge(authenticated).with_state(state)
}
```

- [ ] **Step 3.5: Write the `TestApp` harness**

Write `games/tap-trading/backend/api/tests/common/mod.rs`:

```rust
//! In-process test harness. Boots Postgres + Redis containers, runs migrations,
//! returns an `axum::Router` exercised via `tower::ServiceExt::oneshot`.

#![allow(dead_code)] // members are added as tests grow

use std::sync::Arc;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tap_trading_api::aggregator_client::AggregatorClient;
use tap_trading_api::AppState;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use tokio::sync::broadcast;

pub struct TestApp {
    pub pg: PgPool,
    pub state: AppState,
    pub router: axum::Router,
    // Hold container handles so they stay alive for the test's lifetime.
    _pg_container: ContainerAsync<GenericImage>,
    _redis_container: ContainerAsync<GenericImage>,
}

impl TestApp {
    pub async fn start() -> Self {
        Self::start_with(|_state| {}).await
    }

    /// Allow per-test state customization (e.g. swap aggregator base URL).
    pub async fn start_with<F: FnOnce(&mut AppState)>(customize: F) -> Self {
        let pg_container = GenericImage::new("postgres", "16-alpine")
            .with_wait_for(WaitFor::message_on_stderr("database system is ready"))
            .with_env_var("POSTGRES_USER", "test")
            .with_env_var("POSTGRES_PASSWORD", "test")
            .with_env_var("POSTGRES_DB", "test")
            .with_exposed_port(5432.tcp())
            .start()
            .await
            .expect("postgres container");
        let pg_host = pg_container.get_host().await.unwrap();
        let pg_port = pg_container.get_host_port_ipv4(5432).await.unwrap();
        let pg_url = format!("postgres://test:test@{pg_host}:{pg_port}/test");

        let redis_container = GenericImage::new("redis", "7-alpine")
            .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
            .with_exposed_port(6379.tcp())
            .start()
            .await
            .expect("redis container");
        let redis_host = redis_container.get_host().await.unwrap();
        let redis_port = redis_container.get_host_port_ipv4(6379).await.unwrap();
        let redis_url = format!("redis://{redis_host}:{redis_port}");

        let pg = PgPoolOptions::new()
            .max_connections(10)
            .connect(&pg_url)
            .await
            .expect("connect pg");
        tap_trading_migrate::run_migrations(&pg).await.expect("migrations");

        let redis_client = redis::Client::open(redis_url).unwrap();
        let redis = redis::aio::ConnectionManager::new(redis_client).await.unwrap();
        let (broadcast_tx, _) = broadcast::channel(256);

        // Default aggregator base URL: nonsense. Tests that exercise the
        // aggregator path override it via `start_with`.
        let mut state = AppState {
            pg: pg.clone(),
            redis,
            aggregator: Arc::new(AggregatorClient::new("http://127.0.0.1:1".to_string())),
            broadcast: broadcast_tx,
        };
        customize(&mut state);
        let router = tap_trading_api::router(state.clone());

        Self {
            pg,
            state,
            router,
            _pg_container: pg_container,
            _redis_container: redis_container,
        }
    }
}
```

- [ ] **Step 3.6: Write the middleware integration tests**

Write `games/tap-trading/backend/api/tests/middleware_account_id.rs`:

```rust
mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::TestApp;
use tower::util::ServiceExt;

#[tokio::test]
async fn missing_header_returns_401() {
    let app = TestApp::start().await;
    // /v1/me doesn't exist yet (Task 4) — use the temporary anchor route from
    // Task 3.4. We hit a route that requires the middleware by adding a debug
    // ping route in Task 3.7 below.
    let resp = app.router.clone().oneshot(
        Request::builder().uri("/v1/ping").body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn unknown_header_lazy_creates_account_and_signup_ledger() {
    let app = TestApp::start().await;
    let resp = app.router.clone().oneshot(
        Request::builder()
            .uri("/v1/ping")
            .header("x-account-id", "brand-new-user")
            .body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let (count_accounts,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM accounts")
        .fetch_one(&app.pg).await.unwrap();
    assert_eq!(count_accounts, 1);
    let (balance,): (i64,) = sqlx::query_as("SELECT balance FROM accounts WHERE external_id = $1")
        .bind("brand-new-user").fetch_one(&app.pg).await.unwrap();
    assert_eq!(balance, 10_000);
    let (count_signup,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM points_ledger WHERE kind = 'SIGNUP'"
    ).fetch_one(&app.pg).await.unwrap();
    assert_eq!(count_signup, 1);
}

#[tokio::test]
async fn second_call_reuses_account() {
    let app = TestApp::start().await;
    for _ in 0..3 {
        let _ = app.router.clone().oneshot(
            Request::builder()
                .uri("/v1/ping")
                .header("x-account-id", "repeat-user")
                .body(Body::empty()).unwrap()
        ).await.unwrap();
    }
    let (count_accounts,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM accounts")
        .fetch_one(&app.pg).await.unwrap();
    assert_eq!(count_accounts, 1);
    let (count_signup,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM points_ledger WHERE kind = 'SIGNUP'"
    ).fetch_one(&app.pg).await.unwrap();
    assert_eq!(count_signup, 1);
}

#[tokio::test]
async fn empty_header_returns_400() {
    let app = TestApp::start().await;
    let resp = app.router.clone().oneshot(
        Request::builder()
            .uri("/v1/ping")
            .header("x-account-id", "")
            .body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn too_long_header_returns_400() {
    let app = TestApp::start().await;
    let huge = "x".repeat(200);
    let resp = app.router.clone().oneshot(
        Request::builder()
            .uri("/v1/ping")
            .header("x-account-id", huge)
            .body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
```

- [ ] **Step 3.7: Add a `/v1/ping` debug route so the middleware has something to wrap**

This route is the bare minimum needed for commit 3's tests to pass without depending on Task 4's `/v1/me`. It also satisfies clippy's `dead_code` on the middleware function. The route is replaced by real authenticated routes in later tasks — keep it for the duration of this plan; it does no harm and the `/v1/ping` namespace doesn't collide with anything.

Edit `games/tap-trading/backend/api/src/handlers/health.rs`, add:

```rust
use axum::Extension;
use crate::account_ctx::AccountCtx;

pub async fn ping(Extension(_ctx): Extension<AccountCtx>) -> &'static str {
    "pong"
}
```

Edit `lib.rs` `authenticated` router to mount it:

```rust
let authenticated = Router::new()
    .route("/v1/ping", get(handlers::health::ping))
    .layer(from_fn_with_state(state.clone(), middleware::account_id::account_id_middleware));
```

- [ ] **Step 3.8: Run the tests**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-api --test middleware_account_id
```

Expected: 5 tests pass. The container boot adds ~5 s overhead per test process; Postgres + Redis pull is one-time.

- [ ] **Step 3.9: Full verify**

```bash
cd games/tap-trading/backend && cargo check && cargo clippy --all-targets -- -D warnings && cargo test
```

Expected: green across the workspace.

- [ ] **Step 3.10: Commit**

```bash
git add games/tap-trading/backend/api/
git commit -m "feat(tick-api): add x-account-id middleware"
```

---

## Task 4 — `GET /v1/me` endpoint

ADR-0009 §1 attached `AccountCtx`; this handler reads it and returns the account summary. Streak comes from the `streaks` row if present, else 0.

**Files:**
- Create: `games/tap-trading/backend/api/src/handlers/me.rs`
- Create: `games/tap-trading/backend/api/tests/get_me.rs`
- Modify: `games/tap-trading/backend/api/src/handlers/mod.rs`
- Modify: `games/tap-trading/backend/api/src/lib.rs`

- [ ] **Step 4.1: Write the handler**

Write `games/tap-trading/backend/api/src/handlers/me.rs`:

```rust
//! `GET /v1/me`. ADR-0009 §1 supplies the `AccountCtx`; we read the row.

use axum::extract::State;
use axum::{Extension, Json};
use serde::Serialize;
use sqlx::FromRow;

use crate::account_ctx::AccountCtx;
use crate::error::ApiError;
use crate::state::AppState;

#[derive(Debug, Serialize, FromRow)]
pub struct MeResponse {
    pub account_id: i64,
    pub external_id: String,
    pub balance: i64,
    pub lifetime_points_won: i64,
    pub tier: i16,
    pub current_streak: i32,
}

pub async fn get_me(
    State(state): State<AppState>,
    Extension(ctx): Extension<AccountCtx>,
) -> Result<Json<MeResponse>, ApiError> {
    let row = sqlx::query_as::<_, (i64, i64, i16)>(
        "SELECT balance, lifetime_points_won, tier FROM accounts WHERE id = $1"
    )
    .bind(ctx.id)
    .fetch_one(&state.pg)
    .await?;
    let streak: Option<(i32,)> = sqlx::query_as(
        "SELECT current_streak FROM streaks WHERE account_id = $1"
    )
    .bind(ctx.id)
    .fetch_optional(&state.pg)
    .await?;
    Ok(Json(MeResponse {
        account_id: ctx.id,
        external_id: ctx.external_id,
        balance: row.0,
        lifetime_points_won: row.1,
        tier: row.2,
        current_streak: streak.map(|s| s.0).unwrap_or(0),
    }))
}
```

- [ ] **Step 4.2: Wire the route**

Edit `games/tap-trading/backend/api/src/handlers/mod.rs`, add `pub mod me;`.

Edit `lib.rs`, add to the `authenticated` Router:

```rust
.route("/v1/me", get(handlers::me::get_me))
```

- [ ] **Step 4.3: Write the integration test**

Write `games/tap-trading/backend/api/tests/get_me.rs`:

```rust
mod common;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use common::TestApp;
use tower::util::ServiceExt;

#[tokio::test]
async fn returns_default_state_for_new_account() {
    let app = TestApp::start().await;
    let resp = app.router.clone().oneshot(
        Request::builder()
            .uri("/v1/me")
            .header("x-account-id", "new-me")
            .body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 8 * 1024).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["external_id"], "new-me");
    assert_eq!(v["balance"], 10_000);
    assert_eq!(v["lifetime_points_won"], 0);
    assert_eq!(v["tier"], 1);
    assert_eq!(v["current_streak"], 0);
}
```

- [ ] **Step 4.4: Run + verify**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-api --test get_me
cargo clippy --all-targets -- -D warnings
```

Expected: green.

- [ ] **Step 4.5: Commit**

```bash
git add games/tap-trading/backend/api/
git commit -m "feat(tick-api): add get /v1/me endpoint"
```

---

## Task 5 — Redis token-bucket rate limiter

ADR-0009 §6. Capacity 10, refill 10/sec linear (so 0.1 s per token). Key `tap:rl:{account_id}`, TTL 2 s. Atomic via Lua EVAL.

**Files:**
- Create: `games/tap-trading/backend/api/src/middleware/rate_limit.rs`
- Create: `games/tap-trading/backend/api/tests/rate_limit.rs`
- Modify: `games/tap-trading/backend/api/src/middleware/mod.rs`

- [ ] **Step 5.1: Write the rate limiter**

Write `games/tap-trading/backend/api/src/middleware/rate_limit.rs`:

```rust
//! Per-account token-bucket rate limiter. ADR-0009 §6.
//!
//! Bucket: hash `tap:rl:{account_id}` with fields `tokens` (f64) and `ts_ms`
//! (i64). Capacity 10. Refill 10 tokens/sec linear (1 token / 100 ms).
//! On each call: refill = (now_ms - ts_ms) * 10 / 1000; new = min(cap, old + refill);
//! if new >= 1 → consume 1, write back, allow; else → reject with the
//! milliseconds-until-1 as `Retry-After` (ceil to seconds, minimum 1).

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;

use crate::account_ctx::AccountCtx;
use crate::error::ApiError;
use crate::now::now_ms;
use crate::state::AppState;

const CAP: f64 = 10.0;
const REFILL_PER_SEC: f64 = 10.0;
const TTL_SECS: u64 = 2;

// KEYS[1] = bucket key, ARGV[1] = now_ms, ARGV[2] = cap, ARGV[3] = refill_per_sec, ARGV[4] = ttl
// Returns: { allowed (0|1), retry_after_ms }
const SCRIPT: &str = r#"
local key = KEYS[1]
local now_ms = tonumber(ARGV[1])
local cap = tonumber(ARGV[2])
local refill_per_sec = tonumber(ARGV[3])
local ttl = tonumber(ARGV[4])

local data = redis.call('HMGET', key, 'tokens', 'ts_ms')
local tokens = tonumber(data[1])
local ts_ms = tonumber(data[2])
if tokens == nil then
  tokens = cap
  ts_ms = now_ms
end
local elapsed = math.max(0, now_ms - ts_ms)
local refilled = math.min(cap, tokens + (elapsed * refill_per_sec) / 1000.0)
local allowed = 0
local retry_after_ms = 0
if refilled >= 1.0 then
  refilled = refilled - 1.0
  allowed = 1
else
  retry_after_ms = math.ceil((1.0 - refilled) * 1000.0 / refill_per_sec)
end
redis.call('HSET', key, 'tokens', tostring(refilled), 'ts_ms', tostring(now_ms))
redis.call('EXPIRE', key, ttl)
return { allowed, retry_after_ms }
"#;

pub async fn rate_limit_middleware(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let ctx = req
        .extensions()
        .get::<AccountCtx>()
        .cloned()
        .ok_or(ApiError::MissingAccountId)?;
    let mut redis = state.redis.clone();
    let key = format!("tap:rl:{}", ctx.id);
    let res: Vec<i64> = redis::Script::new(SCRIPT)
        .key(key)
        .arg(now_ms())
        .arg(CAP)
        .arg(REFILL_PER_SEC)
        .arg(TTL_SECS)
        .invoke_async(&mut redis)
        .await?;
    let allowed = res.first().copied().unwrap_or(0);
    if allowed == 0 {
        let retry_after_ms = res.get(1).copied().unwrap_or(1000);
        let secs = ((retry_after_ms as u64).saturating_add(999)) / 1000;
        return Err(ApiError::RateLimited { retry_after_secs: secs.max(1) });
    }
    Ok(next.run(req).await)
}
```

Lua choice: a hash with `tokens` + `ts_ms` keeps the read+update atomic in one round-trip. Capacity 10 + refill 10/sec means a single normal player (one tap per 5 s cell) is two orders of magnitude under ceiling; a scripted attacker hits the bucket within 100 ms — the numbers come straight from ADR-0009 §6.

- [ ] **Step 5.2: Register the module**

Edit `games/tap-trading/backend/api/src/middleware/mod.rs`, add `pub mod rate_limit;`.

The layer isn't mounted on any route yet — Task 8 wires it to `POST /v1/positions`. This commit ships the function + its tests in isolation.

- [ ] **Step 5.3: Write integration tests**

Write `games/tap-trading/backend/api/tests/rate_limit.rs`. We can't hit the middleware via a route until Task 8 wires it, so this test calls the limiter directly through a one-off route exposed only in `#[cfg(test)]`. Add the test-only route as a doc-commented escape hatch.

Edit `games/tap-trading/backend/api/src/lib.rs`. Add:

```rust
/// For tests only — a route that runs the rate limiter and returns 200 on
/// pass. We expose this from the library so integration tests can exercise
/// the limiter in isolation of POST /v1/positions (wired in Task 8).
#[doc(hidden)]
pub fn router_with_rate_limit_probe(state: AppState) -> Router {
    use axum::middleware::from_fn_with_state;
    Router::new()
        .route("/v1/rl-probe", get(|| async { "ok" }))
        .layer(from_fn_with_state(state.clone(), middleware::rate_limit::rate_limit_middleware))
        .layer(from_fn_with_state(state.clone(), middleware::account_id::account_id_middleware))
        .with_state(state)
}
```

Write `games/tap-trading/backend/api/tests/rate_limit.rs`:

```rust
mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::TestApp;
use tower::util::ServiceExt;

async fn ping(app: &TestApp, account: &str) -> StatusCode {
    let router = tap_trading_api::router_with_rate_limit_probe(app.state.clone());
    let resp = router.oneshot(
        Request::builder()
            .uri("/v1/rl-probe")
            .header("x-account-id", account)
            .body(Body::empty()).unwrap()
    ).await.unwrap();
    resp.status()
}

#[tokio::test]
async fn eleventh_tap_in_a_second_is_rate_limited() {
    let app = TestApp::start().await;
    // Pin time so refill doesn't sneak tokens back in.
    std::env::set_var("TAP_TEST_NOW_MS", "1000000000000");

    for _ in 0..10 {
        assert_eq!(ping(&app, "speedy").await, StatusCode::OK);
    }
    assert_eq!(ping(&app, "speedy").await, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn bucket_isolated_per_account() {
    let app = TestApp::start().await;
    std::env::set_var("TAP_TEST_NOW_MS", "2000000000000");

    for _ in 0..10 {
        let _ = ping(&app, "alice").await;
    }
    assert_eq!(ping(&app, "alice").await, StatusCode::TOO_MANY_REQUESTS);
    // bob's bucket is untouched.
    assert_eq!(ping(&app, "bob").await, StatusCode::OK);
}

#[tokio::test]
async fn bucket_refills_one_token_per_100ms() {
    let app = TestApp::start().await;
    std::env::set_var("TAP_TEST_NOW_MS", "3000000000000");
    for _ in 0..10 { let _ = ping(&app, "patient").await; }
    assert_eq!(ping(&app, "patient").await, StatusCode::TOO_MANY_REQUESTS);
    // Advance 200 ms → 2 tokens recovered.
    std::env::set_var("TAP_TEST_NOW_MS", "3000000000200");
    assert_eq!(ping(&app, "patient").await, StatusCode::OK);
    assert_eq!(ping(&app, "patient").await, StatusCode::OK);
    assert_eq!(ping(&app, "patient").await, StatusCode::TOO_MANY_REQUESTS);
}
```

Note: `std::env::set_var` is process-global. The three tests run sequentially in this file (the test runner runs `#[tokio::test]`s on a multi-threaded scheduler but with `cargo test --test rate_limit` they share a process). Each test pins its own `TAP_TEST_NOW_MS` window so they don't collide. If we ever see flake from intra-process parallelism, gate this file behind `--test-threads=1`; for now the time offsets are far enough apart that it doesn't matter.

- [ ] **Step 5.4: Run + verify**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-api --test rate_limit -- --test-threads=1
cargo clippy --all-targets -- -D warnings
```

Expected: all 3 tests pass. The `--test-threads=1` guards against the shared `TAP_TEST_NOW_MS`.

- [ ] **Step 5.5: Commit**

```bash
git add games/tap-trading/backend/api/
git commit -m "feat(tick-api): add redis token-bucket rate limiter"
```

---

## Task 6 — Aggregator HTTP client

Implement the real `AggregatorClient::replay` against `GET /ring/:asset/:seq?run_id=N`. Wiremock-backed unit tests cover every status code per ADR-0008 §5.

**Files:**
- Modify: `games/tap-trading/backend/api/src/aggregator_client.rs`

- [ ] **Step 6.1: Implement `replay`**

Replace the `unimplemented!` body in `aggregator_client.rs`:

```rust
pub async fn replay(
    &self,
    asset: AssetSymbol,
    seq: u64,
    run_id: u64,
) -> Result<OracleTick, ReplayError> {
    let asset_str = match asset {
        AssetSymbol::Eth => "ETH",
        AssetSymbol::Btc => "BTC",
        AssetSymbol::Sol => "SOL",
    };
    let url = format!("{}/ring/{}/{}?run_id={}", self.base_url, asset_str, seq, run_id);
    let resp = self.http.get(&url).send().await?;
    match resp.status().as_u16() {
        200 => Ok(resp.json::<OracleTick>().await?),
        404 => Err(ReplayError::UnknownAsset),
        409 | 410 => Err(ReplayError::Stale),
        other => Err(ReplayError::Transport(
            resp.error_for_status().unwrap_err()
        )).map_err(|_| ReplayError::Stale).map(|_: ReplayError| unreachable!())
            .or(Err(ReplayError::Stale))
            .map(|_| unreachable!())
            .or_else(|_: ReplayError| {
                tracing::warn!(status = other, "unexpected aggregator status");
                Err(ReplayError::Stale)
            }),
    }
}
```

(Simplify: unexpected status maps to `Stale` because anything we don't recognize should fail closed — we'd rather reject a tap than commit one against an undefined response. Code shape below cleans up the `match` arm.)

Replace the `match` with the simpler form:

```rust
    match resp.status().as_u16() {
        200 => Ok(resp.json::<OracleTick>().await?),
        404 => Err(ReplayError::UnknownAsset),
        409 | 410 => Err(ReplayError::Stale),
        other => {
            tracing::warn!(status = other, "unexpected aggregator status; treating as stale");
            Err(ReplayError::Stale)
        }
    }
```

Also extend `ReplayError` with a `From<reqwest::Error>` is already wired via `#[from]`; nothing else to add.

- [ ] **Step 6.2: Write inline tests against wiremock**

Append to the bottom of `aggregator_client.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path_regex, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn sample_tick(seq: u64, run_id: u64) -> serde_json::Value {
        serde_json::json!({
            "asset": "BTC", "run_id": run_id, "seq": seq,
            "ts_ms": 1_700_000_000_000_i64, "mid": 50_000.0,
            "vol_annualized": 0.80, "source_count": 3
        })
    }

    #[tokio::test]
    async fn replay_200_returns_tick() {
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path_regex(r"^/ring/BTC/12345$"))
            .and(query_param("run_id", "999"))
            .respond_with(ResponseTemplate::new(200).set_body_json(sample_tick(12345, 999)))
            .mount(&mock)
            .await;
        let client = AggregatorClient::new(mock.uri());
        let tick = client.replay(AssetSymbol::Btc, 12345, 999).await.unwrap();
        assert_eq!(tick.seq, 12345);
        assert_eq!(tick.run_id, 999);
        assert_eq!(tick.mid, 50_000.0);
    }

    #[tokio::test]
    async fn replay_410_is_stale() {
        let mock = MockServer::start().await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(410)).mount(&mock).await;
        let client = AggregatorClient::new(mock.uri());
        assert!(matches!(client.replay(AssetSymbol::Eth, 1, 1).await, Err(ReplayError::Stale)));
    }

    #[tokio::test]
    async fn replay_409_is_stale() {
        let mock = MockServer::start().await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(409)).mount(&mock).await;
        let client = AggregatorClient::new(mock.uri());
        assert!(matches!(client.replay(AssetSymbol::Eth, 1, 1).await, Err(ReplayError::Stale)));
    }

    #[tokio::test]
    async fn replay_404_is_unknown_asset() {
        let mock = MockServer::start().await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(404)).mount(&mock).await;
        let client = AggregatorClient::new(mock.uri());
        assert!(matches!(client.replay(AssetSymbol::Eth, 1, 1).await, Err(ReplayError::UnknownAsset)));
    }

    #[tokio::test]
    async fn replay_500_is_stale() {
        let mock = MockServer::start().await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(500)).mount(&mock).await;
        let client = AggregatorClient::new(mock.uri());
        assert!(matches!(client.replay(AssetSymbol::Eth, 1, 1).await, Err(ReplayError::Stale)));
    }
}
```

- [ ] **Step 6.3: Run + verify**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-api aggregator_client::tests
cargo clippy --all-targets -- -D warnings
```

Expected: 5 tests pass.

- [ ] **Step 6.4: Commit**

```bash
git add games/tap-trading/backend/api/src/aggregator_client.rs
git commit -m "feat(tick-api): add aggregator http client"
```

---

## Task 7 — Cell validation, stake-tier check, drift calc

Pure functions, table-driven tests. No DB, no Redis, no HTTP.

**Files:**
- Create: `games/tap-trading/backend/api/src/validation.rs`
- Modify: `games/tap-trading/backend/api/src/lib.rs` (add `pub mod validation;`)

- [ ] **Step 7.1: Write the module**

Write `games/tap-trading/backend/api/src/validation.rs`:

```rust
//! Pure validation. Tested without DB / Redis / HTTP.
//!
//! Spec: ADR-0009 §4 step 3 (cell), `PRD.md` line 23 (stake tiers),
//! ADR-0009 §4 step 6 (drift gate, 3%).

use tap_trading_oracle_types::AssetSymbol;

use crate::error::ApiError;

/// Allowed Tier-1 stake values. `PRD.md` line 23.
pub const STAKE_TIERS_V1: &[i64] = &[50, 100, 500, 1000];

/// Drift tolerance: 3.0% (ADR-0009 §4 step 6).
pub const DRIFT_TOLERANCE: f64 = 0.03;

/// Cell window length in milliseconds (v1: fixed 5s).
pub const CELL_DURATION_MS: i64 = 5_000;

/// Lock window in milliseconds before close (taps inside this window reject).
pub const LOCK_WINDOW_MS: i64 = 1_000;

#[derive(Debug, Clone, Copy)]
pub struct CellInput {
    pub asset: &'static str,
    pub strike_lo: f64,
    pub strike_hi: f64,
    pub t_open_ms: i64,
    pub t_close_ms: i64,
    pub stake_points: i64,
}

pub fn parse_asset(raw: &str) -> Result<AssetSymbol, ApiError> {
    match raw {
        "ETH" => Ok(AssetSymbol::Eth),
        "BTC" => Ok(AssetSymbol::Btc),
        "SOL" => Ok(AssetSymbol::Sol),
        _ => Err(ApiError::UnknownAsset),
    }
}

pub fn validate_stake_tier(stake_points: i64) -> Result<(), ApiError> {
    if STAKE_TIERS_V1.contains(&stake_points) {
        Ok(())
    } else {
        Err(ApiError::InvalidStake)
    }
}

pub fn validate_cell(
    t_open_ms: i64,
    t_close_ms: i64,
    strike_lo: f64,
    strike_hi: f64,
    now_ms: i64,
) -> Result<(), ApiError> {
    if !(strike_lo > 0.0 && strike_hi > strike_lo) {
        return Err(ApiError::InvalidCell);
    }
    if t_close_ms - t_open_ms != CELL_DURATION_MS {
        return Err(ApiError::InvalidCell);
    }
    if t_open_ms % CELL_DURATION_MS != 0 {
        return Err(ApiError::InvalidCell);
    }
    if now_ms + LOCK_WINDOW_MS >= t_close_ms {
        return Err(ApiError::LockWindow);
    }
    Ok(())
}

/// Returns `true` iff `|server - client| / server > DRIFT_TOLERANCE`.
/// `server` must be positive — the multiplier floor (`MATH_SPEC §4.1`)
/// guarantees this; we still defensively reject `server <= 0` as drift.
pub fn drift_exceeded(server: f64, client: f64) -> bool {
    if server <= 0.0 {
        return true;
    }
    ((server - client).abs() / server) > DRIFT_TOLERANCE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_asset_table() {
        for (raw, ok) in [("ETH", true), ("BTC", true), ("SOL", true), ("DOGE", false), ("", false)] {
            assert_eq!(parse_asset(raw).is_ok(), ok, "raw={raw}");
        }
    }

    #[test]
    fn validate_stake_tier_table() {
        for (stake, ok) in [(50, true), (100, true), (500, true), (1000, true),
                            (0, false), (49, false), (5000, false), (-1, false)] {
            assert_eq!(validate_stake_tier(stake).is_ok(), ok, "stake={stake}");
        }
    }

    #[test]
    fn validate_cell_rejects_misaligned_open() {
        // t_open_ms = 1_000_001 → not a 5s boundary.
        let res = validate_cell(1_000_001, 1_005_001, 100.0, 101.0, 999_000);
        assert!(matches!(res, Err(ApiError::InvalidCell)));
    }

    #[test]
    fn validate_cell_rejects_wrong_duration() {
        // 6s window.
        let res = validate_cell(1_000_000, 1_006_000, 100.0, 101.0, 999_000);
        assert!(matches!(res, Err(ApiError::InvalidCell)));
    }

    #[test]
    fn validate_cell_rejects_strike_lo_ge_hi() {
        let res = validate_cell(1_000_000, 1_005_000, 100.0, 100.0, 999_000);
        assert!(matches!(res, Err(ApiError::InvalidCell)));
    }

    #[test]
    fn validate_cell_rejects_inside_lock_window() {
        // now + 1000 == t_close → reject.
        let res = validate_cell(1_000_000, 1_005_000, 100.0, 101.0, 1_004_000);
        assert!(matches!(res, Err(ApiError::LockWindow)));
        // now + 999 < t_close → ok.
        let ok = validate_cell(1_000_000, 1_005_000, 100.0, 101.0, 1_003_999);
        assert!(ok.is_ok());
    }

    #[test]
    fn drift_at_exactly_three_percent_passes() {
        // 3.0% exactly is NOT exceeded (the gate is strict >).
        assert!(!drift_exceeded(1.0, 0.97));
        assert!(!drift_exceeded(1.0, 1.03));
    }

    #[test]
    fn drift_above_three_percent_rejects() {
        assert!(drift_exceeded(1.0, 0.9699));
        assert!(drift_exceeded(1.0, 1.0301));
    }

    #[test]
    fn drift_zero_server_rejects() {
        assert!(drift_exceeded(0.0, 1.0));
        assert!(drift_exceeded(-1.0, 1.0));
    }
}
```

- [ ] **Step 7.2: Add the module to `lib.rs`**

Add `pub mod validation;` to the module list in `lib.rs`.

- [ ] **Step 7.3: Run + verify**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-api validation::tests
cargo clippy --all-targets -- -D warnings
```

Expected: green.

- [ ] **Step 7.4: Commit**

```bash
git add games/tap-trading/backend/api/
git commit -m "feat(tick-api): add cell validation and drift calc"
```

---

## Task 8 — `POST /v1/positions` happy path

Wire ADR-0009 §4 steps 3 → 9 (no idempotency, no rate-limit on this commit). The idempotency layer lands in Task 10; rate-limit is layered in Task 8.4 (we add the rate-limit layer to this route in this commit because it's a single line and the limiter is already tested in Task 5).

**Files:**
- Create: `games/tap-trading/backend/api/src/handlers/positions.rs`
- Create: `games/tap-trading/backend/api/src/db.rs`
- Create: `games/tap-trading/backend/api/tests/post_positions_happy.rs`
- Modify: `games/tap-trading/backend/api/src/handlers/mod.rs`
- Modify: `games/tap-trading/backend/api/src/lib.rs`

- [ ] **Step 8.1: Write `db.rs`**

Write `games/tap-trading/backend/api/src/db.rs`:

```rust
//! sqlx query helpers. Keep handlers free of SQL string literals.

use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::error::ApiError;

pub struct InsertPositionInput<'a> {
    pub account_id: i64,
    pub asset: &'a str,
    pub strike_lo: f64,
    pub strike_hi: f64,
    pub t_open_ms: i64,
    pub t_close_ms: i64,
    pub stake_points: i64,
    pub multiplier_at_tap: f64,
    pub oracle_seq_at_tap: i64,
    pub oracle_run_id_at_tap: i64,
    pub client_request_id: Uuid,
    pub client_fingerprint: Option<&'a str>,
    pub now_ms: i64,
}

/// Select balance with row-level lock; serializes concurrent taps for one account.
pub async fn select_balance_for_update(
    tx: &mut Transaction<'_, Postgres>,
    account_id: i64,
) -> Result<i64, ApiError> {
    let (balance,): (i64,) = sqlx::query_as(
        "SELECT balance FROM accounts WHERE id = $1 FOR UPDATE"
    )
    .bind(account_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(balance)
}

/// INSERT with ON CONFLICT DO NOTHING; returns Some(id) if newly inserted,
/// None if the (account_id, client_request_id) pair already exists.
pub async fn insert_position(
    tx: &mut Transaction<'_, Postgres>,
    i: &InsertPositionInput<'_>,
) -> Result<Option<i64>, ApiError> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        INSERT INTO positions (
            account_id, asset, strike_lo, strike_hi, t_open_ms, t_close_ms,
            stake_points, multiplier_at_tap, status,
            oracle_seq_at_tap, oracle_run_id_at_tap, client_request_id,
            client_fingerprint, created_at_ms
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'OPEN', $9, $10, $11, $12, $13)
        ON CONFLICT ON CONSTRAINT positions_dedup_request DO NOTHING
        RETURNING id
        "#,
    )
    .bind(i.account_id)
    .bind(i.asset)
    .bind(i.strike_lo)
    .bind(i.strike_hi)
    .bind(i.t_open_ms)
    .bind(i.t_close_ms)
    .bind(i.stake_points)
    .bind(i.multiplier_at_tap)
    .bind(i.oracle_seq_at_tap)
    .bind(i.oracle_run_id_at_tap)
    .bind(i.client_request_id)
    .bind(i.client_fingerprint)
    .bind(i.now_ms)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(|(id,)| id))
}

pub async fn insert_tap_stake_ledger(
    tx: &mut Transaction<'_, Postgres>,
    account_id: i64,
    stake_points: i64,
    position_id: i64,
    now_ms: i64,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"INSERT INTO points_ledger (account_id, kind, delta, ref_id, created_at_ms)
           VALUES ($1, 'TAP_STAKE', $2, $3, $4)"#,
    )
    .bind(account_id)
    .bind(-stake_points)
    .bind(position_id)
    .bind(now_ms)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub async fn debit_balance(
    tx: &mut Transaction<'_, Postgres>,
    account_id: i64,
    stake_points: i64,
    now_ms: i64,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"UPDATE accounts
           SET balance = balance - $2, last_active_ms = $3
           WHERE id = $1"#,
    )
    .bind(account_id)
    .bind(stake_points)
    .bind(now_ms)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Look up an existing (account_id, client_request_id) row. Used by Task 10's
/// idempotency path; ships here so `db.rs` is a single PR-reviewable module.
pub async fn find_position_by_request_id(
    pool: &PgPool,
    account_id: i64,
    request_id: Uuid,
) -> Result<Option<ExistingPosition>, ApiError> {
    let row: Option<(i64, sqlx::types::BigDecimal, String, i64)> = sqlx::query_as(
        r#"SELECT id, multiplier_at_tap, status, t_close_ms
           FROM positions WHERE account_id = $1 AND client_request_id = $2"#,
    )
    .bind(account_id)
    .bind(request_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(id, mult, status, t_close)| ExistingPosition {
        id,
        multiplier_at_tap: mult.to_string().parse::<f64>().unwrap_or(0.0),
        status,
        t_close_ms: t_close,
    }))
}

#[derive(Debug, Clone)]
pub struct ExistingPosition {
    pub id: i64,
    pub multiplier_at_tap: f64,
    pub status: String,
    pub t_close_ms: i64,
}
```

The `sqlx::types::BigDecimal` parse path is the v1-simple way to ferry `NUMERIC(10, 4)` back to `f64`. Drift tolerance (3%) is two orders of magnitude above any precision artifact. Worker-side / settlement read paths should use a different conversion if they need bit-exactness; the API doesn't.

- [ ] **Step 8.2: Write `handlers/positions.rs`**

Write `games/tap-trading/backend/api/src/handlers/positions.rs`:

```rust
//! POST /v1/positions — tap commit. ADR-0009 §4.
//!
//! Pipeline steps 3 → 9 land here. Step 1 (rate limit) is a tower layer; step
//! 2 (idempotency) lands in Task 10.

use axum::extract::State;
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use tap_trading_oracle_types::AssetSymbol;
use tap_trading_pricing_engine::{
    compute_multiplier, Cell, OracleState, PricingConfig,
};
use uuid::Uuid;

use crate::account_ctx::AccountCtx;
use crate::aggregator_client::ReplayError;
use crate::db::{
    debit_balance, insert_position, insert_tap_stake_ledger, select_balance_for_update,
    InsertPositionInput,
};
use crate::error::ApiError;
use crate::now::now_ms;
use crate::state::AppState;
use crate::validation::{drift_exceeded, parse_asset, validate_cell, validate_stake_tier};

#[derive(Debug, Deserialize)]
pub struct TapRequest {
    pub client_request_id: Uuid,
    pub asset: String,
    pub strike_lo: f64,
    pub strike_hi: f64,
    pub t_open_ms: i64,
    pub t_close_ms: i64,
    pub stake_points: i64,
    pub client_multiplier: f64,
    pub oracle_seq_at_tap: i64,
    pub oracle_run_id_at_tap: i64,
    #[serde(default)]
    pub client_fingerprint: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TapResponse {
    pub position_id: i64,
    pub multiplier_at_tap: f64,
    pub status: String,
    pub t_close_ms: i64,
}

pub async fn post_position(
    State(state): State<AppState>,
    Extension(ctx): Extension<AccountCtx>,
    Json(req): Json<TapRequest>,
) -> Result<(axum::http::StatusCode, Json<TapResponse>), ApiError> {
    let now = now_ms();

    // Step 3 — cell + stake tier validation.
    let asset = parse_asset(&req.asset)?;
    validate_stake_tier(req.stake_points)?;
    validate_cell(req.t_open_ms, req.t_close_ms, req.strike_lo, req.strike_hi, now)?;
    if req.oracle_seq_at_tap < 0 || req.oracle_run_id_at_tap < 0 {
        return Err(ApiError::InvalidCell);
    }

    // Step 4 — replay quote from aggregator.
    let tick = match state.aggregator.replay(
        asset,
        req.oracle_seq_at_tap as u64,
        req.oracle_run_id_at_tap as u64,
    ).await {
        Ok(t) => t,
        Err(ReplayError::Stale) => return Err(ApiError::StaleQuote),
        Err(ReplayError::UnknownAsset) => return Err(ApiError::UnknownAsset),
        Err(_) => return Err(ApiError::StaleQuote),
    };

    // Step 5 — server recompute. `oracle_types::AssetSymbol` is a re-export of
    // `pricing_engine::AssetSymbol`, so the same value flows through both.
    let cell = Cell {
        asset,
        strike_lo: req.strike_lo,
        strike_hi: req.strike_hi,
        t_open_ms: req.t_open_ms as u64,
        t_close_ms: req.t_close_ms as u64,
    };
    let oracle = OracleState {
        asset,
        spot: tick.mid,
        sigma_annualized: tick.vol_annualized,
        timestamp_ms: tick.ts_ms as u64,
    };
    let server_mult = compute_multiplier(&cell, &oracle, &PricingConfig::default(), now as u64);

    // Step 6 — drift gate.
    if drift_exceeded(server_mult, req.client_multiplier) {
        return Err(ApiError::DriftExceeded { server_multiplier: server_mult });
    }

    // Step 7+8 — balance + atomic commit + NOTIFY.
    let mut tx = state.pg.begin().await?;
    let balance = select_balance_for_update(&mut tx, ctx.id).await?;
    if balance < req.stake_points {
        return Err(ApiError::InsufficientBalance);
    }
    let asset_text = match asset { AssetSymbol::Eth => "ETH", AssetSymbol::Btc => "BTC", AssetSymbol::Sol => "SOL" };
    let inserted = insert_position(&mut tx, &InsertPositionInput {
        account_id: ctx.id,
        asset: asset_text,
        strike_lo: req.strike_lo,
        strike_hi: req.strike_hi,
        t_open_ms: req.t_open_ms,
        t_close_ms: req.t_close_ms,
        stake_points: req.stake_points,
        multiplier_at_tap: server_mult,
        oracle_seq_at_tap: req.oracle_seq_at_tap,
        oracle_run_id_at_tap: req.oracle_run_id_at_tap,
        client_request_id: req.client_request_id,
        client_fingerprint: req.client_fingerprint.as_deref(),
        now_ms: now,
    }).await?;
    let position_id = match inserted {
        Some(id) => id,
        None => {
            // Conflict on positions_dedup_request — Task 10 handles the idempotent
            // replay. In this commit, treat as "another writer won" and surface as
            // a 409 via Internal until Task 10 lands the retry-from-step-2 flow.
            return Err(ApiError::Internal(anyhow::anyhow!("dup_request_pre_idempotency")));
        }
    };
    insert_tap_stake_ledger(&mut tx, ctx.id, req.stake_points, position_id, now).await?;
    debit_balance(&mut tx, ctx.id, req.stake_points, now).await?;
    // NOTIFY tap_new_position is wired in Task 11; we leave it out here so the
    // happy-path test stays focused on the commit pipeline.
    tx.commit().await?;

    Ok((axum::http::StatusCode::CREATED, Json(TapResponse {
        position_id,
        multiplier_at_tap: server_mult,
        status: "OPEN".to_string(),
        t_close_ms: req.t_close_ms,
    })))
}

```

- [ ] **Step 8.3: Wire the route**

Edit `lib.rs`. In `authenticated`, add the route + the rate-limit layer (so this commit ships rate-limited tap-commit):

```rust
use axum::routing::post;

let authenticated = Router::new()
    .route("/v1/me", get(handlers::me::get_me))
    .route("/v1/ping", get(handlers::health::ping))
    .route("/v1/positions", post(handlers::positions::post_position)
        .route_layer(from_fn_with_state(state.clone(), middleware::rate_limit::rate_limit_middleware)))
    .layer(from_fn_with_state(state.clone(), middleware::account_id::account_id_middleware));
```

`route_layer` scopes the rate limiter to `POST /v1/positions` only — `/v1/me`, `/v1/ping`, history etc. are NOT rate-limited.

Add `pub mod db;` to `lib.rs`.

- [ ] **Step 8.4: Write the happy-path integration test**

Write `games/tap-trading/backend/api/tests/post_positions_happy.rs`:

```rust
mod common;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use common::TestApp;
use serde_json::json;
use std::sync::Arc;
use tap_trading_api::aggregator_client::AggregatorClient;
use tower::util::ServiceExt;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn happy_path_debits_balance_writes_position() {
    // Pin time to a 5s-aligned boundary 2s before t_close so we are outside
    // the 1s lock window.
    let now_ms: i64 = 1_748_345_670_000;            // t_open
    let t_close_ms: i64 = now_ms + 5_000;           // t_close
    let pinned_now: i64 = now_ms + 3_000;           // 2s before close — outside lock
    std::env::set_var("TAP_TEST_NOW_MS", pinned_now.to_string());

    // Mock aggregator returning a tick with a known (mid, vol).
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex(r"^/ring/BTC/12345$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "asset": "BTC", "run_id": 999, "seq": 12345,
            "ts_ms": pinned_now, "mid": 50_000.0, "vol_annualized": 0.80,
            "source_count": 3
        })))
        .mount(&mock).await;
    let aggregator_uri = mock.uri();

    let app = TestApp::start_with(|s| {
        s.aggregator = Arc::new(AggregatorClient::new(aggregator_uri));
    }).await;

    // Cell narrow band around spot 50000 — server multiplier will be near the
    // floor for a 5s window (~1.35). Send a client multiplier within 3%.
    let req = json!({
        "client_request_id": "00000000-0000-0000-0000-0000000000aa",
        "asset": "BTC",
        "strike_lo": 49_999.5,
        "strike_hi": 50_000.5,
        "t_open_ms": now_ms,
        "t_close_ms": t_close_ms,
        "stake_points": 100,
        "client_multiplier": 1.35,
        "oracle_seq_at_tap": 12345,
        "oracle_run_id_at_tap": 999,
        "client_fingerprint": "test-fp"
    });

    let resp = app.router.clone().oneshot(
        Request::builder()
            .method("POST")
            .uri("/v1/positions")
            .header("x-account-id", "happy-tester")
            .header("content-type", "application/json")
            .body(Body::from(req.to_string())).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = to_bytes(resp.into_body(), 8 * 1024).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(v["position_id"].as_i64().unwrap() > 0);
    assert_eq!(v["status"], "OPEN");
    assert_eq!(v["t_close_ms"], t_close_ms);
    // multiplier_at_tap should be server's recompute, near 1.35 floor.
    let mult = v["multiplier_at_tap"].as_f64().unwrap();
    assert!(mult >= 1.30 && mult <= 1.45, "got mult={mult}");

    // Balance debited by 100 from 10_000.
    let (balance,): (i64,) = sqlx::query_as("SELECT balance FROM accounts WHERE external_id = $1")
        .bind("happy-tester").fetch_one(&app.pg).await.unwrap();
    assert_eq!(balance, 9_900);

    // One position row, status OPEN.
    let (count, status): (i64, String) = sqlx::query_as(
        "SELECT COUNT(*), MAX(status) FROM positions"
    ).fetch_one(&app.pg).await.unwrap();
    assert_eq!(count, 1);
    assert_eq!(status, "OPEN");

    // One TAP_STAKE ledger row with delta = -100.
    let (delta,): (i64,) = sqlx::query_as(
        "SELECT delta FROM points_ledger WHERE kind = 'TAP_STAKE'"
    ).fetch_one(&app.pg).await.unwrap();
    assert_eq!(delta, -100);
}
```

- [ ] **Step 8.5: Run + verify**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-api --test post_positions_happy -- --test-threads=1
cargo clippy --all-targets -- -D warnings
```

Expected: green.

- [ ] **Step 8.6: Commit**

```bash
git add games/tap-trading/backend/api/
git commit -m "feat(tick-api): add post /v1/positions happy path"
```

---

## Task 9 — `POST /v1/positions` error paths

One integration test per error code. The implementations already exist (Task 8 returns each error from the right step). This task adds the regression-trapping tests + any small handler tweaks to surface the right code.

**Files:**
- Create: `games/tap-trading/backend/api/tests/post_positions_errors.rs`

- [ ] **Step 9.1: Write the error-path tests**

Write `games/tap-trading/backend/api/tests/post_positions_errors.rs`:

```rust
mod common;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use common::TestApp;
use serde_json::json;
use std::sync::Arc;
use tap_trading_api::aggregator_client::AggregatorClient;
use tower::util::ServiceExt;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn base_request() -> serde_json::Value {
    json!({
        "client_request_id": "00000000-0000-0000-0000-000000000001",
        "asset": "BTC",
        "strike_lo": 49_999.5,
        "strike_hi": 50_000.5,
        "t_open_ms": 1_748_345_670_000_i64,
        "t_close_ms": 1_748_345_675_000_i64,
        "stake_points": 100,
        "client_multiplier": 1.35,
        "oracle_seq_at_tap": 12345,
        "oracle_run_id_at_tap": 999,
    })
}

async fn post(app: &TestApp, account: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
    let resp = app.router.clone().oneshot(
        Request::builder()
            .method("POST").uri("/v1/positions")
            .header("x-account-id", account)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string())).unwrap()
    ).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 8 * 1024).await.unwrap();
    let v = if bytes.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&bytes).unwrap() };
    (status, v)
}

#[tokio::test]
async fn invalid_stake_returns_400() {
    std::env::set_var("TAP_TEST_NOW_MS", "1748345673000");
    let app = TestApp::start().await;
    let mut req = base_request(); req["stake_points"] = json!(77);
    let (status, body) = post(&app, "alice", req).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_stake");
}

#[tokio::test]
async fn unknown_asset_returns_400() {
    std::env::set_var("TAP_TEST_NOW_MS", "1748345673000");
    let app = TestApp::start().await;
    let mut req = base_request(); req["asset"] = json!("DOGE");
    let (status, body) = post(&app, "bob", req).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "unknown_asset");
}

#[tokio::test]
async fn lock_window_returns_400() {
    // now = t_close - 500ms → inside the 1s lock window.
    std::env::set_var("TAP_TEST_NOW_MS", "1748345674500");
    let app = TestApp::start().await;
    let (status, body) = post(&app, "carol", base_request()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "lock_window");
}

#[tokio::test]
async fn stale_quote_returns_422() {
    std::env::set_var("TAP_TEST_NOW_MS", "1748345673000");
    let mock = MockServer::start().await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(410)).mount(&mock).await;
    let uri = mock.uri();
    let app = TestApp::start_with(|s| { s.aggregator = Arc::new(AggregatorClient::new(uri)); }).await;
    let (status, body) = post(&app, "dave", base_request()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "stale_quote");
}

#[tokio::test]
async fn drift_exceeded_returns_422_with_server_mult() {
    std::env::set_var("TAP_TEST_NOW_MS", "1748345673000");
    let mock = MockServer::start().await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(json!({
        "asset": "BTC", "run_id": 999, "seq": 12345, "ts_ms": 1748345673000_i64,
        "mid": 50000.0, "vol_annualized": 0.80, "source_count": 3
    }))).mount(&mock).await;
    let uri = mock.uri();
    let app = TestApp::start_with(|s| { s.aggregator = Arc::new(AggregatorClient::new(uri)); }).await;

    // Client claims 5.0 — server recompute is near floor 1.35 → drift > 3%.
    let mut req = base_request(); req["client_multiplier"] = json!(5.0);
    let (status, body) = post(&app, "eve", req).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "drift_exceeded");
    assert!(body["server_multiplier"].as_f64().is_some());
}

#[tokio::test]
async fn insufficient_balance_returns_422() {
    std::env::set_var("TAP_TEST_NOW_MS", "1748345673000");
    let mock = MockServer::start().await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(json!({
        "asset": "BTC", "run_id": 999, "seq": 12345, "ts_ms": 1748345673000_i64,
        "mid": 50000.0, "vol_annualized": 0.80, "source_count": 3
    }))).mount(&mock).await;
    let uri = mock.uri();
    let app = TestApp::start_with(|s| { s.aggregator = Arc::new(AggregatorClient::new(uri)); }).await;

    // Drain account to 50.
    sqlx::query("UPDATE accounts SET balance = 50 WHERE external_id = $1")
        .bind("frank")
        // Account doesn't exist yet — lazy-create first via a /v1/me call.
        .execute(&app.pg).await.ok();
    let _ = post(&app, "frank", base_request()).await; // creates account
    sqlx::query("UPDATE accounts SET balance = 50 WHERE external_id = $1")
        .bind("frank").execute(&app.pg).await.unwrap();

    let mut req = base_request(); req["stake_points"] = json!(100);
    req["client_request_id"] = json!("00000000-0000-0000-0000-000000000002");
    let (status, body) = post(&app, "frank", req).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "insufficient_balance");
}

#[tokio::test]
async fn malformed_body_returns_400() {
    std::env::set_var("TAP_TEST_NOW_MS", "1748345673000");
    let app = TestApp::start().await;
    let resp = app.router.clone().oneshot(
        Request::builder().method("POST").uri("/v1/positions")
            .header("x-account-id", "garbage")
            .header("content-type", "application/json")
            .body(Body::from("{bad json")).unwrap()
    ).await.unwrap();
    assert!(resp.status().is_client_error());
}
```

The `insufficient_balance` test has to dance around lazy account creation: the first POST creates the account (with balance 10_000), the UPDATE then drains it to 50, the second POST exercises the actual failure.

- [ ] **Step 9.2: Run + verify**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-api --test post_positions_errors -- --test-threads=1
cargo clippy --all-targets -- -D warnings
```

Expected: 7 tests pass.

- [ ] **Step 9.3: Commit**

```bash
git add games/tap-trading/backend/api/tests/post_positions_errors.rs
git commit -m "feat(tick-api): add post /v1/positions error paths"
```

---

## Task 10 — Idempotency layer for tap commit

ADR-0009 §4 step 2 + the ON CONFLICT retry path. Same `client_request_id` twice must return identical responses and write exactly one ledger row.

**Files:**
- Modify: `games/tap-trading/backend/api/src/handlers/positions.rs`
- Create: `games/tap-trading/backend/api/tests/idempotency.rs`

- [ ] **Step 10.1: Add the pre-flight lookup + ON CONFLICT recovery**

Edit `games/tap-trading/backend/api/src/handlers/positions.rs`. Add a helper that returns a `TapResponse` from an `ExistingPosition`:

```rust
fn replay_response(existing: crate::db::ExistingPosition) -> Json<TapResponse> {
    Json(TapResponse {
        position_id: existing.id,
        multiplier_at_tap: existing.multiplier_at_tap,
        status: existing.status,
        t_close_ms: existing.t_close_ms,
    })
}
```

Then, at the top of `post_position` (immediately after `let now = now_ms();`), add the pre-flight idempotency lookup:

```rust
    // ADR-0009 §4 step 2 — idempotency pre-flight. Cheap SELECT before any
    // validation; a known request_id wins regardless of whether the rest of
    // the payload has changed (the client may have refreshed and lost state).
    if let Some(existing) = crate::db::find_position_by_request_id(
        &state.pg, ctx.id, req.client_request_id
    ).await? {
        return Ok((axum::http::StatusCode::OK, replay_response(existing)));
    }
```

And replace the previous `None => Err(ApiError::Internal(...))` branch in the INSERT path with a recursive "GOTO step 2" — concretely, fetch the now-existing row and return its replay:

```rust
    let position_id = match inserted {
        Some(id) => id,
        None => {
            // Concurrent retry wrote the row between our pre-flight SELECT and
            // our INSERT. ROLLBACK and replay-respond from the now-existing row.
            tx.rollback().await?;
            let existing = crate::db::find_position_by_request_id(
                &state.pg, ctx.id, req.client_request_id
            ).await?
                .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("conflict-but-row-missing")))?;
            return Ok((axum::http::StatusCode::OK, replay_response(existing)));
        }
    };
```

A retry returns `200 OK`, a fresh tap returns `201 Created` — both with the same body shape. Clients can treat them identically; the status distinction is informational.

- [ ] **Step 10.2: Write the idempotency test**

Write `games/tap-trading/backend/api/tests/idempotency.rs`:

```rust
mod common;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use common::TestApp;
use serde_json::json;
use std::sync::Arc;
use tap_trading_api::aggregator_client::AggregatorClient;
use tower::util::ServiceExt;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn same_request_id_returns_identical_response_and_one_ledger_row() {
    let pinned_now: i64 = 1_748_345_673_000;
    std::env::set_var("TAP_TEST_NOW_MS", pinned_now.to_string());

    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "asset": "BTC", "run_id": 999, "seq": 12345, "ts_ms": pinned_now,
            "mid": 50000.0, "vol_annualized": 0.80, "source_count": 3
        }))).mount(&mock).await;
    let uri = mock.uri();
    let app = TestApp::start_with(|s| { s.aggregator = Arc::new(AggregatorClient::new(uri)); }).await;

    let body = json!({
        "client_request_id": "00000000-0000-0000-0000-0000000000ff",
        "asset": "BTC", "strike_lo": 49999.5, "strike_hi": 50000.5,
        "t_open_ms": 1_748_345_670_000_i64, "t_close_ms": 1_748_345_675_000_i64,
        "stake_points": 100, "client_multiplier": 1.35,
        "oracle_seq_at_tap": 12345, "oracle_run_id_at_tap": 999
    });

    async fn fire(app: &TestApp, body: &serde_json::Value) -> (StatusCode, serde_json::Value) {
        let r = app.router.clone().oneshot(
            Request::builder().method("POST").uri("/v1/positions")
                .header("x-account-id", "idem-tester")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string())).unwrap()
        ).await.unwrap();
        let s = r.status();
        let bytes = to_bytes(r.into_body(), 8 * 1024).await.unwrap();
        (s, serde_json::from_slice(&bytes).unwrap())
    }

    let (s1, b1) = fire(&app, &body).await;
    assert_eq!(s1, StatusCode::CREATED);
    let (s2, b2) = fire(&app, &body).await;
    assert_eq!(s2, StatusCode::OK);

    assert_eq!(b1["position_id"], b2["position_id"]);
    assert_eq!(b1["multiplier_at_tap"], b2["multiplier_at_tap"]);
    assert_eq!(b1["t_close_ms"], b2["t_close_ms"]);

    let (positions,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM positions").fetch_one(&app.pg).await.unwrap();
    assert_eq!(positions, 1);
    let (ledger,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM points_ledger WHERE kind = 'TAP_STAKE'")
        .fetch_one(&app.pg).await.unwrap();
    assert_eq!(ledger, 1);
    let (balance,): (i64,) = sqlx::query_as("SELECT balance FROM accounts WHERE external_id = $1")
        .bind("idem-tester").fetch_one(&app.pg).await.unwrap();
    assert_eq!(balance, 9_900); // debited once, not twice.
}
```

- [ ] **Step 10.3: Run + verify**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-api --test idempotency -- --test-threads=1
cargo clippy --all-targets -- -D warnings
```

Expected: green.

- [ ] **Step 10.4: Commit**

```bash
git add games/tap-trading/backend/api/
git commit -m "feat(tick-api): add idempotency layer to tap commit"
```

---

## Task 11 — `NOTIFY tap_new_position`

ADR-0009 §5. Final statement of the commit transaction; payload is `position_id` as a decimal string.

**Files:**
- Modify: `games/tap-trading/backend/api/src/handlers/positions.rs`
- Create: `games/tap-trading/backend/api/tests/notify.rs`

- [ ] **Step 11.1: Emit the NOTIFY**

In `handlers/positions.rs`, immediately before `tx.commit().await?;` add:

```rust
    sqlx::query("SELECT pg_notify('tap_new_position', $1)")
        .bind(position_id.to_string())
        .execute(&mut *tx)
        .await?;
```

We use `pg_notify(channel, payload)` rather than `NOTIFY channel, 'payload'` so the payload binds cleanly through sqlx (`NOTIFY` doesn't accept parameters — only the function form does).

- [ ] **Step 11.2: Write the LISTEN-side test**

Write `games/tap-trading/backend/api/tests/notify.rs`:

```rust
mod common;

use axum::body::Body;
use axum::http::Request;
use common::TestApp;
use serde_json::json;
use sqlx::postgres::PgListener;
use std::sync::Arc;
use std::time::Duration;
use tap_trading_api::aggregator_client::AggregatorClient;
use tokio::time::timeout;
use tower::util::ServiceExt;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn notify_emitted_on_successful_commit() {
    let pinned_now: i64 = 1_748_345_673_000;
    std::env::set_var("TAP_TEST_NOW_MS", pinned_now.to_string());
    let mock = MockServer::start().await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(json!({
        "asset": "BTC", "run_id": 999, "seq": 12345, "ts_ms": pinned_now,
        "mid": 50000.0, "vol_annualized": 0.80, "source_count": 3
    }))).mount(&mock).await;
    let uri = mock.uri();
    let app = TestApp::start_with(|s| { s.aggregator = Arc::new(AggregatorClient::new(uri)); }).await;

    let mut listener = PgListener::connect_with(&app.pg).await.unwrap();
    listener.listen("tap_new_position").await.unwrap();

    let body = json!({
        "client_request_id": "00000000-0000-0000-0000-0000000000bb",
        "asset": "BTC", "strike_lo": 49999.5, "strike_hi": 50000.5,
        "t_open_ms": 1_748_345_670_000_i64, "t_close_ms": 1_748_345_675_000_i64,
        "stake_points": 100, "client_multiplier": 1.35,
        "oracle_seq_at_tap": 12345, "oracle_run_id_at_tap": 999
    });
    let _ = app.router.clone().oneshot(
        Request::builder().method("POST").uri("/v1/positions")
            .header("x-account-id", "notify-tester")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string())).unwrap()
    ).await.unwrap();

    let notif = timeout(Duration::from_secs(2), listener.recv()).await
        .expect("NOTIFY did not arrive within 2s").unwrap();
    assert_eq!(notif.channel(), "tap_new_position");
    let payload = notif.payload();
    let id: i64 = payload.parse().expect("payload is a decimal i64");
    assert!(id > 0);
}
```

- [ ] **Step 11.3: Run + verify**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-api --test notify -- --test-threads=1
cargo clippy --all-targets -- -D warnings
```

Expected: green within 2 s on the timeout budget.

- [ ] **Step 11.4: Commit**

```bash
git add games/tap-trading/backend/api/
git commit -m "feat(tick-api): emit notify on new position"
```

---

## Task 12 — `GET /v1/me/history` + `GET /v1/positions/:id`

Cursor-based history paginated by `created_at_ms DESC`, plus single-position lookup with ownership check (403 if account_id doesn't match).

**Files:**
- Modify: `games/tap-trading/backend/api/src/handlers/me.rs`
- Modify: `games/tap-trading/backend/api/src/handlers/positions.rs`
- Modify: `games/tap-trading/backend/api/src/lib.rs`
- Create: `games/tap-trading/backend/api/tests/get_history.rs`

- [ ] **Step 12.1: Implement `GET /v1/me/history`**

Append to `handlers/me.rs`:

```rust
use axum::extract::Query;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct HistoryQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub cursor: Option<i64>,
}

fn default_limit() -> i64 { 50 }

#[derive(Debug, Serialize)]
pub struct HistoryItem {
    pub position_id: i64,
    pub asset: String,
    pub strike_lo: String,
    pub strike_hi: String,
    pub t_open_ms: i64,
    pub t_close_ms: i64,
    pub stake_points: i64,
    pub multiplier_at_tap: String,
    pub status: String,
    pub created_at_ms: i64,
    pub settlement: Option<HistorySettlement>,
}

#[derive(Debug, Serialize)]
pub struct HistorySettlement {
    pub outcome: String,
    pub points_delta: i64,
    pub settled_at_ms: i64,
}

#[derive(Debug, Serialize)]
pub struct HistoryResponse {
    pub positions: Vec<HistoryItem>,
    pub next_cursor: Option<i64>,
}

pub async fn get_history(
    State(state): State<AppState>,
    Extension(ctx): Extension<AccountCtx>,
    Query(q): Query<HistoryQuery>,
) -> Result<Json<HistoryResponse>, ApiError> {
    let limit = q.limit.clamp(1, 200);
    let cursor = q.cursor.unwrap_or(i64::MAX);
    type Row = (i64, String, sqlx::types::BigDecimal, sqlx::types::BigDecimal,
                i64, i64, i64, sqlx::types::BigDecimal, String, i64,
                Option<String>, Option<i64>, Option<i64>);
    let rows: Vec<Row> = sqlx::query_as(
        r#"SELECT p.id, p.asset, p.strike_lo, p.strike_hi, p.t_open_ms, p.t_close_ms,
                   p.stake_points, p.multiplier_at_tap, p.status, p.created_at_ms,
                   s.outcome, s.points_delta, s.settled_at_ms
            FROM positions p
            LEFT JOIN settlements s ON s.position_id = p.id
            WHERE p.account_id = $1 AND p.created_at_ms < $2
            ORDER BY p.created_at_ms DESC
            LIMIT $3"#,
    )
    .bind(ctx.id).bind(cursor).bind(limit + 1)
    .fetch_all(&state.pg).await?;

    let has_more = rows.len() as i64 > limit;
    let mut items: Vec<HistoryItem> = rows.into_iter().take(limit as usize).map(|r| HistoryItem {
        position_id: r.0,
        asset: r.1,
        strike_lo: r.2.to_string(),
        strike_hi: r.3.to_string(),
        t_open_ms: r.4,
        t_close_ms: r.5,
        stake_points: r.6,
        multiplier_at_tap: r.7.to_string(),
        status: r.8,
        created_at_ms: r.9,
        settlement: match (r.10, r.11, r.12) {
            (Some(o), Some(d), Some(s)) => Some(HistorySettlement { outcome: o, points_delta: d, settled_at_ms: s }),
            _ => None,
        },
    }).collect();
    let next_cursor = if has_more { items.last().map(|i| i.created_at_ms) } else { None };
    if !has_more { /* nothing */ } else { items.truncate(limit as usize); }
    Ok(Json(HistoryResponse { positions: items, next_cursor }))
}
```

- [ ] **Step 12.2: Implement `GET /v1/positions/:id`**

Append to `handlers/positions.rs`:

```rust
use axum::extract::Path;

#[derive(Debug, serde::Serialize)]
pub struct PositionDetail {
    pub position_id: i64,
    pub asset: String,
    pub strike_lo: String,
    pub strike_hi: String,
    pub t_open_ms: i64,
    pub t_close_ms: i64,
    pub stake_points: i64,
    pub multiplier_at_tap: String,
    pub status: String,
    pub created_at_ms: i64,
    pub settlement: Option<crate::handlers::me::HistorySettlement>,
}

pub async fn get_position_by_id(
    State(state): State<AppState>,
    Extension(ctx): Extension<AccountCtx>,
    Path(id): Path<i64>,
) -> Result<axum::Json<PositionDetail>, ApiError> {
    type Row = (i64, i64, String, sqlx::types::BigDecimal, sqlx::types::BigDecimal,
                i64, i64, i64, sqlx::types::BigDecimal, String, i64,
                Option<String>, Option<i64>, Option<i64>);
    let row: Option<Row> = sqlx::query_as(
        r#"SELECT p.id, p.account_id, p.asset, p.strike_lo, p.strike_hi,
                   p.t_open_ms, p.t_close_ms, p.stake_points, p.multiplier_at_tap,
                   p.status, p.created_at_ms,
                   s.outcome, s.points_delta, s.settled_at_ms
            FROM positions p
            LEFT JOIN settlements s ON s.position_id = p.id
            WHERE p.id = $1"#,
    )
    .bind(id).fetch_optional(&state.pg).await?;
    let r = row.ok_or(ApiError::NotFound)?;
    if r.1 != ctx.id { return Err(ApiError::Forbidden); }
    Ok(axum::Json(PositionDetail {
        position_id: r.0,
        asset: r.2,
        strike_lo: r.3.to_string(),
        strike_hi: r.4.to_string(),
        t_open_ms: r.5,
        t_close_ms: r.6,
        stake_points: r.7,
        multiplier_at_tap: r.8.to_string(),
        status: r.9,
        created_at_ms: r.10,
        settlement: match (r.11, r.12, r.13) {
            (Some(o), Some(d), Some(s)) => Some(crate::handlers::me::HistorySettlement {
                outcome: o, points_delta: d, settled_at_ms: s,
            }),
            _ => None,
        },
    }))
}
```

- [ ] **Step 12.3: Wire the routes**

Edit `lib.rs` `authenticated`:

```rust
.route("/v1/me/history", get(handlers::me::get_history))
.route("/v1/positions/:id", get(handlers::positions::get_position_by_id))
```

- [ ] **Step 12.4: Write the integration test**

Write `games/tap-trading/backend/api/tests/get_history.rs`:

```rust
mod common;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use common::TestApp;
use serde_json::json;
use std::sync::Arc;
use tap_trading_api::aggregator_client::AggregatorClient;
use tower::util::ServiceExt;
use uuid::Uuid;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn tap_once(app: &TestApp, account: &str, rid: Uuid, t_open_ms: i64) {
    let body = json!({
        "client_request_id": rid,
        "asset": "BTC", "strike_lo": 49999.5, "strike_hi": 50000.5,
        "t_open_ms": t_open_ms, "t_close_ms": t_open_ms + 5_000,
        "stake_points": 100, "client_multiplier": 1.35,
        "oracle_seq_at_tap": 12345, "oracle_run_id_at_tap": 999
    });
    let _ = app.router.clone().oneshot(
        Request::builder().method("POST").uri("/v1/positions")
            .header("x-account-id", account)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string())).unwrap()
    ).await.unwrap();
}

#[tokio::test]
async fn history_returns_open_positions() {
    let pinned_now: i64 = 1_748_345_673_000;
    std::env::set_var("TAP_TEST_NOW_MS", pinned_now.to_string());
    let mock = MockServer::start().await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(json!({
        "asset": "BTC", "run_id": 999, "seq": 12345, "ts_ms": pinned_now,
        "mid": 50000.0, "vol_annualized": 0.80, "source_count": 3
    }))).mount(&mock).await;
    let uri = mock.uri();
    let app = TestApp::start_with(|s| { s.aggregator = Arc::new(AggregatorClient::new(uri)); }).await;

    tap_once(&app, "histuser", Uuid::from_u128(1), 1_748_345_670_000).await;

    let resp = app.router.clone().oneshot(
        Request::builder().uri("/v1/me/history?limit=10")
            .header("x-account-id", "histuser").body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 16 * 1024).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["positions"].as_array().unwrap().len(), 1);
    assert_eq!(v["positions"][0]["status"], "OPEN");
    assert!(v["positions"][0]["settlement"].is_null());
}

#[tokio::test]
async fn position_by_id_forbidden_for_other_account() {
    let pinned_now: i64 = 1_748_345_673_000;
    std::env::set_var("TAP_TEST_NOW_MS", pinned_now.to_string());
    let mock = MockServer::start().await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(json!({
        "asset": "BTC", "run_id": 999, "seq": 12345, "ts_ms": pinned_now,
        "mid": 50000.0, "vol_annualized": 0.80, "source_count": 3
    }))).mount(&mock).await;
    let uri = mock.uri();
    let app = TestApp::start_with(|s| { s.aggregator = Arc::new(AggregatorClient::new(uri)); }).await;

    tap_once(&app, "owner", Uuid::from_u128(2), 1_748_345_670_000).await;
    let (pid,): (i64,) = sqlx::query_as("SELECT id FROM positions LIMIT 1").fetch_one(&app.pg).await.unwrap();

    let resp = app.router.clone().oneshot(
        Request::builder().uri(format!("/v1/positions/{pid}"))
            .header("x-account-id", "intruder").body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn position_by_id_404_for_unknown() {
    std::env::set_var("TAP_TEST_NOW_MS", "1748345673000");
    let app = TestApp::start().await;
    let resp = app.router.clone().oneshot(
        Request::builder().uri("/v1/positions/999999")
            .header("x-account-id", "ghost").body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
```

- [ ] **Step 12.5: Run + verify**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-api --test get_history -- --test-threads=1
cargo clippy --all-targets -- -D warnings
```

Expected: 3 tests pass.

- [ ] **Step 12.6: Commit**

```bash
git add games/tap-trading/backend/api/
git commit -m "feat(tick-api): add me history and position by id"
```

---

## Task 13 — `WS /stream` re-broadcast

API maintains one outbound WS to the aggregator; received frames are forwarded byte-for-byte to a `tokio::sync::broadcast::Sender<String>`. Each connected client gets a `Receiver` and pumps frames onto its own WS. Independent 5 s ping to clients.

**Files:**
- Create: `games/tap-trading/backend/api/src/handlers/stream.rs`
- Modify: `games/tap-trading/backend/api/src/aggregator_client.rs`
- Modify: `games/tap-trading/backend/api/src/main.rs`
- Modify: `games/tap-trading/backend/api/src/lib.rs`
- Create: `games/tap-trading/backend/api/tests/ws_stream.rs`

- [ ] **Step 13.1: Implement the aggregator WS subscriber task**

Append to `aggregator_client.rs`:

```rust
use futures_util::{SinkExt, StreamExt};
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message;

/// Spawn a long-lived task that connects to the aggregator's `WS /stream` and
/// re-broadcasts every received frame (Text frames only) to `tx`. On
/// disconnect, it sleeps 500ms and reconnects.
pub fn spawn_aggregator_subscriber(base_url: String, tx: broadcast::Sender<String>) {
    tokio::spawn(async move {
        loop {
            let ws_url = base_url
                .replacen("http://", "ws://", 1)
                .replacen("https://", "wss://", 1) + "/stream";
            tracing::info!(%ws_url, "connecting to aggregator WS");
            let conn = tokio_tungstenite::connect_async(&ws_url).await;
            match conn {
                Ok((ws, _)) => {
                    let (mut sink, mut stream) = ws.split();
                    while let Some(msg) = stream.next().await {
                        match msg {
                            Ok(Message::Text(t)) => {
                                let _ = tx.send(t);
                            }
                            Ok(Message::Ping(p)) => { let _ = sink.send(Message::Pong(p)).await; }
                            Ok(Message::Close(_)) | Err(_) => break,
                            _ => {}
                        }
                    }
                }
                Err(e) => tracing::warn!(error = %e, "aggregator WS connect failed"),
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    });
}
```

Buffer rationale: `broadcast::channel(256)` (~4 s at 20 Hz × 3 assets = 60 msg/s). A slow client that lags by more than 256 frames gets `RecvError::Lagged`; we drop the connection so it can reconnect and re-sync via the aggregator. We don't try to be lossless on the API fanout — the canonical history is in Postgres / the aggregator.

- [ ] **Step 13.2: Spawn the subscriber from `main.rs`**

In `main.rs`, immediately after `let (broadcast_tx, _) = broadcast::channel(256);` add:

```rust
    tap_trading_api::aggregator_client::spawn_aggregator_subscriber(
        std::env::var("TAP_AGGREGATOR_URL").unwrap(),
        broadcast_tx.clone(),
    );
```

Tests do NOT call this — they push frames directly through `state.broadcast.send(...)` so they don't need a real upstream WS server.

- [ ] **Step 13.3: Implement the `/stream` handler**

Write `games/tap-trading/backend/api/src/handlers/stream.rs`:

```rust
//! WS /stream — re-broadcast aggregator frames to clients.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;

use crate::state::AppState;

pub async fn ws_stream(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle(socket, state))
}

async fn handle(mut socket: WebSocket, state: AppState) {
    let mut rx = state.broadcast.subscribe();
    let mut ping = tokio::time::interval(Duration::from_secs(5));
    ping.tick().await; // skip the immediate tick
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
```

`/stream` does NOT require `X-Account-Id` (the upstream stream is public — the aggregator already accepts anonymous subscribers). Mount on the `public` router.

- [ ] **Step 13.4: Wire the route**

Edit `lib.rs`. Add `pub mod stream` via `handlers/mod.rs`, and on `public`:

```rust
use axum::routing::any;

let public = Router::new()
    .route("/healthz", get(handlers::health::healthz))
    .route("/metrics", get(handlers::health::metrics))
    .route("/stream", any(handlers::stream::ws_stream));
```

- [ ] **Step 13.5: Write the WS integration test**

Write `games/tap-trading/backend/api/tests/ws_stream.rs`:

```rust
mod common;

use common::TestApp;
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::time::timeout;

#[tokio::test]
async fn client_receives_broadcasted_frames() {
    let app = TestApp::start().await;

    // Bind the router to a real TCP port so we can open a WS to it.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app.router.clone();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let url = format!("ws://{}/stream", addr);
    let (mut ws, _) = timeout(Duration::from_secs(2), tokio_tungstenite::connect_async(url))
        .await.unwrap().unwrap();

    // Push a synthetic frame through the broadcast.
    let synth = r#"{"type":"tick","asset":"BTC","run_id":1,"seq":1,"ts_ms":0,"mid":50000.0,"vol_annualized":0.8,"source_count":3}"#;
    let _ = app.state.broadcast.send(synth.to_string());

    let received = timeout(Duration::from_secs(2), ws.next()).await
        .expect("WS frame did not arrive").unwrap().unwrap();
    assert!(received.is_text());
    assert_eq!(received.into_text().unwrap(), synth);

    // Cleanly close.
    let _ = ws.close(None).await;
}

#[tokio::test]
async fn slow_client_dropped_on_lag() {
    let app = TestApp::start().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app.router.clone();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap(); });

    let url = format!("ws://{}/stream", addr);
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    // Don't read from `ws` — let it lag.

    // Push 300 frames to overflow the 256 buffer.
    for i in 0..300 {
        let _ = app.state.broadcast.send(format!(r#"{{"type":"heartbeat","ts_ms":{i}}}"#));
    }
    // Allow the server to detect the lag and close.
    tokio::time::sleep(Duration::from_millis(200)).await;
    // Drain pending frames until close or none.
    let mut closed = false;
    while let Ok(Some(msg)) = timeout(Duration::from_millis(500), ws.next()).await {
        if let Ok(m) = msg {
            if m.is_close() { closed = true; break; }
        } else { break; }
    }
    assert!(closed, "expected server to close the connection on lag");
}
```

- [ ] **Step 13.6: Run + verify**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-api --test ws_stream
cargo clippy --all-targets -- -D warnings
```

Expected: 2 tests pass within a few seconds. The slow-client test may run for ~700 ms.

- [ ] **Step 13.7: Commit**

```bash
git add games/tap-trading/backend/api/
git commit -m "feat(tick-api): add ws /stream re-broadcast"
```

---

## Task 14 — Prometheus metrics

`taps_committed_total`, `taps_rejected_total{reason}`, `tap_handler_duration_seconds`.

**Files:**
- Create: `games/tap-trading/backend/api/src/metrics.rs`
- Modify: `games/tap-trading/backend/api/src/state.rs`
- Modify: `games/tap-trading/backend/api/src/handlers/positions.rs`
- Modify: `games/tap-trading/backend/api/src/handlers/health.rs`
- Modify: `games/tap-trading/backend/api/src/main.rs`
- Create: `games/tap-trading/backend/api/tests/metrics.rs`

- [ ] **Step 14.1: Write `metrics.rs`**

Write `games/tap-trading/backend/api/src/metrics.rs`:

```rust
//! Prometheus counters/histograms. Single shared registry per process.

use prometheus::{
    register_counter_vec_with_registry, register_histogram_vec_with_registry,
    CounterVec, HistogramVec, Registry,
};
use std::sync::Arc;

pub struct Metrics {
    pub registry: Registry,
    pub taps_committed_total: CounterVec,
    pub taps_rejected_total: CounterVec,
    pub tap_handler_duration_seconds: HistogramVec,
}

impl Metrics {
    pub fn new() -> Arc<Self> {
        let registry = Registry::new();
        let taps_committed_total = register_counter_vec_with_registry!(
            "taps_committed_total", "Total successful tap commits.",
            &["asset"], registry
        ).unwrap();
        let taps_rejected_total = register_counter_vec_with_registry!(
            "taps_rejected_total", "Total rejected tap commits by reason.",
            &["reason"], registry
        ).unwrap();
        let tap_handler_duration_seconds = register_histogram_vec_with_registry!(
            "tap_handler_duration_seconds", "Tap handler wall time in seconds.",
            &["outcome"], registry
        ).unwrap();
        Arc::new(Self {
            registry, taps_committed_total, taps_rejected_total, tap_handler_duration_seconds,
        })
    }
}
```

- [ ] **Step 14.2: Add `metrics: Arc<Metrics>` to `AppState`**

Edit `state.rs`:

```rust
use crate::metrics::Metrics;
// ...
pub metrics: std::sync::Arc<Metrics>,
```

Edit `main.rs` and `TestApp::start_with` to populate it.

- [ ] **Step 14.3: Instrument the handler**

In `handlers/positions.rs` `post_position`, wrap the body in a timer + record on outcome:

```rust
    let started = std::time::Instant::now();
    let outcome_label;
    let result: Result<(axum::http::StatusCode, Json<TapResponse>), ApiError> = async {
        // ... existing body, returning Ok/Err
    }.await;
    match &result {
        Ok(_) => {
            state.metrics.taps_committed_total.with_label_values(&[&req.asset]).inc();
            outcome_label = "ok";
        }
        Err(e) => {
            let reason = match e {
                ApiError::InvalidStake => "invalid_stake",
                ApiError::UnknownAsset => "unknown_asset",
                ApiError::LockWindow => "lock_window",
                ApiError::InvalidCell => "invalid_cell",
                ApiError::StaleQuote => "stale_quote",
                ApiError::DriftExceeded { .. } => "drift_exceeded",
                ApiError::InsufficientBalance => "insufficient_balance",
                ApiError::RateLimited { .. } => "rate_limited",
                _ => "internal",
            };
            state.metrics.taps_rejected_total.with_label_values(&[reason]).inc();
            outcome_label = "err";
        }
    }
    state.metrics.tap_handler_duration_seconds
        .with_label_values(&[outcome_label])
        .observe(started.elapsed().as_secs_f64());
    result
```

In practice the existing body is already a sequence of `?`-laden expressions; refactor by extracting it into a private async fn `tap_inner(...) -> Result<...>` and calling `let result = tap_inner(...).await;` so the metric wrapping is one place.

- [ ] **Step 14.4: Wire `/metrics`**

In `handlers/health.rs`:

```rust
use axum::extract::State;
use crate::state::AppState;
use prometheus::Encoder;

pub async fn metrics(State(state): State<AppState>) -> String {
    let encoder = prometheus::TextEncoder::new();
    let mf = state.metrics.registry.gather();
    let mut buf = Vec::new();
    encoder.encode(&mf, &mut buf).ok();
    String::from_utf8(buf).unwrap_or_default()
}
```

Update `lib.rs` so `/metrics` route picks up the state (`get(handlers::health::metrics)` — already does via `with_state`).

- [ ] **Step 14.5: Test**

Write `games/tap-trading/backend/api/tests/metrics.rs`:

```rust
mod common;

use axum::body::{to_bytes, Body};
use axum::http::Request;
use common::TestApp;
use serde_json::json;
use std::sync::Arc;
use tap_trading_api::aggregator_client::AggregatorClient;
use tower::util::ServiceExt;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn scrape(app: &TestApp) -> String {
    let resp = app.router.clone().oneshot(
        Request::builder().uri("/metrics").body(Body::empty()).unwrap()
    ).await.unwrap();
    let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn counters_move_on_commit_and_reject() {
    let pinned_now: i64 = 1_748_345_673_000;
    std::env::set_var("TAP_TEST_NOW_MS", pinned_now.to_string());
    let mock = MockServer::start().await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(json!({
        "asset": "BTC", "run_id": 999, "seq": 12345, "ts_ms": pinned_now,
        "mid": 50000.0, "vol_annualized": 0.80, "source_count": 3
    }))).mount(&mock).await;
    let uri = mock.uri();
    let app = TestApp::start_with(|s| { s.aggregator = Arc::new(AggregatorClient::new(uri)); }).await;

    // Success.
    let _ = app.router.clone().oneshot(Request::builder().method("POST").uri("/v1/positions")
        .header("x-account-id", "metrics-user").header("content-type", "application/json")
        .body(Body::from(json!({
            "client_request_id": "00000000-0000-0000-0000-000000000aaa",
            "asset": "BTC", "strike_lo": 49999.5, "strike_hi": 50000.5,
            "t_open_ms": 1_748_345_670_000_i64, "t_close_ms": 1_748_345_675_000_i64,
            "stake_points": 100, "client_multiplier": 1.35,
            "oracle_seq_at_tap": 12345, "oracle_run_id_at_tap": 999
        }).to_string())).unwrap()).await.unwrap();

    // Drift reject.
    let _ = app.router.clone().oneshot(Request::builder().method("POST").uri("/v1/positions")
        .header("x-account-id", "metrics-user").header("content-type", "application/json")
        .body(Body::from(json!({
            "client_request_id": "00000000-0000-0000-0000-000000000aab",
            "asset": "BTC", "strike_lo": 49999.5, "strike_hi": 50000.5,
            "t_open_ms": 1_748_345_670_000_i64, "t_close_ms": 1_748_345_675_000_i64,
            "stake_points": 100, "client_multiplier": 10.0,
            "oracle_seq_at_tap": 12345, "oracle_run_id_at_tap": 999
        }).to_string())).unwrap()).await.unwrap();

    let m = scrape(&app).await;
    assert!(m.contains("taps_committed_total{asset=\"BTC\"} 1"));
    assert!(m.contains("taps_rejected_total{reason=\"drift_exceeded\"} 1"));
    assert!(m.contains("tap_handler_duration_seconds"));
}
```

- [ ] **Step 14.6: Run + verify**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-api --test metrics -- --test-threads=1
cargo clippy --all-targets -- -D warnings
```

Expected: green.

- [ ] **Step 14.7: Commit**

```bash
git add games/tap-trading/backend/api/
git commit -m "feat(tick-api): expose prometheus metrics"
```

---

## Task 15 — Concurrent taps balance invariant

100 simultaneous taps for the same account, balance = N · stake. Exactly N must succeed; the rest must return 422 `insufficient_balance`. Final balance = 0. Ledger has exactly N `TAP_STAKE` rows. `SELECT … FOR UPDATE` serializes the contention.

**Files:**
- Create: `games/tap-trading/backend/api/tests/concurrency.rs`

- [ ] **Step 15.1: Write the test**

Write `games/tap-trading/backend/api/tests/concurrency.rs`:

```rust
mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::TestApp;
use serde_json::json;
use std::sync::Arc;
use tap_trading_api::aggregator_client::AggregatorClient;
use tokio::task::JoinSet;
use tower::util::ServiceExt;
use uuid::Uuid;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn n_of_100_concurrent_taps_succeed() {
    let pinned_now: i64 = 1_748_345_673_000;
    std::env::set_var("TAP_TEST_NOW_MS", pinned_now.to_string());
    let mock = MockServer::start().await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(json!({
        "asset": "BTC", "run_id": 999, "seq": 12345, "ts_ms": pinned_now,
        "mid": 50000.0, "vol_annualized": 0.80, "source_count": 3
    }))).mount(&mock).await;
    let uri = mock.uri();
    let app = TestApp::start_with(|s| { s.aggregator = Arc::new(AggregatorClient::new(uri)); }).await;

    // Lazy-create + set balance to exactly 5 · 100 = 500.
    let _ = app.router.clone().oneshot(Request::builder().uri("/v1/me")
        .header("x-account-id", "concurrent").body(Body::empty()).unwrap()).await.unwrap();
    sqlx::query("UPDATE accounts SET balance = 500 WHERE external_id = $1")
        .bind("concurrent").execute(&app.pg).await.unwrap();

    // Bypass the rate limiter for this test by spamming separate request_ids
    // ON one account. Rate limit caps at 10/sec so we need to disable it for
    // the test — handle by skipping the limiter on this test via a dedicated
    // probe route. SIMPLE: tune the limit via env, OR fire serially with
    // small delays. For invariant correctness, fire serially but in many
    // concurrent tokio tasks WITHOUT rate-limit interference by pointing all
    // tasks at a probe that runs the inner handler without the limiter.
    //
    // Implementation: expose `router_without_rate_limit(state)` from lib.rs
    // (already present analogously to `router_with_rate_limit_probe`); this
    // test uses that variant.
    let router = tap_trading_api::router_without_rate_limit(app.state.clone());

    let mut joins = JoinSet::new();
    for _ in 0..100 {
        let r = router.clone();
        let rid = Uuid::new_v4();
        joins.spawn(async move {
            let body = json!({
                "client_request_id": rid,
                "asset": "BTC", "strike_lo": 49999.5, "strike_hi": 50000.5,
                "t_open_ms": 1_748_345_670_000_i64, "t_close_ms": 1_748_345_675_000_i64,
                "stake_points": 100, "client_multiplier": 1.35,
                "oracle_seq_at_tap": 12345, "oracle_run_id_at_tap": 999
            });
            let resp = r.oneshot(Request::builder().method("POST").uri("/v1/positions")
                .header("x-account-id", "concurrent").header("content-type", "application/json")
                .body(Body::from(body.to_string())).unwrap()).await.unwrap();
            resp.status()
        });
    }
    let mut ok = 0; let mut insuf = 0; let mut other = 0;
    while let Some(r) = joins.join_next().await {
        match r.unwrap() {
            StatusCode::CREATED => ok += 1,
            StatusCode::UNPROCESSABLE_ENTITY => insuf += 1,
            _ => other += 1,
        }
    }
    assert_eq!(ok, 5);
    assert_eq!(insuf, 95);
    assert_eq!(other, 0);

    let (balance,): (i64,) = sqlx::query_as("SELECT balance FROM accounts WHERE external_id = $1")
        .bind("concurrent").fetch_one(&app.pg).await.unwrap();
    assert_eq!(balance, 0);
    let (ledger_count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM points_ledger WHERE kind = 'TAP_STAKE'"
    ).fetch_one(&app.pg).await.unwrap();
    assert_eq!(ledger_count, 5);
}
```

To support the test, add to `lib.rs`:

```rust
#[doc(hidden)]
pub fn router_without_rate_limit(state: AppState) -> Router {
    use axum::middleware::from_fn_with_state;
    use axum::routing::{get, post};
    let public = Router::new()
        .route("/healthz", get(handlers::health::healthz))
        .route("/metrics", get(handlers::health::metrics));
    let authenticated = Router::new()
        .route("/v1/me", get(handlers::me::get_me))
        .route("/v1/me/history", get(handlers::me::get_history))
        .route("/v1/positions", post(handlers::positions::post_position))
        .route("/v1/positions/:id", get(handlers::positions::get_position_by_id))
        .layer(from_fn_with_state(state.clone(), middleware::account_id::account_id_middleware));
    public.merge(authenticated).with_state(state)
}
```

The test-only variant is gated behind `#[doc(hidden)]` and named explicitly so reviewers don't mistake it for the production router. Production code still uses `router(state)`.

- [ ] **Step 15.2: Run + verify**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-api --test concurrency
cargo clippy --all-targets -- -D warnings
```

Expected: green. The test typically runs in 3–6 s on a warm postgres container.

- [ ] **Step 15.3: Commit**

```bash
git add games/tap-trading/backend/api/
git commit -m "test(tick-api): concurrent taps balance invariant"
```

---

## Task 16 — Lock-at-tap invariant

`MATH_SPEC §4.3`: the value written to `positions.multiplier_at_tap` is the server's recompute, not the client's claim — even when both pass the drift gate. Send a `client_multiplier` that differs from the server's by < 3%; assert the DB row matches a test-side independent recompute.

**Files:**
- Create: `games/tap-trading/backend/api/tests/lock_at_tap.rs`

- [ ] **Step 16.1: Write the test**

Write `games/tap-trading/backend/api/tests/lock_at_tap.rs`:

```rust
mod common;

use axum::body::Body;
use axum::http::Request;
use common::TestApp;
use serde_json::json;
use std::sync::Arc;
use tap_trading_api::aggregator_client::AggregatorClient;
use tap_trading_pricing_engine::{compute_multiplier, AssetSymbol, Cell, OracleState, PricingConfig};
use tower::util::ServiceExt;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn committed_multiplier_matches_server_recompute_not_client_claim() {
    let pinned_now: i64 = 1_748_345_673_000;
    std::env::set_var("TAP_TEST_NOW_MS", pinned_now.to_string());

    let mid = 50_000.0;
    let vol = 0.80;
    let t_open_ms: i64 = 1_748_345_670_000;
    let t_close_ms: i64 = t_open_ms + 5_000;
    let strike_lo = 49_999.5;
    let strike_hi = 50_000.5;

    let mock = MockServer::start().await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(json!({
        "asset": "BTC", "run_id": 999, "seq": 12345, "ts_ms": pinned_now,
        "mid": mid, "vol_annualized": vol, "source_count": 3
    }))).mount(&mock).await;
    let uri = mock.uri();
    let app = TestApp::start_with(|s| { s.aggregator = Arc::new(AggregatorClient::new(uri)); }).await;

    // Independent recompute — must match the handler's compute_multiplier call.
    let expected_server_mult = compute_multiplier(
        &Cell { asset: AssetSymbol::Btc, strike_lo, strike_hi,
                t_open_ms: t_open_ms as u64, t_close_ms: t_close_ms as u64 },
        &OracleState { asset: AssetSymbol::Btc, spot: mid,
                       sigma_annualized: vol, timestamp_ms: pinned_now as u64 },
        &PricingConfig::default(),
        pinned_now as u64,
    );
    // Send a client multiplier that's 2% off — within the 3% gate but distinct.
    let client_mult = expected_server_mult * 1.02;

    let body = json!({
        "client_request_id": "00000000-0000-0000-0000-0000000000cc",
        "asset": "BTC", "strike_lo": strike_lo, "strike_hi": strike_hi,
        "t_open_ms": t_open_ms, "t_close_ms": t_close_ms,
        "stake_points": 100, "client_multiplier": client_mult,
        "oracle_seq_at_tap": 12345, "oracle_run_id_at_tap": 999
    });
    let _ = app.router.clone().oneshot(Request::builder().method("POST").uri("/v1/positions")
        .header("x-account-id", "lock-tester").header("content-type", "application/json")
        .body(Body::from(body.to_string())).unwrap()).await.unwrap();

    // DB value must match server recompute, NOT the client claim.
    let (committed,): (sqlx::types::BigDecimal,) = sqlx::query_as(
        "SELECT multiplier_at_tap FROM positions"
    ).fetch_one(&app.pg).await.unwrap();
    let committed_f64: f64 = committed.to_string().parse().unwrap();
    let delta_to_server = (committed_f64 - expected_server_mult).abs();
    let delta_to_client = (committed_f64 - client_mult).abs();
    assert!(delta_to_server < 1e-3,
        "committed {committed_f64} should match server {expected_server_mult} (delta {delta_to_server})");
    assert!(delta_to_client > delta_to_server,
        "committed {committed_f64} should NOT match client {client_mult} (delta_client {delta_to_client} vs server {delta_to_server})");
}
```

The two assertions together pin the invariant: the committed value is close to the server's value AND further from the client's value. If a regression flips the field assignment so the client's value wins, the second assertion catches it.

- [ ] **Step 16.2: Run + verify**

```bash
cd games/tap-trading/backend && cargo test -p tap-trading-api --test lock_at_tap -- --test-threads=1
cargo clippy --all-targets -- -D warnings
```

Expected: green.

- [ ] **Step 16.3: Commit**

```bash
git add games/tap-trading/backend/api/tests/lock_at_tap.rs
git commit -m "test(tick-api): lock-at-tap invariant"
```

---

## Final verification

- [ ] **Step F1: All 16 commits land on `feat/tap-trading`**

```bash
git log --oneline -16
```

Expected: 16 commits with the subjects from the Commit map, newest at top, on top of the existing Plan A + B history.

- [ ] **Step F2: Tick workspace is fully green**

```bash
cd games/tap-trading/backend && cargo check && cargo clippy --all-targets -- -D warnings && cargo test -- --test-threads=1
```

Expected: green. Total test count grows by ~25 integration tests + ~15 unit tests; runtime is dominated by container boot (~5 s per test process). On CI this is ~3–5 min for the full Tick suite.

- [ ] **Step F3: Root workspace still builds**

```bash
cd "$(git rev-parse --show-toplevel)" && cargo check --workspace && cargo clippy --workspace -- -D warnings && cargo test --workspace
```

Expected: same green baseline as before this plan started.

- [ ] **Step F4: Migration applies in dev-env init**

```bash
./scripts/init-worktree-dev.sh
```

Expected: `tap-trading-migrate run` (Plan B) re-applies the amended migration without error.

- [ ] **Step F5: Document deviations for the next reviewer**

Add to the PR description (do NOT add to commit bodies; CLAUDE.md prefers no body):

> This plan deviates from spec deliberately:
> - **Rate-limit burst.** `SYSTEM_DESIGN.md §3.3` says burst 20; ADR-0009 §6 says burst 10. We implement ADR-0009's number — it's the more recent and binding contract.
> - **Tap-commit response.** `SYSTEM_DESIGN.md §3.3` returns `multiplier_locked` + `expected_payout`; ADR-0009 §4 returns `multiplier_at_tap` + `status`. We implement ADR-0009 — clients compute `expected_payout` themselves; `status` lets the idempotent replay path return terminal state.
> - **Schema delta amended in place.** Plan A's `20260523120000_create_tick_schema.sql` is staged on this branch and not yet merged; per ADR-0009 §3 we edit it in place rather than ship a `_add_columns` follow-up.
>
> The two `SYSTEM_DESIGN.md` deviations (rate-limit burst, tap-commit response) need a docs PR against `SYSTEM_DESIGN.md` before the next plan lands.

---

## Plan F preview (not in scope here)

This plan completes the API service. Subsequent plans pick up:

- **Plan D** ships `tap-trading-settlement-worker`. It LISTENs on `tap_new_position`, hydrates an in-memory open-position cache, monitors the oracle stream, and writes `settlements` rows + flips `positions.status`. This plan's `OPEN` positions become Plan D's input; the schema is already wired (UNIQUE on `settlements.position_id` enforces at-most-once).
- **zkLogin auth.** ADR-0009 §1 lists the swap: replace `X-Account-Id` middleware with a JWT verifier that populates `AccountCtx` from validated claims. Handlers don't change.
- **Tier-2+ stakes.** Extend `STAKE_TIERS_V1` in `validation.rs` (or fold into a per-account-tier lookup) once Tier 2 is defined in `PRD.md`.
- **Leaderboard, quests, share-render endpoints.** Out of MVP scope per `SYSTEM_DESIGN.md §3.4/§3.5/§3.7`; phase-in when the product calls for them.
