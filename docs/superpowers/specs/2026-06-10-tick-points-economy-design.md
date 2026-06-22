# Tick Points Economy & Leaderboard — Design Spec

**Date:** 2026-06-10
**Status:** Draft for review
**Scope:** Tick's self-contained points economy + leaderboard. No coupling to
the Dopamint central `points`/`leaderboards` services (ADR-0013 separation
upheld). Identity (JWT/JWKS) is the only shared platform contract.

## 1. Goal & shape

Retention-first economy on a **single currency, read two ways**:

- **Spendable balance** — what you wager and run down. The dopamine faucet:
  generous, refillable, intentionally a little loose so the loop never dies.
- **Lifetime net P&L** — `Σ(realized winnings) − Σ(stakes)`. Scarce, drives
  tiers + leaderboard + the future airdrop. The faucet **cannot** inflate it.

The wager loop itself (stake → locked multiplier → forward-settled win/loss)
is the reward engine and is already built. This spec adds the economy *around*
it: the net read, the faucets/sink, streaks (wire the stub), and the
leaderboard.

**Deliberately out of scope** (cut for a polished core): streak-freeze,
near-miss FX, escalating/first-win login bonuses, quest progress UI, cosmetics,
and the daily-quest loop (the `daily_quests` table stays; the loop isn't built).
Bridging to platform points/leaderboards is deferred to the Phase-5 airdrop ADR.

## 2. Currency & ledger semantics

One balance. The invariant that makes "generous but not loose" work is a
**one-way valve**: minted points credit `balance` only; they never touch the
net read.

`points_ledger.kind` values:

| kind | balance | lifetime_net | Notes |
|---|---|---|---|
| `TAP_PAYOUT` | `+payout` | `+(payout − stake)` | win settle (worker) |
| (loss) | — | `−stake` | no ledger row; net maintained at settle |
| `TAP_REFUND` | `+stake` | — | void/gap refund; not a realized bet |
| `SIGNUP_BONUS` | `+10,000` | — | faucet (one-time) |
| `DAILY_LOGIN` | `+500` | — | faucet (flat, UTC-daily) |
| `COMEBACK` | `+250` | — | faucet (metered; §4) |

`lifetime_net` is **realized** — it moves only at settlement, never at tap-debit
(an open position is a pending bet, not a loss yet). This keeps the single
controlled writer (the settlement worker) authoritative for the scarce read.

## 3. Streaks (wire the existing stub)

`streaks` table and `settlements.{streak_at_credit, streak_bonus}` columns
exist; `settle_win` currently hardcodes `0 / 1.000`. Wire it:

- `settle_win`: lock the account's `streaks` row (`FOR UPDATE`),
  `current_streak += 1`, `max_streak = GREATEST(...)`.
- `settle_loss`: `current_streak = 0`.
- Bonus when `current_streak ≥ 5`: `streak_bonus = LEAST(1.5, 1 + 0.02·streak)`;
  `payout = floor(stake · multiplier_at_tap · streak_bonus)`. Record
  `streak_at_credit` and `streak_bonus` in the settlement row (audit trail).
- Streak is defined in **settlement order**, serialized per account by the row
  lock. The bonus is earned winnings, so it flows into `lifetime_net` and the
  daily board like any payout.

## 4. Faucets, sink, and bust handling

**Faucets** (all `balance`-only, per §2):
- Onboarding: 10,000 (existing `signup_bonus_at_ms` gate).
- Daily login: flat 500, once per UTC day.
- **Bust comeback:** when `balance < min_stake (50)`, grant 250 — **metered** to
  ≤3/UTC-day (count `COMEBACK` ledger rows for today before granting). Never
  fully stuck; never farmable into the net read.

**Sink:** the house margin (`(1 − margin)/P_touch`, margin ≈ 10%) baked into
every multiplier. This is the only macro sink. Tune **faucet income ≈ rake at
the median player** in shadow mode so the median stays *alive but pressured*
over a ~1h session (PRD R9). The near-cell floor (`1.50 + 0.025·τ`, ADR-0012) is
a deliberate loss-leader; calibrate `floor_a/floor_b` + `house_margin` so in-band
**realized EV** stays house-favorable (in-band taps are generous, not risk-free —
settlement is forward-looking, so every tap can lose).

## 5. Leaderboard (Option A — self-contained)

Two boards, both off Tick's own Postgres — **no Redis**. The settlement worker
is Postgres-only (sqlx, no redis dep); only the api carries Redis (rate-limit).
A Redis ZSET would add a new worker dependency *and* a dual-write that desyncs
if the worker crashes between the Postgres commit and the `ZINCRBY`. Instead
both aggregates are maintained **inside the settle transaction** (single
controlled writer, atomic — consistency for free).

- **Daily** — top 20 by **net P&L this UTC day**, from a maintained aggregate
  table `daily_net(account_id, utc_date, net)` upserted in the settle tx.
  Read: `… WHERE utc_date = $today ORDER BY net DESC LIMIT 20` (indexed).
- **All-time** — top 20 by `accounts.lifetime_net`, indexed
  `ORDER BY lifetime_net DESC LIMIT 20`.

Both boards use **net** (consistent with the lifetime read and PRD "net points
won"). Daily resets on **UTC-day** (each day is its own `daily_net` row) — a
true rolling-24h window can't be a single maintained counter; flag this one
deviation from PRD MVP-11's "24h" for sign-off.

**API:** `GET /v1/leaderboard?board=daily|alltime&limit=20`, JWT-gated (reuses
the platform identity Tick already integrates). Top-20 rows enriched with
`display_name`, win-rate, biggest multiplier via a bounded per-row lookup (≤20).
The api *may* cache the top-20 read in its existing Redis for ~60s to shed read
load — optional, reads only; the worker write path stays Postgres.

**Rebuild path:** if an aggregate is ever lost/corrupted, recompute from
`settlements ⨝ positions` — stake lives on `positions` and loss settlements
store `points_delta = 0`, so net is **not** derivable from `settlements` alone.

## 6. Read/write model (why this shape)

- **Writers:** stake debit (api, ≤10/s/user) and settlement (worker, one
  controlled writer per position). `lifetime_net` and `daily_net` are maintained
  **only at settle**, in the same transaction as the settlement insert — single
  controlled writer, atomic with the outcome.
- **Readers:** leaderboard top-20 (~60s refresh, many users, seconds-latency
  OK); `/v1/me` balance (per user, frequent).
- **Growth:** `points_ledger` / `positions` / `settlements` are append-only and
  unbounded; `daily_net` is bounded by `active_users × retained_days`.

Therefore: **do not** `SUM(...) GROUP BY` over the growing ledger on the live
leaderboard path. Both aggregates are maintained at write time —
`accounts.lifetime_net` as an indexed column and the `daily_net` row upserted
per settle — so top-N reads are index-only and independent of table size.

## 7. Schema changes

```sql
-- accounts: realized net P&L, signed, maintained at settle.
ALTER TABLE accounts ADD COLUMN lifetime_net BIGINT NOT NULL DEFAULT 0;
CREATE INDEX accounts_lifetime_net ON accounts (lifetime_net DESC);

-- daily board aggregate, upserted in the settle transaction.
CREATE TABLE daily_net (
  account_id BIGINT NOT NULL REFERENCES accounts(id),
  utc_date   DATE   NOT NULL,
  net        BIGINT NOT NULL DEFAULT 0,
  PRIMARY KEY (account_id, utc_date)
);
CREATE INDEX daily_net_rank ON daily_net (utc_date, net DESC);
```

`lifetime_points_won` (gross) stays as a secondary stat. Tiers move from gross →
`lifetime_net`; tier unlocks are **wider stake ranges only** (no cosmetics).

**Settlement write-pattern change (don't miss this):** `settle_loss` today
touches **no** `accounts` row. Under this design every settlement updates net:
- `settle_win`: `lifetime_net += (payout − stake)`; `daily_net.net += (payout − stake)`.
- `settle_loss`: `lifetime_net −= stake`; `daily_net.net −= stake`. *(new account write on every loss)*
- `settle_void`: no net change (refunded; not a realized bet).

`utc_date` is derived from the settling tick's timestamp (UTC). Upsert via
`INSERT … ON CONFLICT (account_id, utc_date) DO UPDATE SET net = daily_net.net + …`.

## 8. Anti-farming

1. **Net metric** is the primary scarcity lever — volume-grinding at the house
   edge drives net *down*, so it can't farm rank/airdrop.
2. **Floor EV calibration** (§4) keeps in-band cells house-favorable in realized
   terms.
3. **Sybil** (PRD R6): zkLogin one-identity-per-provider, behavioral clustering,
   sub-linear cluster weighting, recency-weighted airdrop.
4. **Throughput** (10 taps/s) bounds speed, not EV — it is not an EV control.

## 9. Build status (what's done vs new)

- **Done:** wager loop, forward settlement, `balance`/`lifetime_points_won`,
  `points_ledger`, `streaks`/`daily_quests`/`settlements` tables, `/v1/me`.
- **New:** `lifetime_net` column + `daily_net` table, both maintained in the
  settle tx (incl. the new `settle_loss` account write); streak-bonus wiring;
  `DAILY_LOGIN`/`COMEBACK` faucets; `GET /v1/leaderboard`; tier source → net;
  UI leaderboard view + hook (none exists today).

## 10. Open decisions

1. **Net P&L as the lifetime/airdrop metric** — assumed yes (the "not loose"
   lever). Confirm.
2. **Daily board = UTC-day reset** (vs literal rolling-24h). Recommend UTC-day.

(Both boards are net — §5 — consistent with PRD "net points won".)

**Conscious tradeoff to note:** net P&L over a house-edge game is
variance-dominated, so the all-time board ranks *outcomes* (luck) more than the
chart-reading *skill* the PRD sells. Acceptable for a dopamine board, but it is a
choice — a skill-leaning rank (e.g. win-rate or risk-adjusted) is a later option
if "skill matters" needs reinforcing on the board itself.
