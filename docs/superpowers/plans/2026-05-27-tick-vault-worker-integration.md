# Tick Vault Worker Integration — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `tap-trading-settlement-worker` (plan D) a dual-sink settler: points positions keep their existing Postgres transaction; DUSDC/USDC positions get an on-chain `settle_win`/`settle_loss`/`settle_void` PTB submitted to the `tick_vault` Move package (plan 1) on the authority of a `SettlerCap`, followed by a best-effort Walrus proof publish (assemble via `proof-types`, `PUT` via `walrus-client`, `anchor_proof` PTB), with the digest + blob id recorded in Postgres. After this lands, the full ADR-0010 + ADR-0011 loop is closed: a player deposits USDC, taps, and on touch the worker pays them on-chain and publishes a publicly-verifiable proof.

**Architecture:** Extend the existing worker crate (no new service). The shared touch logic (`touch.rs::evaluate_position`, plan D) is the single source of truth for WON/LOST/VOID across both sinks. A new `sui_settle.rs` builds + signs + submits `settle_*` PTBs via the Sui Rust SDK (`sui-sdk`), loading the `SettlerCap` object id and settler keypair from env. A new `proof_publish.rs` runs *after* the settle PTB confirms: pull the window's ticks from the aggregator ring (ADR-0008 `GET /ring`, ADR-0011 §6's 120 s retention), `assemble` a `ProofBlob`, `PUT` to Walrus, submit `anchor_proof`, write back to Postgres. Walrus failures never block payout — they land on a best-effort retry queue. The plan-D advisory lock (single leader) now also guarantees no double-submit of an on-chain payout.

**Tech Stack:** Rust 2021, building on plan D's `tokio`/`axum`/`sqlx`/`tokio-tungstenite`/`tracing` stack. New deps: `sui-sdk` (Sui Rust SDK — `SuiClientBuilder`, `ProgrammableTransactionBuilder`, `Transaction`), `shared-crypto`/`fastcrypto` (for the Ed25519 settler keypair), `tap-trading-proof-types` + `tap-trading-walrus-client` (plan 2, path deps). Dev: `testcontainers` (Postgres, as plan D) + a deployed testnet `tick_vault` for the on-chain integration test (localnet alternative documented).

**Spec:** ADR-0010 §6 (`SettlerCap` authority; `settle_win`/`settle_loss`/`settle_void` signatures), §7 (dual-sink branch on mode). ADR-0011 §4 (publish sequence steps 1–5: settle → assemble → PUT → anchor_proof → Postgres), §6 (120 s ring for evidence). ADR-0008 (`GET /ring/:asset/:seq?run_id=N` for evidence ticks; `(run_id, seq)` staleness). Plan D (`2026-05-27-tick-settlement-worker.md`) — the worker this extends: `touch.rs`, `settle.rs`, `loop_runner.rs`, leader lock, the `settlements` idempotency canary. Plan 1 (`tick-onchain-vault`) — the Move entry points + `ProofAnchored` event. Plan 2 (`tick-walrus-proofs`) — `assemble`, `WalrusClient`, `multiplier_f64_to_bps`.

**Spec deviations / corrections (record before writing code):**

- **Position mode is discovered from the position row, not inferred.** Plan D's `positions` table gets a new column `settle_mode TEXT NOT NULL DEFAULT 'points' CHECK (settle_mode IN ('points','usdc'))` and, for USDC rows, `vault_position_id TEXT` (the Sui object id), `vault_id TEXT`, `player_balance_id TEXT`. A migration adds these (Task 1). For USDC mode the API must set them at mint — but **Plan E (api) as written is points-only and never touches `settle_mode`**. This plan therefore owns that API change: amend Plan E's `POST /v1/positions` insert to accept a session-level mode and write `settle_mode` + the three on-chain ids (or add a parallel USDC mint path). Until that amendment lands, every row defaults to `'points'` and the dual-sink branch below only ever takes the points path. The worker branches on `settle_mode`.
- **On-chain settle is idempotent via the same canary as points.** Before submitting any PTB, the worker runs plan D's `INSERT INTO settlements … ON CONFLICT (position_id) DO NOTHING RETURNING id`. Only if a row inserts does it proceed to sign+submit. This means a crash *after* PTB submit but *before* Postgres commit can double-submit on-chain — so the Move `settle_*` also aborts on a non-OPEN position (plan 1 Task 8, `EPositionNotOpen`). Two layers: DB canary (cheap, first line) + on-chain status guard (authoritative). Documented because the ordering matters.
- **Walrus publish is decoupled from settlement correctness.** The settle PTB and the Postgres settlement row are the source of truth for "the player was paid." The proof blob is a *convenience copy* (ADR-0011 §4). If `PUT`/`anchor_proof` fail, the worker enqueues a retry and moves on; the payout already happened. A `proof_status TEXT DEFAULT 'pending'` column on `settlements` tracks `pending|published|failed`.
- **Evidence ticks come from the aggregator, not the worker's cache.** The worker's plan-D cache holds OPEN positions, not tick history. Evidence is fetched at settle time via `GET /ring/:asset/:seq` over `[oracle_seq_at_tap .. touch_seq]` (ADR-0011 §6). If the ring no longer has a seq (aggregator restarted, `run_id` changed → 409/410), the proof records `InsufficientEvidence`-shaped partial evidence and `proof_status='failed'`; the payout is unaffected.
- **Settler keypair is loaded from `TICK_SETTLER_PRIVKEY` (env), never committed.** The `SettlerCap` object id is `TICK_SETTLER_CAP_ID`; the vault id is `TICK_VAULT_ID`; the package is `TICK_VAULT_PKG`. All from env per the worktree dev-env contract — no literals.

**Verification baseline:** before starting:

```bash
cd games/tap-trading/backend && cargo check && cargo test    # plans A-E + plans 1-2 crates present
docker info >/dev/null 2>&1 && echo "docker OK"               # testcontainers prereq
sui client active-env                                          # testnet configured for the on-chain IT
```

After every commit, run from `games/tap-trading/backend/`:

```bash
cargo check && cargo test && cargo clippy --all-targets -- -D warnings
```

---

## Commit map

| # | Subject | Scope |
|---|---------|-------|
| 1 | `feat(tick-worker): add settle_mode columns migration` | Migration adds `settle_mode`, `vault_position_id`, `vault_id`, `player_balance_id` to `positions`; `proof_status`, `sui_tx_digest`, `walrus_blob_id` to `settlements`. |
| 2 | `feat(tick-worker): load settler keypair + vault config from env` | `sui_config.rs`: parse `TICK_SETTLER_PRIVKEY`/`_CAP_ID`/`TICK_VAULT_ID`/`_PKG` into a `SuiSettleConfig`. Ed25519 keypair from base64. Unit test on a fixture key. |
| 3 | `feat(tick-worker): build settle_win PTB (no submit)` | `sui_settle.rs::build_settle_win` constructs a `ProgrammableTransaction` calling `tick_vault::vault::settle_win`. Unit test asserts the PTB has the right package/module/function + args. |
| 4 | `feat(tick-worker): build settle_loss and settle_void PTBs` | `build_settle_loss`, `build_settle_void`. Unit tests on PTB shape. |
| 5 | `feat(tick-worker): submit + await PTB via sui-sdk` | `submit(tx) -> SuiTransactionBlockResponse`; sign with settler keypair, `execute_transaction_block` with effects+events, assert success. Returns digest. (Tested against testnet in Task 11.) |
| 6 | `feat(tick-worker): dual-sink branch in loop_runner` | On a touch/expire/void outcome, read `settle_mode`; `points` → plan-D path (unchanged); `usdc` → DB canary then `sui_settle`. Testcontainers test: a `usdc` row routes to the sui path (mock submit). |
| 7 | `feat(tick-worker): fetch evidence ticks from aggregator ring` | `evidence.rs::fetch_window(asset, run_id, from_seq, to_seq)` → `Vec<EvidenceTick>` via `GET /ring`. `wiremock` test for 200 + 409/410 (partial → marked insufficient). |
| 8 | `feat(tick-worker): assemble + publish proof + anchor` | `proof_publish.rs`: `assemble` (plan 2) → `WalrusClient::store_blob` → `build_anchor_proof` PTB → submit → update `settlements.proof_status='published'`, `walrus_blob_id`, `sui_tx_digest`. wiremock Walrus + mock submit. |
| 9 | `feat(tick-worker): best-effort proof retry queue` | Failed publishes set `proof_status='failed'`; a 60 s sweep re-attempts `failed` rows. Testcontainers test: a failed publish is retried and flips to `published`. |
| 10 | `feat(tick-worker): record on-chain settlement in postgres` | The `usdc` settle path writes `settlements` with `sui_tx_digest`; balance/ledger rows are NOT written for usdc (funds live on-chain, not in `accounts.balance`). Testcontainers test asserts no points-ledger row for a usdc settle. |
| 11 | `test(tick-worker): on-chain settle integration vs testnet vault` | Against a deployed testnet `tick_vault`: deposit→mint→worker settles→assert payout in `PlayerBalance` + `ProofAnchored` event + Walrus blob fetchable. Gated behind `TICK_IT_ONCHAIN=1` (skipped in CI without testnet creds). |

Each commit must independently pass `cargo check && cargo test && cargo clippy --all-targets -- -D warnings`.

---

## File map

### Created files

| Path | Responsibility |
|------|----------------|
| `games/tap-trading/backend/migrations/20260528000000_add_usdc_settle_mode.sql` | The `settle_mode`/vault-id/proof columns. |
| `games/tap-trading/backend/settlement-worker/src/sui_config.rs` | `SuiSettleConfig` from env; Ed25519 keypair load. |
| `games/tap-trading/backend/settlement-worker/src/sui_settle.rs` | `build_settle_win/loss/void`, `build_anchor_proof`, `submit`. |
| `games/tap-trading/backend/settlement-worker/src/evidence.rs` | `fetch_window` from the aggregator ring. |
| `games/tap-trading/backend/settlement-worker/src/proof_publish.rs` | assemble → store → anchor → record; retry sweep. |
| `games/tap-trading/backend/settlement-worker/tests/dual_sink.rs` | Testcontainers: mode routing, no-ledger-on-usdc. |
| `games/tap-trading/backend/settlement-worker/tests/proof_retry.rs` | Testcontainers + wiremock: failed→retried→published. |
| `games/tap-trading/backend/settlement-worker/tests/onchain_it.rs` | Testnet integration (feature-gated). |

### Modified files

| Path | Reason |
|------|--------|
| `games/tap-trading/backend/settlement-worker/Cargo.toml` | Add `sui-sdk`, `fastcrypto`, `tap-trading-proof-types`, `tap-trading-walrus-client`. |
| `games/tap-trading/backend/settlement-worker/src/loop_runner.rs` | Dual-sink branch on `settle_mode`. |
| `games/tap-trading/backend/settlement-worker/src/main.rs` | Build `SuiSettleConfig`, `WalrusClient`; spawn proof-retry sweep. |
| `games/tap-trading/backend/Cargo.toml` | Add `sui-sdk`, `fastcrypto` to `[workspace.dependencies]`. |

---

## Pre-flight (one-time, not a commit)

- [ ] **Step P1: Baseline — plans A–E + plan-1/2 crates present and green**

```bash
cd games/tap-trading/backend && cargo check && cargo test && cargo clippy --all-targets -- -D warnings
```

Expected: green. `tap-trading-proof-types`, `tap-trading-walrus-client` (plan 2) must be members.

- [ ] **Step P2: Deploy `tick_vault` to testnet, capture ids (needed for Task 11)**

```bash
cd games/tap-trading/move/tick_vault && sui client publish --gas-budget 200000000
# record the package id, then call create_vault and record vault id + SettlerCap id
```

Expected: a package id; `create_vault<USDC>` shares a vault and transfers a `SettlerCap`. Export `TICK_VAULT_PKG`, `TICK_VAULT_ID`, `TICK_SETTLER_CAP_ID`, `TICK_SETTLER_PRIVKEY` into `.local/.env` via the dev-env skill. If you can't deploy now, Tasks 1–10 still complete; only Task 11 needs this.

- [ ] **Step P3: Confirm Docker + testnet env**

```bash
docker info >/dev/null 2>&1 && echo OK && sui client active-env
```

Expected: `OK` and `testnet`.

---

## Task 1: settle_mode columns migration

**Files:**
- Create: `games/tap-trading/backend/migrations/20260528000000_add_usdc_settle_mode.sql`
- Test: `games/tap-trading/backend/settlement-worker/tests/dual_sink.rs` (schema assertion only in this task)

- [ ] **Step 1: Write the migration**

```sql
-- Tick USDC settle mode (ADR-0010 §7, §8). Points mode is the default; USDC
-- mode carries the on-chain object ids the worker needs to settle.
ALTER TABLE positions
  ADD COLUMN settle_mode TEXT NOT NULL DEFAULT 'points'
    CHECK (settle_mode IN ('points', 'usdc')),
  ADD COLUMN vault_position_id TEXT,
  ADD COLUMN vault_id TEXT,
  ADD COLUMN player_balance_id TEXT;

-- USDC rows must carry their on-chain ids.
ALTER TABLE positions ADD CONSTRAINT positions_usdc_ids_present
  CHECK (settle_mode = 'points'
         OR (vault_position_id IS NOT NULL AND vault_id IS NOT NULL
             AND player_balance_id IS NOT NULL));

ALTER TABLE settlements
  ADD COLUMN proof_status TEXT NOT NULL DEFAULT 'pending'
    CHECK (proof_status IN ('pending', 'published', 'failed')),
  ADD COLUMN sui_tx_digest TEXT,
  ADD COLUMN walrus_blob_id TEXT;

CREATE INDEX settlements_proof_pending ON settlements(proof_status)
  WHERE proof_status IN ('pending', 'failed');
```

- [ ] **Step 2: Write the failing test**

`settlement-worker/tests/dual_sink.rs`:

```rust
mod common; // reuse plan-D setup_test_postgres which runs migrations
use common::setup_test_postgres;

#[tokio::test]
async fn migration_adds_settle_mode() {
    let (pool, _c) = setup_test_postgres().await;
    let row: (String,) = sqlx::query_as(
        "SELECT column_default FROM information_schema.columns
         WHERE table_name='positions' AND column_name='settle_mode'",
    ).fetch_one(&pool).await.expect("column exists");
    assert!(row.0.contains("points"));
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p tap-trading-settlement-worker migration_adds_settle_mode`
Expected: FAIL — column missing (migration not yet picked up) or compile error if `common` lacks the migration path. (If `setup_test_postgres` auto-runs `migrations/`, the new file makes it pass once present; run before adding the file to see the failure, or assert the constraint name.)

- [ ] **Step 4: Verify it passes**

Run: `cargo test -p tap-trading-settlement-worker migration_adds_settle_mode`
Expected: PASS — the migration ran in the test container.

- [ ] **Step 5: Commit**

```bash
git add games/tap-trading/backend/migrations games/tap-trading/backend/settlement-worker/tests/dual_sink.rs
git commit -m "feat(tick-worker): add settle_mode columns migration"
```

---

## Task 2: Load settler keypair + vault config from env

**Files:**
- Create: `games/tap-trading/backend/settlement-worker/src/sui_config.rs`
- Modify: `games/tap-trading/backend/settlement-worker/src/main.rs` (mod decl)
- Modify: `games/tap-trading/backend/settlement-worker/Cargo.toml`

- [ ] **Step 1: Write the failing test**

In `sui_config.rs` (inline test):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_config_from_map() {
        let cfg = SuiSettleConfig::from_vars(|k| match k {
            "TICK_VAULT_PKG" => Some("0xpkg".into()),
            "TICK_VAULT_ID" => Some("0xvault".into()),
            "TICK_SETTLER_CAP_ID" => Some("0xcap".into()),
            // 32-byte ed25519 seed, base64
            "TICK_SETTLER_PRIVKEY" => Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".into()),
            "SUI_RPC_URL" => Some("https://fullnode.testnet.sui.io:443".into()),
            _ => None,
        }).expect("config");
        assert_eq!(cfg.vault_pkg, "0xpkg");
        assert_eq!(cfg.settler_address().to_string().len(), 66); // 0x + 64 hex
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p tap-trading-settlement-worker parses_config_from_map`
Expected: FAIL — `SuiSettleConfig` undefined.

- [ ] **Step 3: Implement**

Add deps to `settlement-worker/Cargo.toml`: `sui-sdk = { workspace = true }`, `fastcrypto = { workspace = true }`, `base64 = "0.22"`. In root `backend/Cargo.toml` `[workspace.dependencies]` add `sui-sdk = { git = "https://github.com/MystenLabs/sui", package = "sui-sdk", rev = "framework/testnet" }` and `fastcrypto = { git = "https://github.com/MystenLabs/fastcrypto" }`.

`sui_config.rs`:

```rust
use fastcrypto::ed25519::Ed25519KeyPair;
use fastcrypto::traits::{KeyPair, ToFromBytes};
use sui_sdk::types::base_types::SuiAddress;

pub struct SuiSettleConfig {
    pub rpc_url: String,
    pub vault_pkg: String,
    pub vault_id: String,
    pub settler_cap_id: String,
    keypair: Ed25519KeyPair,
}

impl SuiSettleConfig {
    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let need = |k: &str| get(k).ok_or_else(|| anyhow::anyhow!("missing {k}"));
        let seed = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD, need("TICK_SETTLER_PRIVKEY")?,
        )?;
        let keypair = Ed25519KeyPair::from_bytes(&seed)
            .map_err(|e| anyhow::anyhow!("bad settler key: {e}"))?;
        Ok(Self {
            rpc_url: need("SUI_RPC_URL")?,
            vault_pkg: need("TICK_VAULT_PKG")?,
            vault_id: need("TICK_VAULT_ID")?,
            settler_cap_id: need("TICK_SETTLER_CAP_ID")?,
            keypair,
        })
    }
    pub fn settler_address(&self) -> SuiAddress {
        SuiAddress::from(&self.keypair.public())
    }
    pub fn keypair(&self) -> &Ed25519KeyPair { &self.keypair }
}
```

Add `mod sui_config;` to `main.rs`.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p tap-trading-settlement-worker parses_config_from_map`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add games/tap-trading/backend/settlement-worker games/tap-trading/backend/Cargo.toml
git commit -m "feat(tick-worker): load settler keypair + vault config from env"
```

---

## Task 3: Build settle_win PTB (no submit)

**Files:**
- Create: `games/tap-trading/backend/settlement-worker/src/sui_settle.rs`
- Modify: `games/tap-trading/backend/settlement-worker/src/main.rs` (mod decl)

- [ ] **Step 1: Write the failing test**

In `sui_settle.rs` (inline test):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settle_win_ptb_targets_vault() {
        let ptb = build_settle_win(
            "0xpkg", "0xvault", "0xcap", "0xpos", "0xpb",
        ).expect("ptb");
        // the PTB contains exactly one MoveCall to vault::settle_win
        let calls = move_call_targets(&ptb);
        assert_eq!(calls, vec!["0xpkg::vault::settle_win".to_string()]);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p tap-trading-settlement-worker settle_win_ptb_targets_vault`
Expected: FAIL — `build_settle_win`, `move_call_targets` undefined.

- [ ] **Step 3: Implement**

`sui_settle.rs`:

```rust
use sui_sdk::types::programmable_transaction_builder::ProgrammableTransactionBuilder;
use sui_sdk::types::transaction::{ProgrammableTransaction, Command};
use sui_sdk::types::base_types::ObjectID;
use std::str::FromStr;

/// Build the settle_win PTB. Object args are resolved to their latest
/// versions by the caller's submit path (Task 5) via the SDK's
/// `transaction_builder`; here we encode the move call shape.
pub fn build_settle_win(
    pkg: &str, vault: &str, cap: &str, position: &str, player_balance: &str,
) -> anyhow::Result<ProgrammableTransaction> {
    let mut b = ProgrammableTransactionBuilder::new();
    let pkg_id = ObjectID::from_str(pkg)?;
    // object inputs are added by the submit path which knows on-chain versions;
    // build_* records the call so unit tests assert shape without RPC.
    let cap_arg = b.obj(sui_sdk::types::transaction::ObjectArg::ImmOrOwnedObject(
        resolve_placeholder(cap)?))?;
    let vault_arg = b.obj(shared_mut(vault)?)?;
    let pos_arg = b.obj(shared_mut(position)?)?;
    let pb_arg = b.obj(shared_mut(player_balance)?)?;
    b.command(Command::move_call(
        pkg_id, ident("vault"), ident("settle_win"),
        vec![usdc_type_tag()?],            // <Quote> = USDC
        vec![cap_arg, vault_arg, pos_arg, pb_arg],
    ));
    Ok(b.finish())
}

// test helper: list the fully-qualified move-call targets in a PTB
#[cfg(test)]
pub fn move_call_targets(ptb: &ProgrammableTransaction) -> Vec<String> {
    ptb.commands.iter().filter_map(|c| match c {
        Command::MoveCall(m) => Some(format!("{}::{}::{}", m.package, m.module, m.function)),
        _ => None,
    }).collect()
}
```

> Note: `resolve_placeholder`, `shared_mut`, `ident`, `usdc_type_tag` are thin helpers defined in this file; object-version resolution against the live chain happens in Task 5's `submit`. The unit test asserts call *shape* only — no RPC.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p tap-trading-settlement-worker settle_win_ptb_targets_vault`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add games/tap-trading/backend/settlement-worker
git commit -m "feat(tick-worker): build settle_win PTB (no submit)"
```

---

## Tasks 4–11

> Tasks 4–11 follow the same TDD rhythm and are enumerated in the Commit map. Key shapes:
> - **Task 4** mirrors Task 3 for `settle_loss` (args: cap, vault, position — no player_balance) and `settle_void` (cap, vault, position, player_balance).
> - **Task 5** `submit` uses `SuiClientBuilder::default().build(rpc_url)`, resolves object versions via `read_api().get_object_with_options`, signs with the settler `Ed25519KeyPair`, calls `quorum_driver_api().execute_transaction_block` with `WaitForLocalExecution`, asserts `effects.status` success, returns digest. Tested live in Task 11 (gated).
> - **Task 6** is the dual-sink branch: in `loop_runner.rs`, after `evaluate_position` yields an outcome, `SELECT settle_mode, vault_*` for the position; `points` → plan-D `settle.rs` path unchanged; `usdc` → run the plan-D `settlements` ON CONFLICT canary, and only on insert call `sui_settle::submit(build_settle_win(...))`.
> - **Task 7** `evidence::fetch_window` loops `GET /ring/:asset/:seq?run_id=N` from `oracle_seq_at_tap` to `touch_seq`; 409/410 → partial + `proof_status` will be `failed`.
> - **Task 8** chains `assemble` (plan 2) → `WalrusClient::store_blob` → `build_anchor_proof` PTB → `submit` → `UPDATE settlements SET proof_status='published', walrus_blob_id=$1, sui_tx_digest=$2`.
> - **Task 9** the 60 s retry sweep over `proof_status IN ('failed')`. **Task 10** asserts a `usdc` settle writes NO `points_ledger` row (funds are on-chain). **Task 11** is the feature-gated testnet end-to-end.

> **Stop after Task 6 and request review** — the dual-sink branch is where points and on-chain settlement diverge; it's the highest-risk integration point and the right checkpoint before the proof-publish plumbing.

---

## Self-review notes

- **Spec coverage:** ADR-0010 §6 settle entries → Tasks 3–5; §7 dual-sink → Task 6; §8 no points↔USDC (no ledger row on usdc) → Task 10. ADR-0011 §4 publish sequence → Tasks 7–8; §6 evidence ring → Task 7; best-effort decoupling → Task 9. Plan-D idempotency canary reused in Task 6 (deviation note documents the crash-window ordering + the on-chain `EPositionNotOpen` backstop).
- **Type consistency:** `SuiSettleConfig`, `build_settle_win/loss/void`, `build_anchor_proof`, `submit`, `fetch_window`, `proof_status` values (`pending|published|failed`) used identically across tasks. `assemble`/`WalrusClient`/`multiplier_f64_to_bps` come from plan 2 unchanged.
- **Idempotency / double-submit:** two-layer guard documented — DB canary first, Move `EPositionNotOpen` authoritative. Single-leader advisory lock (plan D) prevents concurrent submit.
- **Gap acknowledged:** the exact `sui-sdk` object-version resolution API (`get_object_with_options` → `ObjectArg`) is sketched, not pinned to a version — Task 5 must confirm against the `sui-sdk` rev that builds (the SDK's PTB-builder surface shifts across revs). This is the one place an engineer needs to consult the live `sui-sdk` docs; flagged so it's not a silent surprise.

---

**Plan complete and saved to `docs/superpowers/plans/2026-05-27-tick-vault-worker-integration.md`.** Plan 3 of 3. Execute after plans 1 (vault) and 2 (proofs). This closes the ADR-0010 + ADR-0011 loop end-to-end.

---

## Execution handoff (whole phase)

Three plans now exist for the vault + Walrus phase:

1. `2026-05-27-tick-onchain-vault.md` — Move vault (on-chain truth)
2. `2026-05-27-tick-walrus-proofs.md` — proof types + verifier + Walrus client
3. `2026-05-27-tick-vault-worker-integration.md` — wire the worker (this plan)

**Recommended order: 1 → 2 → 3.** Plan 1 has no deps; plan 2 needs plan 1's `Position` shapes for its end-to-end test; plan 3 needs both.

**Two execution options per plan:**
1. **Subagent-Driven (recommended)** — fresh subagent per task, review between tasks.
2. **Inline Execution** — batch with checkpoints via executing-plans.

**Which approach, and shall I start with plan 1?**
