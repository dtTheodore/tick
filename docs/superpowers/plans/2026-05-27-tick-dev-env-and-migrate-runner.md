# Tick Dev-Env Wiring + Migration Runner — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the second Tick backend plan. Two deliverables: (1) a `tap-trading-migrate` crate that exposes both a CLI (`run`, `info`) and a library function `run_migrations(&PgPool)` so the three Plan C/D/E services and their integration tests share one canonical migration path; (2) the worktree dev-env wiring (`scripts/worktree-env.sh`, `scripts/sync-service-envs.sh`, `scripts/ensure-worktree-coherence.sh`, `mprocs.yaml`, `scripts/start-headless.sh` — the "four must move together", plus the headless runner that the contract treats as a fifth) for three future services (`tap-trading-api`, `tap-trading-oracle-aggregator`, `tap-trading-settlement-worker`). The service crates don't exist yet; the env wiring is staged so Plans C/D/E can boot cleanly when they land.

**Architecture:** `tap-trading-migrate` lives inside the self-contained workspace at `games/tap-trading/backend/` (created by Plan A). It embeds Plan A's genesis migration via `sqlx::migrate!("../migrations")` so the resulting binary needs no filesystem at deploy time. The library function is the load-bearing surface: every Plan C/D/E integration test starts a Testcontainers Postgres and calls `tap_trading_migrate::run_migrations(&pool).await?` exactly once per test, eliminating drift between "the migrations CI applies" and "the migrations tests apply". The CLI is a thin wrapper around the same library so the operator path (`docker run … migrate run`) and the test path can never disagree. Dev-env wiring follows the repo's existing `worktree-env.sh → sync-service-envs.sh → ensure-worktree-coherence.sh → mprocs.yaml + start-headless.sh` chain — one new entry per layer per service, with `tap-migrate` ordered as a synchronous foreground guard *before* the three long-running services start.

**Tech Stack:** Rust 2021, `sqlx` 0.8 (`postgres`, `runtime-tokio-rustls`, `migrate` features), `tokio` 1 (`rt-multi-thread`, `macros`), `clap` 4 (derive), `thiserror` 1, `anyhow` 1, `tracing` + `tracing-subscriber`. Migration table name is overridden to `_tap_sqlx_migrations` to avoid colliding with platform's default `_sqlx_migrations` table on the shared `PLATFORM_DB_URL`. Integration tests use `testcontainers` 0.23 + `testcontainers-modules` 0.11 (`postgres` feature).

**Spec:** `games/tap-trading/docs/SYSTEM_DESIGN.md §2` (the eight tables already shipped in Plan A's `20260523120000_create_tick_schema.sql`), `games/tap-trading/docs/SYSTEM_DESIGN.md §3` (api), `§5.2` (settlement-worker), and `ORACLE_SPEC.md` (oracle-aggregator) — only their port and env requirements; the crate implementations are out of scope. Repo `CLAUDE.md` "Worktrees & local-dev contract" governs the dev-env edits.

**Spec deviations / corrections (record before writing code):**
- **Migration-table collision with the platform schema.** The repo's existing `init-worktree-dev.sh` line 78 runs `sqlx migrate run --database-url "$PLATFORM_DB_URL" --source "$REPO_ROOT/migrations"` which uses the default `_sqlx_migrations` table. If `tap-trading-migrate` also writes to the same default table on the same database, the two migration sets cross-reference each other's checksums and the next migration on either side trips a checksum-mismatch error. **Resolution:** `tap-trading-migrate` overrides `Migrator::table_name` to `_tap_sqlx_migrations`. Plan A's table names (`accounts`, `positions`, `points_ledger`, `streaks`, `settlements`, `daily_quests`, `snapshots`, `flags`) were grep-checked against `migrations/*.sql` at root and there is **no table-name collision** with platform tables (`users`, `point_events`, `sessions`). The two migration sets coexist in the `public` schema of the same database, distinguished only by table prefixes — this matches the read/write pattern (each service owns its own tables, no cross-table joins) and avoids an extra Postgres database for now. Recorded as ADR fodder for the first Tick PR.
- **Plan A's File map promised 8 per-table migration files; the executed result was 1 consolidated file** (`20260523120000_create_tick_schema.sql`). The library API embeds the directory, so this is invisible to callers — but the engineer reading Plan A and then this one should not be surprised by the count.
- **Service crates referenced here do not yet exist.** `tap-trading-api`, `tap-trading-oracle-aggregator`, `tap-trading-settlement-worker` are Plan C/D/E concerns. The env wiring is staged so the FIRST commit of those plans only needs to add a `bin` target; ports, env files, and process entries are already correct.

**Verification baseline:** before starting, confirm Plan A's deliverables are green:

```bash
cd games/tap-trading/backend && cargo check --workspace && cargo test --workspace && cargo clippy --workspace -- -D warnings
cd "$(git rev-parse --show-toplevel)" && cargo check --workspace && cargo test --workspace && cargo clippy --workspace -- -D warnings
```

Both must pass with no warnings. After every Rust-touching commit in this plan, re-run the Tick-workspace commands. Script-only commits (Tasks 4–7) verify against `shellcheck` (if installed) and `./scripts/ensure-worktree-coherence.sh` exiting 0 against a freshly initialized worktree.

---

## Commit map

| # | Subject | Scope |
|---|---------|-------|
| 1 | `chore(tick-migrate): scaffold migrate crate` | New crate registered in workspace; deps wired in `[workspace.dependencies]`; empty `main.rs` + `lib.rs` stub; `cargo check -p tap-trading-migrate` passes. |
| 2 | `feat(tick-migrate): implement run and library` | `pub async fn run_migrations(pool: &PgPool)` plus `MigrateError`; clap `Cli { Run, Info }` enum; `Run` wired; integration test against Testcontainers Postgres asserts the 8 Tick tables exist post-run. |
| 3 | `feat(tick-migrate): implement info subcommand` | `Info` lists applied migrations from `_tap_sqlx_migrations`; integration test runs `run` then `info`, asserts exactly 1 applied row, prints version + description. |
| 4 | `feat(tick-dev-env): add tap service ports` | `scripts/worktree-env.sh` exports `TAP_API_PORT`, `TAP_AGGREGATOR_PORT`, `TAP_WORKER_METRICS_PORT` plus URL helpers; `init-worktree-dev.sh` writes them into `.local/.env`. |
| 5 | `feat(tick-dev-env): sync envs and coherence` | `scripts/sync-service-envs.sh` writes three new `.env` files; `scripts/ensure-worktree-coherence.sh` adds three `check_port` calls + URL checks. |
| 6 | `feat(tick-dev-env): add tap procs to mprocs` | `mprocs.yaml` gains `tap-migrate` (one-shot, exits 0) and `tap-aggregator`, `tap-worker`, `tap-api` (long-running; will fail-to-find-crate until Plans C/D/E ship — documented). |
| 7 | `feat(tick-dev-env): add tap procs to headless` | `scripts/start-headless.sh` runs `tap-migrate` synchronously (NOT via `start_proc`) as a guard, then `start_proc` for the three long-running services; PIDs/logs follow existing `tmp/` convention. |

Each Rust-touching commit (1–3) must independently pass `cargo check && cargo test && cargo clippy -- -D warnings` from `games/tap-trading/backend/`. Script-only commits (4–7) verify via `shellcheck scripts/*.sh` (if available) and `./scripts/ensure-worktree-coherence.sh --quiet` exiting 0 after `./scripts/init-worktree-dev.sh` from a clean `.local/` state. Commit 6 additionally requires `python3 -c "import yaml; yaml.safe_load(open('mprocs.yaml'))"` to succeed.

---

## File map

### Created files

| Path | Responsibility |
|------|----------------|
| `games/tap-trading/backend/migrate/Cargo.toml` | Crate metadata: name `tap-trading-migrate`, both `[[bin]]` and `[lib]` targets. |
| `games/tap-trading/backend/migrate/src/lib.rs` | `pub async fn run_migrations(pool: &PgPool) -> Result<(), MigrateError>`; `pub enum MigrateError`; `pub async fn list_applied(pool: &PgPool) -> Result<Vec<AppliedMigration>, MigrateError>`; `pub const MIGRATION_TABLE: &str = "_tap_sqlx_migrations"`. |
| `games/tap-trading/backend/migrate/src/main.rs` | Clap CLI: `Cli { subcommand: Command }`, `Command::{ Run, Info }`. Reads `TAP_DB_URL` (preferred) or `PLATFORM_DB_URL` from env; opens single `PgPool`; dispatches. |
| `games/tap-trading/backend/migrate/tests/migrate.rs` | Integration test against `testcontainers` Postgres image: `run_migrations` succeeds; queries `information_schema.tables` for the 8 Tick tables; `list_applied` returns exactly 1 row. |
| `games/tap-trading/backend/api/.env.example` | Documents `TAP_API_PORT`, `TAP_DB_URL`, `TAP_AGGREGATOR_WS_URL`, `RUST_LOG`. Doc only — the live file is written by `sync-service-envs.sh`. |
| `games/tap-trading/backend/oracle-aggregator/.env.example` | Documents `TAP_AGGREGATOR_PORT`, `RUST_LOG`. Future Pyth Hermes / CEX endpoints land with Plan C. |
| `games/tap-trading/backend/settlement-worker/.env.example` | Documents `TAP_WORKER_METRICS_PORT`, `TAP_DB_URL`, `TAP_AGGREGATOR_WS_URL`, `RUST_LOG`. |

### Modified files

| Path | Change |
|------|--------|
| `games/tap-trading/backend/Cargo.toml` | Add `"migrate"` to `[workspace.members]`. Add `tokio`, `sqlx`, `clap`, `thiserror`, `anyhow`, `tracing`, `tracing-subscriber`, `testcontainers`, `testcontainers-modules` to `[workspace.dependencies]`. |
| `scripts/worktree-env.sh` | Add three `export TAP_*_PORT=...` lines and `TAP_API_URL` / `TAP_AGGREGATOR_WS_URL` / `TAP_DB_URL` derivations. |
| `scripts/init-worktree-dev.sh` | Append the new ports/URLs to the canonical `.env` heredoc. |
| `scripts/sync-service-envs.sh` | Write three new env files for the three future service crates. |
| `scripts/ensure-worktree-coherence.sh` | Add `check_port` for each of the three new ports + URL assertions. |
| `mprocs.yaml` | Add `tap-migrate`, `tap-aggregator`, `tap-worker`, `tap-api` process entries. |
| `scripts/start-headless.sh` | Run `tap-migrate` synchronously (foreground); `start_proc` for the three long-running services; add `wait_http` for `tap-api`. |

No edits to root `Cargo.toml` — the Tick workspace is self-contained per Plan A.

---

## Pre-flight (one-time, not a commit)

- [ ] **Step P1: Verify Plan A is green inside the Tick workspace**

```bash
cd games/tap-trading/backend
cargo check --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

Expected: three commands exit 0 with no warnings. Plan A landed `tap-trading-pricing-engine` plus migrations; this plan builds on that.

- [ ] **Step P2: Verify root workspace baseline is green**

```bash
cd "$(git rev-parse --show-toplevel)"
cargo check --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

Expected: same green baseline. We must not start work on top of a broken root workspace.

- [ ] **Step P3: Confirm no table-name collision with the platform schema**

```bash
comm -12 \
  <(grep -hE '^CREATE TABLE [a-z]+' games/tap-trading/backend/migrations/*.sql | awk '{print $3}' | sort -u) \
  <(grep -hE '^CREATE TABLE [a-z]+' migrations/*.sql | awk '{print $3}' | sort -u)
```

Expected: empty output (the intersection of Tick and platform table names is empty). If any table name overlaps (e.g. both define `accounts`), stop and re-open the schema decision — we'd need a separate Postgres schema or database, not just a separate migration table.

- [ ] **Step P4: Confirm Docker is running** (required for the Testcontainers-based integration test in Task 2)

```bash
docker info >/dev/null 2>&1 && echo "docker ok" || echo "docker NOT running"
```

Expected: `docker ok`. If `docker NOT running`, start Docker Desktop / colima before continuing — Task 2's integration test spins up a Postgres container.

- [ ] **Step P5: Note the existing port scheme**

```bash
grep -E 'export [A-Z_]+_PORT=' scripts/worktree-env.sh
```

Expected output (already in the repo):

```
export PLATFORM_API_PORT=$((3000 + OFFSET))
export TRIVIA_SHOW_BACKEND_PORT=$((3100 + OFFSET))
export PLATFORM_DASHBOARD_UI_PORT=$((5173 + OFFSET))
export GAME_2048_UI_PORT=$((5180 + OFFSET))
export PLATFORM_DB_PORT=$((5432 + OFFSET))
export PLATFORM_REDIS_PORT=$((6379 + OFFSET))
```

The natural continuation in the `3xxx` band: `TAP_API_PORT=3200`, `TAP_AGGREGATOR_PORT=3300`. For the worker's Prometheus-style metrics endpoint we stay in the same band rather than jumping to `9xxx` (keeps everything in one contiguous range easy to skim): `TAP_WORKER_METRICS_PORT=3400`. These three values are locked at this step; do not revisit them mid-plan.

---

## Task 1 — Scaffold `tap-trading-migrate` crate

Empty crate with the right deps wired in. After this task, `cargo check -p tap-trading-migrate` passes and the binary entrypoint exists (`tap-trading-migrate --help` returns clap's stub help). No real migration code yet — that lands in Task 2 under TDD.

**Files:**
- Modify: `games/tap-trading/backend/Cargo.toml`
- Create: `games/tap-trading/backend/migrate/Cargo.toml`
- Create: `games/tap-trading/backend/migrate/src/lib.rs`
- Create: `games/tap-trading/backend/migrate/src/main.rs`

- [ ] **Step 1.1: Add the crate to the workspace and declare its dependencies**

Edit `games/tap-trading/backend/Cargo.toml`. The current `[workspace.members]` lists only `"pricing-engine"`. Change it to:

```toml
[workspace]
resolver = "2"
members = [
    "pricing-engine",
    "migrate",
]
```

Then extend `[workspace.dependencies]` so every member crate consumes pinned versions from one place. After Plan A this section looks like:

```toml
[workspace.dependencies]
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
proptest = "1.5"
libm = "0.2"
```

Replace it with:

```toml
[workspace.dependencies]
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
proptest = "1.5"
libm = "0.2"

# Runtime
tokio = { version = "1", features = ["rt-multi-thread", "macros", "signal", "time"] }

# Database
sqlx = { version = "0.8", default-features = false, features = [
    "postgres",
    "runtime-tokio-rustls",
    "migrate",
    "chrono",
    "macros",
] }

# CLI
clap = { version = "4", features = ["derive", "env"] }

# Error handling
anyhow = "1"
thiserror = "1"

# Observability
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

# Integration-test infra (only used as dev-dep, but pinned at workspace level)
testcontainers = "0.23"
testcontainers-modules = { version = "0.11", features = ["postgres"] }
```

Pin versions here, not in member crates. This matches the rest of the repo convention.

- [ ] **Step 1.2: Write the migrate crate manifest**

Write `games/tap-trading/backend/migrate/Cargo.toml`:

```toml
[package]
name = "tap-trading-migrate"
version = "0.0.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true
publish.workspace = true

[[bin]]
name = "tap-trading-migrate"
path = "src/main.rs"

[lib]
name = "tap_trading_migrate"
path = "src/lib.rs"

[dependencies]
sqlx = { workspace = true }
tokio = { workspace = true }
clap = { workspace = true }
thiserror = { workspace = true }
anyhow = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }

[dev-dependencies]
testcontainers = { workspace = true }
testcontainers-modules = { workspace = true }
```

Both `[[bin]]` and `[lib]` are present so Plan C/D/E integration tests can `use tap_trading_migrate::run_migrations` *and* operators can `docker run … tap-trading-migrate run`.

- [ ] **Step 1.3: Stub the library**

Write `games/tap-trading/backend/migrate/src/lib.rs`:

```rust
//! Tick migration runner — library API consumed by Plan C/D/E integration
//! tests and by each service's startup as a "migrate-or-fail" boot guard.
//!
//! Spec: this plan's header. Migration source: `games/tap-trading/backend/migrations/`.

use sqlx::PgPool;
use thiserror::Error;

/// Postgres table where applied migrations are recorded.
///
/// Overridden away from sqlx's default `_sqlx_migrations` because the same
/// database hosts the platform-side migrations (which use the default name).
/// Two migration sets sharing one record table would cross-corrupt checksums.
pub const MIGRATION_TABLE: &str = "_tap_sqlx_migrations";

/// One row from `_tap_sqlx_migrations` projected into a public type so callers
/// don't depend on `sqlx::migrate` internals. Populated in Task 3.
#[derive(Debug, Clone)]
pub struct AppliedMigration {
    pub version: i64,
    pub description: String,
    pub installed_on_ms: i64,
    pub execution_time_ms: i64,
    pub success: bool,
}

/// Errors `run_migrations` and `list_applied` surface to callers.
#[derive(Debug, Error)]
pub enum MigrateError {
    #[error("sqlx migrate: {0}")]
    Sqlx(#[from] sqlx::migrate::MigrateError),
    #[error("sqlx: {0}")]
    Driver(#[from] sqlx::Error),
}

/// Apply every Tick migration to `pool`. Idempotent: rerunning is a no-op
/// once every migration's checksum matches what's already in
/// `_tap_sqlx_migrations`.
///
/// Stub for Task 1; implemented in Task 2.3.
pub async fn run_migrations(pool: &PgPool) -> Result<(), MigrateError> {
    let _ = pool;
    let _ = MIGRATION_TABLE;
    unimplemented!("Task 2.3");
}

/// List applied migrations. Stub for Task 1; implemented in Task 3.2.
pub async fn list_applied(pool: &PgPool) -> Result<Vec<AppliedMigration>, MigrateError> {
    let _ = pool;
    unimplemented!("Task 3.2");
}
```

`unimplemented!()` (not `todo!()`) so the integration test in Task 2 panics with a clear message if accidentally wired up too early.

- [ ] **Step 1.4: Stub the binary**

Write `games/tap-trading/backend/migrate/src/main.rs`:

```rust
//! Tick migration runner CLI. See `lib.rs` for the library API integration
//! tests consume.

use clap::Parser;

#[derive(Debug, Parser)]
#[command(name = "tap-trading-migrate", version, about = "Run Tick database migrations")]
struct Cli {
    /// Database URL. Falls back to `TAP_DB_URL`, then `PLATFORM_DB_URL`.
    #[arg(long, env = "TAP_DB_URL")]
    database_url: Option<String>,
}

fn main() -> anyhow::Result<()> {
    let _cli = Cli::parse();
    anyhow::bail!("Task 2.5 wires subcommands")
}
```

- [ ] **Step 1.5: Verify the workspace builds**

```bash
cd games/tap-trading/backend
cargo check -p tap-trading-migrate
cargo clippy -p tap-trading-migrate -- -D warnings
```

Expected: both succeed with no warnings. Cargo downloads the new deps on first run — that's fine. If `cargo check` errors on the `unimplemented!()` lifetime, the function signature is wrong (verify the `Result` wrap).

- [ ] **Step 1.6: Confirm the binary runs and prints help**

```bash
cd games/tap-trading/backend
cargo run -p tap-trading-migrate -- --help
```

Expected: clap prints the auto-generated help for `tap-trading-migrate`, listing `--database-url` and `--help`. Exit code 0.

- [ ] **Step 1.7: Commit**

```bash
git add games/tap-trading/backend/Cargo.toml games/tap-trading/backend/migrate/
git commit -m "chore(tick-migrate): scaffold migrate crate"
```

---

## Task 2 — Implement `run_migrations` library + `run` CLI subcommand

The load-bearing surface of the crate. Both the library function and the `run` subcommand resolve to the same code path so operators and integration tests apply the same migrations the same way.

**Files:**
- Modify: `games/tap-trading/backend/migrate/src/lib.rs`
- Modify: `games/tap-trading/backend/migrate/src/main.rs`
- Create: `games/tap-trading/backend/migrate/tests/migrate.rs`

- [ ] **Step 2.1: Write the integration test first (failing)**

Write `games/tap-trading/backend/migrate/tests/migrate.rs`:

```rust
//! Integration test: run `run_migrations` against a Testcontainers Postgres
//! and assert every Tick table exists. The same fixtures power Task 3.

use sqlx::postgres::PgPoolOptions;
use sqlx::Row;
use tap_trading_migrate::{run_migrations, MIGRATION_TABLE};
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

/// Names of the eight tables Plan A's genesis migration creates.
const TICK_TABLES: &[&str] = &[
    "accounts",
    "daily_quests",
    "flags",
    "points_ledger",
    "positions",
    "settlements",
    "snapshots",
    "streaks",
];

async fn fresh_pool() -> (testcontainers::ContainerAsync<Postgres>, sqlx::PgPool) {
    let container = Postgres::default().start().await.expect("postgres container");
    let host_port = container.get_host_port_ipv4(5432).await.expect("host port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{host_port}/postgres");
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect");
    (container, pool)
}

#[tokio::test]
async fn applies_all_tick_tables() {
    let (_container, pool) = fresh_pool().await;

    run_migrations(&pool).await.expect("run_migrations");

    for table in TICK_TABLES {
        let row = sqlx::query(
            "SELECT COUNT(*)::BIGINT AS n
             FROM information_schema.tables
             WHERE table_schema = 'public' AND table_name = $1",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .expect("query");
        let n: i64 = row.try_get("n").expect("n");
        assert_eq!(n, 1, "table {table} missing after migrate");
    }
}

#[tokio::test]
async fn rerun_is_idempotent() {
    let (_container, pool) = fresh_pool().await;
    run_migrations(&pool).await.expect("first run");
    run_migrations(&pool).await.expect("second run no-op");
}

#[tokio::test]
async fn uses_custom_migration_table() {
    let (_container, pool) = fresh_pool().await;
    run_migrations(&pool).await.expect("run");

    let row = sqlx::query(
        "SELECT COUNT(*)::BIGINT AS n
         FROM information_schema.tables
         WHERE table_schema = 'public' AND table_name = $1",
    )
    .bind(MIGRATION_TABLE)
    .fetch_one(&pool)
    .await
    .expect("query");
    let n: i64 = row.try_get("n").expect("n");
    assert_eq!(n, 1, "{MIGRATION_TABLE} should be created by sqlx");

    // The default sqlx table must NOT exist — that's the whole point of the override.
    let row = sqlx::query(
        "SELECT COUNT(*)::BIGINT AS n
         FROM information_schema.tables
         WHERE table_schema = 'public' AND table_name = '_sqlx_migrations'",
    )
    .fetch_one(&pool)
    .await
    .expect("query");
    let n: i64 = row.try_get("n").expect("n");
    assert_eq!(n, 0, "default _sqlx_migrations must not exist");
}
```

- [ ] **Step 2.2: Confirm the tests fail with the stub**

```bash
cd games/tap-trading/backend
cargo test -p tap-trading-migrate --test migrate
```

Expected: all three tests panic at `run_migrations` with `not implemented: Task 2.3`. If the container fails to start, recheck `docker info` from Pre-flight Step P4.

- [ ] **Step 2.3: Implement `run_migrations`**

Edit `games/tap-trading/backend/migrate/src/lib.rs`. Add `use std::borrow::Cow;` near the top, then replace the `run_migrations` body:

```rust
/// Build the Migrator with the custom table name. Embeds every file in
/// `games/tap-trading/backend/migrations/` at compile time so the binary
/// needs no filesystem at deploy time.
///
/// The macro path is relative to *this crate's* `Cargo.toml`
/// (`games/tap-trading/backend/migrate/`), so we step up one directory
/// to reach the workspace-root `migrations/`.
fn tick_migrator() -> sqlx::migrate::Migrator {
    let mut migrator = sqlx::migrate!("../migrations").clone();
    migrator.table_name = Cow::Borrowed(MIGRATION_TABLE);
    migrator
}

pub async fn run_migrations(pool: &PgPool) -> Result<(), MigrateError> {
    let migrator = tick_migrator();
    migrator.run(pool).await?;
    Ok(())
}
```

Why `Migrator.clone()` + mutate `table_name`: `sqlx::migrate!()` returns a `&'static Migrator`, but `Migrator: Clone`. Cloning yields an owned value whose `table_name: Cow<'static, str>` field is publicly mutable in sqlx 0.8.

- [ ] **Step 2.4: Run the integration tests again**

```bash
cd games/tap-trading/backend
cargo test -p tap-trading-migrate --test migrate
```

Expected: `applies_all_tick_tables`, `rerun_is_idempotent`, and `uses_custom_migration_table` all pass. Likely failures + remediation:
- `table {x} missing after migrate`: the migrator never saw `migrations/`. The `sqlx::migrate!()` macro path is relative to *the macro caller's manifest dir*, i.e. `games/tap-trading/backend/migrate/`. The migrations live one level up at `games/tap-trading/backend/migrations/` — confirm you used `"../migrations"`, not `"./migrations"`.
- `default _sqlx_migrations must not exist`: the `table_name` override didn't apply. Check that you mutated `migrator.table_name`, not a local copy.

- [ ] **Step 2.5: Wire the `run` subcommand**

Replace `games/tap-trading/backend/migrate/src/main.rs`:

```rust
//! Tick migration runner CLI. Mirrors the library API in `lib.rs` so the
//! operator path (`docker run … tap-trading-migrate run`) and the test
//! path (`tap_trading_migrate::run_migrations(&pool)`) can never disagree.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use sqlx::postgres::PgPoolOptions;
use tap_trading_migrate::{run_migrations, MIGRATION_TABLE};

#[derive(Debug, Parser)]
#[command(name = "tap-trading-migrate", version, about = "Tick migration runner")]
struct Cli {
    /// Database URL. Falls back to `TAP_DB_URL`, then `PLATFORM_DB_URL`.
    #[arg(long, env = "TAP_DB_URL")]
    database_url: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Apply every pending Tick migration.
    Run,
    /// List migrations already applied to the target database.
    Info,
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

fn resolve_db_url(arg: Option<String>) -> Result<String> {
    if let Some(url) = arg {
        return Ok(url);
    }
    if let Ok(url) = std::env::var("TAP_DB_URL") {
        return Ok(url);
    }
    std::env::var("PLATFORM_DB_URL")
        .context("set --database-url, TAP_DB_URL, or PLATFORM_DB_URL")
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let cli = Cli::parse();
    let url = resolve_db_url(cli.database_url)?;

    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .context("connect to database")?;

    match cli.command {
        Command::Run => {
            tracing::info!(table = MIGRATION_TABLE, "applying migrations");
            run_migrations(&pool).await.context("run_migrations")?;
            tracing::info!("migrations applied");
        }
        Command::Info => {
            anyhow::bail!("Task 3 wires the info subcommand");
        }
    }

    Ok(())
}
```

- [ ] **Step 2.6: Smoke-test the binary against the running dev Postgres**

```bash
./scripts/init-worktree-dev.sh
source ./scripts/worktree-env.sh
cd games/tap-trading/backend
cargo run -p tap-trading-migrate -- run --database-url "$PLATFORM_DB_URL"
```

Expected: `info applying migrations` then `info migrations applied`, exit 0. Re-run the same command — it should still exit 0 (idempotent). Then verify the tables landed in the same database without colliding with the platform's tables:

```bash
docker compose exec -T postgres psql -U dopamint -d dopamint -c "\dt public.*"
```

Expected: lists both platform tables (`users`, `point_events`, `sessions`, `_sqlx_migrations`) AND Tick tables (`accounts`, `points_ledger`, …, `_tap_sqlx_migrations`). Both migration tables coexist.

- [ ] **Step 2.7: Clippy + check + test**

```bash
cd games/tap-trading/backend
cargo check && cargo clippy --all-targets -- -D warnings && cargo test
```

Expected: green across the whole Tick workspace.

- [ ] **Step 2.8: Commit**

```bash
git add games/tap-trading/backend/migrate/src/ games/tap-trading/backend/migrate/tests/
git commit -m "feat(tick-migrate): implement run and library"
```

---

## Task 3 — Implement `info` subcommand + `list_applied` library function

The diagnostic surface. Operators run `tap-trading-migrate info` to confirm applied-migration state; Plan C/D/E tests rarely need this, but it's the natural pair of `run` and zero extra cost given the library function.

**Files:**
- Modify: `games/tap-trading/backend/migrate/src/lib.rs`
- Modify: `games/tap-trading/backend/migrate/src/main.rs`
- Modify: `games/tap-trading/backend/migrate/tests/migrate.rs`

- [ ] **Step 3.1: Add the failing tests**

Append to `games/tap-trading/backend/migrate/tests/migrate.rs`:

```rust
#[tokio::test]
async fn list_applied_after_run_returns_one_row() {
    let (_container, pool) = fresh_pool().await;
    run_migrations(&pool).await.expect("run");

    let applied = tap_trading_migrate::list_applied(&pool)
        .await
        .expect("list_applied");

    // Plan A shipped one genesis migration: 20260523120000_create_tick_schema.sql.
    assert_eq!(applied.len(), 1, "expected exactly 1 migration row");
    let row = &applied[0];
    assert_eq!(row.version, 20_260_523_120_000);
    assert!(
        row.description.contains("create_tick_schema"),
        "unexpected description: {}",
        row.description
    );
    assert!(row.success, "migration must be marked success");
}

#[tokio::test]
async fn list_applied_on_empty_db_returns_empty_vec() {
    let (_container, pool) = fresh_pool().await;
    // Don't run migrations — the table doesn't exist yet.
    let applied = tap_trading_migrate::list_applied(&pool)
        .await
        .expect("list_applied tolerates missing table");
    assert!(applied.is_empty(), "expected no rows: {applied:?}");
}
```

- [ ] **Step 3.2: Implement `list_applied`**

Replace the stub in `games/tap-trading/backend/migrate/src/lib.rs`:

```rust
pub async fn list_applied(pool: &PgPool) -> Result<Vec<AppliedMigration>, MigrateError> {
    // The migration table doesn't exist until `run_migrations` has been
    // called at least once. Treat that as "no migrations applied yet" rather
    // than an error so operators can run `info` against a fresh database
    // without surprise.
    let table_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM information_schema.tables
             WHERE table_schema = 'public' AND table_name = $1
         )",
    )
    .bind(MIGRATION_TABLE)
    .fetch_one(pool)
    .await?;

    if !table_exists {
        return Ok(Vec::new());
    }

    // Postgres won't let us parameterize an identifier, so we interpolate the
    // table name. Safe because MIGRATION_TABLE is a compile-time constant we
    // control — no user input is concatenated.
    let sql = format!(
        "SELECT version,
                description,
                (EXTRACT(EPOCH FROM installed_on) * 1000)::BIGINT AS installed_on_ms,
                execution_time AS execution_time_ms,
                success
         FROM {MIGRATION_TABLE}
         ORDER BY version ASC"
    );
    let rows = sqlx::query(&sql).fetch_all(pool).await?;

    let mut applied = Vec::with_capacity(rows.len());
    for row in rows {
        use sqlx::Row;
        applied.push(AppliedMigration {
            version: row.try_get("version")?,
            description: row.try_get("description")?,
            installed_on_ms: row.try_get("installed_on_ms")?,
            execution_time_ms: row.try_get("execution_time_ms")?,
            success: row.try_get("success")?,
        });
    }
    Ok(applied)
}
```

`execution_time` in `_*sqlx_migrations` is stored as `BIGINT` nanoseconds in sqlx 0.8; the `AppliedMigration.execution_time_ms` field name is the *public* contract — its value here is the raw sqlx number, which callers can treat as "monotonic count, units depend on sqlx version". The test asserts the field is present, not a specific unit.

- [ ] **Step 3.3: Wire the `Info` subcommand**

In `games/tap-trading/backend/migrate/src/main.rs`, replace the `Command::Info` arm:

```rust
        Command::Info => {
            let applied = tap_trading_migrate::list_applied(&pool)
                .await
                .context("list_applied")?;
            if applied.is_empty() {
                println!("no migrations applied yet (table {MIGRATION_TABLE} absent or empty)");
            } else {
                println!("{} applied migration(s):", applied.len());
                for row in &applied {
                    let status = if row.success { "ok" } else { "FAILED" };
                    println!(
                        "  [{status}] {ver:>20} {desc}",
                        ver = row.version,
                        desc = row.description,
                    );
                }
            }
        }
```

- [ ] **Step 3.4: Re-run tests**

```bash
cd games/tap-trading/backend
cargo test -p tap-trading-migrate --test migrate
```

Expected: all 5 tests pass. If `list_applied_after_run_returns_one_row` asserts `applied.len() == 1` and you get a higher number, Plan A's migrations directory has gained files since this plan was written — adjust the expected count.

- [ ] **Step 3.5: Smoke-test against dev Postgres**

```bash
source ./scripts/worktree-env.sh
cd games/tap-trading/backend
cargo run -p tap-trading-migrate -- info --database-url "$PLATFORM_DB_URL"
```

Expected:

```
1 applied migration(s):
  [ok]      20260523120000 create_tick_schema
```

If the description column shows `unknown` or empty, the migration filename's text portion isn't being captured — confirm the genesis migration's filename is `20260523120000_create_tick_schema.sql`.

- [ ] **Step 3.6: Full verify**

```bash
cd games/tap-trading/backend
cargo check && cargo clippy --all-targets -- -D warnings && cargo test
```

Expected: green.

- [ ] **Step 3.7: Commit**

```bash
git add games/tap-trading/backend/migrate/src/ games/tap-trading/backend/migrate/tests/
git commit -m "feat(tick-migrate): implement info subcommand"
```

---

## Task 4 — Add Tap service ports to the worktree env

Three new env vars plus URL helpers, mirroring the existing pattern. After this commit, sourcing `scripts/worktree-env.sh` exports the three values; running `./scripts/init-worktree-dev.sh` from a clean `.local/` writes them into the canonical `.env`.

**Files:**
- Modify: `scripts/worktree-env.sh`
- Modify: `scripts/init-worktree-dev.sh`

- [ ] **Step 4.1: Add the three port exports**

Edit `scripts/worktree-env.sh`. After the existing `export PLATFORM_REDIS_PORT=$((6379 + OFFSET))` line (around line 32), append three new exports keeping the existing comment intact:

```sh
export TAP_API_PORT=$((3200 + OFFSET))
export TAP_AGGREGATOR_PORT=$((3300 + OFFSET))
export TAP_WORKER_METRICS_PORT=$((3400 + OFFSET))
```

Base values chosen at Pre-flight Step P5; do not revisit.

- [ ] **Step 4.2: Add URL helpers**

In the same file, find the existing URL block (starts with `export PLATFORM_API_URL=`). After the last line of that block (`export PLATFORM_REDIS_URL=...`), append:

```sh
export TAP_API_URL="http://localhost:${TAP_API_PORT}"
export TAP_AGGREGATOR_WS_URL="ws://localhost:${TAP_AGGREGATOR_PORT}/ws"
export TAP_DB_URL="${PLATFORM_DB_URL}"
```

`TAP_DB_URL` is an alias of `PLATFORM_DB_URL` — same Postgres instance, same database, separate tables, separate `_tap_sqlx_migrations` table. Aliasing keeps the two URLs guaranteed-equal so future code can read either.

- [ ] **Step 4.3: Extend the canonical `.env` heredoc**

Edit `scripts/init-worktree-dev.sh`. Find the heredoc that writes `.local/.env` (starts at the line `cat >"$tmpfile" <<EOF`). The existing port block ends with `PLATFORM_REDIS_PORT=${PLATFORM_REDIS_PORT}`. After that line, insert:

```
TAP_API_PORT=${TAP_API_PORT}
TAP_AGGREGATOR_PORT=${TAP_AGGREGATOR_PORT}
TAP_WORKER_METRICS_PORT=${TAP_WORKER_METRICS_PORT}
```

The URL block in the same heredoc ends with `PLATFORM_REDIS_URL=${PLATFORM_REDIS_URL}`. After that line, insert:

```
TAP_API_URL=${TAP_API_URL}
TAP_AGGREGATOR_WS_URL=${TAP_AGGREGATOR_WS_URL}
TAP_DB_URL=${TAP_DB_URL}
```

- [ ] **Step 4.4: Verify the exports take effect from a clean run**

```bash
rm -rf .local
./scripts/init-worktree-dev.sh
grep -E '^(TAP_|PLATFORM_)(API_PORT|AGGREGATOR_PORT|WORKER_METRICS_PORT|API_URL|AGGREGATOR_WS_URL|DB_URL)' .local/.env | sort
```

Expected: 6 new `TAP_*` lines present alongside the existing `PLATFORM_*` lines.

For a worktree where `$OFFSET=0` the printout is:

```
PLATFORM_API_URL=http://localhost:3000
PLATFORM_DB_URL=postgres://dopamint:dopamint@127.0.0.1:5432/dopamint
TAP_AGGREGATOR_PORT=3300
TAP_AGGREGATOR_WS_URL=ws://localhost:3300/ws
TAP_API_PORT=3200
TAP_API_URL=http://localhost:3200
TAP_DB_URL=postgres://dopamint:dopamint@127.0.0.1:5432/dopamint
TAP_WORKER_METRICS_PORT=3400
```

If the offset is non-zero, every port is shifted by the same amount.

- [ ] **Step 4.5: Commit**

```bash
git add scripts/worktree-env.sh scripts/init-worktree-dev.sh
git commit -m "feat(tick-dev-env): add tap service ports"
```

---

## Task 5 — Sync env files + coherence checks for the three services

The "four must move together" contract demands `sync-service-envs.sh` and `ensure-worktree-coherence.sh` ship in the same commit as any new port. They live in the same task here.

**Files:**
- Modify: `scripts/sync-service-envs.sh`
- Modify: `scripts/ensure-worktree-coherence.sh`
- Create: `games/tap-trading/backend/api/.env.example`
- Create: `games/tap-trading/backend/oracle-aggregator/.env.example`
- Create: `games/tap-trading/backend/settlement-worker/.env.example`

- [ ] **Step 5.1: Add three `write_env` calls**

Edit `scripts/sync-service-envs.sh`. After the existing `trivia-show backend` block (which ends with the `EOF\n)"` near the bottom of the file), append:

```sh
# tap-trading-api — its own port, DB URL, aggregator WS URL, log level.
write_env "$REPO_ROOT/games/tap-trading/backend/api/.env" "$(cat <<EOF
TAP_API_PORT=${TAP_API_PORT}
TAP_DB_URL=${TAP_DB_URL}
TAP_AGGREGATOR_WS_URL=${TAP_AGGREGATOR_WS_URL}
RUST_LOG=info,tower_http=debug
EOF
)"

# tap-trading-oracle-aggregator — its own port + log level.
write_env "$REPO_ROOT/games/tap-trading/backend/oracle-aggregator/.env" "$(cat <<EOF
TAP_AGGREGATOR_PORT=${TAP_AGGREGATOR_PORT}
RUST_LOG=info,tower_http=debug
EOF
)"

# tap-trading-settlement-worker — metrics port, DB URL, aggregator WS URL, log level.
write_env "$REPO_ROOT/games/tap-trading/backend/settlement-worker/.env" "$(cat <<EOF
TAP_WORKER_METRICS_PORT=${TAP_WORKER_METRICS_PORT}
TAP_DB_URL=${TAP_DB_URL}
TAP_AGGREGATOR_WS_URL=${TAP_AGGREGATOR_WS_URL}
RUST_LOG=info
EOF
)"
```

`write_env` already `mkdir -p`s the target directory (line 18 in the file), so these targets don't require the service directories to exist on disk first.

- [ ] **Step 5.2: Add coherence checks**

Edit `scripts/ensure-worktree-coherence.sh`. Find the existing `check_port` block and append the three new lines after `check_port PLATFORM_REDIS_PORT "$PLATFORM_REDIS_PORT"`:

```sh
check_port TAP_API_PORT             "$TAP_API_PORT"
check_port TAP_AGGREGATOR_PORT      "$TAP_AGGREGATOR_PORT"
check_port TAP_WORKER_METRICS_PORT  "$TAP_WORKER_METRICS_PORT"
```

Then find the existing URL-assertion block (the one with `api_url`, `trivia_url`, `db_url`, `redis_url`) and append after the `redis_url` block:

```sh
tap_api_url="$(read_env TAP_API_URL)"
[[ "$tap_api_url" == "$TAP_API_URL" ]] || \
  fail "TAP_API_URL is $tap_api_url but worktree-env.sh expects $TAP_API_URL"

tap_agg_url="$(read_env TAP_AGGREGATOR_WS_URL)"
[[ "$tap_agg_url" == "$TAP_AGGREGATOR_WS_URL" ]] || \
  fail "TAP_AGGREGATOR_WS_URL is $tap_agg_url but worktree-env.sh expects $TAP_AGGREGATOR_WS_URL"

tap_db_url="$(read_env TAP_DB_URL)"
[[ "$tap_db_url" == "$TAP_DB_URL" ]] || \
  fail "TAP_DB_URL is $tap_db_url but worktree-env.sh expects $TAP_DB_URL"
```

- [ ] **Step 5.3: Write the three `.env.example` doc files**

These are documentation only — they record which variables each service will read once Plans C/D/E land. They are *not* sourced at runtime; `sync-service-envs.sh` writes a separate `.env` (no `.example` suffix) that the service actually reads.

Write `games/tap-trading/backend/api/.env.example`:

```
# Documents which env vars tap-trading-api reads. The live file
# (.env, no suffix) is generated by scripts/sync-service-envs.sh — do not
# edit it by hand. Override values only via .local/.env.

TAP_API_PORT=
TAP_DB_URL=
TAP_AGGREGATOR_WS_URL=
RUST_LOG=
```

Write `games/tap-trading/backend/oracle-aggregator/.env.example`:

```
# Documents which env vars tap-trading-oracle-aggregator reads. The live
# file (.env, no suffix) is generated by scripts/sync-service-envs.sh.

TAP_AGGREGATOR_PORT=
RUST_LOG=
```

Write `games/tap-trading/backend/settlement-worker/.env.example`:

```
# Documents which env vars tap-trading-settlement-worker reads. The live
# file (.env, no suffix) is generated by scripts/sync-service-envs.sh.

TAP_WORKER_METRICS_PORT=
TAP_DB_URL=
TAP_AGGREGATOR_WS_URL=
RUST_LOG=
```

- [ ] **Step 5.4: Verify end-to-end against a clean worktree**

```bash
rm -rf .local games/tap-trading/backend/api/.env \
              games/tap-trading/backend/oracle-aggregator/.env \
              games/tap-trading/backend/settlement-worker/.env
./scripts/init-worktree-dev.sh
./scripts/ensure-worktree-coherence.sh
```

Expected: `init-worktree-dev.sh` finishes with `[init] worktree dev env ready`. `ensure-worktree-coherence.sh` prints `[coherence] ok (offset=$N)`. Then inspect the three new files:

```bash
ls -la games/tap-trading/backend/{api,oracle-aggregator,settlement-worker}/.env
head -1 games/tap-trading/backend/api/.env
head -1 games/tap-trading/backend/oracle-aggregator/.env
head -1 games/tap-trading/backend/settlement-worker/.env
```

Expected: three files exist, each with the right port as its first line.

- [ ] **Step 5.5: Commit**

```bash
git add scripts/sync-service-envs.sh scripts/ensure-worktree-coherence.sh \
        games/tap-trading/backend/api/.env.example \
        games/tap-trading/backend/oracle-aggregator/.env.example \
        games/tap-trading/backend/settlement-worker/.env.example
git commit -m "feat(tick-dev-env): sync envs and coherence"
```

---

## Task 6 — Add Tap processes to `mprocs.yaml`

Four new process entries. `tap-migrate` is a one-shot — it exits 0 once migrations apply, then disappears from the running set. The other three are long-running `cargo watch` processes for live reload; they will exit non-zero with `error: package ID specification 'tap-trading-api' did not match any packages` until Plans C/D/E land. **That failure mode is expected and documented in the entry comments.**

**Files:**
- Modify: `mprocs.yaml`

- [ ] **Step 6.1: Append the four process entries**

Edit `mprocs.yaml`. After the last entry (`game-2048-ui`), append:

```yaml
  tap-migrate:
    shell: |
      set -e
      source ./scripts/worktree-env.sh
      ./scripts/ensure-worktree-coherence.sh --quiet
      # One-shot: runs migrations and exits. Re-run on demand from mprocs UI.
      cargo run -p tap-trading-migrate --manifest-path games/tap-trading/backend/Cargo.toml -- run

  # The three entries below WILL FAIL with
  #   "error: package ID specification '<name>' did not match any packages"
  # until Plans C/D/E ship the corresponding bin crates. Leave them here so
  # those plans only need to add the crate, not edit this file.
  tap-aggregator:
    shell: |
      set -e
      source ./scripts/worktree-env.sh
      ./scripts/ensure-worktree-coherence.sh --quiet
      cargo watch --workdir games/tap-trading/backend -x 'run -p tap-trading-oracle-aggregator'

  tap-worker:
    shell: |
      set -e
      source ./scripts/worktree-env.sh
      ./scripts/ensure-worktree-coherence.sh --quiet
      cargo watch --workdir games/tap-trading/backend -x 'run -p tap-trading-settlement-worker'

  tap-api:
    shell: |
      set -e
      source ./scripts/worktree-env.sh
      ./scripts/ensure-worktree-coherence.sh --quiet
      cargo watch --workdir games/tap-trading/backend -x 'run -p tap-trading-api'
```

`--manifest-path` is required because the Tick workspace is *not* a member of the root workspace (per Plan A). For the watch processes, `--workdir` sets `cargo watch`'s working directory so the inner `cargo run -p ...` resolves against the Tick workspace's `Cargo.toml`.

- [ ] **Step 6.2: Verify the YAML parses**

```bash
python3 -c "import yaml; data = yaml.safe_load(open('mprocs.yaml')); print(sorted(data['procs'].keys()))"
```

Expected: prints the alphabetized list of process names, including `tap-aggregator`, `tap-api`, `tap-migrate`, `tap-worker`. If YAML parse errors, indentation drifted — `mprocs.yaml` uses two-space indents under `procs:`.

- [ ] **Step 6.3: Verify `tap-migrate` runs end-to-end (other entries will fail; that's expected)**

We don't launch mprocs in this verification step — running it interactively requires a TTY. Instead invoke the `tap-migrate` shell directly:

```bash
set -e
source ./scripts/worktree-env.sh
./scripts/ensure-worktree-coherence.sh --quiet
cargo run -p tap-trading-migrate --manifest-path games/tap-trading/backend/Cargo.toml -- run
```

Expected: `info applying migrations` then `info migrations applied`, exit 0. Re-run — still exit 0.

- [ ] **Step 6.4: Commit**

```bash
git add mprocs.yaml
git commit -m "feat(tick-dev-env): add tap procs to mprocs"
```

---

## Task 7 — Wire Tap processes into `start-headless.sh`

Headless mode is what AI agents and CI use. `tap-migrate` runs synchronously (foreground) before any long-running service so the database is ready when they boot; the long-running entries follow the existing `start_proc` pattern.

**Files:**
- Modify: `scripts/start-headless.sh`

- [ ] **Step 7.1: Add a synchronous migrate step before `start_proc`**

Edit `scripts/start-headless.sh`. The existing start sequence runs `start_compose postgres postgres`, then `start_compose redis redis`, then `start_proc platform-api-gateway …`, etc.

After `start_compose redis redis` and BEFORE the first `start_proc`, insert:

```sh
# tap-migrate — synchronous, foreground. Must complete before any service
# that reads Tick tables boots. Logs to tmp/tap-migrate.log; exit code is
# load-bearing — a failure here aborts the rest of start-headless.
echo "[start] tap-migrate (foreground)"
if ! cargo run -p tap-trading-migrate \
      --manifest-path games/tap-trading/backend/Cargo.toml \
      --quiet \
      -- run >>"$LOG_DIR/tap-migrate.log" 2>&1; then
    echo "[start] tap-migrate FAILED — see $LOG_DIR/tap-migrate.log" >&2
    exit 1
fi
echo "[ready] tap-migrate"
```

The foreground guard is the critical bit — using `start_proc` would background it via `nohup`, and the three Tap services that read Tick tables could race the migration.

- [ ] **Step 7.2: Add the three long-running `start_proc` calls**

After the existing `start_proc trivia-show-backend …` line, append three lines using the same pattern:

```sh
start_proc tap-aggregator        cargo run -p tap-trading-oracle-aggregator --manifest-path games/tap-trading/backend/Cargo.toml -q
start_proc tap-worker            cargo run -p tap-trading-settlement-worker --manifest-path games/tap-trading/backend/Cargo.toml -q
start_proc tap-api               cargo run -p tap-trading-api --manifest-path games/tap-trading/backend/Cargo.toml -q
```

These will fail with `error: package ID specification 'tap-trading-api' did not match any packages` (and the analogous error for the other two) until Plans C/D/E land — the PID file will be written, but the process will exit non-zero almost immediately. `start_proc` does not check exit codes, so `start-headless.sh` itself still completes; only the affected logs in `tmp/` will show the error.

- [ ] **Step 7.3: Add a `wait_http` probe for `tap-api`**

After the existing `wait_http trivia-show-backend …` line, append:

```sh
wait_http tap-api "$TAP_API_URL/health" 30 || true
```

`|| true` keeps the script from aborting when Plan C/D/E aren't in yet. Once `tap-trading-api` ships a real `/health` route, this probe becomes load-bearing.

- [ ] **Step 7.4: Smoke-test the headless start**

```bash
./scripts/start-headless.sh --stop || true
./scripts/start-headless.sh
```

Expected:
- `[start] tap-migrate (foreground)` then `[ready] tap-migrate` — must succeed; if it fails, stop everything.
- `[start] tap-aggregator (pid …) -> tmp/tap-aggregator.log`, same for `tap-worker`, `tap-api`. These all "start" but immediately exit non-zero. PID files persist with stale PIDs; that's the existing pattern.
- `wait_http tap-api ... FAILED to respond at …/health within 30s` — this is the expected outcome until Plan C/D/E land. `|| true` swallows it.

Inspect the failure logs to confirm the expected error string:

```bash
grep -h 'did not match any packages' tmp/tap-aggregator.log tmp/tap-worker.log tmp/tap-api.log
```

Expected: three lines, one per service, each containing `error: package ID specification '<name>' did not match any packages`. If any log shows a *different* error (e.g. a syntax error in the Tick workspace `Cargo.toml`), stop and fix — the workspace must build clean.

Then clean up:

```bash
./scripts/start-headless.sh --stop
```

- [ ] **Step 7.5: Commit**

```bash
git add scripts/start-headless.sh
git commit -m "feat(tick-dev-env): add tap procs to headless"
```

---

## Final verification

- [ ] **Step F1: All 7 commits land on the current branch**

```bash
git log --oneline -7
```

Expected: 7 commits with the subjects from the Commit map, newest at top.

- [ ] **Step F2: Tick workspace still builds, clippies, and tests cleanly**

```bash
cd games/tap-trading/backend
cargo check && cargo clippy --all-targets -- -D warnings && cargo test
```

Expected: green. Test summary: Plan A's ~20+ tests + 5 new integration tests in `tap-trading-migrate` (`applies_all_tick_tables`, `rerun_is_idempotent`, `uses_custom_migration_table`, `list_applied_after_run_returns_one_row`, `list_applied_on_empty_db_returns_empty_vec`).

- [ ] **Step F3: Root workspace still builds (unchanged)**

```bash
cd "$(git rev-parse --show-toplevel)"
cargo check --workspace && cargo test --workspace && cargo clippy --workspace -- -D warnings
```

Expected: same green baseline as before this plan started.

- [ ] **Step F4: Clean-room dev-env bring-up**

```bash
./scripts/start-headless.sh --stop || true
rm -rf .local tmp \
       games/tap-trading/backend/api/.env \
       games/tap-trading/backend/oracle-aggregator/.env \
       games/tap-trading/backend/settlement-worker/.env
./scripts/init-worktree-dev.sh
./scripts/ensure-worktree-coherence.sh
./scripts/start-headless.sh
```

Expected:
- `init-worktree-dev.sh` finishes with `[init] worktree dev env ready`.
- `ensure-worktree-coherence.sh` prints `[coherence] ok (offset=$N)`.
- `start-headless.sh` prints `[ready] tap-migrate` after the foreground migrate step succeeds.
- The three Tap long-running services emit `error: package ID specification … did not match any packages` to their logs — **expected** until Plans C/D/E ship the crates.

Inspect the persisted Tick state in the dev Postgres:

```bash
source ./scripts/worktree-env.sh
docker compose exec -T postgres psql -U dopamint -d dopamint -c "\dt public.*" | grep -E '_tap_sqlx_migrations|accounts|positions|settlements'
```

Expected: rows for `_tap_sqlx_migrations`, `accounts`, `positions`, `settlements` (and the other 5 Tick tables). The platform's `_sqlx_migrations` row is also present — both migration tables coexist.

Stop services:

```bash
./scripts/start-headless.sh --stop
```

- [ ] **Step F5: Document the "expected failures" in the PR description**

Add to the PR description (NOT the commit body — the convention is no body unless necessary):

> Plan B leaves three intentional failures in place to be resolved by Plans C/D/E:
> - `mprocs.yaml` entries `tap-aggregator`, `tap-worker`, `tap-api` will fail with `error: package ID specification '<name>' did not match any packages` until the corresponding bin crates are added.
> - `start-headless.sh` will start their PIDs and they'll immediately exit non-zero, leaving stale entries under `tmp/pids/`. This is expected; `wait_http tap-api .../health` is `|| true`-guarded.
> - The three `.env` files written by `sync-service-envs.sh` point at services that don't yet read them. They're written eagerly so Plan C/D/E only needs to ship a crate, not edit any script.
>
> Migration-table collision: `tap-trading-migrate` writes to `_tap_sqlx_migrations`; the platform writes to the default `_sqlx_migrations`. Both coexist in the same database. Table-name collision check (grep across `migrations/*.sql` and `games/tap-trading/backend/migrations/*.sql`) was clean at PR time — no overlapping table names.

---

## Plan C/D/E preview (not in scope here)

Plan C/D/E pick up where this leaves off. Each plan adds one bin crate; the env, ports, mprocs entries, and headless runner are already wired:

- **Plan C** — `tap-trading-oracle-aggregator` (Pyth Hermes + 3 CEX WS, median+EWMA, WS broadcast). Reads `TAP_AGGREGATOR_PORT`. Also adds the library crate `tap-trading-oracle-types` for the shared `OracleQuote` shape. Spec: `ORACLE_SPEC.md`.
- **Plan D** — `tap-trading-settlement-worker` (in-memory open-position cache, idempotent settle, advisory-lock leader election). Reads `TAP_WORKER_METRICS_PORT`, `TAP_DB_URL`, `TAP_AGGREGATOR_WS_URL`. On startup it calls `tap_trading_migrate::run_migrations(&pool).await?` as a migrate-or-fail boot guard. Spec: `SYSTEM_DESIGN.md §5.2`.
- **Plan E** — `tap-trading-api` (axum REST + WS, zkLogin verifier, tap commit with drift check using `tap-trading-pricing-engine`, leaderboard, quests). Reads `TAP_API_PORT`, `TAP_DB_URL`, `TAP_AGGREGATOR_WS_URL`. Same migrate-or-fail boot guard. Spec: `SYSTEM_DESIGN.md §3`.

After Plan E, `./scripts/start-headless.sh` produces a fully-running Tick stack with no expected failures.
