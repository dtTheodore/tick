# ADR-0011 — Walrus per-tap proof anchoring & verifiable replay

**Date:** 2026-05-27
**Status:** Accepted
**Workstream:** Tick (tap-trading)
**Supersedes:** —
**Superseded by:** —

## Context

ADR-0010 makes the off-chain settler the authority that moves money out
of the `tick_vault`. That is "credibly centralized": fast, but the
player must trust that the settler honestly detected (or didn't detect)
a touch. Pacifica and Euphoria operate exactly this way and offer **no
way to audit a single settlement** — you cannot prove the price did or
did not enter your band.

Tick's PRD product principle #1 is *"Real math, not random rewards.
Multipliers come from a published touch-probability formula. Users can
verify the math."* This ADR makes that literal: every settled position
publishes a self-contained proof blob to Walrus, anchored on Sui, that
anyone can fetch and replay to independently confirm the multiplier and
the outcome.

This is the data-availability half of the Hyperliquid pattern. The
settler is the sequencer (fast, off-chain, ordered); Walrus is the DA
layer (every input + output published, publicly replayable); the Sui
`tick_anchor` merkle root is the state commitment. Validity comes from
replayability, not from on-chain execution.

This also fits the hackathon's Walrus track framing as a value-add even
though we submit primarily to DeFi & Payments: Walrus is used as a
*verifiable data layer*, which is the track's exact remit.

Scope note: in v1 we publish proofs for **DUSDC-mode** settlements
(real money on the line → verifiability matters most, and volume is
bounded). Points-mode settlements are not blobbed in v1 — they're
free-to-play and the weekly merkle snapshot (Phase 3) already covers
the points-history integrity story. Forecast §1 covers extending to
points mode if volume economics allow.

## Decision

### 1. One Walrus blob per settled DUSDC position

Written at settlement time, after the on-chain `settle_*` PTB confirms.
The blob is the complete evidence needed to replay one tap:

```jsonc
{
  "v": 1,                              // proof schema version
  "position_id": "0x…",               // Sui object id of the Position
  "vault_id": "0x…",
  "owner": "0x…",
  "asset": "BTC",
  "band": { "lo": 75832000000000, "hi": 75842000000000 },  // oracle base units
  "window": { "t_open_ms": 1779564600000, "t_close_ms": 1779564660000 },
  "stake": 100000,                     // Quote base units
  "multiplier_bps": 19580,            // locked at tap
  "quote_at_tap": {                    // what the pricing engine computed at tap
    "oracle_run_id": 173…,
    "oracle_seq": 48213,
    "mid": 75837.06,
    "vol_annualized": 0.61,
    "formula_version": "hui_bgk_v1",
    "floor_curve": "1.30+0.01*tau"
  },
  "settlement": {
    "outcome": "WON",                 // WON | LOST | VOID
    "touch_seq": 48999,               // the oracle seq where the band was first touched (null if LOST/VOID)
    "touch_mid": 75838.40,
    "evidence_ticks": [               // every oracle tick over [t_open, t_close]
      { "seq": 48213, "ts_ms": …, "mid": 75837.06 },
      …
      { "seq": 48999, "ts_ms": …, "mid": 75838.40 }
    ],
    "settled_at_ms": …,
    "sui_tx_digest": "…"              // the settle_* PTB digest (filled after confirm)
  }
}
```

`evidence_ticks` is the load-bearing field: it is the full oracle path
over the window, so a verifier can re-run touch detection
(`first seq where strike_lo ≤ mid ≤ strike_hi`) and confirm the
outcome. The ticks are the same `OracleTick` values from ADR-0008,
fetched from the aggregator's ring buffer at settlement time.

### 2. Blob id is recorded on-chain via an event, not in the Position

The settler, after publishing to Walrus, emits a Move event from
`tick_vault`:

```move
public struct ProofAnchored has copy, drop, store {
    position_id: ID,
    walrus_blob_id: vector<u8>,        // Walrus blob id bytes
    outcome: u8,
    settled_at_ms: u64,
}
```

We anchor via **event, not a field on `Position`**, because the
`settle_*` PTB and the Walrus write are two operations: the PTB settles
+ pays, then the blob is written, then a cheap `anchor_proof(cap,
position_id, blob_id)` call emits the event. Putting the blob id on the
`Position` would force the blob write *before* settlement (we don't have
the `sui_tx_digest` yet) or a mutate-after-settle (extra write to a
soon-dead object). The event is indexable, immutable, and decoupled.

### 3. Weekly merkle root of blob ids → `tick_anchor` (Phase 3 tie-in)

The existing Phase-3 `anchor-publisher` cron (`SYSTEM_DESIGN §1`) is
extended: in addition to the accounts merkle root, it publishes a
weekly merkle root of `(position_id, walrus_blob_id, outcome)` tuples
to the `tick_anchor` Move package. This gives a compact on-chain
commitment to the entire week's proof set — a verifier can prove a
specific blob id was part of the committed set without trusting the
indexer. v1 ships the blob-write + event; the weekly root is Phase 3,
unchanged from the existing schedule.

### 4. A new `walrus-publisher` responsibility, co-located in the worker

The blob write is done by the settlement worker itself (not a separate
service) to keep the settle→publish path atomic-ish and single-leader.
Sequence per DUSDC settlement:

1. Build + submit `settle_*` PTB → await confirm → capture digest.
2. Assemble proof JSON (ticks pulled from the aggregator ring buffer
   via ADR-0008 `GET /ring/:asset/:seq`).
3. `PUT` the blob to a Walrus publisher (`publisher.walrus-testnet`,
   HTTP API per Walrus docs), capture `blob_id`.
4. Submit `anchor_proof(cap, position_id, blob_id)` PTB.
5. Write `(position_id, sui_tx_digest, walrus_blob_id)` to Postgres
   `settlements` for the indexer/API.

Steps 3–4 are best-effort with retry: if Walrus is down, the money is
already settled correctly on-chain (step 1); the proof is published on
the next retry sweep. A position is never left unpaid because Walrus
hiccuped. The proof can lag the payout; it must not block it.

### 5. Public verifier — a pure, dependency-light replay function

Ship `tap-trading-proof-verifier`, a no-IO library crate (+ thin CLI
and a browser-WASM build target) that takes a proof blob and returns
`VerifyResult`:

```rust
pub enum VerifyResult {
    Valid,
    MultiplierMismatch { claimed_bps: u64, recomputed_bps: u64 },
    OutcomeMismatch { claimed: Outcome, recomputed: Outcome },
    InsufficientEvidence,             // evidence_ticks don't span the window
}
```

It re-runs two checks: (a) recompute `multiplier_bps` from
`quote_at_tap` using `tap-trading-pricing-engine` (the *same* crate the
server used — shared code, no reimplementation drift), and (b) re-run
touch detection over `evidence_ticks` and compare to `outcome`. Anyone
can run it against any blob fetched from a Walrus aggregator. The
"Verify this tap" button on a share card is this function compiled to
WASM, running client-side.

### 6. Aggregator gains short-term tick retention for evidence

ADR-0008 §8 made the aggregator stateless with a 500 ms ring buffer.
Evidence assembly needs the full window (up to 60 s of ticks). The
aggregator's ring is extended to retain **120 s** of ticks per asset
(2400 entries at 20 Hz — trivial memory) so the worker can pull a
complete `evidence_ticks` array at settlement. This is a parameter
change to ADR-0008's ring, not a semantic one; the `(run_id, seq)`
contract is unchanged.

## Consequences

- Every DUSDC settlement produces a publicly-replayable proof. The
  trust model is explicit: off-chain execution, verifiable DA. Tick can
  truthfully claim "the only tap-trading game where every payout is
  auditable."
- The settlement worker takes on Walrus-publisher duties and a
  best-effort retry queue. Settlement correctness does not depend on
  Walrus availability; only proof *timeliness* does.
- The aggregator's memory grows from ~10 to ~2400 ticks/asset. Still
  negligible (≈100 KB/asset).
- `tap-trading-pricing-engine` becomes a verification dependency, not
  just a server dependency. Its public API is now a compatibility
  surface — changing the formula requires a `formula_version` bump so
  old proofs still verify against the version they were computed with.
- Walrus blob storage has a cost (WAL tokens) and a retention question.
  On testnet this is free; mainnet needs a storage-funding model
  (Forecast §2).

## Forecast

- **Points-mode proofs:** if points volume justifies it, blob points
  settlements too — but batch them (one blob per N taps, or per minute)
  rather than one-per-tap, since points have no per-settlement money at
  risk. The schema's `v` field absorbs the batched shape.
- **Storage funding (mainnet):** fund Walrus storage from the vault's
  spread revenue; expired blobs (older than the dispute window) can be
  garbage-collected once their merkle root is anchored on Sui — the
  root is the durable commitment, the blob is the convenience copy.
- **Seal-encrypted proofs:** if a player wants their tap history
  private, encrypt the blob with Seal and share the key only with the
  verifier they choose. The anchor/event flow is unchanged.
- **MemWal agent memory:** the Mirror Mode agent (PRD Phase 2) reads a
  player's proof blobs as its memory via MemWal, learning their style.
  This is the AI-agent surface the Walrus track explicitly asks for;
  the proof blobs defined here are its substrate.
