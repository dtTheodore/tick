# ADR-0008 — Tick oracle wire protocol

**Date:** 2026-05-27
**Status:** Accepted
**Workstream:** Tick (tap-trading)
**Supersedes:** —
**Superseded by:** —

## Context

Plan C ships `tap-trading-oracle-aggregator` (the price aggregation
service); Plans D and E both consume its output. `ORACLE_SPEC.md`
specifies the aggregation math (median + EWMA blend, freshness gates,
DEGRADED status) but does not pin the on-the-wire message format,
sequence semantics, or replay query contract.

Three downstream consumers depend on this surface — the settlement
worker subscribes to the live tick stream, the API re-broadcasts the
same stream to clients, and the API replays past ticks at tap-commit
time to recompute the locked multiplier. Without a fixed contract,
each will encode its own opinion of "what an oracle tick looks like"
and they will drift.

The pricing engine (`tap-trading-pricing-engine`, Plan A) uses `f64`
for spot and annualized vol; the schema (`positions.strike_lo/hi`,
`settlements.oracle_price`) uses `NUMERIC(20, 8)`. The wire format
sits in the middle and must converge.

## Decision

### 1. Shared library crate `tap-trading-oracle-types`

A no-IO library crate owns every wire type. The aggregator, worker,
and API all depend on it; nothing else does. This is the **only**
location where `OracleMessage` and friends are defined — no shadow
copies in any service.

### 2. Message envelope

```rust
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OracleMessage {
    Tick(OracleTick),
    Status(OracleStatus),
    Heartbeat { ts_ms: i64 },
}
```

Encoding is JSON. Binary (msgpack, protobuf) is deferred — JSON is
trivially debuggable in a browser WS inspector, and the messages are
small (≤200 bytes each at 20 Hz × 3 assets = ~12 KB/s/client, well
under any realistic budget). Re-evaluate if we exceed 10k WS clients
per node.

### 3. `OracleTick` shape

```rust
pub struct OracleTick {
    pub asset: AssetSymbol,            // re-exported from pricing-engine
    pub run_id: u64,                   // aggregator boot id
    pub seq: u64,                      // monotonic per (asset, run_id), starts at 0
    pub ts_ms: i64,
    pub mid: f64,                      // mid price, decimal
    pub vol_annualized: f64,           // EWMA annualized vol, decimal
    pub source_count: u8,              // active sources contributing
}
```

`mid` and `vol_annualized` are `f64` to match the pricing engine's
input signature and avoid a conversion at every drift check. The
schema uses `NUMERIC(20, 8)` only at write time (worker writes
`settlements.oracle_price`); `f64 → NUMERIC` conversion happens at the
sqlx boundary, where the precision loss is bounded by the 8 fractional
digits the column already permits.

### 4. `seq` and `run_id` semantics

`seq` is monotonic **per (asset, run_id)** and starts at 0 on
aggregator boot. `run_id` is the aggregator's boot identifier — a
fresh `u64` (unix-ms or random) assigned at process start and bumped
on every restart.

The pair `(asset, run_id, seq)` uniquely identifies a tick across the
service's lifetime. Consumers MUST check both `run_id` and `seq` on
replay: a request for `(asset=BTC, run_id=A, seq=12345)` against an
aggregator now running `run_id=B` is a stale quote, not a hit, even
if the new `run_id=B` happens to also have a `seq=12345`.

### 5. Aggregator endpoints

- **`WS /stream`** — server pushes `OracleMessage` JSON frames. One
  WS connection per client, all assets multiplexed. Aggregator emits a
  `Heartbeat` every 5 s; clients drop the connection if no message
  arrives for 15 s and reconnect.
- **`GET /ring/:asset/:seq?run_id=N`** — returns the `OracleTick`
  JSON for that `(asset, run_id, seq)`. Responses:
  - `200` with `OracleTick` body
  - `410 Gone` if the seq is older than the 500 ms window (10 entries
    at 20 Hz) — caller must treat as stale quote
  - `409 Conflict` if `run_id` is missing or doesn't match the
    aggregator's current `run_id` — caller treats as stale quote
  - `404 Not Found` if `asset` is unknown
- **`GET /healthz`** — 200 only if every supported asset has a *fresh*
  tick (newer than `DEGRADED_HYSTERESIS_MS`) with `source_count ≥ 2`;
  503 otherwise. Freshness is required: the aggregator stops pushing
  ticks while an asset is degraded, so a stale last-good tick (which
  still has `source_count ≥ 2`) must not read healthy.
- **`GET /metrics`** — Prometheus text exposition. Stub-only in
  Plan C; metrics fill in later.

### 6. `Status` and `Heartbeat`

```rust
pub struct OracleStatus {
    pub asset: AssetSymbol,
    pub state: OracleStreamState,    // Normal | Degraded
    pub reason: String,              // human-readable; not parsed
    pub run_id: u64,                 // same field as in OracleTick
}

pub enum OracleStreamState { Normal, Degraded }
```

`Heartbeat { ts_ms }` carries no other fields. It's a liveness signal
only; consumers MUST NOT treat the absence of a heartbeat as a price
change.

### 7. Cold-start vol

If fewer than 30 seconds of return data exist for an asset, the
aggregator emits ticks with `vol_annualized = 0.60` (60% annualized,
a deliberately conservative default that will reject most "tight
band" cells via the floor curve). `source_count` reflects actual
sources. No separate `cold_start: bool` field — the floor curve is
the right safety net, and adding the flag invites callers to
silently special-case it.

### 8. Aggregator stateless in Postgres

The aggregator does NOT write to Postgres. Its state lives entirely
in memory: source connections, per-asset return deques, ring buffers.
This decouples it from DB outages and lets a future "secondary
aggregator" node serve replay queries without coordination. If we
need history later, log to Kafka or a write-only side-channel; do
not put it in the primary transactional path.

## Consequences

- The wire format is JSON; debuggability is high and bandwidth is
  fine at v1 scale. Binary formats are a swap when we cross the
  client threshold above.
- `f64` precision is bounded by IEEE-754, which is sufficient at the
  pricing-engine's tolerance and the schema's `NUMERIC(20, 8)`
  storage. Drift-check tolerance (3% per ADR-0009) is two orders of
  magnitude above any precision artifact.
- A restart of the aggregator invalidates every in-flight client's
  `oracle_seq_at_tap` cached in flight. The API's drift check
  surfaces this as `stale_quote` via the 409 Conflict from `/ring`.
  Clients re-fetch a fresh quote and re-display.
- The library crate is the single point of truth — every consumer
  parses against the same struct definition. Schema drift across
  services is a compile error, not a runtime mystery.

## Forecast

- Binary encoding (msgpack or protobuf): swap inside `OracleMessage::
  encode`/`decode`; wire-format change but no semantic change.
- Multi-asset ranges (e.g. forex pairs): extend `AssetSymbol`. No
  message-shape change.
- Cross-aggregator replication: emit ticks to Kafka or a shared
  Redis Stream and let any node replay. The `(run_id, seq)` contract
  was designed for this and does not need to change.
