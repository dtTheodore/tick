# Tick — Oracle Spec

**Status:** v0.1
**Owner:** Oracle Aggregator service
**Audience:** Engineers building `backend/oracle-aggregator/` + on-call ops + auditors
**Companions:** `PRD.md` §14, `MATH_SPEC.md` §3

> **Why this spec exists:** the live price line + multiplier flicker is what users feel as "fairness." Raw Pyth has discrete 400ms jumps and occasional outlier publishers. Pacifica SWIM solved this with multi-source EWMA across CEX + DEX. This doc is how we replicate that on Sui.

---

## 0. Locked Decisions

- **Server emit cadence**: 20 Hz (every 50 ms) per asset.
- **Client display refresh**: 10 Hz (every 100 ms). Client sub-samples server ticks to avoid visual flicker on multipliers and price line.
- **Aggregator state ring buffer**: keep the last 500 ms (10 ticks per asset) addressable by `aggregator_seq` so `POST /positions` can replay multiplier at the exact tick the client was rendering. See §5.6. **USDC mode extends this to 120 s** (ADR-0011 §6) so the settlement worker can assemble a full `[t_open, t_close]` evidence-tick array for each Walrus proof blob; the 500 ms figure is the points-mode tap-replay window, the 120 s figure is the proof-evidence window. Same ring, longer retention; `(run_id, seq)` semantics unchanged (ADR-0008).
- **Sources**: Pyth Hermes + Binance + Bybit + OKX. Confirmed for v1.

---

## 1. Goals & non-goals

**Goals:**
- Provide a single smooth `spot` price per supported asset to: (a) clients (web + mobile PWA), (b) the settlement worker, (c) the pricing engine for `σ̂`
- Defend against single-publisher manipulation and feed outages
- Anchor integrity via Pyth's signed feed as one of the four sources (so the aggregate has cryptographic backing without per-tick audit overhead)

**Non-goals:**
- On-chain price storage for every tick (impossible; off-chain is the system of record)
- Real-time delivery to consensus latency (no smart contract reads our ticks; only the weekly snapshot does)
- HFT-grade microsecond latency (we target 50ms aggregator-broadcast cadence; that is invisible at 5-30s cell windows)

---

## 2. Supported assets — Phase 1

| Asset | Symbol | Display tick | Pyth feed ID (mainnet) | External venues |
|---|---|---|---|---|
| Ether | ETH/USD | Δ$0.50 | `ff61491a931112ddf1bd8147cd1b641375f79f5825126d665480874634fd0ace` | Binance `ethusdt@aggTrade`, Bybit `ETHUSDT`, OKX `ETH-USDT` |
| Bitcoin | BTC/USD | Δ$10 | `e62df6c8b4a85fe1a67db44dc12de5db330f7ac66b72dc658afedf0f4a415b43` | Binance `btcusdt@aggTrade`, Bybit `BTCUSDT`, OKX `BTC-USDT` |
| Solana | SOL/USD | Δ$0.10 | `ef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d` | Binance `solusdt@aggTrade`, Bybit `SOLUSDT`, OKX `SOL-USDT` |

Phase 2 expansion: SUI, DEEP, WAL, DOGE, PEPE, SHIB, WIF — all via Pyth, but external CEX venue selection is per-asset (some memecoins only have one good CEX feed).

Pyth feed IDs sourced from `docs.pyth.network/price-feeds/price-feeds`. They are 32-byte hex (no leading `0x`). The off-chain aggregator reads the **Stable channel (mainnet)** only, in every environment — the Beta (testnet) channel was dropped because its sparse, jumpy ticks destabilized the realized-vol estimate and made the multiplier grid flicker.

---

## 3. Sui-native Pyth integration

We don't read Pyth on-chain in v1 (no per-tap on-chain settlement). Reference addresses kept here for Phase 3+ when the weekly anchor + future real-money mode reintroduce on-chain Pyth reads:

| Field | Mainnet | Testnet |
|---|---|---|
| Pyth Package ID | `0x04e20ddf36af412a4096f9014f4a565af9e812db9a05cc40254846cf6ed0ad91` | `0xabf837e98c26087cba0883c0a7a28326b1fa3c5e1e2c5abdb486f9e8f594c837` |
| Pyth State ID (stable across upgrades) | `0x1f9310238ee9298fb703c3419030b35b22bb1cc37113e3bb5007c99aec79e5b8` | `0x243759059f4c3111179da5878c12f68d612c21a8d54d85edc86164bb18be1c7c` |
| Wormhole State ID | `0xaeab97f96cf9877fee2883315d459552b2b921edc16d7ceac6eab944dd88919c` | `0x31358d198147da50db32eda2562951d53973a0c0ad5ed738e9b17d88b213d790` |
| Hermes endpoint | `https://hermes.pyth.network` | `https://hermes-beta.pyth.network` |

**Critical**: package ID changes on Pyth upgrades; state ID is stable. Always derive the live package ID at runtime via `SuiPythClient.getPackageId()`. Hardcoding bricks the integration on upgrade.

---

## 4. The aggregator design

### 4.1 Service overview

Location: `backend/oracle-aggregator/` (Rust + tokio)
Output: WebSocket fanout (single endpoint per environment, multiplexed by asset)
Cadence: emit 1 tick per asset per 50ms (= 20 Hz)
Latency budget: source tick → emitted tick ≤ 100ms p95

### 4.2 Input subscriptions

Per asset, the service maintains **four** parallel WS subscriptions:

```
Source           URL                                             Format
─────────────────────────────────────────────────────────────────────────────
Pyth Hermes      wss://hermes.pyth.network/ws                    Signed VAA stream (binary)
                                                                 OR REST SSE /v2/updates/price/stream

Binance Spot     wss://stream.binance.com:9443/ws/{symbol}@aggTrade
                                                                 JSON aggTrade event

Bybit Spot       wss://stream.bybit.com/v5/public/spot           JSON publicTrade event
                                                                 (subscribe to publicTrade.{symbol})

OKX Spot         wss://ws.okx.com:8443/ws/v5/public              JSON trades-all event
                                                                 (subscribe to channel "trades")
```

Each source's connection is wrapped in a backoff/reconnect loop. On disconnect: exponential backoff 100ms → 30s, jitter ±10%.

### 4.3 Tick normalization

Each source produces a `SourceTick`:

```rust
struct SourceTick {
    source: SourceId,        // Pyth | Binance | Bybit | OKX
    asset: AssetSymbol,
    price: f64,              // USD
    timestamp_ms: u64,        // server-received time, NOT exchange-provided
    pyth_conf_bps: Option<u32>,  // only for Pyth
}
```

### 4.4 Aggregation pipeline (per asset, every 50ms)

```
For each asset, every 50ms:

  1. Snapshot the latest tick from each source
       latest = { Pyth: t_p, Binance: t_b, Bybit: t_y, OKX: t_o }

  2. Reject stale sources
       active = { src ∈ latest | now() − t_src.timestamp_ms ≤ 1000ms }

  3. Reject low-confidence Pyth (if Pyth in active)
       if Pyth.conf_bps > 100 (1%):
         active.remove(Pyth)

  4. Reject if too few sources
       if |active| < 2:
         emit "DEGRADED" status, do NOT emit a new tick
         clients must surface "stabilizing prices, taps paused"

  5. Median across active sources (robust to single outlier)
       median_price = median([src.price for src in active])

  6. EWMA smooth
       smooth_t = α · median_price + (1 − α) · smooth_{t-1}
       with α = 0.6   (heavier weight on new data — tap UX needs responsiveness)

  7. Emit AggregatedTick
       {
         asset, smooth_t, median_price, sources_used: [...],
         timestamp_ms: now(), aggregator_seq: monotonic counter
       }

  8. Push to: WS broadcast channel, settlement worker channel.
```

### 4.5 Rationale

- **Median, not mean.** A single bad Pyth update (one publisher misreporting) can shift the mean. Median is robust.
- **EWMA with α=0.6.** Heavier than Binance's typical 0.94 because tap UX needs fast response, not statistical smoothness over minutes. Tune via UX playtest.
- **Server-received timestamp.** Exchange-provided timestamps are unreliable and skewed. Server-received is the integrity anchor.
- **Reject if `|active| < 2`.** Single-source emission is a vector for any one venue to manipulate; we'd rather pause taps than emit garbage.

---

## 5. Client subscription protocol

> **Wire format authority:** `docs/decisions/0008-tick-oracle-wire-protocol.md`
> (ADR-0008) is authoritative for on-the-wire field names and shapes; it
> supersedes the examples below where they differ (`ts_ms` not
> `timestamp_ms`, `source_count` not `sources_used`, `mid` not `price`, no
> `median` field on the wire). The examples here are kept for intent.

### 5.1 WebSocket endpoint

```
wss://oracle.tick.xyz/v1/ticks         (production)
wss://oracle-testnet.tick.xyz/v1/ticks (testnet)
```

### 5.2 Subscribe message

> **v1 (ADR-0008 §2):** the server streams **all** assets by default and
> ignores client app frames — there is no subscribe handshake yet. The
> message below is the planned per-asset subscription for a later version.

```json
{ "op": "subscribe", "assets": ["ETH", "BTC", "SOL"] }
```

### 5.3 Tick message (server → client)

```json
{
  "type": "tick",
  "asset": "ETH",
  "run_id": 1747526400000,
  "seq": 9847234,
  "ts_ms": 1747526400123,
  "mid": 3812.45,
  "vol_annualized": 0.78,
  "source_count": 4
}
```

Every emitted tick carries the asset's current `σ̂` (annualized), so clients compute multipliers locally without extra round-trips.

### 5.4 Status message

```json
{ "type": "status", "asset": "ETH", "state": "degraded", "run_id": 1747526400000,
  "reason": "Pyth excluded (conf 145 bps); Bybit stale 1.2s" }
```

Clients display a "stabilizing prices" overlay; tap buttons disable.

### 5.5 Heartbeat

Every 5 seconds, server sends:
```json
{ "type": "heartbeat", "ts_ms": 1747526400000 }
```

Client disconnects + reconnects if no heartbeat in 15s.

### 5.6 Cadence & lock-at-tap replay

- **Server emit**: 20 Hz per asset (every 50 ms). Each emitted `AggregatedTick` carries a monotonic `seq` so clients and the API can refer to a specific past tick.
- **Client display refresh**: 10 Hz per cell (every 100 ms). The client *receives* ticks at 20 Hz but only *re-renders multipliers* every other tick. Reasons: (a) 50 ms multiplier re-render flickers tabular nums; (b) human visual perception saturates around 10–15 Hz for numerical change.
- **State ring buffer**: the aggregator retains emitted state indexed by `seq`. Points-mode tap-replay needs only the last 500 ms (≈10 ticks/asset). USDC mode (ADR-0011 §6) needs the full cell window for proof evidence, so retention is **120 s** (≈2400 ticks/asset at 20 Hz, ≈100 KB/asset — negligible). The API replays the tap tick from this ring; the settlement worker pulls the `[oracle_seq_at_tap .. touch_seq]` slice from it at settle time via `GET /ring/:asset/:seq`.

This is the mechanic that makes lock-at-tap honest. The client posts `oracle_seq_at_tap = <last seq it rendered>`; the API recomputes the cell's multiplier against the state at that seq; if the API's state has rotated past that seq (>500 ms gap, e.g., the user took 600 ms to commit after the tap UI was shown), the API rejects with `stale_quote` and the client surfaces a re-tap prompt.

```
client renders tick seq=914_352 @ display refresh tick
user taps cell  → client posts (cell, multiplier_at_tap=5.2, oracle_seq_at_tap=914_352)
api receives    → looks up seq 914_352 in ring buffer (ok, 200ms ago)
                → recomputes multiplier at that state = 5.18
                → |5.2 - 5.18| / 5.18 = 0.4% drift → accept
                → INSERT positions WITH multiplier_at_tap = 5.18 (server value)
                → respond 200 with locked = 5.18
```

---

## 6. Anti-manipulation thresholds

| Threshold | Default | Where applied | Rationale |
|---|---|---|---|
| `pyth.conf / pyth.price ≤ 100 bps` | 1% | Reject Pyth from median during news events | Confidence interval spikes ~15× during major moves |
| Source freshness `≤ 1000ms` | 1s | Drop stale source from median | A source that hasn't ticked in 1s is unreliable for high-freq games |
| Minimum active sources `≥ 2` | 2 | Pause emission below threshold | Single-source emission = manipulation vector |
| EMA divergence `|spot − pyth_ema| / pyth_ema ≤ 200 bps` | 2% | Cross-check; warn if violated, don't block | Pyth EMA is smoothed; large divergence = real news, not bad data |
| Spot move `|r_i| ≤ 5 · σ̂_{i-1}` | 5σ | Bump σ̂ immediately on violation | Vol regime shift detection |
| Tap window vs settlement window | tap_close = settle_close − 1s | Block taps in final 1s | Prevents tap-after-tick attacks (PRD MVP-08) |
| Aggregator emission rate | 20 Hz (50ms) | Hard limit per asset | Prevents server-side spam / fingerprinting |

All thresholds are config-driven via `backend/oracle-aggregator/config.toml`. Config changes are recorded in git history with PR review.

---

## 7. Failure modes & operational responses

| Failure mode | Detection | Automatic response | Operator action |
|---|---|---|---|
| Pyth Hermes WS disconnect | Heartbeat miss | Reconnect with backoff; rely on Binance/Bybit/OKX | Page if >5 min |
| All 4 sources stale | Aggregation step 4 | Emit "DEGRADED"; pause taps; alert | Investigate; manual restart if needed |
| Pyth confidence interval spike | Aggregation step 3 | Pyth dropped from median; continue with 3 sources | Watchlist; no immediate page |
| Single source price divergence >5% from median | Per-tick check | Reject outlier; log warning | Investigate if persistent |
| WS broadcast lag to clients | Client heartbeat detects | Client reconnects | Check load balancer |
| Sui chain outage | (irrelevant to live game) | n/a | Snapshot publisher (Phase 3+) retries; game continues |
| Pyth network upgrade changes package ID | n/a (v1 doesn't read Pyth on-chain) | n/a | Update `tick_anchor` Move dep before snapshot week (Phase 3+) |

---

## 8. Reliability targets

| Metric | Target | Measurement |
|---|---|---|
| Aggregator uptime | 99.9% | Status page; emit metrics to PostHog |
| Per-tick latency (source → emit) p95 | ≤ 100ms | Server-side histogram |
| Per-tick latency (emit → client) p95 | ≤ 200ms | Client-side measurement, sampled |
| `DEGRADED` status duration / month | ≤ 5 min total | Status page incident log |
| Audit log gap | ≤ 1 second | Continuous monitor; alert on gap > 5s |

---

## 9. Deployment notes

- Run two aggregator instances behind a load balancer; only one is leader (active emission) at a time, leader-elected via Postgres advisory lock. The other is hot-standby for fast failover.
- Source WS subscriptions: per-instance. Both instances subscribe; only leader emits. Standby can promote in ≤2s.
- Run in 2 regions (e.g., Frankfurt + Singapore) once Phase 2 international launch happens; for v1, single region (Frankfurt) is sufficient.

---

## 10. Open questions

| # | Question | Stakeholders |
|---|---|---|
| O1 | Pyth Hermes WS vs SSE — which is preferred for our latency budget? Test both. | Eng |
| O2 | Should we subscribe to Coinbase too as a 5th source? Tradeoff: more redundancy vs more variance. | Eng |
| O3 | Per-settlement on-chain integrity proof: when do we re-introduce this? Phase 5 (real-money mode) is the natural trigger. | Legal + Eng |
| O4 | Per-asset `α` tuning — do we keep 0.6 globally, or per-asset (BTC slower than memes)? | Eng + UX playtest |
| O5 | What happens during Sui consensus outages (Jan 14 2026 precedent)? Game still works (off-chain), but weekly snapshot is delayed. Document SLA. | Ops |

---

## 11. References

- Pyth contract addresses on Sui: https://docs.pyth.network/price-feeds/contract-addresses/sui
- Pyth feed catalog: https://docs.pyth.network/price-feeds/price-feeds
- Pyth Hermes API: https://hermes.pyth.network/docs/
- Pyth price aggregation algorithm: https://docs.pyth.network/price-feeds/how-pyth-works/price-aggregation
- Pyth best practices: https://docs.pyth.network/price-feeds/best-practices
- DeepBook margin oracle (production reference for the 4-check validation pattern): https://github.com/MystenLabs/deepbookv3/blob/main/packages/deepbook_margin/sources/helper/oracle.move
- Binance WS docs: https://github.com/binance/binance-spot-api-docs/blob/master/web-socket-streams.md
- Bybit v5 WS docs: https://bybit-exchange.github.io/docs/v5/ws/connect
- OKX v5 WS docs: https://www.okx.com/docs-v5/en/#overview-websocket
- Pacifica SWIM oracle pattern (multi-source EWMA): https://pacifica.gitbook.io/docs/swim/swim

