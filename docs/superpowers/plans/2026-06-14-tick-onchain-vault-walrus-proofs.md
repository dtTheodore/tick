# Tick on-chain vault + Walrus proofs — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (inline, this session) to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the verifiable USDC vault loop for Tick — players custody testnet USDC in an on-chain `tick_vault`, the settlement worker pays winners via a `SettlerCap`, every USDC settlement publishes a Walrus proof, and a pure verifier replays it — then prove it end-to-end on testnet.

**Architecture:** A new Move package `tick_vault` (custody + settler-authorized payouts + on-chain exposure caps + idempotency). The aggregator retains 120 s of ticks and gains a range endpoint for proof evidence. Touch detection is extracted to a shared crate so the worker and the verifier agree by construction. The worker gains a USDC sink that signs `settle_*`/`anchor_proof` via the `sui` CLI (behind a `SettlerClient` trait) and publishes proofs via the `walrus` CLI (behind a `ProofPublisher` trait). Settlement correctness never depends on Walrus.

**Tech Stack:** Sui Move 2024.beta, `sui`/`walrus` CLIs (testnet), Rust (tokio/axum/sqlx, edition 2021), `tap-trading-pricing-engine`, Postgres.

**Spec:** `docs/superpowers/specs/2026-06-14-tick-onchain-vault-walrus-proofs-design.md`. Reconciles ADR-0010, ADR-0011, and the four 2026-05-27 plans (`tick-onchain-vault`, `tick-walrus-proofs`, `tick-vault-worker-integration`, `tick-spec-sync-vault-walrus`). Where this plan and a 2026-05-27 plan diverge, **this plan wins** (signing via CLI not sui-sdk; shared touch crate; range endpoint; USDC terminology).

**Deviations recorded before coding (Rule 7/12):**
- Worker signs via `sui`/`walrus` CLIs behind traits, NOT `sui-sdk` and NOT `platform-lib-sui` (which is zkLogin-only). Phase-5 swaps the impl behind the trait.
- Verifier re-runs the worker's **path-segment** touch via the shared `tap-trading-touch` crate, NOT point-sampling.
- Evidence is one range call, not N single-seq calls.
- Per-cell cap is per-mint (aggregated-per-band `Table` deferred).

**Verification baseline (run once before starting):**
```bash
cd /Users/thangta/WorkProject/commandoss/dopamint
git branch --show-current        # expect: local/tick-ui
sui client active-env            # expect: testnet
sui client active-address        # funded address
walrus info | head -3            # walrus reachable
```

---

## File map

### Created
- `games/tap-trading/move/tick_vault/{Move.toml, sources/vault.move, tests/vault_tests.move}`
- `games/tap-trading/backend/touch/{Cargo.toml, src/lib.rs}` — `tap-trading-touch`
- `games/tap-trading/backend/proof-types/{Cargo.toml, src/lib.rs, tests/fixtures/proof_won.json}`
- `games/tap-trading/backend/proof-verifier/{Cargo.toml, src/lib.rs, src/bin/proof_verify.rs}`
- `games/tap-trading/backend/settlement-worker/src/{sui_settle.rs, evidence.rs, proof_publish.rs, usdc_sink.rs, onchain_config.rs}`
- `games/tap-trading/backend/migrations/20260614000000_add_usdc_settle_mode.sql`
- `games/tap-trading/scripts/deploy-tick-vault.sh`, `games/tap-trading/scripts/e2e-onchain.sh`

### Modified
- `games/tap-trading/backend/Cargo.toml` — add 3 workspace members + deps (`base64`, `tempfile`)
- `games/tap-trading/backend/oracle-aggregator/src/{constants.rs, ring_buffer.rs, api.rs}` — 120 s ring + range endpoint
- `games/tap-trading/backend/settlement-worker/{Cargo.toml, src/lib.rs, src/touch.rs, src/loop_runner.rs, src/cache.rs}` — depend on shared touch; carry `settle_mode` + on-chain ids; dual-sink dispatch
- Five `games/tap-trading/docs/*.md` — spec-sync (final task)

---

## Task 1: tick_vault Move package — structs, caps, errors

**Files:**
- Create: `games/tap-trading/move/tick_vault/Move.toml`
- Create: `games/tap-trading/move/tick_vault/sources/vault.move`

- [ ] **Step 1: Write `Move.toml`** (match repo convention; generic over `usdc` named address so the same code serves USDC and the e2e test coin)

```toml
[package]
name = "dopamint_tick_vault"
edition = "2024.beta"

[dependencies]
Sui = { git = "https://github.com/MystenLabs/sui.git", subdir = "crates/sui-framework/packages/sui-framework", rev = "framework/testnet" }

[addresses]
dopamint_tick_vault = "0x0"

[dev-dependencies]

[dev-addresses]
```

- [ ] **Step 2: Write `sources/vault.move`** — structs/caps/errors/constants per the `tick-onchain-vault` plan, with the invariants from spec §4 made explicit. Use the **full struct + function bodies from `docs/superpowers/plans/2026-05-27-tick-onchain-vault.md` Tasks 2–10 verbatim** (they are faithful to ADR-0010), confirming each of these is present:
  - Structs: `VaultConfig has store`, `GameVault<phantom Quote> has key`, `PlayerBalance<phantom Quote> has key`, `Position<phantom Quote> has key` (with `is_bullish: bool`), `SettlerCap has key, store`, `AdminCap has key, store`, `ProofAnchored has copy, drop, store`.
  - Errors `EInvalidBand=1 … EInsufficientPlayerBalance=10`; constants `MULTIPLIER_FLOOR_BPS=10_000`, `BPS_DENOM=10_000`, `STATUS_OPEN=0/WON=1/LOST=2/VOID=3`.
  - `mul_bps(stake, bps): u64` via `u128` intermediate.

- [ ] **Step 3: Build (no tests yet)**

Run: `cd games/tap-trading/move/tick_vault && sui move build`
Expected: compiles; if the framework rev errors, align `rev` to the installed CLI's testnet framework (the build message names the expected rev). Commit the generated `Move.lock`.

- [ ] **Step 4: Commit**
```bash
git add games/tap-trading/move/tick_vault
git commit -m "feat(tick-vault): scaffold move package structs and caps"
```

---

## Task 2: tick_vault entries — deposit/mint/settle/anchor with invariants

**Files:**
- Modify: `games/tap-trading/move/tick_vault/sources/vault.move`

- [ ] **Step 1: Add entries** (signatures pinned by spec §4):
  - `create_vault<Quote>(settler, per_cell_max_liability, max_directional_imbalance_bps, treasury_min_buffer_bps, max_multiplier_bps, ctx)` — shares vault, transfers `SettlerCap`→settler, `AdminCap`→sender.
  - `open_balance<Quote>(ctx)`, `deposit<Quote>(&mut PlayerBalance, Coin<Quote>)`, `withdraw<Quote>(&mut PlayerBalance, amount, ctx): Coin<Quote>` (asserts `available >= amount`).
  - `mint<Quote>(vault, pb, asset:u8, strike_lo, strike_hi, t_open_ms, t_close_ms, stake, multiplier_bps, oracle_seq_at_tap, oracle_run_id, is_bullish, ctx)` — assert order: `!paused`(9), `strike_lo<strike_hi`(1), `multiplier_bps>=MULTIPLIER_FLOOR_BPS`(2), `multiplier_bps<=config.max_multiplier_bps`(3), `available>=stake`(10), per-cell `liability<=per_cell_max_liability`(4), directional imbalance(5), treasury buffer(6). Then stake→treasury, `total_open_liability += liability`, bull/bear branch on `is_bullish`, transfer `Position` to sender.
  - `settle_win<Quote>(cap, vault, position, player_balance)` / `settle_loss<Quote>(cap, vault, position)` / `settle_void<Quote>(cap, vault, position, player_balance)`.

- [ ] **Step 2: Bake in the two correctness invariants (spec §4):**
  - Each `settle_*` asserts `cap.vault_id == object::id(vault)` (`ECapVaultMismatch=7`) AND `position.status == STATUS_OPEN` (`EPositionNotOpen=8`), then sets the terminal status. (Idempotency: a re-submit aborts.)
  - Each `settle_*` decrements `total_open_liability` by the position's `liability` and decrements the matching `bullish/bearish_liability` — **on win, loss, AND void**.
  - `settle_win` pays `mul_bps(stake, multiplier_bps)` treasury→`player_balance.available`; `settle_void` refunds `stake` treasury→`player_balance.available`; `settle_loss` transfers nothing (stake already in treasury).
  - `anchor_proof(cap, position_id: ID, blob_id: vector<u8>)` asserts cap match and emits `ProofAnchored`.
  - `set_paused(&AdminCap, &mut GameVault, bool)`, `update_config(&AdminCap, &mut GameVault, …)`.
  - Read accessors: `settler`, `treasury_value`, `available`, plus test-only liability getters.

- [ ] **Step 3: Build**

Run: `cd games/tap-trading/move/tick_vault && sui move build`
Expected: compiles clean.

- [ ] **Step 4: Commit**
```bash
git add games/tap-trading/move/tick_vault/sources/vault.move
git commit -m "feat(tick-vault): deposit/mint/settle/anchor with caps and idempotency"
```

---

## Task 3: tick_vault Move tests

**Files:**
- Create: `games/tap-trading/move/tick_vault/tests/vault_tests.move`

- [ ] **Step 1: Write tests** with a `#[test_only]` 6-decimal mintable coin `E2E_COIN` so no faucet is needed. Cover, by behavior name:
  - `mint_rejects_multiplier_above_cap` → `#[expected_failure(abort_code=vault::EMultiplierAboveCap)]`
  - `mint_rejects_inverted_band` → `EInvalidBand`
  - `mint_rejects_below_floor` → `EMultiplierBelowFloor`
  - `mint_rejects_per_cell_cap` → `ECellCapExceeded`
  - `mint_rejects_directional_cap` → `EDirectionalCapExceeded`
  - `mint_rejects_treasury_buffer` → `ETreasuryBufferExceeded`
  - `mint_rejects_insufficient_balance` → `EInsufficientPlayerBalance`
  - `settle_rejects_wrong_cap` → `ECapVaultMismatch`
  - `settle_win_then_settle_again_aborts` → `EPositionNotOpen` (idempotency)
  - `settle_win_pays_exact_and_decrements_liability` — assert `player_balance.available` rises by `stake*mult/10000`, `total_open_liability` returns to 0.
  - `settle_loss_decrements_liability_no_payout`
  - `settle_void_refunds_stake_and_decrements_liability`
  - `solvency_under_directional_cap` — mint bullish to the cap, mass-`settle_win`, assert treasury never underflows (no abort, treasury_value stays ≥ 0).

- [ ] **Step 2: Run**

Run: `cd games/tap-trading/move/tick_vault && sui move test`
Expected: all tests pass.

- [ ] **Step 3: Commit**
```bash
git add games/tap-trading/move/tick_vault/tests/vault_tests.move
git commit -m "test(tick-vault): caps, idempotency, payout exactness, solvency"
```

---

## Task 4: Deploy tick_vault to testnet + capture IDs

**Files:**
- Create: `games/tap-trading/scripts/deploy-tick-vault.sh`

- [ ] **Step 1: Write the deploy script** — publishes the package, calls `create_vault<Quote>` with generous testnet caps, parses the package id / shared vault id / `SettlerCap` id from `--json` output, writes them to `tmp/tick-vault-deploy.json`, and appends the env keys to `.local/.env` (`TICK_VAULT_PKG`, `TICK_VAULT_ID`, `TICK_SETTLER_CAP_ID`, `TICK_SETTLER_ADDRESS`, `TICK_QUOTE_TYPE`, `SUI_RPC_URL`). `Quote` = the e2e test coin type for the first deploy (no faucet dependence); a USDC re-deploy is the same script with `TICK_QUOTE_TYPE=0xa1ec…::usdc::USDC`. Use `set -euo pipefail`; resolve paths relative to repo root.

- [ ] **Step 2: Run deploy**

Run: `bash games/tap-trading/scripts/deploy-tick-vault.sh`
Expected: prints package id + vault id + cap id; `tmp/tick-vault-deploy.json` exists; `sui client object <TICK_VAULT_ID> --json` shows a shared `GameVault`.

- [ ] **Step 3: Commit the script** (NOT the ids — they live in `.local/.env`/`tmp`, gitignored)
```bash
git add games/tap-trading/scripts/deploy-tick-vault.sh
git commit -m "feat(tick-vault): testnet deploy + id-capture script"
```

---

## Task 5: Aggregator — 120 s ring + range endpoint

**Files:**
- Modify: `games/tap-trading/backend/oracle-aggregator/src/constants.rs`
- Modify: `games/tap-trading/backend/oracle-aggregator/src/ring_buffer.rs`
- Modify: `games/tap-trading/backend/oracle-aggregator/src/api.rs`

- [ ] **Step 1: Write the failing test** in `ring_buffer.rs` for a contiguous range read:
```rust
#[test]
fn range_returns_inclusive_span_in_seq_order() {
    let ring = AssetRing::default();
    for seq in 100..=130 { ring_push(&ring, seq); } // helper pushes a tick at seq
    let got = ring.range(AssetSymbol::Btc, /*run_id*/ 1, 110, 120);
    assert!(matches!(&got, RangeLookup::Hit(v) if v.len() == 11 && v[0].seq == 110 && v[10].seq == 120));
}
```

- [ ] **Step 2: Bump retention** in `constants.rs`: `RING_SIZE` 10 → `2400` with a comment `// 120 s @ 20 Hz; ADR-0011 §6 proof-evidence window`.

- [ ] **Step 3: Implement `range`** in `ring_buffer.rs` returning `RangeLookup { Hit(Vec<OracleTick>), Gone, Conflict }`: `Conflict` if run_id mismatches newest; `Gone` if `from_seq` < oldest retained seq; else collect entries with `from_seq <= seq <= to_seq` in order.

- [ ] **Step 4: Run the test**

Run: `cargo test -p tap-trading-oracle-aggregator range_returns_inclusive_span -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Add the HTTP route** in `api.rs`: `GET /ring/:asset/range` with `Query { run_id, from_seq, to_seq }`. Reject `to_seq < from_seq` (400) and `to_seq - from_seq > RING_SIZE` (400). Map `Hit`→200 JSON array, `Conflict`→409, `Gone`→410, unknown asset→404. Mirror the existing `/ring/:asset/:seq` handler's asset parsing.

- [ ] **Step 6: Build + test the crate**

Run: `cargo test -p tap-trading-oracle-aggregator`
Expected: PASS.

- [ ] **Step 7: Commit**
```bash
git add games/tap-trading/backend/oracle-aggregator/src
git commit -m "feat(tick-oracle): 120s evidence ring + range endpoint"
```

---

## Task 6: Extract shared touch crate

**Files:**
- Create: `games/tap-trading/backend/touch/Cargo.toml`, `games/tap-trading/backend/touch/src/lib.rs`
- Modify: `games/tap-trading/backend/Cargo.toml` (add member `touch` + workspace dep `tap-trading-touch = { path = "touch" }`)
- Modify: `games/tap-trading/backend/settlement-worker/src/touch.rs` (re-export shared logic), `settlement-worker/Cargo.toml`

- [ ] **Step 1: Create `tap-trading-touch`** with the pure functions moved verbatim from `settlement-worker/src/touch.rs`: `segment_intersects_band(prev, cur, lo, hi) -> bool` and a band-only predicate `path_touches_band(prev_mid: Option<f64>, cur_mid: f64, lo: f64, hi: f64) -> bool` (the segment-or-point fallback). Move the existing unit tests for these. Depend on `tap-trading-oracle-types` only if needed (prefer raw f64 args so the verifier needn't pull oracle-types).

- [ ] **Step 2: Re-point the worker** — `settlement-worker/src/touch.rs::evaluate_position` calls `tap_trading_touch::path_touches_band(...)` instead of its local copy; keep `TouchOutcome` and `evaluate_position` (with the `t_open/t_close` window logic) in the worker since they reference `PositionRef`.

- [ ] **Step 3: Run worker touch tests**

Run: `cargo test -p tap-trading-settlement-worker touch`
Expected: PASS (behavior unchanged).

- [ ] **Step 4: Commit**
```bash
git add games/tap-trading/backend/touch games/tap-trading/backend/Cargo.toml games/tap-trading/backend/settlement-worker
git commit -m "refactor(tick): extract shared tap-trading-touch crate"
```

---

## Task 7: proof-types crate

**Files:**
- Create: `games/tap-trading/backend/proof-types/{Cargo.toml, src/lib.rs, tests/fixtures/proof_won.json}`
- Modify: `games/tap-trading/backend/Cargo.toml`

- [ ] **Step 1: Define `ProofBlob`** and nested types per spec §6 / ADR-0011 §1 (serde, `Outcome` UPPERCASE). Add `pub fn multiplier_f64_to_bps(m: f64) -> u64 { (m * 10_000.0).floor() as u64 }` and `pub const BPS_EPSILON: u64 = 1;`. Add `EvidenceTick { seq, ts_ms, mid }` and an `assemble(AssembleInput) -> ProofBlob` constructor.

- [ ] **Step 2: Write the failing round-trip test** + commit a golden `proof_won.json` fixture:
```rust
#[test]
fn proof_blob_round_trips_and_floors_bps() {
    assert_eq!(multiplier_f64_to_bps(1.9580), 19580);
    assert_eq!(multiplier_f64_to_bps(1.95809), 19580); // floor
    let json = include_str!("fixtures/proof_won.json");
    let blob: ProofBlob = serde_json::from_str(json).unwrap();
    assert_eq!(blob.settlement.outcome, Outcome::Won);
    let reser = serde_json::to_string(&blob).unwrap();
    assert!(serde_json::from_str::<ProofBlob>(&reser).is_ok());
}
```

- [ ] **Step 3: Run**

Run: `cargo test -p tap-trading-proof-types`
Expected: PASS.

- [ ] **Step 4: Commit**
```bash
git add games/tap-trading/backend/proof-types games/tap-trading/backend/Cargo.toml
git commit -m "feat(tick-proof): proof blob types + floor bps conversion"
```

---

## Task 8: proof-verifier crate (lib + CLI)

**Files:**
- Create: `games/tap-trading/backend/proof-verifier/{Cargo.toml, src/lib.rs, src/bin/proof_verify.rs}`
- Modify: `games/tap-trading/backend/Cargo.toml`

- [ ] **Step 1: Implement `verify(&ProofBlob) -> VerifyResult`** (spec §6): build `Cell`/`OracleState` from the blob (`band.lo/hi as f64 / 1e9`; `now_ms = window.t_open_ms`), call `tap_trading_pricing_engine::compute_multiplier`, floor with `multiplier_f64_to_bps`, compare to `claimed` within `BPS_EPSILON`. Then re-run touch over `evidence_ticks` using `tap_trading_touch::path_touches_band` segment-by-segment (consecutive ticks), find the first crossing seq; `InsufficientEvidence` if `evidence_ticks` don't span `[t_open, t_close]`; compare derived outcome to `claimed`. Order: insufficient-evidence → multiplier → outcome → `Valid`.

- [ ] **Step 2: Write tests** (all four `VerifyResult` variants, reusing the golden fixture): `valid`, `MultiplierMismatch` (tamper `multiplier_bps`), `OutcomeMismatch` (flip an evidence tick out of band), `InsufficientEvidence` (truncate evidence). Include one **segment-leap** case: evidence where no single tick is inside the band but a segment crosses it → verifier returns the same WON the worker would (guards the §1 conflict-1 regression).

- [ ] **Step 3: Run**

Run: `cargo test -p tap-trading-proof-verifier`
Expected: PASS.

- [ ] **Step 4: CLI** `proof_verify.rs`: read a JSON file arg (or `--blob-id <id> --aggregator <url>` to GET then verify), print the `VerifyResult` and exit non-zero on non-`Valid`.

- [ ] **Step 5: Smoke the CLI**

Run: `cargo run -p tap-trading-proof-verifier --bin proof-verify -- games/tap-trading/backend/proof-types/tests/fixtures/proof_won.json`
Expected: prints `Valid`, exit 0.

- [ ] **Step 6: Commit**
```bash
git add games/tap-trading/backend/proof-verifier games/tap-trading/backend/Cargo.toml
git commit -m "feat(tick-proof): replay verifier (lib + CLI) reusing pricing engine"
```

> WASM build (`wasm-bindgen verify_json`) is a stated deferral; attempt only if `wasm-pack`/`wasm32` target is already present.

---

## Task 9: DB migration — settle_mode + on-chain ids + proof_status

**Files:**
- Create: `games/tap-trading/backend/migrations/20260614000000_add_usdc_settle_mode.sql`

- [ ] **Step 1: Write the migration** (spec §7): the `positions` columns + present-ids constraint, the `settlements` columns + partial proof-pending index. Pure `ALTER TABLE … ADD COLUMN`, all additive, defaults keep existing rows valid (`settle_mode='points'`, `proof_status='pending'` — but existing settlements are points so the index only matters for new usdc rows).

- [ ] **Step 2: Apply against a throwaway PG** (or the dev DB via the migrate runner)

Run: `cargo run -p tap-trading-migrate` (or the project's migrate command after `./scripts/init-worktree-dev.sh`)
Expected: migration applies; `\d positions` shows the new columns.

- [ ] **Step 3: Commit**
```bash
git add games/tap-trading/backend/migrations/20260614000000_add_usdc_settle_mode.sql
git commit -m "feat(tick): positions.settle_mode + on-chain ids + proof_status"
```

---

## Task 10: Worker — onchain config + SettlerClient (sui CLI) trait

**Files:**
- Create: `games/tap-trading/backend/settlement-worker/src/onchain_config.rs`, `src/sui_settle.rs`
- Modify: `settlement-worker/Cargo.toml` (`base64`, `tempfile`, `tap-trading-proof-types`, `tap-trading-proof-verifier`? no — verifier not needed at runtime), `src/lib.rs`

- [ ] **Step 1: `onchain_config.rs`** — `OnchainConfig::from_env()` reading `TICK_VAULT_PKG`, `TICK_VAULT_ID`, `TICK_SETTLER_CAP_ID`, `TICK_SETTLER_ADDRESS`, `TICK_QUOTE_TYPE`, `SUI_RPC_URL` (all required, no fallback; error if missing — Rule "fail loud"). Returns `Option` only at the top level so points-only runs without these vars still boot.

- [ ] **Step 2: `SettlerClient` trait** + a `SuiCliSettler` impl:
```rust
#[async_trait::async_trait]
pub trait SettlerClient: Send + Sync {
    async fn settle_win(&self, p: &OnchainPosition) -> anyhow::Result<String>;   // returns tx digest
    async fn settle_loss(&self, p: &OnchainPosition) -> anyhow::Result<String>;
    async fn settle_void(&self, p: &OnchainPosition) -> anyhow::Result<String>;
    async fn anchor_proof(&self, position_id: &str, blob_id: &str) -> anyhow::Result<String>;
}
```
`SuiCliSettler` builds `sui client call --package $PKG --module vault --function settle_win --type-args $QUOTE --args $CAP $VAULT $POSITION $PLAYER_BALANCE --gas-budget … --json`, signing as `TICK_SETTLER_ADDRESS` (ensure it is the active address, or `sui client switch --address` first), parses `effects.status` (must be success) and the digest. `anchor_proof` passes the blob id bytes. Shell out with `tokio::process::Command`; treat a Move abort in `effects.status` as a typed error so the dual-sink can distinguish `EPositionNotOpen` (already settled → benign) from real failures.

- [ ] **Step 3: Unit-test the arg builder** (pure, no chain): a `move_call_args(...)` helper that returns the `Vec<String>` of CLI args; assert package/module/function/type-arg/object-id ordering. (Rule 9: the test encodes the PTB contract.)

Run: `cargo test -p tap-trading-settlement-worker sui_settle`
Expected: PASS.

- [ ] **Step 4: Commit**
```bash
git add games/tap-trading/backend/settlement-worker
git commit -m "feat(tick-worker): onchain config + sui-cli settler client"
```

---

## Task 11: Worker — evidence fetch + Walrus ProofPublisher (walrus CLI)

**Files:**
- Create: `games/tap-trading/backend/settlement-worker/src/evidence.rs`, `src/proof_publish.rs`
- Modify: `settlement-worker/Cargo.toml`, `src/lib.rs`

- [ ] **Step 1: `evidence.rs`** — `fetch_window(http, aggregator_url, asset, run_id, from_seq, to_seq) -> anyhow::Result<Vec<EvidenceTick>>` hitting `GET /ring/:asset/range?run_id&from_seq&to_seq` (one call). Map 409/410 to a typed `EvidenceIncomplete` so the caller marks the proof `failed` without blocking payout.

- [ ] **Step 2: `ProofPublisher` trait** + `WalrusCliPublisher`:
```rust
#[async_trait::async_trait]
pub trait ProofPublisher: Send + Sync {
    async fn store(&self, bytes: &[u8]) -> anyhow::Result<String>;     // returns blob id
    async fn read(&self, blob_id: &str) -> anyhow::Result<Vec<u8>>;
}
```
`WalrusCliPublisher` writes bytes to a `tempfile`, runs `walrus store --epochs <N> --json <file>`, parses the blob id (`newlyCreated.blobObject.blobId` or `alreadyCertified.blobId`); `read` runs `walrus read <blob_id> --out <tempfile>` (or stdout). Use `WALRUS_STORE_EPOCHS` (default 5).

- [ ] **Step 3: Test the parsers** with captured `walrus store --json` sample output (both `newlyCreated` and `alreadyCertified` shapes).

Run: `cargo test -p tap-trading-settlement-worker proof_publish`
Expected: PASS.

- [ ] **Step 4: Commit**
```bash
git add games/tap-trading/backend/settlement-worker
git commit -m "feat(tick-worker): evidence range fetch + walrus-cli publisher"
```

---

## Task 12: Worker — dual-sink dispatch + proof retry sweep

**Files:**
- Create: `games/tap-trading/backend/settlement-worker/src/usdc_sink.rs`
- Modify: `settlement-worker/src/cache.rs` (carry `settle_mode` + on-chain ids on `PositionRef`), `src/loop_runner.rs`, `src/settle.rs` (usdc canary + proof-status update), `src/main.rs` (wire config + clients + sweep), `src/lib.rs`

- [ ] **Step 1: Extend `PositionRef` + hydration** in `cache.rs` to select `settle_mode, vault_position_id, vault_id, player_balance_id, owner_address`. Points rows keep `settle_mode='points'`.

- [ ] **Step 2: `usdc_sink.rs::settle_usdc(outcome, ctx, pos)`** implementing spec §8 sequence: (1) DB canary insert (reuse `settle.rs` helper that writes the `settlements` row with `proof_status='pending'`, no `points_ledger`); skip if not fresh. (2) `SettlerClient::settle_win/loss/void` → digest; if the Move abort is `EPositionNotOpen`, treat as already-settled and continue to mark settled. (3) `evidence::fetch_window` over `[oracle_seq_at_tap .. touch_seq]`. (4) assemble `ProofBlob` → `ProofPublisher::store` → blob id. (5) `anchor_proof`. (6) `UPDATE settlements SET proof_status='published', sui_tx_digest, walrus_blob_id`. Any failure at 3–6 logs + sets `proof_status='failed'` and returns Ok (payout already done).

- [ ] **Step 3: Dispatch in `loop_runner.rs`** — after `evaluate_position` yields Win/Expire, branch: `points` → existing `settle::settle_win/loss`; `usdc` → `usdc_sink::settle_usdc`. Void path (`process_gap_recovery`) branches the same way.

- [ ] **Step 4: Proof retry sweep** — a periodic task selecting `settlements` rows with `proof_status IN ('pending','failed')` whose position is `usdc`, re-running steps 3–6. Bounded batch; logs what it retries (no silent caps).

- [ ] **Step 5: Routing test** (testcontainers PG, mocked `SettlerClient`/`ProofPublisher`): a `usdc` position routes to the Sui path and writes **no** `points_ledger` row; a `points` position routes to Postgres unchanged; a failed publish flips `failed`→`published` on the sweep.

Run: `cargo test -p tap-trading-settlement-worker dual_sink`
Expected: PASS.

- [ ] **Step 6: Commit**
```bash
git add games/tap-trading/backend/settlement-worker
git commit -m "feat(tick-worker): usdc dual-sink + proof publish + retry sweep"
```

---

## Task 13: E2E on testnet

**Files:**
- Create: `games/tap-trading/scripts/e2e-onchain.sh`

- [ ] **Step 1: Write `e2e-onchain.sh`** (`set -euo pipefail`, paths relative to repo root) doing spec §9:
  1. Ensure a player keypair exists in the sui keystore; fund it with gas + mint/faucet the `TICK_QUOTE_TYPE` coin to it (test coin: a `mint` entry; USDC: Circle faucet).
  2. As player: `open_balance`, `deposit`, `mint` a Position with `t_open/t_close` a few seconds out, a band straddling the live mid (to force a WIN) — capture the Position object id + PlayerBalance id.
  3. `INSERT` the matching `positions` row (`settle_mode='usdc'`, the four on-chain ids, `oracle_seq_at_tap/run_id` from a current aggregator tick) so the worker hydrates it. (This stands in for the deferred API mint-record endpoint.)
  4. Wait for the worker to settle; poll `sui client object <POSITION>` until `status=WON` and `settlements.proof_status='published'`.
  5. Assert: PlayerBalance.available increased by `stake*mult/10000`; `ProofAnchored` event present (`sui client events`/tx); `walrus read <blob_id>` succeeds; pipe the blob through `proof-verify` → `Valid`.
  6. Negative: re-issue `settle_win` for the same Position via the settler → expect Move abort `EPositionNotOpen`.

- [ ] **Step 2: Run it** (worker + aggregator must be up via the worktree dev-env headless runner)

Run: `bash games/tap-trading/scripts/e2e-onchain.sh`
Expected: prints each assertion ✓ and a final `E2E PASS`. Iterate until it passes (the goal's success criterion).

- [ ] **Step 3: Commit**
```bash
git add games/tap-trading/scripts/e2e-onchain.sh
git commit -m "test(tick): on-chain e2e — mint→settle→proof→verify"
```

---

## Task 14: Spec-sync docs (doc-only)

**Files:**
- Modify: `games/tap-trading/docs/{MATH_SPEC.md, ORACLE_SPEC.md, SYSTEM_DESIGN.md, PRD.md, TESTING_STRATEGY.md}`

- [ ] **Step 1: Apply the five edits** verbatim from `docs/superpowers/plans/2026-05-27-tick-spec-sync-vault-walrus.md` Tasks 1–5 (MATH_SPEC §4.4 float→bps; ORACLE_SPEC 120 s ring; SYSTEM_DESIGN vault §1.1 + service rows + §1 override; PRD §9 + R9/R10; TESTING_STRATEGY §10). Preserve the unrelated in-flight MATH_SPEC pricing-engine edit (additive only). Normalize "DUSDC"→"USDC" in any new text.

- [ ] **Step 2: Verify anchors**

Run: `grep -n "4.4 Float→bps\|120 s\|tick_vault\|R9\|## 10" games/tap-trading/docs/*.md`
Expected: matches in each file.

- [ ] **Step 3: Commit** (one commit per file per the spec-sync plan's commit map).

---

## Self-review

- **Spec coverage:** §3 custody → Task 13 uses test keypairs (non-custodial, guests excluded noted in docs Task 14). §4 vault → Tasks 1–4. §5 aggregator → Task 5. §6 touch/proof/verifier/walrus → Tasks 6–8, 11. §7 DB → Task 9. §8 dual-sink → Tasks 10–12. §9 e2e → Task 13. §10 deferrals are explicit. §11 risks (CLI signing, faucet, touch drift) each have a mitigation in-plan.
- **Type consistency:** `SettlerClient`/`ProofPublisher` trait method names are used identically in Tasks 10–12; `multiplier_f64_to_bps`/`BPS_EPSILON` defined in Task 7 and consumed in Task 8; `RangeLookup`/`range()` defined in Task 5 and consumed in Task 11.
- **No placeholders:** every code-touching step shows the concrete signature, command, and expected output; large unchanged Move source explicitly points to the 2026-05-27 plan to copy verbatim (deliberate, not a placeholder).
- **TDD/commits:** each task is test-first where a pure unit exists (ring range, proof types, verifier, arg builders, dual-sink routing) and ends in a commit. Move tests precede deploy.
