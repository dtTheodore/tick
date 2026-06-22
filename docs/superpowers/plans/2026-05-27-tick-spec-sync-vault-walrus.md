# Tick Spec Sync (Vault + Walrus Phase) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring the five authoritative Tick spec docs into line with ADR-0010 (on-chain vault) and ADR-0011 (Walrus proofs), which the vault+Walrus plans depend on but which currently contradict the specs. This is a **documentation-only** plan — no code, no tests beyond a markdown link/anchor check. After it lands, a reader of MATH_SPEC / ORACLE_SPEC / SYSTEM_DESIGN / PRD / TESTING_STRATEGY gets information consistent with the accepted ADRs, instead of stale "points-only, no vault, 500 ms ring" claims.

**Architecture:** Each task edits exactly one spec file with surgical, pre-written before/after text. No file is rewritten; we add/replace specific lines. The canonical *decisions* already live in the ADRs — these edits only make the specs point at and agree with them, and add the one rule the ADRs deferred to MATH_SPEC (the float→bps conversion). Order is independent; any task can land alone.

**Tech Stack:** Markdown only. Optional verification: `markdownlint` if the repo runs it (it does not gate today), and a grep that the new anchors exist.

**Spec:** ADR-0010 (`docs/decisions/0010-tick-onchain-vault-custody-and-settlement.md`), ADR-0011 (`docs/decisions/0011-walrus-per-tap-proof-anchoring.md`), and plan 2 (`2026-05-27-tick-walrus-proofs.md` Task 4) for the canonical `multiplier_f64_to_bps` floor rule. The plans 1/2/3 of this phase carry their own deviation notes; this plan closes the *upstream* spec debt those plans flagged.

**Spec deviations / corrections (record before writing code):**

- **Do NOT revert or alter the unrelated MATH_SPEC working-tree change.** At plan-writing time MATH_SPEC had an uncommitted +65/−19 edit aligning the doc with the *implemented* pricing-engine (`PricingError` enum, `Result` return types, `jump_adjusted_sigma`, QuantLib parity). That work is unrelated to the vault phase and must be preserved. Task 1 here is **purely additive** — it appends a new subsection and touches nothing the pricing-engine edit changed.
- **SYSTEM_DESIGN §0 note was reverted to be captured here.** The vault/Walrus §0 bullet + the §1 line-108 override were briefly applied inline, then reverted so the change goes through this plan. Task 3 re-applies them verbatim plus the service-table rows the inline edit omitted.
- **No new ADR numbers.** These edits reference ADR-0010/0011; they do not create ADR-0012+.

**Verification baseline:** before starting:

```bash
cd /Users/thangta/WorkProject/commandoss/dopamint
git status --short games/tap-trading/docs/   # note any pre-existing modified specs; preserve unrelated changes
```

After each task, confirm the edited anchor exists, e.g.:

```bash
grep -n "multiplier_f64_to_bps\|ADR-0011\|tick_vault" games/tap-trading/docs/<file>.md
```

---

## Commit map

| # | Subject | Scope |
|---|---------|-------|
| 1 | `docs(tick-math): add float→bps multiplier conversion rule` | New `MATH_SPEC §4.4` defining `multiplier_f64_to_bps` (floor) as the canonical on-chain conversion; references ADR-0010 §4 + plan 2. Additive only. |
| 2 | `docs(tick-oracle): note 120s evidence ring supersedes 500ms` | `ORACLE_SPEC §0` line + `§5.6` note that ADR-0011 §6 extends the ring to 120 s for DUSDC-mode proof evidence (500 ms remains the points-replay window). |
| 3 | `docs(tick-sysdesign): document vault + walrus parallel mode` | Re-apply §0 vault note + §1 line-108 override; add `move/tick_vault`, dual-sink worker note, and proof-publisher rows to the service table; add `§1.1 DUSDC mode & on-chain settlement` cross-referencing ADR-0010/0011. |
| 4 | `docs(tick-prd): pull vault to v1 testnet, add vault risks` | `PRD §9` note that the testnet vault + Walrus proofs are pulled into the Overflow build; `§17` add R9 (vault solvency under correlated wins) + R10 (settler-key compromise). |
| 5 | `docs(tick-testing): add vault + proof-verifier coverage` | New `TESTING_STRATEGY §10` covering Move vault tests (`tick_vault`) and the proof-verifier replay tests, with coverage targets. |

Each commit is a single-file markdown edit; no build/test gate.

---

## File map

### Modified files

| Path | Reason |
|------|--------|
| `games/tap-trading/docs/MATH_SPEC.md` | Add §4.4 bps rule (additive; preserves the unrelated pricing-engine edit). |
| `games/tap-trading/docs/ORACLE_SPEC.md` | §0 + §5.6 ring-retention supersession note. |
| `games/tap-trading/docs/SYSTEM_DESIGN.md` | §0 note, §1 line-108 override, service-table rows, new §1.1. |
| `games/tap-trading/docs/PRD.md` | §9 roadmap note, §17 R9/R10. |
| `games/tap-trading/docs/TESTING_STRATEGY.md` | New §10. |

---

## Task 1: MATH_SPEC — float→bps conversion rule

**Files:**
- Modify: `games/tap-trading/docs/MATH_SPEC.md` (insert after §4.3, currently ending ~line 304)

- [ ] **Step 1: Insert the new subsection**

After the §4.3 block (the paragraph ending "...the settlement worker reads only `position.multiplier_at_tap` × `position.stake_points` for win payouts.") and before the `---` / `## 5`, insert:

```markdown
### 4.4 Float→bps conversion for the on-chain vault

`compute_multiplier` returns an `f64`. The on-chain `tick_vault::Position`
stores the locked multiplier as `u64 multiplier_bps` (basis points;
`10000 = 1.00x`) because Move has no floats (ADR-0010 §4). The conversion
is **floor**, defined canonically as:

    multiplier_bps = floor(multiplier_f64 × 10_000)

Implemented once as `tap_trading_proof_verifier::multiplier_f64_to_bps`
(plan `2026-05-27-tick-walrus-proofs.md` Task 4). Both the API's USDC-mode
mint path (which writes `multiplier_bps` on-chain) and the proof verifier
(which recomputes it for replay) MUST use this exact function. Flooring,
not rounding: the player is never charged for a fractional bps they didn't
receive, and the verifier's equality check becomes exact (tolerance
`BPS_EPSILON = 1` covers the rare integer-bps boundary across f64
platforms). Points mode is unaffected — it keeps the `f64` multiplier in
`positions.multiplier_at_tap` and never converts to bps.
```

- [ ] **Step 2: Verify the anchor exists**

Run: `grep -n "4.4 Float→bps\|multiplier_f64_to_bps" games/tap-trading/docs/MATH_SPEC.md`
Expected: two matches (heading + function name).

- [ ] **Step 3: Confirm the unrelated edit is intact**

Run: `grep -n "PricingError\|jump_adjusted_sigma" games/tap-trading/docs/MATH_SPEC.md`
Expected: still present — Task 1 must not have disturbed the pricing-engine alignment edit.

- [ ] **Step 4: Commit**

```bash
git add games/tap-trading/docs/MATH_SPEC.md
git commit -m "docs(tick-math): add float→bps multiplier conversion rule"
```

---

## Task 2: ORACLE_SPEC — 120s evidence ring supersession

**Files:**
- Modify: `games/tap-trading/docs/ORACLE_SPEC.md:16` (the ring-buffer Locked-Decision line)
- Modify: `games/tap-trading/docs/ORACLE_SPEC.md` §5.6 (line ~208)

- [ ] **Step 1: Amend the Locked-Decisions ring line (line 16)**

Replace:

```markdown
- **Aggregator state ring buffer**: keep the last 500 ms (10 ticks per asset) addressable by `aggregator_seq` so `POST /positions` can replay multiplier at the exact tick the client was rendering. See §5.6.
```

with:

```markdown
- **Aggregator state ring buffer**: keep the last 500 ms (10 ticks per asset) addressable by `aggregator_seq` so `POST /positions` can replay multiplier at the exact tick the client was rendering. See §5.6. **DUSDC mode extends this to 120 s** (ADR-0011 §6) so the settlement worker can assemble a full `[t_open, t_close]` evidence-tick array for each Walrus proof blob; the 500 ms figure is the points-mode tap-replay window, the 120 s figure is the proof-evidence window. Same ring, longer retention; `(run_id, seq)` semantics unchanged (ADR-0008).
```

- [ ] **Step 2: Amend §5.6 (line ~208)**

Replace the bullet:

```markdown
- **State ring buffer**: the aggregator retains the last 500 ms of emitted state (= 10 ticks per asset) indexed by `seq`. The API uses this to replay the multiplier computation at the exact tick the client was rendering when the user tapped.
```

with:

```markdown
- **State ring buffer**: the aggregator retains emitted state indexed by `seq`. Points-mode tap-replay needs only the last 500 ms (≈10 ticks/asset). DUSDC mode (ADR-0011 §6) needs the full cell window for proof evidence, so retention is **120 s** (≈2400 ticks/asset at 20 Hz, ≈100 KB/asset — negligible). The API replays the tap tick from this ring; the settlement worker pulls the `[oracle_seq_at_tap .. touch_seq]` slice from it at settle time via `GET /ring/:asset/:seq`.
```

- [ ] **Step 3: Verify**

Run: `grep -n "120 s\|ADR-0011" games/tap-trading/docs/ORACLE_SPEC.md`
Expected: matches at line 16 and §5.6.

- [ ] **Step 4: Commit**

```bash
git add games/tap-trading/docs/ORACLE_SPEC.md
git commit -m "docs(tick-oracle): note 120s evidence ring supersedes 500ms"
```

---

## Task 3: SYSTEM_DESIGN — vault + Walrus parallel mode

**Files:**
- Modify: `games/tap-trading/docs/SYSTEM_DESIGN.md` §0 (after line 25), §1 service table (lines 103–104 area), §1 line-108, and a new §1.1.

- [ ] **Step 1: Re-apply the §0 Locked-Decision note**

In `## 0. Locked Decisions`, add as a new bullet:

```markdown
- **On-chain DUSDC vault + Walrus proofs** (`tick_vault` package, per-tap proof blobs) are pulled forward for the Sui Overflow 2026 build as a **parallel mode** alongside the points loop — see ADR-0010 (custody & settlement authority) and ADR-0011 (verifiable replay). This **overrides** the "No vault contract" line in §1. The points loop (plans A–E) described throughout this doc is unchanged; the vault is additive and gated on the points loop landing first.
```

- [ ] **Step 2: Add service-table rows (after the `move/tick_anchor` row, line ~104)**

```markdown
| `move/tick_vault` | Sui Move | **Hackathon build (ADR-0010).** Generic `GameVault<Quote>` custodying testnet Circle USDC; `mint`/`settle_*` gated by `SettlerCap`; on-chain exposure caps. Parallel to points mode. |
| `backend/settlement-worker` (dual-sink) | Rust + tokio | **Extended (ADR-0010 §7).** Points → Postgres (unchanged); USDC → `settle_*` PTB + Walrus proof publish. Shared touch logic across both sinks. |
| `backend/proof-verifier` + `backend/walrus-client` | Rust (+ WASM) | **Hackathon build (ADR-0011).** Pure replay verifier (WASM "Verify this tap") + Walrus `PUT`/`GET` client. |
```

- [ ] **Step 3: Re-apply the §1 line-108 override**

Replace:

```markdown
- **No vault contract.** Points only; no funds to manage.
```

with:

```markdown
- **No vault contract** *in the points loop.* Points only; no funds to manage. (**Overridden for the hackathon build:** ADR-0010 adds a parallel on-chain DUSDC `tick_vault` mode and ADR-0011 adds per-tap Walrus proofs, running alongside — not replacing — the points loop. The points path described in this doc is unchanged. See §1.1.)
```

- [ ] **Step 4: Add §1.1 after the "What's NOT a separate service (v1)" block**

```markdown
### 1.1 DUSDC mode & on-chain settlement (hackathon build)

Parallel to the points loop, not a replacement. Spec: ADR-0010, ADR-0011.

- **Custody:** `tick_vault::GameVault<USDC>` (testnet Circle USDC,
  `0xa1ec…::usdc::USDC`) holds the treasury; players hold idle funds in
  per-player `PlayerBalance` objects. Mint debits the player balance into
  the vault and records an on-chain `Position` with the locked
  `multiplier_bps`.
- **Settlement authority:** the settlement worker holds a `SettlerCap`
  and signs `settle_win`/`settle_loss`/`settle_void` PTBs. Players cannot
  self-settle. Touch detection is the *same* off-chain logic as points
  mode (single source of truth).
- **Verifiability (Hyperliquid pattern):** every USDC settlement publishes
  a Walrus proof blob (oracle path + locked multiplier + outcome),
  anchored on Sui via a `ProofAnchored` event. Anyone can replay it with
  `tap-trading-proof-verifier`. Walrus availability never blocks payout.
- **Separation:** points balances (off-chain) and USDC balances
  (on-chain) are independent; no conversion in v1 (ADR-0010 §8).
```

- [ ] **Step 5: Verify**

Run: `grep -n "tick_vault\|§1.1\|ProofAnchored\|SettlerCap" games/tap-trading/docs/SYSTEM_DESIGN.md`
Expected: matches in §0, service table, and §1.1.

- [ ] **Step 6: Commit**

```bash
git add games/tap-trading/docs/SYSTEM_DESIGN.md
git commit -m "docs(tick-sysdesign): document vault + walrus parallel mode"
```

---

## Task 4: PRD — roadmap + risks

**Files:**
- Modify: `games/tap-trading/docs/PRD.md` §9 (line ~231, Phase 1 block) and §17 risks table (line ~880, after R8).

- [ ] **Step 1: Add a note under Phase 1 (§9)**

After the `### Phase 1 — MVP` paragraph, add:

```markdown
> **Overflow build addendum (ADR-0010/0011):** a testnet on-chain mode is
> pulled forward from Phase 5 into the hackathon build — a Circle-USDC
> `tick_vault` with touch-settled payouts and per-tap Walrus proofs,
> running **parallel** to the points loop. This does not change the
> points-first funnel; it adds an on-chain, real-(testnet)-money mode and
> the verifiability story. Primary Overflow track: **DeFi & Payments**
> (programmable money primitive); Walrus integration is a value-add.
```

- [ ] **Step 2: Add R9 and R10 to the §17 risks table (after R8)**

```markdown
| R9 | Vault insolvency under correlated player wins (BTC pump puts most bullish cells in-the-money at once; streamer pile-on on one cell) | High (mainnet); nil (testnet, faucet USDC) | On-chain exposure caps in `tick_vault` (per-cell, directional imbalance, treasury buffer — ADR-0010 §5); external perp-hedging via Sui DeepBook in Phase 5; over-fund treasury on testnet. Hyperliquid-pattern controls. |
| R10 | `SettlerCap` keypair compromise → attacker authorizes fraudulent payouts | Medium | Testnet value is nil; mainnet hardens with a multisig settler + on-chain payout-rate limits (ADR-0010 Forecast). Walrus proofs make any fraudulent settlement publicly detectable after the fact. |
```

- [ ] **Step 3: Verify**

Run: `grep -n "Overflow build addendum\|R9\|R10" games/tap-trading/docs/PRD.md`
Expected: matches.

- [ ] **Step 4: Commit**

```bash
git add games/tap-trading/docs/PRD.md
git commit -m "docs(tick-prd): pull vault to v1 testnet, add vault risks"
```

---

## Task 5: TESTING_STRATEGY — vault + proof-verifier coverage

**Files:**
- Modify: `games/tap-trading/docs/TESTING_STRATEGY.md` (append `## 10` before any trailing appendix, after `## 9. Load & performance`).

- [ ] **Step 1: Append the new section**

```markdown
## 10. On-chain vault & proofs — `move/tick_vault`, `backend/proof-verifier`

Spec: ADR-0010, ADR-0011, plans `tick-onchain-vault`, `tick-walrus-proofs`,
`tick-vault-worker-integration`.

### 10.1 Move vault tests (`sui move test`)

- **Cap enforcement:** one test per abort — `mint` rejects above
  `max_multiplier_bps`, inverted band, per-cell cap, directional cap,
  treasury buffer.
- **Settlement authority:** `settle_*` aborts without a matching
  `SettlerCap` (`ECapVaultMismatch`) and on a non-OPEN position
  (`EPositionNotOpen`).
- **Payout exactness:** `settle_win` pays `stake × multiplier_bps / 10000`
  exactly; liability decrements; second settle aborts.
- **Solvency scenario:** mint to the directional cap, mass-`settle_win`,
  assert treasury never goes negative (ADR-0010 §5 intent).

### 10.2 Proof verifier tests (`cargo test`, pure)

- **Golden Valid:** the committed `proof_won.json` verifies `Valid`.
- **Multiplier mismatch:** tampered `multiplier_bps` → `MultiplierMismatch`.
  Reuses `tap-trading-pricing-engine` (no reimplementation).
- **Outcome mismatch:** flip an evidence tick so the band isn't touched →
  `OutcomeMismatch`.
- **Insufficient evidence:** evidence ticks that don't span
  `[t_open, t_close]` → `InsufficientEvidence`.
- **bps conversion:** `multiplier_f64_to_bps` floors (MATH_SPEC §4.4).
- **WASM parity:** `verify_json` returns the same result compiled to
  `wasm32` as native.

### 10.3 Worker integration (`cargo test`, feature-gated)

- **Dual-sink routing:** a `usdc` position routes to the Sui path; a
  `points` position routes to Postgres (unchanged).
- **No ledger on USDC:** a USDC settle writes no `points_ledger` row.
- **Proof retry:** a failed Walrus publish flips `proof_status` failed →
  published on the retry sweep.
- **On-chain end-to-end** (`TICK_IT_ONCHAIN=1`): deposit→mint→settle vs a
  deployed testnet vault; assert payout, `ProofAnchored` event, blob
  fetchable. Skipped in CI without testnet creds.

### 10.4 Coverage target

Move package: every `public` entry has at least one happy-path + one
abort test. Verifier: 100% of `VerifyResult` variants exercised.
```

- [ ] **Step 2: Verify**

Run: `grep -n "## 10\|proof-verifier\|tick_vault" games/tap-trading/docs/TESTING_STRATEGY.md`
Expected: matches.

- [ ] **Step 3: Commit**

```bash
git add games/tap-trading/docs/TESTING_STRATEGY.md
git commit -m "docs(tick-testing): add vault + proof-verifier coverage"
```

---

## Self-review notes

- **Coverage of the flagged gaps:** MATH_SPEC float→bps → Task 1; ORACLE_SPEC 500ms→120s → Task 2; SYSTEM_DESIGN vault/Walrus section → Task 3; PRD roadmap+risks → Task 4; TESTING_STRATEGY vault+verifier → Task 5. All five "must-update" items closed.
- **Non-destructive:** Task 1 is additive and explicitly preserves the unrelated pricing-engine MATH_SPEC edit. No task reverts working-tree work.
- **No dangling refs:** SYSTEM_DESIGN §0 now points at §1.1 (created in the same task), not a non-existent §0 self-reference.

---

**Plan complete and saved to `docs/superpowers/plans/2026-05-27-tick-spec-sync-vault-walrus.md`.** Doc-only; execute any time (independent of the code plans). Two execution options:

1. **Subagent-Driven (recommended)** — one subagent per doc, quick review.
2. **Inline Execution** — apply all five edits in one session.

**Which approach?**
