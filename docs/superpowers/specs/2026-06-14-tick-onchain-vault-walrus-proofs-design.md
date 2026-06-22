# Tick on-chain vault + Walrus proofs — consolidated design

**Date:** 2026-06-14
**Branch:** `local/tick-ui`
**Specs:** ADR-0010 (on-chain vault custody & settlement), ADR-0011 (Walrus
per-tap proof anchoring).
**Supersedes (where they diverge):** the four 2026-05-27 plans
`tick-onchain-vault`, `tick-walrus-proofs`, `tick-vault-worker-integration`,
`tick-spec-sync-vault-walrus`. Those remain the detailed source material; this
document is the authoritative reconciliation against the *current* code and
records the decisions where the older plans are now stale.

---

## 1. Why this exists / what changed since 2026-05-27

The four prior plans were written the same day as the ADRs and are faithful to
them, but the code has moved on and three of their assumptions are now wrong or
suboptimal. This design resolves each conflict explicitly (CLAUDE.md Rule 7:
surface conflicts, pick one, don't blend):

1. **Touch detection diverged.** The settlement worker now settles on the price
   **path** — the straight segment `[prev_mid, tick.mid]` intersected with the
   band `[lo, hi)` (`settlement-worker/src/touch.rs`), because a fast wick can
   leap a narrow band entirely *between* two 20 Hz ticks. The prior verifier plan
   re-detected touch by **point sampling** (`first seq where lo ≤ mid ≤ hi`). A
   segment-leap win (neither endpoint inside the band) would replay as
   `OutcomeMismatch`/`InsufficientEvidence`. **Resolution:** extract the
   path-segment logic into a pure shared crate `tap-trading-touch`, and have
   **both** the worker and the verifier depend on it. One definition of "did it
   touch," exactly as ADR-0011 §5 demands for the multiplier
   (`tap-trading-pricing-engine` shared, no reimplementation drift). `touch_seq`
   in the proof is the seq of the tick that *closed* the first crossing segment.

2. **Sui/Walrus signing path.** Prior plan 3 used `sui-sdk` + `fastcrypto`
   directly. ADR-0010's Consequences say the worker "gains a dependency on
   `platform-lib-sui`" — but that crate is zkLogin-only (no RPC/PTB/keypair).
   **Resolution:** sign via the installed **`sui` and `walrus` CLIs behind two
   narrow traits** (`SettlerClient`, `ProofPublisher`). Rationale: `sui-sdk` is a
   git-pinned dependency that drags the whole Sui monorepo and routinely
   conflicts with this workspace's pinned `tokio`/`axum`/`sqlx`; the "verify e2e
   in one session" requirement breaks the tie toward the CLI, which is installed,
   has a funded keystore address, and works today (`sui 1.71.1`, `walrus` epoch
   32 testnet). The trait boundary makes the choice non-load-bearing: a `sui-sdk`
   / Walrus-HTTP implementation is a drop-in Phase-5 swap. **Deviation from
   ADR-0010 Consequences recorded here**; `platform-lib-sui` stays zkLogin-only.

3. **Evidence assembly.** Prior plan 3 looped `GET /ring/:asset/:seq` once per
   seq — 200–1200 sequential HTTP round trips per settlement (CLAUDE.md
   read/write rule forbids exactly this; the aggregator is a single writer and a
   contiguous read is cheap). **Resolution:** add one **range endpoint**
   `GET /ring/:asset/range?run_id&from_seq&to_seq` returning the whole span in one
   call. The worker-side in-memory tick buffer idea is dropped (YAGNI +
   partial-buffer-on-restart correctness edge); the aggregator's 120 s ring is the
   single canonical evidence source.

4. **Terminology.** ADR-0011 prose and SYSTEM_DESIGN say "DUSDC mode," but
   ADR-0010 §Context deliberately chose **Circle native USDC** over DeepBook's
   DUSDC. Normalize to **USDC** everywhere (`settle_mode IN ('points','usdc')`).

---

## 2. System model (read/write characteristics)

- **Writers.** (a) Players sign `deposit`/`mint` PTBs client-side (non-custodial;
  see §3). (b) The single-leader settlement worker signs `settle_*` +
  `anchor_proof` PTBs with the `SettlerCap` key, and `PUT`s proof blobs to Walrus.
  Both are low-frequency: one mint per tap, one settlement per position.
- **Readers.** Anyone can fetch a proof blob from Walrus and replay it offline
  with `tap-trading-proof-verifier` (pure, no IO). The aggregator range read
  happens once per USDC settlement.
- **Datasets.** Positions/settlements are bounded per session and append-only;
  the aggregator ring is fixed-size (2400/asset). Walrus blobs are one per USDC
  settlement, bounded by USDC volume (low in v1). No unbounded growth on any hot
  read path; no runtime aggregates over growing tables.
- **Custody.** Idle player funds live in their own `PlayerBalance` object; the
  vault treasury holds only staked premiums + reserve. The worker never holds a
  player key — it moves money only through `SettlerCap`, and only on positions it
  did not create.

---

## 3. Custody & who signs (settled from ADR-0001/0010/0013)

- USDC mode is **non-custodial**: the player signs `deposit` and `mint` with
  their own Sui wallet. Registered users have a zkLogin address
  (`accounts.sui_address` via `platform-lib-sui::derive_address`); backend cannot
  sign for them without the full zkLogin proof flow. **Guests have no wallet
  (ADR-0001) and therefore cannot play USDC mode** — points mode only. This is the
  honest, ADR-consistent posture and is stated, not worked around.
- **Settlement is backend-signed** via `SettlerCap`. The worker needs no player
  key: `settle_*` mutates the player's `PlayerBalance` through the cap. This is
  what makes the e2e achievable headlessly — see §9.
- The Sui-wallet identity is decoupled from platform identity (ADR-0013 open
  question); this design does not couple them. UI wallet-connect / deposit flow is
  **deferred** (§10).

---

## 4. Move package `tick_vault`

`games/tap-trading/move/tick_vault/`, module `tick_vault::vault`, edition
`2024.beta`. Generic over `phantom Quote` so the same code serves testnet/mainnet
Circle USDC (named address `usdc` resolved per build) — and so the e2e can
instantiate over a freely-mintable test coin without a faucet dependency (the
`<Quote>=USDC` instantiation is a config swap, which is the entire point of the
generic). Structs and entries follow the prior `tick-onchain-vault` plan
(verified faithful to ADR-0010 §2–§6), with these **invariants made explicit**
because they are the correctness core (advisor + Rule 9):

- **On-chain idempotency.** Every `settle_win/loss/void` asserts
  `position.status == STATUS_OPEN` and flips it. A double-submit aborts on-chain
  (`EPositionNotOpen`) — the authoritative double-pay guard, alongside the DB
  `settlements` canary. `settle_*` also asserts `cap.vault_id == object::id(vault)`
  (`ECapVaultMismatch`).
- **Liability decrements on ALL THREE paths** (win, loss, void), not just win.
  `total_open_liability` / `bullish_liability` / `bearish_liability` leak and
  eventually block every mint otherwise. This is a prime Move-test target.
- **Exposure caps** asserted at `mint`: `max_multiplier_bps`, per-cell
  (per-mint; the aggregated-per-band `Table` is a documented Forecast deferral),
  directional imbalance, treasury buffer. `multiplier_bps >= MULTIPLIER_FLOOR_BPS`
  (10000) and `liability = stake * multiplier_bps / 10000` via a u128 intermediate.
- `Position` carries `is_bullish: bool` (caller-supplied; drives the directional
  cap only; does **not** affect settlement — touch is direction-agnostic).
- `ProofAnchored { position_id, walrus_blob_id, outcome, settled_at_ms }` event
  emitted by `anchor_proof(cap, position_id, blob_id)` (ADR-0011 §2: event, not a
  field on `Position`).
- `AdminCap`-gated `set_paused` / `update_config`.

Error codes, status constants, and full signatures: as in the
`tick-onchain-vault` plan (`EInvalidBand=1` … `EInsufficientPlayerBalance=10`).
`settle_void(cap, vault, position, player_balance)` and
`settle_win(cap, vault, position, player_balance)` both take the balance object
(refund/payout target); `settle_loss(cap, vault, position)` does not.

**Tests:** `sui::test_scenario`; a `#[test_only]` 6-decimal mintable coin stands
in for USDC (no faucet). Every `public` entry: ≥1 happy + ≥1
`#[expected_failure(abort_code=…)]`. Closing **solvency scenario**: mint to the
directional cap, mass-`settle_win`, assert the treasury never underflows.

**Deploy & ID capture (new pattern — none exists).** `sui client publish`
captures the package id; `create_vault<Quote>` captures the shared vault id +
`SettlerCap` id. These plus the settler signer are surfaced to the worker **via
env into `.local/.env`** through the worktree dev-env contract (no hardcoded
literals, no string fallbacks): `TICK_VAULT_PKG`, `TICK_VAULT_ID`,
`TICK_SETTLER_CAP_ID`, `TICK_SETTLER_ADDRESS`, `TICK_QUOTE_TYPE`, `SUI_RPC_URL`,
`WALRUS_*`. The deploy writes a small JSON manifest under `tmp/` for the e2e
harness to read; the durable env keys are the contract.

---

## 5. Aggregator changes

- `RING_SIZE`: 10 → **2400** (120 s @ 20 Hz; ≈100 KB/asset). `EMIT_PERIOD_MS=50`
  unchanged. `(run_id, seq)` semantics unchanged (ADR-0008).
- New endpoint `GET /ring/:asset/range?run_id=&from_seq=&to_seq=` → JSON array of
  `OracleTick` for the inclusive seq span, in seq order. Errors mirror the
  single-seq endpoint: `409` run_id mismatch, `410` if `from_seq` is older than the
  oldest retained entry (evidence is incomplete → caller marks proof `failed`),
  `404` unknown asset. A bounded span (reject `to_seq - from_seq` > ring capacity)
  guards against abuse.

---

## 6. Shared touch crate + proof crates

- **`tap-trading-touch`** (new, pure, no IO): move `segment_intersects_band` +
  the path-segment evaluation out of the worker. Worker depends on it (behavior
  unchanged); verifier depends on it. Single source of truth for touch.
- **`tap-trading-proof-types`**: `ProofBlob` and nested types exactly per
  ADR-0011 §1 / prior `tick-walrus-proofs` plan (`v`, ids, `band` in oracle base
  units = price×1e9, `window`, `stake`, `multiplier_bps`, `quote_at_tap`,
  `settlement{outcome, touch_seq, touch_mid, evidence_ticks[], settled_at_ms,
  sui_tx_digest}`). `multiplier_f64_to_bps(m) = floor(m * 10_000)` defined here
  (MATH_SPEC §4.4); both the mint path and the verifier use it.
- **`tap-trading-proof-verifier`**: `verify(&ProofBlob) -> VerifyResult`
  (`Valid` | `MultiplierMismatch{claimed,recomputed}` |
  `OutcomeMismatch{claimed,recomputed}` | `InsufficientEvidence`). Recompute the
  multiplier via `tap-trading-pricing-engine` (build `Cell`/`OracleState` from the
  blob; `band.lo/hi / 1e9` back to dollar prices; `now_ms = window.t_open_ms`),
  compare within `BPS_EPSILON = 1`. Re-run **path-segment** touch over
  `evidence_ticks` via `tap-trading-touch`; `InsufficientEvidence` if the ticks
  don't span `[t_open, t_close]` (load-bearing anti-fraud). Ship a `proof-verify`
  CLI (`<file.json>` or `--blob-id --aggregator`); attempt the `wasm-bindgen`
  build (`verify_json`) — if the toolchain isn't trivially available it is a
  stated deferral (§10), since lib+CLI already prove replayability.
- **Walrus client**: behind the `ProofPublisher` trait. Primary impl shells the
  `walrus` CLI (`walrus store --epochs N <file>` → blob id; `walrus read <id>`),
  which works with the installed CLI + config and needs no public HTTP publisher
  to be up. An HTTP impl (`PUT /v1/blobs`, `GET /v1/blobs/:id`) is a documented
  alternative.

---

## 7. DB migration

`positions` (add): `settle_mode TEXT NOT NULL DEFAULT 'points'
CHECK (settle_mode IN ('points','usdc'))`, `vault_position_id TEXT`,
`vault_id TEXT`, `player_balance_id TEXT`, `owner_address TEXT`; constraint
`settle_mode='points' OR (vault_position_id, vault_id, player_balance_id,
owner_address all NOT NULL)`.

`settlements` (add): `proof_status TEXT NOT NULL DEFAULT 'pending'
CHECK (proof_status IN ('pending','published','failed'))`, `sui_tx_digest TEXT`,
`walrus_blob_id TEXT`; partial index on `proof_status IN ('pending','failed')`.
USDC settlement writes the `settlements` row but **no `points_ledger` row** (funds
live on-chain). The `outcome` CHECK already allows `W/L/V`.

The four on-chain ids are captured at mint and carried on the position row so the
worker knows which `PlayerBalance` to credit at settle time.

---

## 8. Worker dual-sink + settle→publish sequence

`loop_runner` after `evaluate_position` branches on the position's `settle_mode`:

- **points** → existing `settle.rs` Postgres path, unchanged.
- **usdc** → (1) DB canary `INSERT … ON CONFLICT (position_id) DO NOTHING
  RETURNING id`; proceed only if a row inserts. (2) `SettlerClient::settle_*`
  (sui CLI PTB) → await success → capture digest; the Move `EPositionNotOpen`
  assert is the authoritative dup guard. (3) fetch evidence via the aggregator
  range endpoint `[oracle_seq_at_tap .. touch_seq]`. (4) assemble `ProofBlob` →
  `ProofPublisher::store` → capture blob id. (5) `anchor_proof` PTB. (6)
  `UPDATE settlements SET proof_status='published', walrus_blob_id, sui_tx_digest`.

**Settlement correctness never depends on Walrus.** Step 2 commits the payout
on-chain and the DB canary row; steps 3–5 are best-effort. Any failure after
step 2 sets `proof_status='failed'`; a periodic sweep retries `pending`/`failed`
rows. A position is never left unpaid because Walrus hiccuped — proof timeliness
can lag, payout cannot. Single-leader advisory lock (plan D) prevents concurrent
submit.

---

## 9. E2E success criterion (the loop "iterate until verified" runs against)

On testnet, headless, using local test keypairs:

> A test **player** keypair `deposit`s + `mint`s a real on-chain `Position`
> (USDC or e2e test coin) → the **worker** detects touch/expiry and signs
> `settle_*` via `SettlerCap` → the payout lands in the owner's `PlayerBalance`
> (treasury balance moves by exactly `stake × multiplier_bps / 10000` on win, 0
> on loss, `stake` refunded on void) → the worker publishes the proof blob to
> Walrus and emits `ProofAnchored` → the blob is fetchable from Walrus →
> `tap-trading-proof-verifier` returns `Valid` on it. Liability fields return to
> their pre-mint values after settlement.

A second `settle_*` on the same position must abort on-chain (idempotency).
Tampering the blob's `multiplier_bps` or an evidence tick must make the verifier
return `MultiplierMismatch` / `OutcomeMismatch`.

---

## 10. Explicitly deferred (stated, not dropped — Rule 12)

- UI: mode toggle, wallet-connect, deposit UI, "Verify this tap" button.
- WASM build of the verifier (lib + CLI prove replayability this session;
  attempted, deferred if the toolchain isn't readily available).
- Hardened API USDC mint-record endpoint with on-chain mint verification at tap
  time — the e2e harness records the position row from the real on-chain mint;
  the API endpoint is a follow-up.
- Weekly merkle root of `(position_id, blob_id, outcome)` → `tick_anchor`
  (Phase 3).
- Aggregated per-band cell cap (`Table`), multisig settler, perp hedging, LP
  shares (ADR-0010 Forecast / Phase 5).
- The doc-only spec-sync edits (MATH_SPEC §4.4, ORACLE_SPEC, SYSTEM_DESIGN, PRD,
  TESTING_STRATEGY) — applied as the final step from the `tick-spec-sync` plan.

---

## 11. Risks

- **Settler signing via CLI keystore.** The worker selects the settler address in
  the sui keystore; not how a production signer should work (Phase-5 swap to
  programmatic signing behind the same trait). Documented.
- **USDC faucet availability** for the live mint. Mitigated: Move tests use a
  dummy coin; the live e2e tries Circle faucet USDC and falls back to a published
  e2e test coin instantiation of the same generic vault if the faucet is dry.
- **Touch-detection drift** between worker and verifier — eliminated by the
  shared `tap-trading-touch` crate (the whole point of §6).
