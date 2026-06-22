# Tick — System Design

**Status:** v0.1
**Owner:** Architecture
**Audience:** Engineers (backend, frontend, Move, ops, SRE)
**Companions:** `PRD.md`, `MATH_SPEC.md`, `ORACLE_SPEC.md`

> **Why this spec exists:** the other docs cover *what* we're building (PRD) and *how each piece works* (MATH / ORACLE). This doc covers *how the pieces fit together*: services, APIs, schemas, auth, deployment, failure recovery. If you join the team tomorrow, this is the doc you read to understand how data flows from a user's tap to a credited point.

---

## 0. Locked Decisions

Anchors from frame-by-frame review of Pacifica SWIM, Euphoria, BC.Game. Do not re-litigate without new evidence.

- **API stack**: Rust + axum. Co-located with settlement worker + oracle aggregator inside the `games/tap-trading/backend/` Cargo workspace. Shared `backend/pricing-engine/` Rust crate.
- **Pricing engine**: canonical impl is the Rust crate at `backend/pricing-engine/` (server-side: drift check, aggregator). Client has a thin TS port at `packages/pricing-engine-ts/` for 10 Hz local recompute (matches Pacifica / Euphoria's responsive-UI pattern). Drift between the two is policed by shared QuantLib parity fixtures in CI — see `MATH_SPEC.md §5`.
- **Paths in this doc** are relative to `games/tap-trading/`. The backend is a self-contained Cargo workspace under `games/tap-trading/backend/`; it does **not** join the platform-wide workspace at `platform/`.
- **Product name**: Tick. Repo lives at `games/tap-trading/`; package scope `@tap-trading/*` for TS, `tap-trading-*` for Rust crates.
- **Settlement**: off-chain Rust worker, first-touch over `[t_open, t_close]` with `t_open` clock-aligned in the future. See `MATH_SPEC.md §1`.
- **Cell duration**: 5 s. Visible columns: 4 mobile / 6 desktop.
- **Telegram**: cut from all phases. Web + mobile PWA only.
- **No energy system in v1.** Points are a finite per-user resource. Full economy in `PRD.md §15`.
- **Tap lifecycle**: `PENDING → LOCKED → SETTLED`. Server roundtrip during `PENDING` is 100–300 ms; client shows `--` placeholder.
- **Drift tolerance**: 3% between client and server multiplier at tap.
- **Per-user tap rate limit**: 10 taps/sec hard ceiling (token bucket).
- **Settlement worker transactional boundary**: one Postgres `BEGIN/COMMIT` wraps `settlements INSERT … RETURNING`, `positions UPDATE`, and `points_ledger INSERT`. See §5.2.
- **Move package** `tick_anchor` mainnet deployment deferred to Phase 3; not in v1 critical path.
- **On-chain USDC vault + Walrus proofs** (`tick_vault` package, per-tap proof blobs) are pulled forward for the Sui Overflow 2026 build as a **parallel mode** alongside the points loop — see ADR-0010 (custody & settlement authority) and ADR-0011 (verifiable replay). This **overrides** the "No vault contract" line in §1. The points loop (plans A–E) described throughout this doc is unchanged; the vault is additive and gated on the points loop landing first.

---

## 1. Service topology

```
                           ┌────────────────────────────────────────┐
                           │  Sui Move package: tick_anchor        │
                           │  - Shared `Anchor` object              │
                           │  - publish_root(week, merkle_root, ...)│
                           │  - verify_membership(...)              │
                           └─────────────────▲──────────────────────┘
                                             │  one tx per week
                                             │
                          ┌──────────────────┴──────────────┐
                          │  anchor-publisher (Rust)        │
                          │  Weekly cron: snapshot accounts │
                          │  → merkle tree → publish on-chain│
                          └──────────────────▲──────────────┘
                                             │  reads
                                             │
   ┌─────────────────────────────────────────┴────────────────────────────┐
   │  Postgres (system of record)                                          │
   │  - accounts, positions, settlements, points_ledger,                   │
   │    quests, streaks, snapshots, flags                                  │
   └─────────▲──────────────────────────────────────────▲──────────────────┘
             │ R/W                                       │ writes per credit
             │                                           │
   ┌─────────┴──────────┐                    ┌──────────┴──────────────┐
   │ backend/api        │                    │ backend/settlement-     │
   │ (Rust + axum)      │                    │ worker (Rust)            │
   │                    │                    │                          │
   │ - REST endpoints   │                    │ - Consumes oracle ticks  │
   │ - WS /me/events    │◀───── Redis ──────▶│ - In-memory open-position│
   │ - zkLogin auth     │  (pub/sub +        │   cache; touch detection │
   │ - Tap rate limit   │   session tokens)  │ - Idempotent credits     │
   │ - Drift validation │                    │   in single Postgres tx  │
   └─────────▲──────────┘                    └──────────▲───────────────┘
             │                                          │
             │                                          │
             │                                          │ broadcasts every 50ms
             │                                          │
             │                              ┌───────────┴──────────────┐
             │                              │ backend/oracle-          │
             │                              │ aggregator (Rust)        │
             │                              │                          │
             │                              │ - Pyth Hermes + 3 CEX WS │
             │                              │ - Median + EMA smoothing │
             │                              │ - WS broadcast to clients│
             │                              │ - WS broadcast to worker │
             │                              └─────────▲────────────────┘
             │                                        │
             │                                        │ subscribes
             │                                        │
   ┌─────────┴────────────────────────────────────────┴────────────────────┐
   │  apps/web (Next.js 15)                                                │
   │  - Same codebase for Web + Mobile PWA. No Telegram surface.           │
   │  - zkLogin via Enoki                                                   │
   │  - Subscribes to oracle aggregator WS for live price line             │
   │  - Recomputes multipliers at 10 Hz client-side from latest oracle tick│
   │  - Calls API for tap commits, history, leaderboard                    │
   │  - Subscribes to /me/events WS for own settle events                  │
   └────────────────────────────────────────────────────────────────────────┘
```

### Service responsibilities (one-liners)

| Service | Language | Responsibility |
|---|---|---|
| `apps/web` | TypeScript / Next.js 15 | Web + Mobile PWA. Renders grid, computes multipliers locally at 10 Hz, calls API on tap with `oracle_seq_at_tap`. |
| `backend/api` | Rust + axum | Auth, position commits, history, leaderboard, quests, share cards. Postgres + Redis. Per-user tap rate limit (10/s). |
| `backend/oracle-aggregator` | Rust + tokio | Multi-source EMA price feed; emits at 50 ms server-side, broadcasts to clients + settlement worker. See `ORACLE_SPEC.md`. |
| `backend/settlement-worker` | Rust + tokio | Consumes oracle ticks; credits points when positions' monitoring windows touch their bands; idempotent via `UNIQUE(position_id)`. |
| `backend/anchor-publisher` | Rust | **Phase 3+ only.** Weekly cron; merkle-roots the accounts table; publishes to Sui `tick_anchor`. Not deployed in v1. |
| `move/tick_anchor` | Sui Move | **Phase 3+ only.** On-chain anchor: stores weekly merkle roots; verifies inclusion proofs. Not deployed in v1. |
| `move/tick_vault` | Sui Move | **Hackathon build (ADR-0010).** Generic `GameVault<Quote>` custodying testnet Circle USDC; `mint`/`settle_*` gated by `SettlerCap`; on-chain exposure caps. Parallel to points mode. |
| `backend/settlement-worker` (dual-sink) | Rust + tokio | **Extended (ADR-0010 §7).** Points → Postgres (unchanged); USDC → `settle_*` PTB + Walrus proof publish. Shared touch logic across both sinks. |
| `backend/proof-verifier` + `backend/walrus-client` | Rust (+ WASM) | **Hackathon build (ADR-0011).** Pure replay verifier (WASM "Verify this tap") + Walrus `PUT`/`GET` client. |

### What's NOT a separate service (v1)

- **No matching engine.** No CLOB. Tappers don't trade against each other.
- **No vault contract** *in the points loop.* Points only; no funds to manage. (**Overridden for the hackathon build:** ADR-0010 adds a parallel on-chain USDC `tick_vault` mode and ADR-0011 adds per-tap Walrus proofs, running alongside — not replacing — the points loop. The points path described in this doc is unchanged. See §1.1.)
- **No real-time keeper for Pyth pull-oracle updates on Sui.** v1 settlement is off-chain. (Phase 5+ real-money mode reintroduces this.)
- **No separate indexer.** Postgres is system of record; we read directly. (At ~100K-1M users we may add a read replica.)

### 1.1 USDC mode & on-chain settlement (hackathon build)

Parallel to the points loop, not a replacement. Spec: ADR-0010, ADR-0011.

- **Custody:** `tick_vault::GameVault<USDC>` (testnet Circle USDC,
  `0xa1ec…::usdc::USDC`) holds the treasury; players hold idle funds in
  per-player `PlayerBalance` objects. Mint debits the player balance into
  the vault and records an on-chain `Position` with the locked
  `multiplier_bps`.
- **Settlement authority:** the settlement worker holds a `SettlerCap`
  and signs `settle_win`/`settle_loss`/`settle_void` PTBs via the `sui`
  CLI behind a `SettlerClient` trait. Players cannot self-settle. Touch
  detection is the *same* off-chain logic as points mode (single source
  of truth, the `tap-trading-touch` crate).
- **Verifiability (Hyperliquid pattern):** every USDC settlement publishes
  a Walrus proof blob (oracle path + locked multiplier + outcome) via the
  `walrus` CLI behind a `ProofPublisher` trait, anchored on Sui via a
  `ProofAnchored` event. Anyone can replay it with
  `tap-trading-proof-verifier`. Walrus availability never blocks payout.
- **Separation:** points balances (off-chain) and USDC balances
  (on-chain) are independent; no conversion in v1 (ADR-0010 §8).

---

## 2. Data model — Postgres schemas

All tables. Conventions: `BIGINT` IDs (BIGSERIAL), `_ms` suffix for unix-ms timestamps, all monetary/point values in unscaled integer units.

### 2.1 `accounts`

```sql
CREATE TABLE accounts (
  id                  BIGSERIAL PRIMARY KEY,
  external_id         TEXT NOT NULL UNIQUE,                    -- "zk:google:<sub>"
  zklogin_sub         TEXT NOT NULL,
  zklogin_iss         TEXT NOT NULL,                           -- "https://accounts.google.com" etc
  display_name        VARCHAR(64),
  tier                SMALLINT NOT NULL DEFAULT 1,
  balance             BIGINT NOT NULL DEFAULT 0,               -- current spendable points; maintained in-tx alongside ledger
  lifetime_points_won BIGINT NOT NULL DEFAULT 0,
  flag_state          VARCHAR(16) NOT NULL DEFAULT 'OK',       -- 'OK' | 'SOFT_FLAG' | 'HARD_FLAG'
  signup_bonus_at_ms  BIGINT,                                  -- null until claimed
  created_at_ms       BIGINT NOT NULL,
  last_active_ms      BIGINT NOT NULL,
  CHECK (balance >= 0)
);

CREATE INDEX accounts_external_id ON accounts(external_id);
CREATE INDEX accounts_lifetime_points ON accounts(lifetime_points_won DESC);
CREATE INDEX accounts_last_active ON accounts(last_active_ms DESC);
```

### 2.2 `positions`

```sql
CREATE TABLE positions (
  id                  BIGSERIAL PRIMARY KEY,
  account_id          BIGINT NOT NULL REFERENCES accounts(id),
  asset               VARCHAR(16) NOT NULL,                    -- 'ETH' | 'BTC' | 'SOL'
  strike_lo           NUMERIC(20, 8) NOT NULL,
  strike_hi           NUMERIC(20, 8) NOT NULL,
  t_open_ms           BIGINT NOT NULL,
  t_close_ms          BIGINT NOT NULL,
  stake_points        BIGINT NOT NULL,
  multiplier_at_tap   NUMERIC(10, 4) NOT NULL,
  status              VARCHAR(16) NOT NULL DEFAULT 'OPEN',     -- 'OPEN' | 'WON' | 'LOST' | 'VOIDED'
  settled_at_ms       BIGINT,                                  -- nullable until settle
  client_fingerprint  TEXT,                                    -- for anti-cheat (browser hash)
  ip_hash             BYTEA,                                   -- rotated daily, salted
  created_at_ms       BIGINT NOT NULL
);

CREATE INDEX positions_account ON positions(account_id, created_at_ms DESC);
CREATE INDEX positions_open ON positions(status, t_close_ms) WHERE status = 'OPEN';
CREATE INDEX positions_settle_window ON positions(asset, t_open_ms, t_close_ms);
```

The `positions_open` partial index is hot — every aggregator tick may query this.

### 2.3 `settlements`

```sql
CREATE TABLE settlements (
  id                  BIGSERIAL PRIMARY KEY,
  position_id         BIGINT NOT NULL UNIQUE REFERENCES positions(id),
  account_id          BIGINT NOT NULL,
  outcome             CHAR(1) NOT NULL,                        -- 'W' | 'L' | 'V' (void)
  points_delta        BIGINT NOT NULL,
  oracle_price        NUMERIC(20, 8) NOT NULL,
  settled_at_ms       BIGINT NOT NULL,
  multiplier_used     NUMERIC(10, 4) NOT NULL,
  streak_at_credit    INT NOT NULL,
  streak_bonus        NUMERIC(5, 3) NOT NULL
);

CREATE INDEX settlements_account ON settlements(account_id, settled_at_ms DESC);
```

UNIQUE on `position_id` enforces at-most-once settlement.

### 2.4 `points_ledger`

Every credit/debit gets a ledger row. Authoritative source of `accounts.lifetime_points_won` reconciliation.

```sql
CREATE TABLE points_ledger (
  id            BIGSERIAL PRIMARY KEY,
  account_id    BIGINT NOT NULL REFERENCES accounts(id),
  kind          VARCHAR(24) NOT NULL,
  -- 'SIGNUP' | 'DAILY_LOGIN' | 'QUEST' | 'TAP_STAKE' | 'TAP_PAYOUT'
  -- | 'STREAK_BONUS' | 'TIER_UP' | 'TOURNAMENT'
  delta         BIGINT NOT NULL,
  ref_id        BIGINT,                                       -- position_id, quest_id, etc
  created_at_ms BIGINT NOT NULL
);

CREATE INDEX ledger_account ON points_ledger(account_id, created_at_ms DESC);
CREATE INDEX ledger_kind ON points_ledger(kind, created_at_ms DESC);
```

Authoritative balance lives on `accounts.balance`, mutated in the same transaction as every ledger insert (see §3.3 `POST /positions` and §5.2 `settle_win`). A nightly reconciliation job verifies `accounts.balance == SUM(points_ledger.delta)` per account and alerts on drift (`SYSTEM_DESIGN §9`).

### 2.5 `streaks`

```sql
CREATE TABLE streaks (
  account_id     BIGINT PRIMARY KEY REFERENCES accounts(id),
  current_streak INT NOT NULL DEFAULT 0,
  max_streak     INT NOT NULL DEFAULT 0,
  updated_at_ms  BIGINT NOT NULL
);
```

### 2.6 `daily_quests`

```sql
CREATE TABLE daily_quests (
  id              BIGSERIAL PRIMARY KEY,
  account_id      BIGINT NOT NULL REFERENCES accounts(id),
  quest_code      VARCHAR(32) NOT NULL,                       -- 'TAP_20', 'WIN_5_STREAK', etc
  utc_date        DATE NOT NULL,
  progress        INT NOT NULL DEFAULT 0,
  target          INT NOT NULL,
  reward_points   INT NOT NULL,
  completed_at_ms BIGINT,
  UNIQUE (account_id, quest_code, utc_date)
);

CREATE INDEX quests_account_date ON daily_quests(account_id, utc_date);
```

### 2.7 `snapshots`

```sql
CREATE TABLE snapshots (
  week_idx        BIGINT PRIMARY KEY,                        -- weeks since epoch_start
  merkle_root     BYTEA NOT NULL,
  total_users     BIGINT NOT NULL,
  total_points    NUMERIC(30, 0) NOT NULL,
  on_chain_tx     TEXT NOT NULL,                             -- Sui tx digest
  published_at_ms BIGINT NOT NULL
);
```

### 2.8 `flags`

```sql
CREATE TABLE flags (
  id            BIGSERIAL PRIMARY KEY,
  account_id    BIGINT NOT NULL REFERENCES accounts(id),
  flag_code     VARCHAR(32) NOT NULL,                        -- 'HIGH_TAP_RATE', 'BROWSER_AUTOMATION', etc
  severity      VARCHAR(8) NOT NULL,                         -- 'SOFT' | 'HARD'
  evidence      JSONB NOT NULL,                              -- snapshot of the triggering pattern
  reviewed_at_ms BIGINT,
  resolution    VARCHAR(16),                                 -- 'UPHELD' | 'CLEARED' | null
  created_at_ms BIGINT NOT NULL
);

CREATE INDEX flags_account ON flags(account_id, created_at_ms DESC);
CREATE INDEX flags_open ON flags(severity) WHERE reviewed_at_ms IS NULL;
```

---

## 3. API surface

Base URL: `https://api.tick.xyz/v1`. All endpoints require auth except `POST /auth/*`.

### 3.1 Auth

```
POST /auth/zklogin
  Body: { jwt: string, ephemeral_pk: string, max_epoch: number, randomness: string, salt: string }
  Returns: { session_token: string, account: AccountSummary }
  Validates the zkLogin proof against Sui's verifier; on first sign-in, creates the account
  AND credits the 10,000-point signup bonus (one Postgres tx: INSERT accounts with balance=10_000
  + INSERT points_ledger SIGNUP +10_000); issues 12h-TTL session token (opaque random, stored
  in Redis).
```

Session token sent on subsequent requests via `Authorization: Bearer <token>` header.

### 3.2 User state

```
GET /me
  Returns: AccountSummary {
    id, display_name, tier, lifetime_points_won, balance,
    streak_now, flag_state
  }

GET /me/history?asset=ETH&limit=50&cursor=<id>
  Returns: { positions: PositionRow[], next_cursor }

GET /me/events  (WebSocket)
  Pushes:
    - { type: 'position_settled', position_id, outcome, points_delta, streak }
    - { type: 'quest_progress', quest_id, progress, completed }
    - { type: 'tier_up', new_tier, bonus }
```

### 3.3 Trading actions

```
POST /positions
  Body: {
    asset: 'ETH'|'BTC'|'SOL',
    strike_lo: number,
    strike_hi: number,
    t_open_ms: number,                   // must be a clock-aligned 5s boundary
    t_close_ms: number,                  // = t_open_ms + 5000
    stake_points: number,
    multiplier_at_tap: number,           // client-displayed mult at moment of tap
    oracle_seq_at_tap: number,           // aggregator seq the client was rendering
    client_fingerprint: string
  }
  Server steps:
    1. Verify session.
    2. Per-user rate limit: token bucket, 10 taps/sec, burst 20. Reject 429 if exceeded.
    3. Validate cell parameters:
       - asset in supported list
       - strike_lo < strike_hi, strike grid aligned for asset (Δ$0.5 ETH, Δ$10 BTC, Δ$0.1 SOL)
       - (t_close_ms - t_open_ms) == 5000 (v1: fixed 5s cells)
       - t_open_ms is on a 5s clock boundary (t_open_ms % 5000 == 0)
       - t_open_ms ≥ now()  (no past taps)
       - t_close_ms - now() ≥ 1000  (1s lock window before close)
       - stake_points is in the allowed set {50, 100, 500, 1000} (Tier 1) or {…5000} (Tier 2+)
    4. Recompute multiplier server-side, using the aggregator state at oracle_seq_at_tap:
       - Read the aggregator state snapshot at that seq (kept in a short-window ring buffer)
       - If seq is older than (now - 500ms) or not in buffer: reject 409 'stale_quote'
       - Run pricing-engine multiplier()
       - Reject 409 'multiplier_drift' if |server_mult - client_mult| / server_mult > 0.03  (3% drift)
    5. Atomic Postgres transaction:
         BEGIN;
           INSERT INTO positions (...)
             VALUES (..., 'OPEN', server_mult, ...)
             RETURNING id;
           INSERT INTO points_ledger (account_id, kind, delta, ref_id)
             VALUES ($account, 'TAP_STAKE', -stake_points, position_id);
           UPDATE accounts SET balance = balance - stake_points
             WHERE id = $account AND balance >= stake_points;
           -- final UPDATE returns row count; if 0, raise (insufficient balance);
         COMMIT;
       (Balance is maintained on accounts row; ledger is the audit trail.)
    6. Return { position_id, multiplier_locked: server_mult, expected_payout: stake * server_mult, t_open_ms, t_close_ms }

  Client tap lifecycle:
    - On tap, render PENDING badge (stake shown, mult shown as "—")
    - POST /positions
    - On 200: replace badge with LOCKED state (stake + server_mult), brief outline animation
    - On 409 (stale_quote | multiplier_drift): clear PENDING, surface "price moved, tap again"
    - On 429: clear PENDING, surface "too fast"

GET /positions/:id
  Returns: PositionRow with full settlement data if settled.

POST /positions/:id/void   (admin only — manual recovery for oracle-data-gap voids)
```

### 3.4 Leaderboard & social

```
GET /leaderboard?period=24h&asset=ALL&limit=20
  Returns: { rows: [{rank, display_name, points, win_rate, biggest_mult}], your_rank }

GET /share/render?position_id=<id>
  Returns: PNG (1200x630), generated via Satori or external service
```

### 3.5 Quests

```
GET /quests/today
  Returns: { quests: [{id, code, progress, target, reward}] }

POST /quests/:id/claim
  Returns: { reward_points, new_balance }
```

### 3.6 Oracle data (read-through to aggregator)

```
WS /ticks
  → forwarded from oracle-aggregator (see ORACLE_SPEC.md §5)
  Subscribe: { op: 'subscribe', assets: ['ETH', 'BTC', 'SOL'] }
  Receives: AggregatedTick { asset, price, median, sources_used, timestamp_ms, seq, vol_annualized }
```

Frontend connects directly to oracle-aggregator's public WS; doesn't go through API for tick stream.

### 3.7 Verifiability (Phase 3+)

```
GET /verify/snapshot/:week
  Returns: {
    merkle_root,
    on_chain_tx,
    proof_template_url      -- guide for users to construct their own proof
  }
```

v1 does not expose per-settlement on-chain verification; weekly merkle snapshots are the integrity anchor. Per-settlement attestation deferred to Phase 5 (real-money mode).

---

## 4. Auth & session model

### 4.1 zkLogin (Web / PWA)

1. Client uses `@mysten/enoki` SDK to perform zkLogin with Google / Apple / Twitter / Facebook
2. Result: ephemeral Sui keypair + zk proof of JWT validity
3. Client posts JWT + proof to `POST /auth/zklogin`
4. API verifies:
   - JWT signature against OIDC issuer's JWKS
   - zk proof against Sui's verifier (using `@mysten/sui` SDK)
   - JWT `iss` and `sub` claims (uniqueness)
5. API creates/looks-up `accounts` row via `external_id = "zk:{iss-shortened}:{sub}"`
6. API issues session token (opaque 32-byte random, base64); stored in Redis with TTL 12h

### 4.2 Session token lifecycle

- Stored in Redis: `session:<token>` → `{account_id, issued_at, expires_at, ip_hash}`
- Renewed on every API call that hits API
- Revoked: client posts `DELETE /auth/session` (removes Redis key)
- Hard-flagged accounts: session is silently kept alive but bot-detection middleware adds X-Flag-State header

### 4.3 Multi-provider accounts

A user signing in via Google zkLogin and again via Twitter zkLogin gets **two separate accounts** in v1 (one per provider `sub`). No cross-provider linking in v1. Account linking would be considered later.

---

## 5. Critical data flows

### 5.1 First-time visitor → first tap

```
 1. User lands on tick.xyz
 2. Client renders splash + grid skeleton
 3. Client subscribes to oracle WS (anonymous read OK for live price line)
 4. User clicks "Sign in with Google"
 5. Enoki returns zkLogin proof
 6. POST /auth/zklogin → returns session_token + AccountSummary
 7. Client loads /me, sees balance=10,000 (signup bonus)
 8. User selects ETH, sees grid with live price + multipliers recomputing at 10 Hz
 9. User taps cell (3812.0, 3812.5) at the +5s column (t_open=now+5s, t_close=now+10s)
10. Client: snapshot displayed mult (5.2x) + current oracle_seq (914_352);
    render PENDING badge with "$1 / —"; POST /positions
11. API: rate-limit OK; replays aggregator state at seq 914_352;
    recomputes mult (5.18x — within 3% drift); accepts;
    BEGIN tx → INSERT positions, INSERT points_ledger TAP_STAKE,
    UPDATE accounts balance -100 → 9900; COMMIT;
    returns { position_id, multiplier_locked: 5.18, expected_payout: 518 }
12. Client replaces PENDING with LOCKED badge "$1 / 5.18x", brief outline animation;
    cell's displayed multiplier continues updating at 10 Hz for visualization
13. 5–10 seconds later, oracle tick enters [3812.0, 3812.5] during the cell's window
14. Settlement worker: in-memory cache finds this position; settle_win() commits
    one tx (INSERT settlements, UPDATE positions WON, INSERT ledger TAP_PAYOUT 518,
    UPDATE accounts balance +518)
15. WS pushes 'position_settled' event to user
16. Client renders "WIN" animation, balance ticks up, streak → 1
```

### 5.2 Settlement worker tick loop (the hot path)

The worker holds an **in-memory cache of OPEN positions** keyed by `(asset, t_open_ms)`, hydrated at startup from Postgres and kept current via Postgres `LISTEN`/`NOTIFY` on insert. The hot loop never round-trips to Postgres for the candidate-scan; only for the settle write.

```rust
// backend/settlement-worker/src/main.rs (sketch)

loop {
    let tick: AggregatedTick = ws_receiver.recv().await?;

    // 1. Scan in-memory open-positions cache for this asset whose monitoring
    //    window contains the tick timestamp. O(open_positions_per_asset),
    //    typically <100; no Postgres I/O.
    let candidates = open_cache.active_positions(
        tick.asset, tick.timestamp_ms
    );

    // 2. Detect touches; credit immediately
    for pos in candidates {
        if tick.price >= pos.strike_lo && tick.price <= pos.strike_hi {
            settle_win(pos, &tick).await?;   // see below; idempotent
        }
    }

    // 3. Expire untouched positions whose t_close_ms is now past.
    //    The cache evicts on settle/expire; periodically we sweep stragglers.
    for pos in open_cache.expired_at(tick.timestamp_ms) {
        settle_loss(pos).await?;
    }
}

/// settle_win: ONE Postgres transaction. The settlements row is the canary —
/// if it inserts, the position UPDATE and points_ledger INSERT must succeed too,
/// or the entire credit is rolled back. UNIQUE(position_id) on settlements
/// makes retries idempotent.
async fn settle_win(pos: Position, tick: &AggregatedTick) -> Result<()> {
    let payout = pos.stake_points * pos.multiplier_at_tap;   // locked at tap

    let mut tx = pool.begin().await?;

    let rows = sqlx::query!(
        "INSERT INTO settlements
            (position_id, account_id, outcome, points_delta,
             oracle_price, settled_at_ms, multiplier_used,
             streak_at_credit, streak_bonus)
         VALUES ($1, $2, 'W', $3, $4, $5, $6, $7, $8)
         ON CONFLICT (position_id) DO NOTHING
         RETURNING id",
        pos.id, pos.account_id, payout,
        tick.price, tick.timestamp_ms, pos.multiplier_at_tap,
        streak_at_credit, streak_bonus
    ).fetch_optional(&mut *tx).await?;

    if rows.is_none() {
        // Position already settled by a previous worker — retry safe no-op.
        tx.rollback().await?;
        return Ok(());
    }

    sqlx::query!(
        "UPDATE positions
            SET status='WON', settled_at_ms=$2
          WHERE id=$1 AND status='OPEN'",
        pos.id, tick.timestamp_ms
    ).execute(&mut *tx).await?;

    sqlx::query!(
        "INSERT INTO points_ledger (account_id, kind, delta, ref_id, created_at_ms)
         VALUES ($1, 'TAP_PAYOUT', $2, $3, $4)",
        pos.account_id, payout, pos.id, tick.timestamp_ms
    ).execute(&mut *tx).await?;

    sqlx::query!(
        "UPDATE accounts
            SET balance = balance + $2,
                lifetime_points_won = lifetime_points_won + $2
          WHERE id=$1",
        pos.account_id, payout
    ).execute(&mut *tx).await?;

    tx.commit().await?;

    open_cache.remove(pos.id);
    push_event(pos.account_id, PositionSettled { position_id: pos.id, outcome: 'W', payout }).await;
    Ok(())
}
```

`settle_loss` is structurally identical but writes outcome `'L'` and `points_delta = 0` (stake was already debited at `POST /positions`).

**Why the transaction matters.** `INSERT settlements ON CONFLICT DO NOTHING RETURNING id` is the gate. If the row was already there (worker restart, retry, dup oracle tick), nothing else runs. If the row inserts, the position UPDATE and ledger INSERT *must* commit together — otherwise we'd have a settlement row pointing to a still-OPEN position, which the next iteration would try to settle again, and the `ON CONFLICT` would block further writes but the position would never close.

**Idempotency** is enforced by `UNIQUE(position_id)` on `settlements`. The `RETURNING id` pattern lets us distinguish "fresh settle" from "already settled" without a separate SELECT.

**Throughput target**: 20 ticks/sec × 3 assets = 60 ticks/sec; ~100 open positions/asset at MVP scale = ~6K position-checks/sec, all in-memory. The Postgres write rate is bounded by settle events (≪ tick rate). No per-tick Postgres scan on the hot path.

### 5.3 Weekly anchor publish

```
Every Monday 00:00 UTC, anchor-publisher runs:

1. SNAPSHOT accounts:
   SELECT id, lifetime_points_won, balance FROM accounts WHERE last_active_ms > (now() - 30d);

2. Build merkle tree:
   leaf_i = SHA256( id || lifetime_points_won || balance )
   Internal nodes are sorted-pair-hashed.

3. Construct PTB:
   tick_anchor::publish_root(
     &mut anchor,
     week_idx,
     merkle_root: bytes,
     total_users: N,
     total_points: SUM(balance),
     &clock,
     &admin_cap
   )

4. Sign with publisher key (HSM); submit tx.

5. On confirm:
   INSERT INTO snapshots (week_idx, merkle_root, ..., on_chain_tx);
   Cache proofs per user for /verify/snapshot/:week queries.

6. Notify Discord ops channel with tx digest + summary.
```

Snapshot frequency: weekly in v1; can move to daily in Phase 2 if storage budget allows.

---

## 6. Move package — `tick_anchor`

```move
module tick_anchor::anchor;

use sui::clock::Clock;
use sui::event;

/// Shared object — one Anchor exists.
public struct Anchor has key {
    id: UID,
    admin: address,
    snapshots: Table<u64, Snapshot>,   // week_idx → snapshot
    current_week: u64
}

public struct Snapshot has store, drop, copy {
    merkle_root: vector<u8>,
    total_users: u64,
    total_points: u128,
    published_at_ms: u64,
}

public struct AdminCap has key, store {
    id: UID,
}

public struct RootPublished has copy, drop {
    week: u64,
    merkle_root: vector<u8>,
    total_users: u64,
    total_points: u128,
    timestamp_ms: u64,
}

/// Admin-only: publish a new weekly snapshot
public entry fun publish_root(
    anchor: &mut Anchor,
    _admin_cap: &AdminCap,
    week: u64,
    merkle_root: vector<u8>,
    total_users: u64,
    total_points: u128,
    clock: &Clock,
) { ... }

/// Public: verify membership via merkle proof
public fun verify_membership(
    anchor: &Anchor,
    week: u64,
    leaf: vector<u8>,                // account_id || lifetime_points || balance hashed
    proof: vector<vector<u8>>,
): bool { ... }
```

Total LOC target: ~250. No funds custodied. Audit-friendly.

Mainnet deployment: deferred to Phase 3 (first snapshot needed at Phase 3 launch).

---

## 7. Deployment topology

### 7.1 Environments

| Env | Sui | Pyth | Domain | Hosting |
|---|---|---|---|---|
| local-dev | testnet | hermes (mainnet) | localhost | docker compose |
| staging | testnet | hermes (mainnet) | staging.tick.xyz | Fly.io single region (Frankfurt) |
| production v1 | mainnet | hermes (mainnet) | tick.xyz | Fly.io single region (Frankfurt) |
| production v2 (Phase 2+) | mainnet | hermes (mainnet) | tick.xyz | Fly.io multi-region (FRA + SIN) |

The Pyth price feed is **mainnet in every environment** — decoupled from the Sui deployment network. The aggregator reads `hermes.pyth.network` (Stable channel) regardless of which Sui network the contracts target; the Beta/testnet Hermes channel was dropped (see ORACLE_SPEC §2).

### 7.2 Production service counts (v1)

| Service | Replicas | Notes |
|---|---|---|
| `apps/web` | Vercel edge (default) | Stateless |
| `backend/api` | 3 instances | Behind LB; sticky to Postgres primary region |
| `backend/oracle-aggregator` | 2 instances | Leader-elected via Postgres advisory lock |
| `backend/settlement-worker` | 2 instances | Leader-elected; only leader processes |
| `backend/anchor-publisher` | 1 instance | Cron-style; awakens weekly |
| Postgres | 1 primary + 1 read-replica | Managed via Fly Postgres or Neon |
| Redis | 1 cluster (3 nodes) | Upstash or Fly Redis |

### 7.3 Leader election

Postgres advisory lock pattern:

```sql
SELECT pg_try_advisory_lock(<service_lock_id>);
```

Standby polls every 1s; promotes to leader if lock released. Leader holds the lock until process death or graceful handoff. Standby has hot-cached state (subscribes to aggregator WS even when not emitting, so promotion is ≤ 2s).

### 7.4 Secrets management

- Pyth API keys: none required for Hermes (public)
- CEX WS: public endpoints, no auth
- Postgres / Redis credentials: Fly secrets
- Anchor publisher signer key: AWS KMS / GCP KMS HSM-backed
- zkLogin / Enoki API keys: Fly secret

---

## 8. Observability

### 8.1 Metrics (Prometheus + Grafana)

Per-service histograms:
- `api_request_duration_ms` (handler, status code)
- `settlement_latency_ms` (oracle tick → DB credit)
- `aggregator_emit_latency_ms` (source tick → emit)
- `oracle_source_freshness_ms` (per source, age)
- `position_settle_outcome_total{outcome}` (counter)
- `points_distributed_total{kind}` (counter)
- `flag_triggered_total{flag_code, severity}` (counter)

### 8.2 Logs (Loki + structured JSON)

Every API request logs: `request_id, account_id (if authed), endpoint, status, latency_ms, ip_hash`.
Every settlement logs: `position_id, outcome, oracle_price, points_delta`.
Every flag triggered logs: full evidence JSON.

### 8.3 Traces (OpenTelemetry)

End-to-end trace from `POST /positions` through Redis → Postgres → response. Sampled at 1% in prod, 100% in staging.

### 8.4 Alerts (PagerDuty)

| Alert | Threshold | Severity |
|---|---|---|
| Aggregator emit gap | > 5s | P1 |
| Aggregator DEGRADED status | > 5 min in 1 hr | P1 |
| Settlement worker queue depth | > 1000 backlog | P2 |
| API error rate | > 1% over 5 min | P2 |
| Postgres replica lag | > 10s | P2 |
| Hard flags triggered | > 10/hr | P3 (Discord, no page) |
| Anchor publish failed | weekly cron | P1 (must publish; can retry) |
| Sui chain outage | (chainstack monitor) | P3 (game continues; snapshot delayed) |

---

## 9. Failure modes & recovery

| Failure | Detection | Recovery |
|---|---|---|
| API instance crash | LB health check | Auto-restart; sticky-less; sessions in Redis survive |
| Settlement worker crash | Leader heartbeat | Standby promotes ≤ 2s; resumes from last processed `aggregator_seq` |
| Aggregator leader crash | Advisory-lock release | Standby promotes ≤ 2s; clients reconnect to standby WS |
| Postgres primary failure | Managed failover | Replica promoted ≤ 30s; API returns 503 during failover |
| Redis cluster failure | API health check fails | Sessions and rate-limit buckets lost; API degrades to "auth required" + token-bucket bypass for ~5 min while clients re-auth |
| Pyth Hermes down | Aggregator marks Pyth stale | Continue with CEX-only median if ≥ 2 CEX sources active |
| All CEX feeds down | Aggregator emits DEGRADED | Clients pause taps; existing OPEN positions continue with last-known price; if window expires with no data → void position, refund stake |
| Sui chain outage | Anchor publisher tx fails | Retry next slot; if outage > 24h, postpone snapshot by 1 week |
| Settlement-worker bug mistakenly credits | Reconciliation job | Daily ledger reconciliation: SUM(deltas) == balance; alerts on drift |

### 9.1 Position void policy

When does a position become VOIDED (refund stake, no payout)?

1. **Oracle gap covers full window**: `t_open_ms` to `t_close_ms` has zero aggregator ticks
2. **DEGRADED status covers full window**: aggregator paused throughout
3. **Admin recovery**: rare; for incident remediation

Void: refund stake to ledger, status → 'VOIDED', user notified via /me/events.

---

## 10. Security boundaries

| Boundary | Layer | Defense |
|---|---|---|
| Untrusted client → API | HTTPS + session token | Token verified per request; rate-limited per account |
| API → Postgres | Network ACL | Postgres only reachable from API + worker subnets |
| API → Redis | Network ACL | Same |
| Aggregator → external WS | Outbound only | No inbound; WS clients can't reach aggregator |
| Anchor publisher → Sui | Outbound only | Signer key in HSM; admin cap held off-chain |
| Admin actions on Anchor | AdminCap object | Held by multisig (2-of-3) Phase 3+ |
| User identity uniqueness | `external_id` UNIQUE | Re-sign-in returns same account |
| Anti-cheat | Background job | Per-account scoring; soft/hard flags |

**Threat model summary**:
- We assume user clients are hostile (browsers can be modified, RNs can be reverse-engineered).
- We assume CEX feeds may be down or briefly manipulated; defense = median + Pyth redundancy.
- We assume Pyth may have publisher issues; defense = confidence-gate + multi-source.
- We assume some accounts will sybil-farm; defense = behavioral flagging + retroactive airdrop weighting.
- We do NOT assume Sui is always up (Jan 2026 outage precedent); game runs without Sui.

---

## 11. Development workflow

### 11.1 Local-dev quickstart

Local dev is driven by the repo's worktree-safe runner (`cmk-worktree-dev-env` skill — see project CLAUDE.md). Do **not** hand-roll `docker compose` or hard-code ports; ports come from `scripts/worktree-env.sh` so multiple worktrees coexist.

```bash
# 1. From the worktree root (once per worktree)
./scripts/init-worktree-dev.sh                # syncs .local/.env and per-service envs

# 2a. Interactive (human session)
mprocs --config mprocs.yaml                   # boots postgres, redis, aggregator, worker, api, web

# 2b. Headless (agent / CI session)
./scripts/start-headless.sh                   # detached; ./scripts/logs.sh <service> to tail
```

When adding a new service that binds a port, edit `scripts/worktree-env.sh`, `scripts/sync-service-envs.sh`, `scripts/ensure-worktree-coherence.sh`, and `mprocs.yaml` / `start-headless.sh` together (the four must move in one PR). The crates themselves read ports from env — never hardcode.

### 11.2 Test data

`scripts/seed.ts` creates:
- 100 test accounts (mix of tiers)
- 10K test positions across past 7 days
- Synthetic oracle history for backtest

### 11.3 Branch model

- `main` = production (auto-deploys to staging; manual promote to prod)
- `feat/*` = feature branches (PR to main)
- `fix/*` = bug fixes
- Move packages: separate repo or `move/` subdir with own Move.toml; redeployed only via manual ops script

---

## 12. Open questions & deferred

| # | Question | Decision needed by | Owner |
|---|---|---|---|
| S2 | Postgres or PlanetScale (MySQL) — Postgres preferred but check pricing | Week 1 | Eng + Ops |
| S3 | Redis: Upstash vs Fly Redis vs self-hosted | Week 2 | Eng + Ops |
| S4 | Settlement worker: single-leader or sharded by asset? | Phase 2 if needed | Eng |
| S5 | Share card renderer: in-process Satori vs external Cloudflare Worker? | Week 4 | Frontend |
| S6 | Multi-region: when do we add SIN region? At what WAT? | Phase 2 | Ops + Product |
| S7 | Anchor publisher: standalone service or cron in api process? | Phase 3 | Eng |
| S8 | Audit: external Move audit for tick_anchor (Phase 3) — which firm? | Phase 2 | Product + Eng |
| ~~S9~~ | _Resolved: credit 10,000-point signup bonus in the same Postgres transaction as `POST /auth/zklogin` account creation — one `SIGNUP` ledger row + balance update, no separate claim flow. Matches §5.1 step 7 (client sees `balance=10,000` immediately on first `/me`)._ | — | — |
| S10 | Real-money mode (Phase 5+): full re-architecture spec — separate doc | Phase 4 | Architecture |

---

## 13. References

- PRD §12 (Technical Architecture) — high-level diagram
- `MATH_SPEC.md` §5 — pricing engine API
- `ORACLE_SPEC.md` §5 — client WS protocol
- Sui zkLogin: https://docs.sui.io/concepts/cryptography/zklogin
- Enoki SDK: https://docs.enoki.mysten.app
- Postgres advisory lock pattern: https://www.postgresql.org/docs/current/explicit-locking.html#ADVISORY-LOCKS
- Sui Move event patterns: https://docs.sui.io/guides/developer/sui-101/using-events

