# Tick — Testing Strategy

**Status:** v0.1
**Owner:** Eng + QA
**Audience:** Every engineer touching the codebase
**Companions:** `PRD.md`, `MATH_SPEC.md` §6 (calibration), `ORACLE_SPEC.md`, `SYSTEM_DESIGN.md`

> **Why this spec exists:** Tick is a math product with real consequences. A bug in the pricing engine means users see wrong multipliers; a bug in the settlement worker means wrong points credited; a bug in anti-cheat means farmers walk away with airdrop weight. Some of these break the integrity moat the whole product is built on. This doc is how we test enough to ship without being paranoid.

---

## 1. Goals & non-goals

**Goals:**
- Catch math errors before they're visible to users (zero tolerance — *the math is the brand*)
- Catch settlement errors before they're written to Postgres (idempotency, race conditions, oracle replay correctness)
- Catch performance regressions before they hit production (60-fps grid, ≤500ms settle latency)
- Catch security/anti-cheat regressions before they leak airdrop weight

**Non-goals:**
- 100% line coverage. We aim for **80% coverage in load-bearing services, 60% elsewhere**.
- Testing every UI permutation. Visual regressions are caught by humans + Storybook + one e2e happy path.
- Testing 3rd-party SDKs (`@mysten/sui`, `@mysten/enoki`, `axum`). Trust the vendor, test our integration boundary.

---

## 2. Test pyramid

```
                    ┌───────────────┐
                    │  E2E (5%)     │   Playwright, full stack, browser
                    │  ~20 tests    │   Run on every release candidate
                    └───────┬───────┘
                            │
                ┌───────────┴───────────┐
                │  Integration (20%)    │   Service-to-Postgres, service-to-WS
                │  ~150 tests           │   Run on every PR
                └───────────┬───────────┘
                            │
            ┌───────────────┴───────────────┐
            │  Unit + Property (75%)        │   Pure functions, mocked deps
            │  ~1000+ tests                 │   Run on every save (vitest watch)
            └───────────────────────────────┘
```

The **base of the pyramid is the pricing engine** — pure functions, no I/O, fastest to test, where bugs hurt most.

---

## 3. Pricing engine — `backend/pricing-engine/` + `packages/pricing-engine-ts/`

The math is the brand. Every test here is non-negotiable. Both the canonical Rust crate and the thin TS port consume the **same** `test/fixtures/quantlib.json` — drift between them is what these tests exist to catch.

### 3.1 QuantLib parity tests

**Goal:** verify both the Rust crate (canonical) and the TS port (client-side display) match QuantLib's C++ reference within 1% relative error across the input domain. The shared-fixture design means a regression in either implementation fails CI.

**Setup:** generate 100 fixtures via Python+QuantLib once during dev, commit to `test/fixtures/quantlib.json`:

```python
# scripts/gen-quantlib-fixtures.py (run once during dev)
import QuantLib as ql, json, random

fixtures = []
for _ in range(100):
    spot   = random.uniform(100, 100_000)
    width  = random.uniform(0.0005, 0.05) * spot     # 0.05%–5% band
    L      = spot - width / 2
    H      = spot + width / 2
    sigma  = random.uniform(0.30, 2.50)              # 30%–250% annualized
    tau    = random.uniform(5, 60) / (365 * 86400)   # 5–60 seconds in years

    # QuantLib Hui engine
    expected = quantlib_double_no_touch(spot, L, H, sigma, tau)
    fixtures.append({ "spot": spot, "L": L, "H": H, "sigma": sigma, "tau_sec": tau * 365 * 86400,
                       "expected_p_no_touch": expected })

with open("test/fixtures/quantlib.json", "w") as f:
    json.dump(fixtures, f, indent=2)
```

**Tests (one per implementation, same fixture file):**

```typescript
// packages/pricing-engine-ts/test/hui.parity.test.ts
import fixtures from '../../../test/fixtures/quantlib.json';
import { huiNoTouch } from '../src';

test.each(fixtures)('QuantLib parity (case $#)', ({ spot, L, H, sigma, tau_sec, expected_p_no_touch }) => {
  const sigmaPerSec = sigma / Math.sqrt(31_557_600);
  const ours = huiNoTouch(spot, L, H, sigmaPerSec, tau_sec);
  const rel_err = Math.abs(ours - expected_p_no_touch) / expected_p_no_touch;
  expect(rel_err).toBeLessThan(0.01);  // 1% relative
});
```

A parallel `tests/hui_quantlib_parity.rs` lives in `backend/pricing-engine/`, reads the same fixture file, and asserts the same 1% bound.

**Pass criterion:** all 100 cases pass on **both** the Rust and the TS suite. **Fail mode:** any single case at >1% relative error in either is a block.

### 3.2 BGK against published values

**Goal:** verify our Broadie-Glasserman-Kou continuity correction matches the published 1997 paper table values to 4 decimal places.

**Source:** BGK 1997 Table 2 (`https://www.columbia.edu/~sk75/mfBGK.pdf` — get the actual numbers from the PDF; do not hand-transcribe without verification).

**Test:**

```typescript
// packages/pricing-engine-ts/test/bgk.published.test.ts (mirrored by backend/pricing-engine/tests/bgk_published.rs)
const BGK_1997_TABLE_2 = [
  // From paper — verify against PDF before committing
  { S: 110, B: 130, T: 0.5, sigma: 0.30, r: 0.10, m: 50,
    continuous_no_touch: /* fill from paper */,
    discrete_actual: /* fill from paper */,
    bgk_corrected: /* fill from paper */ },
  // ... rows for m ∈ {5, 25, 50, 250, 1000}
];

test.each(BGK_1997_TABLE_2)('BGK 1997 Table 2 row $#', (c) => {
  const { LCorrected, HCorrected } = applyBGKCorrection(c.L, c.B, c.sigma, c.T, c.m);
  const result = huiNoTouch(c.S, LCorrected, HCorrected, c.sigma, c.T);
  expect(Math.abs(result - c.bgk_corrected)).toBeLessThan(0.0001);  // 4 decimal places
});

test('β_BGK constant is exactly correct', () => {
  // From paper: β_BGK = −ζ(½) / √(2π) ≈ 0.582597...
  expect(BETA_BGK).toBeCloseTo(0.5826, 4);
});
```

**Pass criterion:** all rows pass AND `β_BGK` constant verified. **Fail mode:** if even the constant is wrong, every downstream calculation is wrong. This is a 30-second test that catches a silent killer.

### 3.3 Property tests (`fast-check`)

**Goal:** verify mathematical properties hold across the full input domain, not just hand-picked cases.

```typescript
// packages/pricing-engine-ts/test/multiplier.property.test.ts (mirrored by backend/pricing-engine/tests/invariants.rs via proptest)
import fc from 'fast-check';
import { computePTouch, computeMultiplier } from '../src';

const validCell = () => fc.record({
  L: fc.float({ min: 100, max: 200 }),
  H: fc.float({ min: 200.01, max: 300 }),   // H > L always
  tauSec: fc.float({ min: 0.1, max: 60 })
});

const validOracle = () => fc.record({
  spot: fc.float({ min: 50, max: 350 }),
  sigmaAnnualized: fc.float({ min: 0.1, max: 5.0 })
});

test('P_touch is in [0, 1]', () => {
  fc.assert(fc.property(validCell(), validOracle(), (cell, oracle) => {
    const p = computePTouch(cell, oracle);
    return p >= 0 && p <= 1;
  }), { numRuns: 1000 });
});

test('P_touch increases with σ (monotonicity)', () => {
  fc.assert(fc.property(
    validCell(), validOracle(), fc.float({ min: 0.01, max: 1.0 }),
    (cell, oracle, sigmaBump) => {
      const pLow = computePTouch(cell, oracle);
      const pHigh = computePTouch(cell, { ...oracle, sigmaAnnualized: oracle.sigmaAnnualized + sigmaBump });
      return pHigh >= pLow - 1e-9;   // tolerance for floating-point jitter
    }
  ), { numRuns: 1000 });
});

test('P_touch increases with τ (monotonicity)', () => {
  fc.assert(fc.property(
    validCell(), validOracle(), fc.float({ min: 0.1, max: 30 }),
    (cell, oracle, tauBump) => {
      const pShort = computePTouch(cell, oracle);
      const pLong = computePTouch({ ...cell, tauSec: cell.tauSec + tauBump }, oracle);
      return pLong >= pShort - 1e-9;
    }
  ));
});

test('multiplier τ-dependent floor is enforced', () => {
  fc.assert(fc.property(validCell(), validOracle(), (cell, oracle) => {
    const m = computeMultiplier(cell, oracle);
    if (m === 0) return true;                       // cell expired/closed
    const tau = (cell.tCloseMs - Date.now()) / 1000;
    const floor = 1.30 + 0.01 * tau;
    return m >= floor - 1e-9;                       // tolerance for float jitter
  }));
});

test('multiplier cap is enforced', () => {
  fc.assert(fc.property(validCell(), validOracle(), (cell, oracle) => {
    return computeMultiplier(cell, oracle) <= 1000;
  }));
});

test('in-band cell returns exactly floor(τ) when math floor would be < raw', () => {
  // When spot ∈ [L, H], the pure math gives mult ≈ 0.9 (P_touch=1, raw=(1-h)/1).
  // The floor curve must always win for in-band cells.
  fc.assert(fc.property(
    fc.float({ min: 100, max: 200 }),  // S
    fc.float({ min: 5, max: 30 }),     // band half-width
    fc.float({ min: 1, max: 60 }),     // tau
    fc.float({ min: 0.3, max: 2.0 }),  // sigma
    (S, w, tau, sigma) => {
      const cell = { L: S - w, H: S + w, tauSec: tau };
      const oracle = { spot: S, sigmaAnnualized: sigma };
      const m = computeMultiplier(cell, oracle);
      const floor = 1.30 + 0.01 * tau;
      return Math.abs(m - floor) < 0.01;            // expect floor wins
    }
  ));
});

test('floor curve matches Pacifica reference values', () => {
  // Calibration anchor: spot in-band on BTC, τ ∈ {5, 10, 30, 50, 70}s
  // → mult ∈ {1.35, 1.40, 1.60, 1.80, 2.00} (within 2% of observed Pacifica)
  const anchors = [
    { tau: 5,  expected: 1.35 },
    { tau: 10, expected: 1.40 },
    { tau: 30, expected: 1.60 },
    { tau: 50, expected: 1.80 },
    { tau: 70, expected: 2.00 },
  ];
  for (const a of anchors) {
    const floor = 1.30 + 0.01 * a.tau;
    expect(Math.abs(floor - a.expected)).toBeLessThan(0.001);
  }
});

test('P_touch decreases with distance from spot to band', () => {
  fc.assert(fc.property(
    fc.float({ min: 100, max: 200 }),   // S
    fc.float({ min: 1, max: 50 }),       // width
    fc.float({ min: 1, max: 30 }),       // tau
    fc.float({ min: 0.3, max: 2.0 }),    // sigma
    fc.float({ min: 5, max: 50 }),       // dist1
    fc.float({ min: 0.01, max: 20 }),    // additional offset for dist2
    (S, w, tau, sigma, dist1, distBump) => {
      const dist2 = dist1 + distBump;
      const oracle = { spot: S, sigmaAnnualized: sigma };
      const cellClose = { L: S + dist1, H: S + dist1 + w, tauSec: tau };
      const cellFar = { L: S + dist2, H: S + dist2 + w, tauSec: tau };
      return computePTouch(cellFar, oracle) <= computePTouch(cellClose, oracle) + 1e-9;
    }
  ));
});
```

**Pass criterion:** zero counterexamples across 1000+ random inputs per property. **Fail mode:** `fast-check` minimizes failing inputs and reports a concrete failing case.

### 3.4 EWMA volatility estimator tests

```typescript
// packages/pricing-engine-ts/test/vol.test.ts (mirrored by backend/pricing-engine/tests/vol.rs)
test('EWMA initializes correctly on cold start', () => { /* ... */ });
test('EWMA converges to constant vol within 60 ticks at λ=0.94', () => { /* ... */ });
test('EWMA reacts to vol spike within 10 ticks', () => { /* ... */ });
test('EWMA never returns negative variance', () => {
  fc.assert(fc.property(
    fc.array(fc.float({ min: -0.5, max: 0.5 }), { minLength: 60 }),
    (returns) => estimateRealizedVol(returns) >= 0
  ));
});
```

### 3.5 Coverage target

**95%+ line coverage** for both `backend/pricing-engine/src/` and `packages/pricing-engine-ts/src/`. Anything less is unacceptable for the math layer.

---

## 4. Oracle aggregator — `backend/oracle-aggregator/`

### 4.1 Unit tests (Rust `#[cfg(test)]`)

```rust
// backend/oracle-aggregator/src/aggregator.rs
#[cfg(test)]
mod tests {
    #[test]
    fn median_of_4_sources_robust_to_outlier() { /* ... */ }

    #[test]
    fn ema_alpha_06_smooths_correctly() { /* ... */ }

    #[test]
    fn drops_pyth_when_confidence_exceeds_100bps() { /* ... */ }

    #[test]
    fn drops_stale_source_above_1000ms() { /* ... */ }

    #[test]
    fn emits_degraded_when_fewer_than_2_active_sources() { /* ... */ }
}
```

### 4.2 Source mock harness

A `MockSource` impl lets tests drive synthetic ticks and assert on aggregator output:

```rust
// backend/oracle-aggregator/src/tests/scenarios.rs
#[tokio::test]
async fn pyth_outage_triggers_3_source_fallback() {
    let agg = Aggregator::with_sources(vec![
        Box::new(MockSource::live("Pyth")),
        Box::new(MockSource::live("Binance")),
        Box::new(MockSource::live("Bybit")),
        Box::new(MockSource::live("OKX")),
    ]);
    agg.start().await;
    let receiver = agg.subscribe("ETH").await;

    // Send ticks; verify normal emission
    push_synthetic_ticks(/* all 4 sources */, 100);
    let tick = receiver.recv().await.unwrap();
    assert_eq!(tick.sources_used.len(), 4);

    // Kill Pyth; verify 3-source emission continues
    agg.disable_source("Pyth");
    push_synthetic_ticks(/* Binance, Bybit, OKX */, 100);
    let tick = receiver.recv().await.unwrap();
    assert_eq!(tick.sources_used.len(), 3);
    assert!(!tick.sources_used.contains(&SourceId::Pyth));

    // Kill 2 more; verify DEGRADED
    agg.disable_source("Binance");
    agg.disable_source("Bybit");
    push_synthetic_ticks(/* OKX only */, 100);
    let status = receiver.recv_status().await.unwrap();
    assert_eq!(status.status, "DEGRADED");
}
```

### 4.3 Latency budget tests

```rust
#[tokio::test]
async fn p95_source_to_emit_latency_under_100ms() {
    // Push 1000 ticks with timestamp metadata; measure histogram
    let latencies = run_synthetic_load(1000);
    assert!(latencies.p95() < Duration::from_millis(100));
}
```

### 4.4 Coverage target

**80% line coverage.** Source-specific I/O (the actual WS reconnect logic) is covered by integration tests, not unit tests.

---

## 5. Settlement worker — `backend/settlement-worker/`

This is where idempotency matters most.

### 5.1 Idempotency tests

```rust
#[tokio::test]
async fn duplicate_settle_attempt_is_noop() {
    let pool = setup_test_postgres().await;
    let pos = insert_open_position(&pool, ...).await;
    let tick = AggregatedTick { price: pos.strike_lo, /* ... */ };

    // First settle credits
    worker.process_tick(&tick).await;
    let balance_after_first = get_balance(&pool, pos.account_id).await;

    // Second settle (worker restart simulation) is no-op
    worker.process_tick(&tick).await;
    let balance_after_second = get_balance(&pool, pos.account_id).await;

    assert_eq!(balance_after_first, balance_after_second);

    // Settlement row count is 1, not 2
    assert_eq!(count_settlements_for_position(&pool, pos.id).await, 1);
}
```

### 5.2 Oracle replay tests

```rust
#[tokio::test]
async fn out_of_order_ticks_handled_correctly() {
    // Ticks at t=1s, t=3s, t=2s — settle uses tick.timestamp, not arrival order
    /* ... */
}

#[tokio::test]
async fn position_voided_when_zero_ticks_in_window() {
    // Open position with t_open..t_close; send no ticks in that window
    // Run expiry pass; position becomes VOIDED, stake refunded
    /* ... */
}
```

### 5.3 Race condition tests

```rust
#[tokio::test]
async fn two_workers_dont_double_credit() {
    // Leader-election simulation: both workers running, only one holds advisory lock
    // Second worker should not credit; UNIQUE(position_id) prevents it anyway
    /* ... */
}
```

### 5.4 Lock-at-tap invariant tests

```rust
#[tokio::test]
async fn settle_uses_position_multiplier_at_tap_not_live_oracle() {
    // Critical invariant: even if the cell's live multiplier has drifted
    // 10× since the user tapped, settlement must use the LOCKED value.
    let pool = setup_test_postgres().await;
    let pos = insert_open_position(&pool, /* multiplier_at_tap */ 5.0, /* stake */ 100).await;

    // Simulate an aggregator tick that would *now* price the same cell at 50×
    let tick = AggregatedTick { /* ..., would produce a 50× multiplier */ };

    worker.process_tick(&tick).await;

    let credit = get_payout_credit(&pool, pos.account_id).await;
    assert_eq!(credit, 500);   // 100 stake × 5.0 LOCKED, NOT 100 × 50
}

#[tokio::test]
async fn position_multiplier_at_tap_is_immutable_after_insert() {
    // Attempt to mutate multiplier_at_tap directly; verify constraint or worker rejects.
    /* ... */
}

#[tokio::test]
async fn settle_tx_is_atomic_settlement_position_ledger() {
    // Inject a Postgres connection drop between INSERT settlements and UPDATE positions.
    // Verify the transaction rolls back fully — no settlement row without matching position UPDATE.
    /* ... */
}
```

### 5.5 Coverage target

**85% line coverage.** All credit/void paths must have explicit tests.

---

## 6. API — `backend/api/`

### 6.1 Integration tests (Postgres + Redis required)

```rust
// backend/api/tests/integration.rs
#[tokio::test]
async fn tap_debits_balance_and_creates_position() {
    let app = TestApp::start().await;   // boots api + postgres + redis
    // Account starts at the signup-bonus balance of 10,000 points.
    let account = app.create_test_account().await;
    let session = app.zklogin_signin(&account).await;

    let res = app.client.post("/v1/positions")
        .header("Authorization", format!("Bearer {}", session))
        .json(&tap_payload_with_stake(100))  // 100-point stake
        .send().await.unwrap();

    assert_eq!(res.status(), 200);
    let balance = app.get_balance(account.id).await;
    assert_eq!(balance, 9_900);  // 10,000 starting − 100 stake
    let positions = app.get_open_positions(account.id).await;
    assert_eq!(positions.len(), 1);
}

#[tokio::test]
async fn tap_rejected_when_server_multiplier_diverges_3pct() {
    /* Client submits multiplier_at_tap = 10.0, server recomputes 10.4;
       4% drift > 3% threshold → server rejects with 409 "multiplier_drift" */
}

#[tokio::test]
async fn tap_with_stale_oracle_seq_rejected() {
    /* Client submits oracle_seq_at_tap = 914_000;
       aggregator ring buffer now starts at seq 914_300 (200ms ahead);
       → 409 "stale_quote" */
}

#[tokio::test]
async fn tap_rejected_in_lock_window() {
    /* Submit with t_close - now() < 1s; server rejects */
}

#[tokio::test]
async fn tap_rejected_when_balance_insufficient() {
    /* Pre-drain account balance below stake amount; tap returns 403 "insufficient_balance" */
}

#[tokio::test]
async fn per_user_rate_limit_at_10_taps_per_second() {
    /* Fire 30 valid taps in 1s from one account; expect 10 (or 20 with burst) succeed,
       rest return 429 "rate_limited" */
}

#[tokio::test]
async fn zklogin_proof_replay_blocked() {
    /* Same JWT submitted twice; second fails */
}
```

### 6.2 Concurrency tests

```rust
#[tokio::test]
async fn 100_concurrent_taps_balance_decrements_consistently() {
    // Account has 500 points balance. 100 simultaneous tap requests at 100pt stake.
    // Verify exactly 5 succeed (5 * 100 = 500), 95 fail with 403 "insufficient_balance".
    // Verify final balance is exactly 0 (no negative, no over-credit).
}

#[tokio::test]
async fn rate_limit_bucket_isolated_per_account() {
    // Account A fires 15 taps in 1s; A sees rate-limit at #11.
    // Account B simultaneously fires 5 taps; B's all succeed.
}
```

### 6.3 Coverage target

**75% line coverage.** Endpoint surface is large; not every error branch needs a test, but every happy path + every authz check does.

---

## 7. Frontend — `apps/web/`

### 7.1 Unit (vitest)

- Multiplier display: matches engine output to 2 decimal places
- Multiplier display refresh: runs at 10 Hz ± 10 ms drift over a 5-second window (verify with `vi.useFakeTimers()`)
- Tap lockout: button disabled in final 1s of column
- Tap lifecycle: PENDING badge renders within one frame of click; transitions to LOCKED on 200 response; clears on 409/429
- Locked-badge value is immutable after server confirms: cell's underlying multiplier may drift; badge value stays at locked value
- Share-card URL: contains tap_id, hash-verifiable

### 7.2 Visual regression (Chromatic or Storybook + Playwright snapshots)

- Grid in default state
- Grid with PENDING tap badge (stake + "—")
- Grid with LOCKED tap badge (stake + multiplier)
- Grid with cell touched (green flash + tension lines)
- Past-column fade state (40% opacity)
- Big-win modal (Tick-kun frame)
- Empty leaderboard
- Low-balance warning state on `<BalancePill>`

### 7.3 Coverage target

**60% line coverage.** Visual diffs cover the rest.

---

## 8. End-to-end — Playwright

One full happy-path test, run against staging on every release candidate.

```typescript
// e2e/happy-path.spec.ts
test('first-time user signs up, taps, wins, sees credit', async ({ page }) => {
  await page.goto('https://staging.tick.xyz');
  await expect(page.locator('canvas#grid')).toBeVisible({ timeout: 3000 });

  // Sign in
  await page.click('text=Sign in with Google');
  await mockOIDCFlow(page);  // helper: bypass real OAuth in staging
  await expect(page.locator('[data-testid=balance]')).toHaveText('10,000');  // signup bonus

  // Tap a cell
  const cell = page.locator('[data-cell="3812.0-3812.5/+5s"]');
  await cell.click();

  // PENDING state appears immediately
  await expect(cell.locator('[data-testid=tap-badge]')).toHaveAttribute('data-state', 'PENDING');
  await expect(cell.locator('[data-testid=tap-badge-mult]')).toHaveText('—');

  // LOCKED state appears within 500ms of click
  await expect(cell.locator('[data-testid=tap-badge]')).toHaveAttribute('data-state', 'LOCKED', { timeout: 500 });
  const lockedMult = await cell.locator('[data-testid=tap-badge-mult]').textContent();
  expect(parseFloat(lockedMult!)).toBeGreaterThanOrEqual(1.30);  // floor

  // Wait for settle (5s window + small buffer)
  await page.waitForTimeout(7_000);
  const outcome = await page.locator('[data-testid=last-outcome]').textContent();
  expect(['WIN', 'LOSS']).toContain(outcome);

  // The badge mult is unchanged even after settle
  const badgeAfter = await cell.locator('[data-testid=tap-badge-mult]').textContent();
  expect(badgeAfter).toBe(lockedMult);

  if (outcome === 'WIN') {
    const newBalance = await page.locator('[data-testid=balance]').textContent();
    expect(parseInt(newBalance!.replace(/,/g, ''))).toBeGreaterThan(10_000);
  }
});
```

### 8.1 E2E principles
- **One happy path is enough.** More than 3 e2e tests = maintenance burden.
- **Mock OIDC, not Pyth.** Real Pyth ticks make tests flaky; mocked oracle gives determinism.
- **Run against staging only.** Prod e2e = bad idea (creates real accounts).

---

## 9. Load & performance

Run weekly, not per-PR. Goal: catch regressions before they hit prod.

### 9.1 Setup

`k6` or `locust` driving:
- 10,000 simulated concurrent users
- Each tapping every ~2 seconds for 30 minutes
- 3 assets, 6 columns × 12 strikes visible

### 9.2 Targets

| Metric | Target |
|---|---|
| Tap commit p95 | < 200ms |
| Settlement latency p95 (oracle tick → DB credit) | < 500ms |
| Aggregator emit p95 (source tick → broadcast) | < 100ms |
| Frontend frame rate at 100 visible cells | ≥ 60 fps |
| Postgres connection pool saturation | < 80% |
| Redis ops/sec | < cluster limit |
| Memory growth over 30 min | < 5% (no leaks) |

### 9.3 Tools
- `k6` for HTTP + WS load
- `flame graph` (`samply` for Rust, Chrome DevTools for frontend) for CPU profiles
- `heaptrack` for memory leak detection on workers

---

## 10. Anti-cheat & adversarial

These are the tests farmers will try if you don't.

### 10.1 Behavioral simulation

```typescript
// e2e/adversarial/farmer-bot.ts — runs against staging
async function farmer_bot() {
  // 1. Sign up via zkLogin (real account)
  // 2. Tap 100 cells/sec for 10 minutes
  // 3. Verify account is soft-flagged within 60s
  // 4. Verify subsequent taps still succeed (we flag, don't ban)
  // 5. Verify account is excluded from leaderboard
  // 6. Verify simulated airdrop weight = 0
}
```

Run nightly. If a farmer-bot reaches Tier 2 without being flagged, anti-cheat is broken.

### 10.2 Replay & front-running

```typescript
test('zkLogin proof cannot be replayed across sessions', /* ... */);
test('Tap with t_close before now is rejected', /* ... */);
test('Server clock skew of 30s is rejected', /* ... */);
test('Multiplier locked at tap time, not at settle time', /* ... */);
```

### 10.3 Content security

```typescript
test('Display name with <script> tag rendered as text, not HTML', /* ... */);
test('Share-card SSRF blocked', /* ... */);
test('SQL injection in cursor param escaped', /* ... */);
```

---

## 11. Shadow mode (pre-launch)

Per `MATH_SPEC §6.3`, before flipping the pricing engine live:

1. Deploy pricing engine v2 in shadow mode for ≥ 7 days
2. For every tap, compute multiplier with BOTH v1 (live) and v2 (shadow)
3. Settle on v1; log v2's "would-have-paid" amount
4. Reconcile nightly: aggregate shadow-v2 PnL across all users vs live-v1
5. If shadow-v2 deviates from live-v1 by >2% across the population, **block the swap**

This catches calibration errors that property tests + parity tests can miss.

---

## 12. Test data & fixtures

### 12.1 Synthetic historical data
- 30 days of 1-second BTC ticks from Binance public archive → `test/fixtures/binance-btc-30d.json` (~2GB; stored on S3, downloaded by CI)
- Used by: pricing-engine calibration tests, settlement-worker replay tests, oracle-aggregator backtest

### 12.2 Seeded test accounts
- `scripts/seed.ts` creates: 100 test accounts across tiers, 10K historical positions, 1 day of synthetic oracle history
- Used by: api integration tests, e2e tests, frontend Storybook fixtures

### 12.3 Test secrets
- Test JWT for zkLogin: signed by a test OIDC issuer running in `docker-compose.test.yml`
- Test Pyth oracle: mock Hermes endpoint that replays canned VAAs

---

## 13. CI/CD integration

### 13.1 Per-PR

```yaml
# .github/workflows/test.yml
on: pull_request
jobs:
  pricing-engine:
    runs-on: ubuntu-latest
    steps:
      - run: bun run --filter pricing-engine test     # unit + property + parity
  rust-services:
    services: { postgres: { image: postgres:16 }, redis: { image: redis:7 } }
    steps:
      - run: cargo test --workspace                   # unit + integration
  frontend:
    steps:
      - run: bun run --filter web test                # vitest unit
      - run: bun run --filter web build               # build must succeed
  visual:
    steps:
      - run: bunx chromatic                           # visual regression (review queue)
```

### 13.2 Pre-release (manual)

```bash
bun run test:e2e:staging       # Playwright vs staging
bun run test:load              # k6 against staging
bun run test:adversarial       # farmer-bot vs staging
```

### 13.3 Nightly

- Full backtest of pricing engine against last 30 days of Binance data
- Anti-cheat replay tests
- Calibration drift check (compare predicted hit rates to observed)

---

## 14. Coverage targets summary

| Component | Line coverage | Critical paths |
|---|---|---|
| `backend/pricing-engine/` + `packages/pricing-engine-ts/` | ≥ 95% each | 100% on `hui`, `bgk`, `multiplier` in both |
| `backend/oracle-aggregator/` | ≥ 80% | 100% on aggregation pipeline |
| `backend/settlement-worker/` | ≥ 85% | 100% on credit/void paths |
| `backend/api/` | ≥ 75% | 100% on auth + tap-commit |
| `apps/web/` | ≥ 60% | 100% on multiplier display + tap flow |

CI blocks merge if coverage drops below target on any path.

---

## 15. What we deliberately DON'T test

- **`@mysten/sui` SDK internals.** We trust Sui's SDK; we test our integration only.
- **`@mysten/enoki` zkLogin flow itself.** We test that our verifier rejects bad proofs; we don't test their JWT lib.
- **Pyth Hermes API.** We test our subscriber behavior; we don't mock-test their service.
- **Postgres or Redis internals.** They have their own test suites.
- **CSS / Tailwind output.** Visual regression catches what matters.
- **Every typescript-level type error.** `tsc --noEmit` runs in CI; we trust it.

---

## 16. Open questions

| # | Question | Owner |
|---|---|---|
| T1 | Visual regression tool: Chromatic (paid) vs Playwright snapshots (self-hosted)? | Frontend |
| T2 | Load test in cloud (real WS) or local (mocked WS)? Tradeoff cost vs realism | Eng + Ops |
| T3 | Should we run e2e against testnet too, or only mocked staging? | Eng |
| T4 | Shadow-mode infrastructure: separate worker or in-process? | Eng |
| T5 | Anti-cheat sim runs nightly — what's the alert mechanism if it fails? | Ops |
| T6 | Test-fixture archive: S3 cost vs commit-everything (~2GB BTC ticks)? | Eng + Ops |

---

## 17. References

- `MATH_SPEC.md` §6 — calibration runbook (the source of the shadow-mode workflow)
- `ORACLE_SPEC.md` §7 — failure modes (the source of aggregator scenario tests)
- `SYSTEM_DESIGN.md` §9 — failure modes (the source of integration tests)
- BGK 1997: https://www.columbia.edu/~sk75/mfBGK.pdf
- QuantLib `AnalyticDoubleBarrierBinaryEngine`: https://github.com/lballabio/QuantLib
- `fast-check` (property testing): https://github.com/dubzzz/fast-check
- `k6` (load testing): https://k6.io
- Playwright: https://playwright.dev

---

## 18. On-chain vault & proofs — `move/tick_vault`, `backend/proof-verifier`

Spec: ADR-0010, ADR-0011, plans `tick-onchain-vault`, `tick-walrus-proofs`,
`tick-vault-worker-integration`.

### 18.1 Move vault tests (`sui move test`)

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

### 18.2 Proof verifier tests (`cargo test`, pure)

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

### 18.3 Worker integration (`cargo test`, feature-gated)

- **Dual-sink routing:** a `usdc` position routes to the Sui path; a
  `points` position routes to Postgres (unchanged).
- **No ledger on USDC:** a USDC settle writes no `points_ledger` row.
- **Proof retry:** a failed Walrus publish flips `proof_status` failed →
  published on the retry sweep.
- **On-chain end-to-end** (`TICK_IT_ONCHAIN=1`): deposit→mint→settle vs a
  deployed testnet vault; assert payout, `ProofAnchored` event, blob
  fetchable. Skipped in CI without testnet creds.

### 18.4 Coverage target

Move package: every `public` entry has at least one happy-path + one
abort test. Verifier: 100% of `VerifyResult` variants exercised.

