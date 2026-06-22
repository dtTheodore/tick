# ADR-0009 — Tick API cross-service contracts

**Date:** 2026-05-27
**Status:** Accepted
**Workstream:** Tick (tap-trading)
**Supersedes:** —
**Superseded by:** —

## Context

Plans B/C/D/E implement the four backend services for the Tick game.
Three contracts cut across them and are not pinned by any of the
existing specs:

1. **Account identity.** zkLogin and session tokens are deferred
   (`PRD.md` MVP, but explicitly out of scope for the core-loop
   plans). Tap-commit needs an account to debit, and we need a
   stand-in that does not get in the way of swapping in real auth
   later.
2. **Table ownership.** The API and the settlement worker both write
   to `positions`, `points_ledger`, and `accounts`. Without an
   explicit split, two services racing for the same row will
   deadlock or silently corrupt balances.
3. **Drift tolerance, rate limit, refund kind, NOTIFY contract.**
   Each of these is a single number or string that becomes part of
   the public surface the moment anyone writes code against it.

`SYSTEM_DESIGN §3.3` describes the tap-commit pipeline but leaves
the numbers and the ledger-kind taxonomy unspecified. `MATH_SPEC §4.3`
fixes the lock-at-tap invariant but says nothing about how the
invariant is enforced in code.

This ADR also reconciles assumptions in earlier drafts against the
actual Plan A schema (`migrations/20260523120000_create_tick_schema.sql`),
which differs from the plan's File map in three load-bearing ways
(noted inline below).

## Decision

### 1. Account identity — `X-Account-Id` dev stand-in

API requests carry `X-Account-Id: <text>` (any non-empty UTF-8, ≤128
chars). The header value maps to `accounts.external_id` (the schema
already has this column as `TEXT NOT NULL UNIQUE`). The internal
`accounts.id` is `BIGSERIAL` and is the value every other table
references — handlers operate on `account_id: i64`, never on the
header value.

Middleware behaviour:
- Missing header → `401 missing_account_id`
- Empty / >128 chars → `400 invalid_account_id`
- Unknown `external_id`: lazy-create the row in a single transaction:
  - `INSERT INTO accounts (external_id, zklogin_sub, zklogin_iss, balance, lifetime_points_won, signup_bonus_at_ms, created_at_ms, last_active_ms) VALUES ($1, 'dev', 'dev', 10000, 0, now_ms(), now_ms(), now_ms()) ON CONFLICT (external_id) DO NOTHING RETURNING id`
  - If the INSERT created the row: `INSERT INTO points_ledger (account_id, kind, delta, ref_id, created_at_ms) VALUES ($id, 'SIGNUP', 10000, NULL, now_ms())`
- Attaches `AccountCtx { id: i64, external_id: String }` to request
  extensions. Handlers read `Extension<AccountCtx>`; downstream code
  never sees the header.

`zklogin_sub` / `zklogin_iss` are populated with `'dev'` / `'dev'`
sentinels because the schema declares them `NOT NULL`. When real
auth ships, the JWT verifier replaces the middleware and writes
real values; downstream handlers do not change because they read
`AccountCtx.id`, not the OIDC fields.

### 2. Table ownership

| Table             | Writer            | Operations                                              |
|-------------------|-------------------|---------------------------------------------------------|
| `accounts`        | API               | INSERT (lazy-create), UPDATE balance & last_active_ms   |
| `accounts.balance`| **Both**          | API debits on tap; worker credits on win, refunds void  |
| `positions`       | API (INSERT) / Worker (UPDATE status) | INSERT once at tap; UPDATE status OPEN→W/L/V at settle |
| `points_ledger`   | **Both**          | API writes SIGNUP, TAP_STAKE; worker writes TAP_PAYOUT, TAP_REFUND |
| `settlements`     | Worker only       | INSERT once per position, idempotent on UNIQUE(position_id) |
| `streaks`         | Worker (later)    | Deferred; SELECT-only in v1                              |
| `daily_quests`    | —                 | Deferred                                                 |
| `snapshots`       | —                 | Deferred                                                 |
| `flags`           | —                 | Deferred                                                 |

`accounts.balance` is the only field both services write. Writes
serialize on the row via `SELECT ... FOR UPDATE` inside the
transaction. The API's transaction is short (debit + insert
position + insert ledger); the worker's transaction is short
(insert settlement + update position + insert ledger + credit). Both
fit comfortably inside a single round-trip; no risk of long-held
locks.

### 3. Schema deltas required for tap-commit

Plan A's actual migration (`migrations/20260523120000_create_tick_schema.sql`)
is staged on `feat/tap-trading` but not yet merged. It is missing
three columns the API needs:

- `positions.oracle_seq_at_tap BIGINT NOT NULL` — the aggregator seq
  the client claims to have based its multiplier on.
- `positions.oracle_run_id_at_tap BIGINT NOT NULL` — paired with the
  seq per ADR-0008 §4.
- `positions.client_request_id UUID NOT NULL` with a partial UNIQUE
  index `UNIQUE (account_id, client_request_id)` — the idempotency
  key (see §4).

Because Plan A's migration is staged but not yet committed on
`feat/tap-trading`, we **amend the genesis migration in place**
rather than ship a follow-up. This is the CLAUDE.md-canonical
answer: one migration per logical change, and the foundation schema
is a single logical change. A follow-up migration would mean two
migrations for the same logical concept (foundation) only because
the work was split across two plans, which is a workflow artifact,
not a schema-history fact.

**Ownership**: Plan E owns the amendment as its first task. The
commit subject is `feat(tick-db): add positions oracle and idempotency cols`
and it lands on top of Plan A's chain before any Plan E API code
references the new columns. Plan E ships no separate `_add_columns`
migration file.

### 4. Tap-commit pipeline (POST /v1/positions)

Request body (JSON):
```
{ "client_request_id": "9b1d…",
  "asset": "BTC", "strike_lo": 50000.0, "strike_hi": 50100.0,
  "t_open_ms": 1748345670000, "t_close_ms": 1748345675000,
  "stake_points": 100, "client_multiplier": 2.5,
  "oracle_seq_at_tap": 12345, "oracle_run_id_at_tap": 1701234567890,
  "client_fingerprint": "..." }
```

`stake_points` must be one of `{ 50, 100, 500, 1000 }` (v1 fixed
stake tiers per `PRD.md` line 23 — Tier-1 menu). Deviating values
→ `400 invalid_stake`. Higher tiers unlock larger menus; v1 is
Tier 1 only.

`client_request_id` is a UUID generated client-side per tap attempt.
On retry, the client MUST reuse the same value. The database has
`UNIQUE (account_id, client_request_id)` on `positions`; a duplicate
INSERT is the dedup mechanism.

Pipeline (in order, fail-fast on each):
1. Rate-limit check (see §6). 429 with `Retry-After` on exceed.
2. Idempotency check: `SELECT id, multiplier_at_tap, status, t_close_ms FROM positions WHERE account_id = $1 AND client_request_id = $2`.
   - If row exists → return `200 OK` with the same response body
     the original request returned. **Do not** re-debit, re-validate,
     or re-NOTIFY. This is the dedup path.
3. Cell validation: known asset, `t_close_ms - t_open_ms == 5000`,
   `t_open_ms % 5000 == 0`, `now_ms() + 1000 < t_close_ms` (lock
   window). 400 with reason on fail.
4. Replay quote from aggregator: `GET /ring/:asset/:seq?run_id=N`.
   - 410 or 409 → 422 `stale_quote`
   - 200 → parse `OracleTick`
5. Server recompute: build `OracleState` from the tick, call
   `compute_multiplier(&cell, &oracle_state, &PricingConfig::default())`.
6. Drift check: `|server - client| / server > 0.03` → 422
   `drift_exceeded` with server multiplier in the body. **3% is the
   v1 tolerance** — wide enough to absorb network skew between the
   client's last tick and the actual replayed tick, tight enough to
   reject manual multiplier inflation.
7. Balance check: `SELECT balance FROM accounts WHERE id = $1 FOR UPDATE`.
   < stake → 422 `insufficient_balance`, ROLLBACK.
8. Atomic commit (single transaction):
   - INSERT positions (status='OPEN', multiplier_at_tap = **server's value**, oracle_seq_at_tap, oracle_run_id_at_tap, client_request_id, ...) — `ON CONFLICT (account_id, client_request_id) DO NOTHING RETURNING id` to handle concurrent retries within the same transaction window; if no row returned, GOTO idempotency lookup at step 2.
   - INSERT points_ledger (kind='TAP_STAKE', delta = -stake, ref_id = position_id)
   - UPDATE accounts SET balance = balance - stake, last_active_ms = now_ms()
   - `NOTIFY tap_new_position, '<position_id>'` (see §5)
   - COMMIT
9. Response 201: `{ position_id, multiplier_at_tap, status, t_close_ms }`.

The locked invariant: the value written to
`positions.multiplier_at_tap` is the **server's** recomputed value,
never the client's claim. The client's value is used only for the
drift comparison. (`MATH_SPEC §4.3`.)

### 5. `tap_new_position` NOTIFY channel

After every successful tap-commit transaction, the API issues
`NOTIFY tap_new_position, '<position_id_as_decimal_string>'` as the
final statement of the transaction. The settlement worker
`LISTEN tap_new_position`; on payload it fetches the row and inserts
into its in-memory cache.

Postgres LISTEN/NOTIFY has no application-level ack. The worker's
durability guarantee comes from re-hydration, not from acking
NOTIFYs:

- On normal operation, the worker receives NOTIFYs in real-time and
  inserts the position into the in-memory cache within
  single-digit milliseconds of the API's COMMIT.
- If the worker's LISTEN connection drops, every NOTIFY emitted
  while it was disconnected is **lost forever** — Postgres does not
  buffer.
- The worker MUST therefore **trigger an immediate re-hydration on
  every LISTEN reconnect** (`SELECT * FROM positions WHERE status = 'OPEN'`),
  not wait for the next periodic scan. The periodic scan exists as a
  belt-and-suspenders safety net (every 30 s, e.g. for the case where
  the LISTEN connection is technically up but stuck) — it is not the
  primary recovery mechanism.

Missing a position for the duration of one cell (≤5 s) means the
position expires LOST even if the oracle would have touched it during
that window. The reconnect-rehydration policy keeps the gap to one
re-hydration round-trip in the worst case — well under the cell
duration.

(Trade-off: a synchronous handshake would push the API's commit
latency up by one round-trip. The async NOTIFY + immediate
reconnect re-hydration is faster on the happy path and correct on
the failure path because re-hydration is idempotent.)

### 6. Rate limit

**10 taps/sec per `accounts.id`, burst 10, refill linear.** Redis
token bucket, key `tap:rl:{account_id}`, TTL 2 s, atomic update via
Lua script. 429 with `Retry-After: 1` on exceed.

Numbers chosen so that a normal player (tapping every 5 s cell) is
two orders of magnitude under the ceiling, but a scripted attacker
hits the bucket within ~100 ms. Tune after we have real abuse data.

### 7. Void refund — new `TAP_REFUND` ledger kind

When the worker voids a position (its monitoring window fully
overlapped an oracle gap), it writes:
- `INSERT settlements (outcome='V', points_delta = stake, multiplier_used = position.multiplier_at_tap, streak_at_credit = 0, streak_bonus = 1.0, ...)` 
- `INSERT points_ledger (kind='TAP_REFUND', delta = +stake, ref_id = position_id)`

`TAP_REFUND` is added to the kind taxonomy (no migration change —
`points_ledger.kind` is `VARCHAR(24)` with no enum constraint). We
deliberately do not overload `TAP_PAYOUT` with a sign flip because
audit queries (`WHERE kind = 'TAP_PAYOUT'`) must continue to mean
"the player won this one"; refunds are a different operational
event.

`settlements.streak_bonus` is `NOT NULL NUMERIC(5, 3)` — the worker
must supply a value even though streak logic is deferred. Default:
`1.000` (no bonus). When streaks ship, the value reflects the
actual bonus applied; old rows stay at 1.0 and are correct.

### 8. Settle-time payout formula

```
payout_points = floor(stake_points * multiplier_at_tap)
```

`multiplier_at_tap` is `NUMERIC(10, 4)` and is the value locked at
tap. `floor` rounds toward zero on the points_delta written to the
ledger. (The 4-digit fractional precision on multiplier means the
maximum rounding error per settlement is < 0.5 points; ignorable.)

## Consequences

- The API is the only writer of `accounts` and `positions` INSERTs.
  The worker is the only writer of `settlements` and the only one
  that flips `positions.status` away from `OPEN`. No cross-service
  contention beyond `accounts.balance` updates, which serialize on
  the row.
- Lazy account creation is a footgun if production runs without
  re-pointing the middleware: any unknown header creates a free
  10K-point account. Swap-in for real auth must delete the
  lazy-create branch entirely (not just gate it on env), so the
  middleware fails closed.
- The schema additions (`oracle_seq_at_tap`, `oracle_run_id_at_tap`,
  `client_request_id`) ship in Plan E by **amending the genesis
  migration in place** (per §3), not as a follow-up file. This is
  valid only because Plan A's migration is still unmerged on
  `feat/tap-trading`. Once the genesis migration has been applied in
  any environment, that window closes and every further schema change
  MUST be a new migration file — one migration per logical change.
- `TAP_REFUND` is a new ledger kind. Any analytics that count
  payouts must filter by `kind IN ('TAP_PAYOUT', 'TAP_REFUND')` if
  they want "credits to the player" and `kind = 'TAP_PAYOUT'` if
  they want "wins".

## Forecast

- Real auth (zkLogin or session tokens): swap the middleware. The
  `AccountCtx` extension contract is unchanged.
- Stake tiers: extend the allowed set in cell validation. No schema
  or wire change.
- Streak bonus at settle: worker reads the streak row before
  insertion, computes `payout_points = floor(stake * multiplier *
  streak_bonus)`, writes `streak_bonus` field. Already wired in
  schema.
- Per-asset rate limits or burst budgets: change Lua script,
  unchanged middleware shape.
- Voided positions accounting: if regulators or auditors care about
  void vs. loss-by-no-touch, the distinction is already in
  `settlements.outcome` (`V` vs `L`).
