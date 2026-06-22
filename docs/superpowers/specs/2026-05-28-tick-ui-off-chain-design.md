# Tick UI — Off-Chain Core Tap UX (v0)

**Date:** 2026-05-28
**Workstream:** Tick — frontend, off-chain phase only
**Branch:** `feat/tap-trading`
**Predecessor:** off-chain backend stack merged at `536b37c`
(tap-trading-api, oracle-aggregator, settlement-worker all booting
clean via `./scripts/start-headless.sh`; CORS layer present;
266/266 workspace tests pass)
**Gating references:**
- `games/tap-trading/docs/PRD.md` §3 (design language), §7 (pillars),
  §8 (MVP scope), §10 (MVP-02..MVP-08, MVP-10)
- `docs/decisions/0009-tick-api-cross-service-contracts.md` (request
  shapes, lock-at-tap contract, error taxonomy)
- `games/tap-trading/docs/MATH_SPEC.md` §4 (multiplier formula,
  τ-floor)
- `games/tap-trading/docs/ORACLE_SPEC.md` (tick shape, `(run_id, seq)`
  invariants)

## Purpose

Ship a minimum interactive surface that lets a tester *feel* the
tap-trading loop: see live ETH price moving against a clock-aligned
grid of future cells with live multipliers, tap a cell, watch it
settle to W/L/V within 5–10 seconds. This is a UX validation
prototype against the existing off-chain backend, not a shippable
consumer product. Single asset, single screen, single tester per
identity. Successful exit criterion: a stranger can land, see the
grid alive, tap a cell, see it light up or not, and form an opinion
about whether the game feels good — without any external help.

## In scope

- One screen, mobile-first viewport (360px minimum), responsive to
  desktop (≥1024px gets the 6-column layout).
- Single asset: **ETH/USD**.
- Live price line drawn left→right across the grid, with comet trail.
- Clock-aligned cell grid: 4 columns × 5s on mobile (20s look-ahead),
  6 × 5s on desktop. Y-axis is the globally-anchored ETH strike
  ladder at Δ$0.5 per row.
- Per-cell multipliers refreshed at 10 Hz client-side via a
  TypeScript port of `tap-trading-pricing-engine`.
- Tap → POST /v1/positions → PENDING → LOCKED → SETTLED lifecycle
  visible per cell.
- Optimistic balance: stake subtracts on PENDING, lands on
  server-confirmed balance after LOCKED.
- One position max per cell — tapping a cell that already has a
  position is a client-side no-op with a visual nudge.
- Many positions across cells in parallel — first-class.
- Header bar: current balance, current mid price, WS connection
  chip, asset name.
- History strip: last 10 settled positions inline at the bottom.
- Auto-identity: localStorage UUID → `X-Account-Id` header. Backend
  lazy-creates the account with a 10k signup bonus.

## Out of scope

- BTC, SOL, asset selector (PRD MVP-02 deferred).
- zkLogin / Google / Apple / Twitter sign-in (PRD MVP-01 deferred —
  belongs to the platform `identity-*` plan track).
- Daily leaderboard, streak tracker, daily quests (PRD MVP-11..14).
- Share cards, math explainer page (PRD MVP-13, MVP-18).
- PWA install, service worker, push notifications (PRD MVP-15).
- Tick-kun mascot, anime aesthetic polish beyond palette + fonts
  (PRD MVP-16 partial — palette yes, mascot no).
- Pro mode toggle, F1 chrome (PRD MVP-17).
- Multi-tap on the *same* cell (deliberately disallowed in this v0;
  PRD MVP-08 phrasing reads "tap multiple cells" not "tap a cell
  multiple times").
- Stake tier selector UI (stake is fixed at 100 points for v0; tier
  selection comes with the auth/economy work).
- Internationalization (English only).
- Sentry / analytics / A-B framework / CSP hardening.

## Authoritative references

- `games/tap-trading/docs/PRD.md` — product surface, MVP gating,
  visual design language.
- `games/tap-trading/docs/MATH_SPEC.md` — multiplier formula the TS
  port must replicate.
- `games/tap-trading/docs/ORACLE_SPEC.md` — oracle tick shape,
  `(run_id, seq)` semantics.
- `docs/decisions/0008-tick-oracle-wire-protocol.md` — WS message
  shape, ring buffer semantics, 120s retention.
- `docs/decisions/0009-tick-api-cross-service-contracts.md` —
  `POST /v1/positions` request/response, error taxonomy, idempotency.
- `games/tap-trading/backend/pricing-engine/src/` — the canonical
  Rust implementation that the TS port mirrors.
- `games/2048/ui/` — the FE conventions (React + Vite + Tailwind +
  shadcn) this project follows.
- `platform/backend/api/gateway/src/lib.rs` — the `CorsLayer` pattern
  the tap-api already adopted.

## Architecture

### Location

```
games/tap-trading/ui/
```

Mirrors `games/2048/ui/`. Adds a new worktree-aware env var
`TAP_UI_PORT` to `scripts/worktree-env.sh` and a `[sync] .env.local`
emit to `sync-service-envs.sh`. Must work alongside other worktrees
without port collision per the worktree contract.

### Stack

- React 19 + Vite 5 + TypeScript (strict).
- TailwindCSS 4 + shadcn/ui (Radix primitives) — matches 2048.
- Bun as package manager + script runner (per repo convention).
- `@tanstack/react-query` v5 — new dep. Owns server cache (`/v1/me`,
  `/v1/me/history`) and post-tap polling.
- React 19's `useSyncExternalStore` + a module-scoped pub/sub for
  the 20 Hz oracle tick stream. No global state library (no Jotai,
  no Zustand, no Redux).
- Fonts: Inter (UI) + Space Grotesk (numbers, multipliers) per PRD §3.
- Palette: per PRD §3 — asphalt black, hot pink, win green, loss red,
  info blue.

### Module layout

```
games/tap-trading/ui/
├── package.json
├── vite.config.ts          # port from TAP_UI_PORT (env), strictPort, cors
├── tsconfig.{app,node}.json
├── components.json         # shadcn config
├── index.html
├── src/
│   ├── main.tsx
│   ├── App.tsx             # single screen: HeaderBar / Grid / HistoryStrip
│   ├── index.css           # tailwind + fonts + global palette tokens
│   ├── lib/
│   │   ├── api.ts          # fetch wrappers: /v1/me, /v1/me/history,
│   │   │                   #   /v1/positions, /v1/positions/:id
│   │   ├── ws.ts           # /v1/stream subscription, reconnect, push to tickStore
│   │   ├── identity.ts     # localStorage guest UUID, X-Account-Id header injector
│   │   ├── tick-store.ts   # pub/sub: latest OracleTick + EWMA volatility state
│   │   ├── positions-store.ts # pub/sub: Map<cellKey, CellPosition>
│   │   ├── time.ts         # wall-clock alignment helpers (next 5s boundary)
│   │   ├── telemetry.ts    # console-only event logger
│   │   └── env.ts          # VITE_TAP_API_URL, VITE_TAP_API_WS_URL — fail-fast
│   ├── pricing/                  # TS port of tap-trading-pricing-engine
│   │   ├── constants.ts
│   │   ├── erfc.ts               # Abramowitz & Stegun 7.1.26 polyfill
│   │   ├── vol.ts                # EWMA + jump-adjusted sigma (pure)
│   │   ├── bgk.ts                # boundary correction
│   │   ├── multiplier.ts         # compute_multiplier + compute_p_touch + first_passage_touch_prob + normal_cdf
│   │   ├── hui.ts                # OPTIONAL — Hui double-barrier series, NOT in the runtime path. Rust exports it but compute_multiplier never calls it; OTM cells use first_passage_touch_prob instead. Only port if you need offline double-barrier validation.
│   │   ├── types.ts              # Cell, OracleState, PricingConfig
│   │   ├── README.md             # "must match Rust within 1e-6; see parity test"
│   │   └── __tests__/
│   │       └── parity.test.ts    # loads fixtures.json, asserts within tolerance
│   ├── components/
│   │   ├── Grid.tsx
│   │   ├── Cell.tsx
│   │   ├── PriceLine.tsx         # SVG overlay
│   │   ├── HeaderBar.tsx
│   │   ├── HistoryStrip.tsx
│   │   └── DebugOverlay.tsx      # ?debug=1 toggle
│   └── hooks/
│       ├── useOracleTicks.ts     # useSyncExternalStore over tickStore
│       ├── useVolatility.ts      # selector: latest σ_annualized
│       ├── useVisibleCells.ts    # derives 4-or-6-column grid from wall clock + strike ladder
│       ├── useCellMultiplier.ts  # selector per cell, runs compute_multiplier
│       ├── useMe.ts              # TanStack Query: balance + history
│       ├── useTap.ts             # mutation: POST /v1/positions
│       └── usePositionPoll.ts    # GET /v1/positions/:id every 500ms until settled
└── tests/fixtures/parity.json    # committed; regenerated from Rust
```

### Data flow

```
WS /v1/stream
  └─► ws.ts onmessage
       └─► tickStore.push(tick)
            ├─► (internal) update EWMA σ via nextVol()
            └─► fan-out to subscribers:
                 ├─► PriceLine: latest mid + trail
                 ├─► useCellMultiplier × N visible cells: recompute μ
                 └─► HeaderBar: current price chip

wall clock (250ms)
  └─► useVisibleCells: derive cellKey grid from now() aligned to 5s
       └─► Grid renders columns of Cell components

user taps Cell
  └─► positions-store.has(cellKey) ? nudge() : enter PENDING
       └─► useTap.mutate({cell, latestTick, clientRequestId})
            └─► POST /v1/positions
                 ├─► 201 → positions-store.set(cellKey, {state:LOCKED, ...})
                 │         └─► usePositionPoll(positionId): every 500ms
                 │              └─► status !== "OPEN" → state:WON|LOST|VOIDED
                 │                   └─► refetch /v1/me + prepend HistoryStrip
                 ├─► 422 stale_quote → positions-store.delete(cellKey) + toast
                 ├─► 422 drift_exceeded → telemetry.drift(server, client) + toast
                 ├─► 422 insufficient_balance → toast + refetch /v1/me
                 └─► 429 → toast + delete
```

### State ownership

| State | Lives in | Lifetime |
|---|---|---|
| Latest oracle tick | `tickStore` (pub/sub) | WS lifetime |
| EWMA σ_annualized | `tickStore` (pub/sub) | WS lifetime; reset on reconnect |
| Visible cell grid | `useVisibleCells` (computed) | derived from wall clock |
| Per-cell position | `positionsStore` (pub/sub, keyed by cellKey) | until cell rotates off-screen |
| Server balance | TanStack Query cache | refetched on settle |
| Server history | TanStack Query cache | refetched on settle |
| Guest identity UUID | localStorage | persistent until cleared |
| WS connection | `ws.ts` module singleton | app lifetime |

## Pricing engine TypeScript port

### Tolerance contract

For any input the backend would accept, the TS implementation
produces a multiplier within **1e-6 absolute** of the Rust
implementation. That's six orders of magnitude tighter than the
3% server-side drift gate.

### Primitive mapping

| Rust | TypeScript |
|---|---|
| `f64::sqrt/abs` | `Math.sqrt/abs` (bit-identical) |
| `f64::exp/ln/powf/sin` | `Math.exp/log/pow/sin` (≤1 ULP drift) |
| `f64::NAN / INFINITY` | `NaN / Infinity` |
| `f64::is_nan/is_finite` | `Number.isNaN/isFinite` (not the globals) |
| `libm::erfc(x)` | `erfc(x)` — `src/pricing/erfc.ts`, A&S 7.1.26 (max err 1.5e-7) |
| `f64::clamp(lo, hi)` | `Math.max(lo, Math.min(hi, x))` |

### State model

`vol.ts` exports a pure `nextVol(prev, newLogReturn, lambda)` →
`VolState`. The tick-store wraps it: every WS tick computes the
log-return from `(prev.mid, new.mid)`, calls `nextVol`, stashes the
result. σ is global (one value across all cells), matching the
Rust impl. On WS reconnect σ resets and re-warms from new ticks.

### Parity testing

A new Rust binary `pricing-engine/src/bin/gen_fixtures.rs` emits
`games/tap-trading/ui/tests/fixtures/parity.json` with ≥500 cases:

- Random `(s0, strike_lo, strike_hi, σ, τ_sec)` across plausible
  ranges (s0 ∈ [10, 1e6], σ ∈ [0.01, 5.0], τ ∈ [0.1, 30.0])
- Boundary cases: s0 at exact band, tiny bands (0.01%), wide bands
  (10%), σ ≈ 0, σ huge, τ ≤ 0
- NaN/Inf inputs
- The same QuantLib-parity cases the Rust suite already uses

Each case carries `{ input, multiplier, p_touch, stage }` where
`stage ∈ "ok" | "boundary_zero" | "convergence_failure" |
"invalid_input"`. Vitest case asserts:

- `stage === "ok"` → `Math.abs(ours - expected) < 1e-6`
- otherwise → `ours` throws the same error variant

`parity.json` is committed. CI runs `bun test` on the UI; a
secondary CI job regenerates fixtures from Rust and diffs against
the committed copy to catch "forgot to regenerate" mistakes.

QuantLib parity tests stay in Rust; TS-vs-Rust parity is the
transitive guarantee.

## Tap lifecycle

### Per-position state machine

```
                   user taps cell with no active position
                          │
                          ▼
                     ┌─────────┐
                     │ PENDING │  optimistic, local-only
                     └────┬────┘  dashed border + pulse
                          │  POST /v1/positions
                          ▼
                  ┌────────┴────────┐
                  │                 │
              ┌───▼───┐         ┌───▼────┐
              │LOCKED │         │REJECTED│
              └───┬───┘         └────────┘
                  │             flash red → delete entry → tappable again
                  │  poll GET /v1/positions/:id every 500ms
                  │  until status != "OPEN" or 10s past t_close_ms
                  ▼
            ┌─────┼─────┬─────┐
            ▼     ▼     ▼     ▼
          WON   LOST  VOIDED settling…  (last = polling timeout)
```

### Multi-cell, single-position-per-cell rule

A cell is keyed by `${strikeLo}-${strikeHi}-${tOpenMs}`. The
positions-store is `Map<cellKey, CellPosition>` — at most one entry
per cell at any time. Tap handler:

```ts
function handleTap(cell, latestTick) {
  if (positionsStore.has(cell.key)) { nudge(cell.key); return; }
  if (Date.now() + 1000 >= cell.t_close_ms) return;
  const clientRequestId = crypto.randomUUID();
  positionsStore.set(cell.key, {
    cellKey: cell.key, clientRequestId, stake: 100, state: "PENDING",
  });
  tapMutation.mutate({ cell, latestTick, clientRequestId });
}
```

Across distinct cells the user can fire as many parallel taps as
they want; backend per-account rate limit (the only throttle) hands
back 429 if too fast.

### Optimistic balance

```
displayed_balance = serverBalance - Σ(p.stake for p in positions if p.state === "PENDING")
```

- PENDING create → subtracts immediately.
- LOCKED → entry stays in store, refetch `/v1/me` → serverBalance
  reflects the server-side debit → sum drops to 0 → balance lands.
- REJECTED → entry deleted → sum drops → balance bounces back.

### Lock-window guard

`useVisibleCells` recomputes the grid every 250ms (cheap; just
wall-clock math). A cell with `now + 1000 >= t_close_ms` renders
as `disabled` (dim, no pointer cursor) → tap handler also guards
defensively. Server-side `LockWindow` rejection should be
unreachable in normal play.

### Why polling, not WS, for settlement

The api re-broadcasts oracle ticks over `/v1/stream` but not
settlement events; adding that requires backend work this v0
skips. At ≤10 concurrent positions per tester session, 500ms
polling per position is:

- Trivial to implement (TanStack Query `refetchInterval` + predicate).
- ~10 extra HTTP requests per tap. Negligible at MVP scale.
- Robust against missed events; polling self-heals where WS doesn't.

When v1 scales to real users, push settlement events through the
existing WS and remove the per-position polling.

## Errors and edge cases

### Error taxonomy

| Source | Failure | UX | Recovery |
|---|---|---|---|
| WS /v1/stream | Connection drop | Header chip "reconnecting" (yellow); price line freezes | Exponential backoff 250ms → 4s cap; σ resets on reconnect |
| | Stale ticks (>2s) | Header chip "stale" (yellow); cells dim | Auto-reconnect fires |
| | Garbled message | Drop, increment counter, keep socket | None |
| POST /v1/positions | 401/403 | Toast "session lost, reload"; full-page error fallback | Manual reload |
| | 422 stale_quote | Cell flash + toast "quote moved, try again"; entry deleted → tappable | User re-taps |
| | 422 drift_exceeded | Toast + `telemetry.drift()` log. **Canary for TS-port bugs.** | User re-taps |
| | 422 insufficient_balance | Toast; balance refetched; entry deleted | None automatic |
| | 422 invalid_cell/lock_window | Should be unreachable via client guards; log as bug | Entry deleted |
| | 429 | Toast "tapping too fast"; entry deleted | User waits |
| | 5xx / timeout | Toast "server hiccup"; entry deleted; NO auto-retry (TanStack mutations default off) | User re-taps |
| GET /v1/positions/:id (poll) | 404 | Stop polling; mark REJECTED with reason "lost" | Should be impossible after 201 |
| | 5xx | Continue polling — transient | Auto |
| | 10s past t_close_ms | Stop; cell stays LOCKED with "settling…" badge | Worker lag signal |
| GET /v1/me, history | Any failure | Stale values; small ↺ retry button | TanStack default 3-retry |
| Pricing engine (TS) | NaN/Inf/Hui non-converge | Render `—×`; cell untappable; console log | Diagnostic only |

### Special edge cases

- **Clock drift.** Client uses `Date.now()` wall-clock. Drift >1s
  causes every tap to fail with `lock_window` or `invalid_cell`. Not
  fixed in v0; tester is expected to have a sane clock. Log a warning
  if api ever returns a `server_time_ms` we can diff against.
- **Tab backgrounded.** On `visibilitychange → visible`:
  1. Force WS reconnect.
  2. Refetch `/v1/me`.
  3. For each LOCKED position, refetch `/v1/positions/:id` once.
  4. Drop any PENDING older than 10s → REJECTED("timeout").
- **Identity cleared.** New UUID → new account → new 10k bonus.
  Expected v0 reset path. Documented in README.
- **First paint before first WS tick.** Cells are untappable; header
  chip "connecting…". Same disabled path as a WS drop.
- **Client μ disagrees with server μ by ≤3%.** Normal and expected.
  Server's locked μ overwrites the client's display on LOCKED.
- **Aggregator restart (new `run_id` mid-session).** Next tick
  carries new run_id; in-memory current run_id updates; next tap
  uses it. No special UI.
- **Two tabs same identity.** Same account; each tab independent WS
  + optimistic balance; converges on refetch.

### Observability (v0 minimum)

- `src/lib/telemetry.ts`: console-only logger (`tap`, `locked`,
  `settled`, `reject`, `wsDrop`, `drift`). No analytics dep.
- `?debug=1` URL toggle → corner overlay: current σ, last 5 ticks,
  open positions table, WS state, optimistic vs server balance.
- Backend Prometheus endpoints (`:3221/metrics`, `:3421/metrics`,
  `:3321/metrics`) already exist; operator hits them with curl
  during a UX-test session.

## Implementation order (high-level)

1. Scaffold: `games/tap-trading/ui/` matching the 2048 layout;
   `TAP_UI_PORT` wired through worktree-env + sync; `bun dev`
   serves a blank screen.
2. `lib/identity.ts` + `lib/env.ts` + `lib/api.ts` → `useMe`
   shows balance (auto-creates the guest account).
3. `lib/ws.ts` + `tickStore` + a debug overlay showing the latest
   tick → proves the WS pipe.
4. `pricing/` port + `tests/fixtures/parity.json` + vitest passing.
5. `useVisibleCells` + `Grid` + `Cell` rendering (no live μ yet,
   static layout against wall clock).
6. `useCellMultiplier` selector → cells show live μ at 10 Hz.
7. `PriceLine` SVG overlay.
8. `useTap` + `positions-store` → end-to-end tap, optimistic
   balance, lifecycle to LOCKED.
9. `usePositionPoll` → settlement detection, cell flash, history
   prepend.
10. Error toasts + nudge animation + lock-window guard.
11. `HistoryStrip` final styling.
12. Visual polish: palette/fonts pass against PRD §3.

Each step ships behind no flag and is exercised by hand before
moving on. The terminal-state acceptance is: a tester loads the
URL, sees the grid alive, taps cells, sees them settle — without
asking the operator any questions.

## Deferred / known gaps

- Stake selector UI — fixed at 100 points in v0.
- WS settlement events — polling stand-in until v1.
- One-position-per-cell server-side constraint — purely client-side
  in v0; revisit if needed.
- Mobile gesture polish (long-press, swipe to dismiss toast) —
  defer.
- High-DPI canvas / animation perf — defer until we observe a
  frame-budget problem.
- Auth wiring — when `identity-*` lands, swap the localStorage UUID
  for the real subject identifier; the `X-Account-Id` middleware
  stays in place as a fallback.
- CORS allowlist tightening — permissive in v0; tighten when prod
  origins exist.
- Multi-tap on same cell — disallowed in v0 by client gate;
  revisit only if PRD explicitly requires it.

## Open questions for review

None blocking. Decisions to confirm in review:

- TS-vs-Rust parity tolerance: **1e-6 absolute** on the multiplier
  output. Tighten if proptest disagrees in practice.
- Settlement polling interval: **500ms**. Lower bound is "feels
  instant for ≤5s windows"; higher bound is "doesn't hammer the api
  during dev."
- Multi-cell-per-render perf budget: **10 Hz × 24 cells = 240
  computes/sec**. If this trips React's reconciler in practice,
  drop to 5 Hz; the breath effect doesn't need 10 Hz to feel alive.
