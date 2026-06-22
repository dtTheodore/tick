# Tick Walrus Proofs — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the off-chain verifiable-proof layer for Tick DUSDC-mode settlements: a `tap-trading-proof-types` crate (the canonical proof-blob schema + assembler), a `tap-trading-proof-verifier` crate (a pure, no-IO replay function that reuses `tap-trading-pricing-engine` to recompute the multiplier and re-run touch detection, compiled to a CLI and to WASM for the "Verify this tap" button), and a `tap-trading-walrus-client` crate (HTTP `PUT`/`GET` against a Walrus publisher/aggregator). After this plan lands, any party can fetch a Walrus blob by id and independently confirm a payout was correct — the data-availability half of ADR-0011.

**Architecture:** Three new library crates in the Tick sub-workspace (`games/tap-trading/backend/`). `proof-types` owns the serde structs and the `ProofBlob::assemble` constructor — no IO, shared by the verifier here and the worker in plan 3. `proof-verifier` depends on `proof-types` + `pricing-engine`; it is pure so it compiles to `wasm32-unknown-unknown` unchanged. `walrus-client` is the only crate with IO (`reqwest`); it does `PUT $PUBLISHER/v1/blobs` → blob id and `GET $AGGREGATOR/v1/blobs/:id` → bytes. The blob *write at settlement time* lives in the worker (plan 3); this plan builds the pieces the worker calls plus the standalone verifier tooling.

**Tech Stack:** Rust 2021. `serde` + `serde_json` (proof schema). `tap-trading-pricing-engine` (path dep — the verifier reuses `compute_multiplier`; **never** reimplements the formula). `reqwest` 0.12 (`rustls-tls`, `json`) for the Walrus client. `clap` 4 for the verifier CLI. `wasm-bindgen` 0.2 + `wasm-pack` for the WASM target. Dev: `wiremock` 0.6 for HTTP-client tests (no live Walrus needed in CI).

**Spec:** ADR-0011 (`docs/decisions/0011-walrus-per-tap-proof-anchoring.md`) — §1 (blob schema, the `ProofBlob` struct mirrors it field-for-field), §5 (`VerifyResult` enum + the two replay checks), §4 (publish sequence — the worker's job, plan 3, but the assembler lives here). ADR-0010 §4 (the on-chain `Position` fields the blob mirrors). `MATH_SPEC §4.3` (lock-at-tap: the verifier recomputes from `quote_at_tap`, the *inputs as captured at tap*, not live state). `games/tap-trading/backend/pricing-engine/src/{types,multiplier}.rs` — the exact `Cell`/`OracleState`/`PricingConfig`/`compute_multiplier` API the verifier consumes.

**Spec deviations / corrections (record before writing code):**

- **Float→bps conversion is defined HERE as the canonical rule** (the MATH_SPEC gap from the phase review). `compute_multiplier` returns `f64`; the on-chain `Position.multiplier_bps` is `u64`. The rule is **floor**: `bps = (m * 10_000.0).floor() as u64`. Defined as `proof_verifier::multiplier_f64_to_bps`. The worker's mint path (plan 3) and the vault's quote (any future on-chain quote) MUST use this same function. Flooring (not rounding) means the player is never charged for a fractional bps they didn't get; it also makes the verifier's equality check exact. A follow-up edit adds this rule to `MATH_SPEC §4.3`.
- **Verifier tolerance is zero on bps, not on f64.** We compare the *recomputed bps* to the *claimed bps* exactly. The f64 intermediate can differ in the last ULP across platforms (WASM vs x86), but `floor(m * 10_000)` is stable for any `m` not within 1e-4 of an integer-bps boundary; for the rare boundary case we allow `±1 bps` slack (`BPS_EPSILON = 1`). Documented in the verifier.
- **`evidence_ticks` completeness check.** The verifier returns `InsufficientEvidence` if the first tick's `ts_ms > window.t_open_ms` or the last tick's `ts_ms < window.t_close_ms` (the array must span the window) — otherwise a settler could omit the tick that proves a touch. This is a load-bearing anti-fraud check, not a nicety.
- **`asset` maps to `AssetSymbol` via an explicit `From`**, not `serde` rename reliance. The blob stores `"BTC"`; `proof-types` provides `fn asset_symbol(&self) -> AssetSymbol` so the verifier feeds the pricing engine the right enum.

**Verification baseline:** before starting:

```bash
cd games/tap-trading/backend && cargo check && cargo test    # plans A-E green; pricing-engine present
rustup target list --installed | grep wasm32-unknown-unknown || rustup target add wasm32-unknown-unknown
```

After every commit, run from `games/tap-trading/backend/`:

```bash
cargo check && cargo test && cargo clippy --all-targets -- -D warnings
```

---

## Commit map

| # | Subject | Scope |
|---|---------|-------|
| 1 | `chore(proof): scaffold proof-types crate` | New `proof-types` member; `serde`/`serde_json` workspace deps. Empty `lib.rs`. |
| 2 | `feat(proof): define ProofBlob schema` | `ProofBlob`, `Band`, `Window`, `QuoteAtTap`, `Settlement`, `EvidenceTick`, `Outcome` structs mirroring ADR-0011 §1. serde round-trip test against a committed `fixtures/proof_won.json`. |
| 3 | `feat(proof): add ProofBlob::assemble constructor` | `assemble(position, quote, outcome, ticks, digest) -> ProofBlob` + `asset_symbol()` helper. Unit test builds a blob and asserts fields. |
| 4 | `chore(proof): scaffold proof-verifier crate` | New `proof-verifier` member; path deps on `proof-types` + `tap-trading-pricing-engine`. `multiplier_f64_to_bps` + `BPS_EPSILON`. Unit test for the conversion rule. |
| 5 | `feat(proof): verify multiplier recompute` | `verify` recomputes `multiplier_bps` from `quote_at_tap` via `compute_multiplier`; returns `MultiplierMismatch` on disagreement. Test: golden Valid; tampered bps → mismatch. |
| 6 | `feat(proof): verify touch detection over evidence` | Re-run first-touch over `evidence_ticks`; compare to `outcome`. `InsufficientEvidence` if ticks don't span window. Table-driven tests: WON/LOST/VOID + gap. |
| 7 | `feat(proof): verifier CLI` | `proof-verify <file.json>` and `proof-verify --blob-id <id> --aggregator <url>` (fetches via walrus-client). Prints `VerifyResult`. Integration test invokes the bin on a fixture. |
| 8 | `chore(walrus): scaffold walrus-client crate` | New `walrus-client` member; `reqwest` workspace dep. `WalrusClient::new(publisher, aggregator)`. |
| 9 | `feat(walrus): store_blob via PUT` | `store_blob(bytes) -> Result<String>` parses `newlyCreated.blobObject.blobId` OR `alreadyCertified.blobId`. `wiremock` test for both response shapes. |
| 10 | `feat(walrus): read_blob via GET` | `read_blob(id) -> Result<Vec<u8>>` against `GET /v1/blobs/:id`. `wiremock` test. |
| 11 | `feat(proof): wasm verify entrypoint` | `#[wasm_bindgen] pub fn verify_json(s: &str) -> String` returning JSON `VerifyResult`. `wasm-pack build --target web` succeeds; documented in crate README. |
| 12 | `test(proof): end-to-end assemble→serialize→verify` | Assemble a WON blob, serialize, verify Valid; flip a tick to break the touch, verify OutcomeMismatch; bump bps, verify MultiplierMismatch. |

Each commit must independently pass `cargo check && cargo test && cargo clippy --all-targets -- -D warnings`.

---

## File map

### Created files

| Path | Responsibility |
|------|----------------|
| `games/tap-trading/backend/proof-types/Cargo.toml` | Crate metadata; serde. |
| `games/tap-trading/backend/proof-types/src/lib.rs` | `ProofBlob` + nested structs; `assemble`; `asset_symbol`. No IO. |
| `games/tap-trading/backend/proof-types/tests/fixtures/proof_won.json` | Golden WON proof for round-trip + verifier tests. |
| `games/tap-trading/backend/proof-verifier/Cargo.toml` | Deps: proof-types, pricing-engine, (wasm) wasm-bindgen. |
| `games/tap-trading/backend/proof-verifier/src/lib.rs` | `verify`, `VerifyResult`, `multiplier_f64_to_bps`, `BPS_EPSILON`, touch re-detection, `verify_json` (wasm). |
| `games/tap-trading/backend/proof-verifier/src/bin/proof-verify.rs` | CLI: verify a file or a fetched blob. |
| `games/tap-trading/backend/walrus-client/Cargo.toml` | Deps: reqwest, serde_json; dev: wiremock. |
| `games/tap-trading/backend/walrus-client/src/lib.rs` | `WalrusClient` with `store_blob`, `read_blob`. |

### Modified files

| Path | Reason |
|------|--------|
| `games/tap-trading/backend/Cargo.toml` | Add the 3 members; add `reqwest`, `clap`, `wasm-bindgen`, `wiremock` to `[workspace.dependencies]`. |

---

## Pre-flight (one-time, not a commit)

- [ ] **Step P1: Tick sub-workspace baseline green**

```bash
cd games/tap-trading/backend && cargo check && cargo test && cargo clippy --all-targets -- -D warnings
```

Expected: green. Plans A–E on HEAD.

- [ ] **Step P2: WASM target installed**

```bash
rustup target add wasm32-unknown-unknown && command -v wasm-pack || cargo install wasm-pack
```

Expected: target present, `wasm-pack` available (Task 11 only).

---

## Task 1: Scaffold proof-types crate

**Files:**
- Create: `games/tap-trading/backend/proof-types/Cargo.toml`
- Create: `games/tap-trading/backend/proof-types/src/lib.rs`
- Modify: `games/tap-trading/backend/Cargo.toml`

- [ ] **Step 1: Add the member to the workspace**

In `games/tap-trading/backend/Cargo.toml`, add `"proof-types"` to `members`.

- [ ] **Step 2: Create `proof-types/Cargo.toml`**

```toml
[package]
name = "tap-trading-proof-types"
version = "0.1.0"
edition.workspace = true

[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }
tap-trading-pricing-engine = { path = "../pricing-engine" }
```

- [ ] **Step 3: Empty lib so it compiles**

`proof-types/src/lib.rs`:

```rust
//! Canonical Tick proof-blob schema. Spec: ADR-0011 §1.
// structs land in Task 2
```

- [ ] **Step 4: Build**

Run: `cargo check -p tap-trading-proof-types`
Expected: success.

- [ ] **Step 5: Commit**

```bash
git add games/tap-trading/backend/proof-types games/tap-trading/backend/Cargo.toml
git commit -m "chore(proof): scaffold proof-types crate"
```

---

## Task 2: Define the ProofBlob schema

**Files:**
- Modify: `games/tap-trading/backend/proof-types/src/lib.rs`
- Create: `games/tap-trading/backend/proof-types/tests/fixtures/proof_won.json`
- Create: `games/tap-trading/backend/proof-types/tests/round_trip.rs`

- [ ] **Step 1: Write the failing test**

`proof-types/tests/round_trip.rs`:

```rust
use tap_trading_proof_types::ProofBlob;

#[test]
fn won_fixture_round_trips() {
    let raw = include_str!("fixtures/proof_won.json");
    let blob: ProofBlob = serde_json::from_str(raw).expect("parse");
    assert_eq!(blob.v, 1);
    assert_eq!(blob.asset, "BTC");
    assert_eq!(blob.settlement.outcome, tap_trading_proof_types::Outcome::Won);
    let reser = serde_json::to_string(&blob).expect("serialize");
    let reparsed: ProofBlob = serde_json::from_str(&reser).expect("reparse");
    assert_eq!(blob, reparsed);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p tap-trading-proof-types won_fixture_round_trips`
Expected: FAIL — `ProofBlob` undefined.

- [ ] **Step 3: Implement the structs (ADR-0011 §1)**

`proof-types/src/lib.rs`:

```rust
//! Canonical Tick proof-blob schema. Spec: ADR-0011 §1.
use serde::{Deserialize, Serialize};
use tap_trading_pricing_engine::AssetSymbol;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProofBlob {
    pub v: u32,
    pub position_id: String,
    pub vault_id: String,
    pub owner: String,
    pub asset: String,            // "BTC" | "ETH" | "SOL"
    pub band: Band,
    pub window: Window,
    pub stake: u64,
    pub multiplier_bps: u64,
    pub quote_at_tap: QuoteAtTap,
    pub settlement: Settlement,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Band { pub lo: u64, pub hi: u64 } // oracle base units (1e9 fixed-point)

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Window { pub t_open_ms: u64, pub t_close_ms: u64 }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuoteAtTap {
    pub oracle_run_id: u64,
    pub oracle_seq: u64,
    pub mid: f64,
    pub vol_annualized: f64,
    pub formula_version: String,  // e.g. "hui_bgk_v1"
    pub floor_curve: String,      // e.g. "1.30+0.01*tau"
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Outcome { Won, Lost, Void }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settlement {
    pub outcome: Outcome,
    pub touch_seq: Option<u64>,
    pub touch_mid: Option<f64>,
    pub evidence_ticks: Vec<EvidenceTick>,
    pub settled_at_ms: u64,
    pub sui_tx_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EvidenceTick { pub seq: u64, pub ts_ms: u64, pub mid: f64 }

impl ProofBlob {
    /// The asset as the pricing-engine enum. Spec: ADR-0011 deviation note.
    pub fn asset_symbol(&self) -> Option<AssetSymbol> {
        match self.asset.as_str() {
            "BTC" => Some(AssetSymbol::Btc),
            "ETH" => Some(AssetSymbol::Eth),
            "SOL" => Some(AssetSymbol::Sol),
            _ => None,
        }
    }
}
```

- [ ] **Step 4: Create the fixture**

`proof-types/tests/fixtures/proof_won.json` (band touched at seq 48999):

```json
{
  "v": 1,
  "position_id": "0xpos",
  "vault_id": "0xvault",
  "owner": "0xowner",
  "asset": "BTC",
  "band": { "lo": 75832000000000, "hi": 75842000000000 },
  "window": { "t_open_ms": 1779564600000, "t_close_ms": 1779564660000 },
  "stake": 100000,
  "multiplier_bps": 19580,
  "quote_at_tap": {
    "oracle_run_id": 173000000,
    "oracle_seq": 48213,
    "mid": 75837.06,
    "vol_annualized": 0.61,
    "formula_version": "hui_bgk_v1",
    "floor_curve": "1.30+0.01*tau"
  },
  "settlement": {
    "outcome": "WON",
    "touch_seq": 48999,
    "touch_mid": 75838.40,
    "evidence_ticks": [
      { "seq": 48213, "ts_ms": 1779564600000, "mid": 75837.06 },
      { "seq": 48999, "ts_ms": 1779564640000, "mid": 75838.40 },
      { "seq": 49200, "ts_ms": 1779564660000, "mid": 75839.10 }
    ],
    "settled_at_ms": 1779564660500,
    "sui_tx_digest": "Habc"
  }
}
```

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test -p tap-trading-proof-types won_fixture_round_trips`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add games/tap-trading/backend/proof-types
git commit -m "feat(proof): define ProofBlob schema"
```

---

## Task 3: ProofBlob::assemble constructor

**Files:**
- Modify: `games/tap-trading/backend/proof-types/src/lib.rs`
- Modify: `games/tap-trading/backend/proof-types/tests/round_trip.rs`

- [ ] **Step 1: Write the failing test**

Add to `round_trip.rs`:

```rust
use tap_trading_proof_types::{assemble, AssembleInput, Outcome, EvidenceTick, Band, Window};

#[test]
fn assemble_builds_won_blob() {
    let blob = assemble(AssembleInput {
        position_id: "0xpos".into(), vault_id: "0xvault".into(), owner: "0xowner".into(),
        asset: "BTC".into(),
        band: Band { lo: 75832000000000, hi: 75842000000000 },
        window: Window { t_open_ms: 1779564600000, t_close_ms: 1779564660000 },
        stake: 100000, multiplier_bps: 19580,
        oracle_run_id: 173000000, oracle_seq: 48213, mid: 75837.06, vol_annualized: 0.61,
        outcome: Outcome::Won, touch_seq: Some(48999), touch_mid: Some(75838.40),
        evidence_ticks: vec![EvidenceTick { seq: 48999, ts_ms: 1779564640000, mid: 75838.40 }],
        settled_at_ms: 1779564660500, sui_tx_digest: "Habc".into(),
    });
    assert_eq!(blob.v, 1);
    assert_eq!(blob.quote_at_tap.formula_version, "hui_bgk_v1");
    assert_eq!(blob.settlement.outcome, Outcome::Won);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p tap-trading-proof-types assemble_builds_won_blob`
Expected: FAIL — `assemble`, `AssembleInput` undefined.

- [ ] **Step 3: Implement**

Add to `proof-types/src/lib.rs`:

```rust
pub const FORMULA_VERSION: &str = "hui_bgk_v1";
pub const FLOOR_CURVE: &str = "1.30+0.01*tau";

pub struct AssembleInput {
    pub position_id: String, pub vault_id: String, pub owner: String, pub asset: String,
    pub band: Band, pub window: Window, pub stake: u64, pub multiplier_bps: u64,
    pub oracle_run_id: u64, pub oracle_seq: u64, pub mid: f64, pub vol_annualized: f64,
    pub outcome: Outcome, pub touch_seq: Option<u64>, pub touch_mid: Option<f64>,
    pub evidence_ticks: Vec<EvidenceTick>, pub settled_at_ms: u64, pub sui_tx_digest: String,
}

pub fn assemble(i: AssembleInput) -> ProofBlob {
    ProofBlob {
        v: 1,
        position_id: i.position_id, vault_id: i.vault_id, owner: i.owner, asset: i.asset,
        band: i.band, window: i.window, stake: i.stake, multiplier_bps: i.multiplier_bps,
        quote_at_tap: QuoteAtTap {
            oracle_run_id: i.oracle_run_id, oracle_seq: i.oracle_seq,
            mid: i.mid, vol_annualized: i.vol_annualized,
            formula_version: FORMULA_VERSION.into(), floor_curve: FLOOR_CURVE.into(),
        },
        settlement: Settlement {
            outcome: i.outcome, touch_seq: i.touch_seq, touch_mid: i.touch_mid,
            evidence_ticks: i.evidence_ticks, settled_at_ms: i.settled_at_ms,
            sui_tx_digest: i.sui_tx_digest,
        },
    }
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p tap-trading-proof-types assemble_builds_won_blob`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add games/tap-trading/backend/proof-types
git commit -m "feat(proof): add ProofBlob::assemble constructor"
```

---

## Task 4: Scaffold proof-verifier + the bps conversion rule

**Files:**
- Create: `games/tap-trading/backend/proof-verifier/Cargo.toml`
- Create: `games/tap-trading/backend/proof-verifier/src/lib.rs`
- Modify: `games/tap-trading/backend/Cargo.toml`

- [ ] **Step 1: Write the failing test**

`proof-verifier/src/lib.rs` (test inline):

```rust
#[cfg(test)]
mod conv_tests {
    use super::multiplier_f64_to_bps;
    #[test]
    fn floors_to_bps() {
        assert_eq!(multiplier_f64_to_bps(1.9580), 19580);
        assert_eq!(multiplier_f64_to_bps(1.95809), 19580); // floored, not rounded
        assert_eq!(multiplier_f64_to_bps(1.0), 10000);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p tap-trading-proof-verifier floors_to_bps`
Expected: FAIL — crate/member/function missing.

- [ ] **Step 3: Implement crate + conversion**

Add `"proof-verifier"` to workspace `members`. Create `proof-verifier/Cargo.toml`:

```toml
[package]
name = "tap-trading-proof-verifier"
version = "0.1.0"
edition.workspace = true

[lib]
crate-type = ["cdylib", "rlib"]   # cdylib for wasm

[dependencies]
tap-trading-proof-types = { path = "../proof-types" }
tap-trading-pricing-engine = { path = "../pricing-engine" }
serde = { workspace = true }
serde_json = { workspace = true }

[target.'cfg(target_arch = "wasm32")'.dependencies]
wasm-bindgen = { workspace = true }
```

`proof-verifier/src/lib.rs`:

```rust
//! Pure replay verifier for Tick proof blobs. Spec: ADR-0011 §5.
/// Canonical float→bps rule (ADR-0011 deviation note). Floor, never round.
pub fn multiplier_f64_to_bps(m: f64) -> u64 {
    (m * 10_000.0).floor() as u64
}
pub const BPS_EPSILON: u64 = 1;
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p tap-trading-proof-verifier floors_to_bps`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add games/tap-trading/backend/proof-verifier games/tap-trading/backend/Cargo.toml
git commit -m "chore(proof): scaffold proof-verifier crate"
```

---

## Tasks 5–12

> Tasks 5–12 follow the same TDD rhythm and are fully enumerated in the Commit map. The two replay checks (Tasks 5–6) are the core:
> - **Multiplier recompute (Task 5):** build a `pricing_engine::Cell` + `OracleState` from `blob.band`/`window`/`quote_at_tap` (note: `Cell.strike_lo/hi` are `f64` dollar prices — divide `band.lo/hi` by 1e9; `oracle.spot = quote.mid`, `sigma = quote.vol_annualized`), call `compute_multiplier(.., now_ms = window.t_open_ms)`, apply `multiplier_f64_to_bps`, compare to `blob.multiplier_bps` within `BPS_EPSILON` → `MultiplierMismatch` on fail.
> - **Touch re-detection (Task 6):** assert `evidence_ticks` spans `[t_open, t_close]` (else `InsufficientEvidence`); scan for the first tick whose `mid*1e9 ∈ [band.lo, band.hi]`; `Won` if found else `Lost` (or `Void` if the array is empty/flagged); compare to `blob.settlement.outcome` → `OutcomeMismatch`.
> - **Tasks 7–10** are the CLI and the `walrus-client` (PUT parses `newlyCreated.blobObject.blobId` else `alreadyCertified.blobId`; GET is `/v1/blobs/:id`; both `wiremock`-tested). **Task 11** is the `#[wasm_bindgen] verify_json`. **Task 12** is the end-to-end tamper test.

> **Stop after Task 6 and request review** — the two verify checks are the security-critical core; everything after is plumbing.

---

## Self-review notes

- **Spec coverage:** ADR-0011 §1→Task 2; §5 `VerifyResult`+checks→Tasks 5–6; §4 assembler→Task 3; WASM "Verify this tap"→Task 11; Walrus HTTP→Tasks 9–10. The MATH_SPEC float→bps gap is closed in Task 4 (canonical `multiplier_f64_to_bps`).
- **Type consistency:** `multiplier_f64_to_bps`, `BPS_EPSILON`, `Outcome`, `EvidenceTick`, `ProofBlob` referenced identically across tasks. `Cell.strike_lo/hi` are `f64` dollars — the band's 1e9 base units are divided down in Task 5 (called out so the engineer doesn't feed base units to the engine).
- **No reimplementation:** the verifier calls `compute_multiplier` from the shipped pricing-engine; it never re-derives Hui/BGK. Formula changes require a `formula_version` bump so old blobs verify against their version.

---

**Plan complete and saved to `docs/superpowers/plans/2026-05-27-tick-walrus-proofs.md`.** Plan 2 of 3 in the vault+Walrus phase. Execute after plan 1 (vault) since Task 12's end-to-end assumes the `Position` field shapes from ADR-0010. Next: plan 3 wires this into the settlement worker.
