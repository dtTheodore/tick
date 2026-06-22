# Tick chart perception fix + 10s window — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the tick chart honest (a near-miss looks like a near-miss, no slope/clamp, no colliding pill) and move to a calmer 10s round so the price has time to genuinely reach a cell — fixing the "didn't settle when it looked reached" complaint, which the investigation proved is rendering/perception, not a settlement bug.

**Architecture:** Settlement is already correct (verified with live bets: outcome == raw-path-crossed-band). Three surgical fronts: (1) bump the round window 5s→10s as a policy constant on client+server (+ update tests); (2) auto-*tier* the strike step to fit the visible price history into the ladder so the line fills the frame and never clamps (center already tracks spot → head stays centered); (3) drop the redundant DOM price pill and shrink the head dot so its glow can't paint across a band edge. Keep the existing light EMA on the line (sub-cell lag, smooth) — honesty comes from scale + marker + window, not from removing smoothing.

**Tech Stack:** Rust (axum API, pricing-engine, settlement-worker), TypeScript/React + Vite + `bun test`, Postgres.

**Cadence note (confirm at review):** "10s min horizon" is implemented as `NEAR_COLUMN_LOCK_MS = 10_000` (soonest bettable cell opens ~10s out). With a 10s window that means resolution 10–20s after a tap. If that feels slow, lower `NEAR_COLUMN_LOCK_MS` only — it's independent of the window length.

---
> **IMPLEMENTATION NOTE (post-execution, 2026-06-11):** Mid-execution the user
> clarified: **keep the 5s round window** — only the *minimum bettable horizon*
> moves to 10s. So Tasks 1–3's window-duration edits were **reverted/not needed**
> (`validation.rs`, the API integration tests, and `nextCellOpenMs` stay at 5s);
> the only timing change shipped is `NEAR_COLUMN_LOCK_MS = 10_000` (Grid). The
> chart work (Tasks 4–5) shipped as planned, plus two tuning additions found
> during live verification: intermediate `STEP_TIERS` (0.3/0.4/0.75) and a 0.62
> fill target in `fitStrikeStep`, so the auto-tier lands on a calm, well-filled
> scale instead of a jagged-tight or stranded-loose one. The EMA was kept (no
> render-source change), per the architecture decision.
---

---

## File structure

**Modify (backend):**
- `games/tap-trading/backend/api/src/validation.rs` — `CELL_DURATION_MS` 5000→10000; update its unit tests to 10s cells.
- `games/tap-trading/backend/api/tests/{post_positions_happy,post_positions_errors,idempotency,concurrency,metrics,notify,get_history}.rs` — cell `t_close = t_open + 5_000` → `+ 10_000` (Testcontainers; run if Docker available).

**Modify (UI):**
- `games/tap-trading/ui/src/lib/time.ts` — `CELL_DURATION_MS` 5000→10000; add pure `fitStrikeStep(...)`.
- `games/tap-trading/ui/src/lib/__tests__/time.test.ts` — `nextCellOpenMs` expectations to 10s boundaries; add `fitStrikeStep` tests.
- `games/tap-trading/ui/src/hooks/useVisibleCells.ts` — derive `step` via `fitStrikeStep` from the visible trail range (was σ-only).
- `games/tap-trading/ui/src/components/Grid.tsx` — `NEAR_COLUMN_LOCK_MS` 2500→10000.
- `games/tap-trading/ui/src/components/PriceLine.tsx` — remove the DOM price pill + its refs; shrink the head dot/halo; align the time-label "current slot" to `CELL_DURATION_MS`.
- `games/tap-trading/ui/src/lib/display-line.ts` — fix the stale comment (hit detection no longer reads this; it feeds the multiplier preview + dot).

**No change (verified):** `pricing-engine` math (window-agnostic — derives τ from the cell), `parity.json` + `gen_fixtures.rs` (test TS↔Rust agreement on self-contained cells, not the window policy), `touch.rs`, `useLiveHitDetection.ts`.

---

## Task 1: Backend — 10s window constant + unit tests

**Files:**
- Modify: `games/tap-trading/backend/api/src/validation.rs:17` and its `#[cfg(test)]` cells.

- [ ] **Step 1: Update the failing unit tests first (they encode the 5s policy)**

In `validation.rs` tests, change the valid-duration cells from 5s to 10s and keep each test's *intent*:

```rust
    #[test]
    fn validate_cell_rejects_misaligned_open() {
        // t_open_ms not a 10s boundary; duration is correct so the misalignment is what fails.
        let res = validate_cell(1_000_001, 1_010_001, 100.0, 101.0, 999_000);
        assert!(matches!(res, Err(ApiError::InvalidCell)));
    }

    #[test]
    fn validate_cell_rejects_wrong_duration() {
        // 6s window ≠ 10s.
        let res = validate_cell(1_000_000, 1_006_000, 100.0, 101.0, 999_000);
        assert!(matches!(res, Err(ApiError::InvalidCell)));
    }

    #[test]
    fn validate_cell_rejects_strike_lo_ge_hi() {
        let res = validate_cell(1_000_000, 1_010_000, 100.0, 100.0, 999_000);
        assert!(matches!(res, Err(ApiError::InvalidCell)));
    }

    #[test]
    fn validate_cell_rejects_inside_lock_window() {
        // now + 1000 == t_close → reject.
        let res = validate_cell(1_000_000, 1_010_000, 100.0, 101.0, 1_009_000);
        assert!(matches!(res, Err(ApiError::LockWindow)));
        // now + 999 < t_close → ok.
        let ok = validate_cell(1_000_000, 1_010_000, 100.0, 101.0, 1_008_999);
        assert!(ok.is_ok());
    }
```

- [ ] **Step 2: Run tests, verify they FAIL**

Run: `cd games/tap-trading/backend && cargo test -p tap-trading-api validation::`
Expected: FAIL (constant still 5000 → `validate_cell_rejects_inside_lock_window`'s ok-case errors with InvalidCell on duration).

- [ ] **Step 3: Flip the constant**

`validation.rs:17`:
```rust
/// Cell window length in milliseconds (v1: fixed 10s).
pub const CELL_DURATION_MS: i64 = 10_000;
```

- [ ] **Step 4: Run tests, verify PASS**

Run: `cargo test -p tap-trading-api validation::`
Expected: PASS.

- [ ] **Step 5: Commit** (deferred — user asked not to commit; skip `git commit`, just stage mentally / leave in working tree).

---

## Task 2: Backend — integration test cells to 10s

**Files:**
- Modify: `api/tests/post_positions_happy.rs:18`, `post_positions_errors.rs:22`, `idempotency.rs:34`, `concurrency.rs:65`, `metrics.rs:61`, `notify.rs:40`, `get_history.rs:18`.

- [ ] **Step 1: Update each hardcoded cell window 5_000 → 10_000**

In each file the cell uses `t_open_ms: 1_748_345_670_000` (a 10s-aligned boundary) with `t_close_ms: …_675_000` (or `t_open + 5_000`). Change every cell so `t_close_ms = t_open_ms + 10_000` (e.g., `1_748_345_680_000`), and where `t_close` is computed (`get_history.rs:18`, `post_positions_happy.rs:18`) change `+ 5_000` → `+ 10_000`. Verify any pinned `now`/lock-window assertions still hold (lock window is 1s before close, unchanged; e.g. `post_positions_errors.rs:83` "now = t_close − 500ms" stays inside-lock).

- [ ] **Step 2: Run (only if Testcontainers/Docker available)**

Run: `cargo test -p tap-trading-api --test post_positions_happy --test post_positions_errors`
Expected: PASS. If Docker is unavailable in this environment, note it explicitly as skipped (do not claim pass).

- [ ] **Step 3: Commit** — deferred.

---

## Task 3: UI — 10s window policy

**Files:**
- Modify: `ui/src/lib/time.ts:1`, `ui/src/lib/__tests__/time.test.ts:5-6`, `ui/src/components/Grid.tsx:25`, `ui/src/components/PriceLine.tsx:408`, `ui/src/lib/display-line.ts`.

- [ ] **Step 1: Update the time unit test first**

`time.test.ts`:
```ts
  expect(nextCellOpenMs(1_000_000_001)).toBe(1_000_010_000);
  expect(nextCellOpenMs(1_000_010_000)).toBe(1_000_020_000);
```

- [ ] **Step 2: Run, verify FAIL**

Run: `cd games/tap-trading/ui && bun test src/lib/__tests__/time.test.ts`
Expected: FAIL (still 5s boundaries).

- [ ] **Step 3: Flip the UI constant + dependents**

`time.ts:1`:
```ts
export const CELL_DURATION_MS = 10_000;
```
`Grid.tsx:25`:
```ts
const NEAR_COLUMN_LOCK_MS = 10_000;
```
`PriceLine.tsx` — import `CELL_DURATION_MS` and align the highlighted time stamp to the round boundary (line ~408):
```ts
        const currentSlot = Math.floor(nowMs / CELL_DURATION_MS) * CELL_DURATION_MS;
```
(Leave the 5s label *stride* as-is — clock stamps every 5s are fine; only the highlight aligns to the round.)
`display-line.ts` — replace the stale top comment (it says hit detection reads this; it does not) with: this value is the line's rendered head, sampled by `useCellMultiplier` for the live multiplier preview and used for the head dot.

- [ ] **Step 4: Run, verify PASS**

Run: `bun test src/lib/__tests__/time.test.ts`
Expected: PASS.

- [ ] **Step 5: Commit** — deferred.

---

## Task 4: UI — auto-tier the strike step to fit the visible history

**Files:**
- Modify: `ui/src/lib/time.ts` (add `fitStrikeStep`), `ui/src/lib/__tests__/time.test.ts` (tests), `ui/src/hooks/useVisibleCells.ts`.

- [ ] **Step 1: Write failing tests for `fitStrikeStep`**

Append to `time.test.ts`:
```ts
import { fitStrikeStep, calibratedStrikeStep } from '../time';

test('fitStrikeStep fits the busy side into the ladder', () => {
  // spot 1000, history ranges 1000..1030 over 15 rows (half=7). Half-span 30.
  // target = 30 / (7*0.85) ≈ 5.04 → snaps to tier 5.
  expect(fitStrikeStep(1000, 1000, 1030, 0.5, 15, null)).toBe(5);
});

test('fitStrikeStep keeps current tier within hysteresis band', () => {
  // Current step 5 already fits a half-span of 20 (fills 20/(7*5)=0.57) → unchanged.
  expect(fitStrikeStep(1000, 1000, 1020, 0.5, 15, 5)).toBe(5);
});

test('fitStrikeStep zooms out when the history would clamp', () => {
  // half-span 70, current step 5 → fills 70/(7*5)=2.0 (>0.95) → re-fit up.
  expect(fitStrikeStep(1000, 930, 1000, 0.5, 15, 5)).toBeGreaterThan(5);
});

test('fitStrikeStep never goes below the σ noise floor', () => {
  // Tiny history range but σ floor dominates → step >= calibratedStrikeStep.
  const floor = calibratedStrikeStep(1000, 0.5);
  expect(fitStrikeStep(1000, 999.99, 1000.01, 0.5, 15, null)).toBeGreaterThanOrEqual(floor);
});
```

- [ ] **Step 2: Run, verify FAIL** — Run: `bun test src/lib/__tests__/time.test.ts` → FAIL (`fitStrikeStep` undefined).

- [ ] **Step 3: Implement `fitStrikeStep` in `time.ts`** (after `calibratedStrikeStep`)

```ts
/**
 * Choose the strike step (one row = one band) so the visible price *history*
 * fills the ladder instead of clamping at an edge — the calm, framed look of a
 * top-tier chart. The ladder center already tracks spot, so the head stays
 * centered; this only sizes the amplitude.
 *
 * - `target` sizes the busier side (farther extreme from spot) to ~85% of the
 *   half-ladder, so the line nearly fills the frame with a little headroom.
 * - Never finer than the σ-calibrated step: a calm range must not zoom in so far
 *   that 20–50 Hz tick noise (and the line's sub-cent EMA lag) blow up to many px.
 * - Hysteresis around the current tier so the ladder doesn't relabel on every
 *   wiggle; we only re-fit when it is about to clamp or is badly under-filling.
 */
export function fitStrikeStep(
  spot: number,
  visibleMin: number,
  visibleMax: number,
  sigmaAnnualized: number,
  rows: number,
  currentStep: number | null,
): number {
  const halfRows = Math.max(1, Math.floor(rows / 2));
  const halfSpan = Math.max(Math.abs(spot - visibleMin), Math.abs(spot - visibleMax), 1e-9);
  const target = halfSpan / (halfRows * 0.85);
  const sigmaFloor = calibratedStrikeStep(spot, sigmaAnnualized);
  const wanted = snapStep(Math.max(target, sigmaFloor));
  if (currentStep === null || !(currentStep > 0)) return wanted;
  const fills = halfSpan / (halfRows * currentStep); // fraction of the half-ladder used
  if (fills > 0.95 || fills < 0.3) return wanted; // about to clamp, or too much dead space
  return currentStep;
}
```

- [ ] **Step 4: Run, verify PASS** — Run: `bun test src/lib/__tests__/time.test.ts` → PASS.

- [ ] **Step 5: Wire it into `useVisibleCells.ts`** (replace the σ-only step block, ~lines 44-60)

```ts
  const snap = tickStore.getSnapshot();
  const sigma = snap.tick?.vol_annualized ?? 0;

  // Visible price extremes over the chart's history window, so the ladder step
  // fits the line into the frame (see fitStrikeStep). Bounded work: trail ≤ ~1200.
  const horizonMs = 30_000;
  const cutoff = Date.now() - horizonMs;
  let vMin = currentMid ?? 0;
  let vMax = currentMid ?? 0;
  for (const p of snap.trail) {
    if (p.ts_ms < cutoff) continue;
    if (p.mid < vMin) vMin = p.mid;
    if (p.mid > vMax) vMax = p.mid;
  }

  const stepRef = useRef<number | null>(null);
  if (currentMid !== null && sigma > 0) {
    stepRef.current = fitStrikeStep(currentMid, vMin, vMax, sigma, ROWS, stepRef.current);
  }
  const step = stepRef.current ?? FALLBACK_STEP;
```

Update the import: `import { CELL_DURATION_MS, calibratedStrikeStep, cellKey, fitStrikeStep, strikeLadderEth } from '@/lib/time';` (drop `calibratedStrikeStep` if no longer directly used — it's used inside `fitStrikeStep`, so the direct import can be removed). Remove the now-unused `stepSigmaRef`.

- [ ] **Step 6: Run the UI unit suite** — Run: `bun test` (in `ui/`). Expected: PASS (parity, multiplier, time, vol).

- [ ] **Step 7: Commit** — deferred.

---

## Task 5: UI — drop the colliding pill + honest head dot

**Files:**
- Modify: `ui/src/components/PriceLine.tsx`.

- [ ] **Step 1: Remove the DOM price pill and its render machinery**

Delete the pill JSX block (`PriceLine.tsx:457-465`, the `spot !== null && (...)` `<div ref={markerYRef} …>`). Remove `markerYRef`, `smoothedYRef`, and the `markerYRef.current.style.transform = …` block (~388-395). The `spot` prop becomes unused in the body — keep the prop (Grid passes it) but it's no longer rendered, or drop it from the interface + the `<PriceLine spot=… />` call site in `Grid.tsx`. Prefer dropping it for cleanliness.

- [ ] **Step 2: Shrink the head dot so its glow can't straddle a band edge**

Replace the marker draw (`PriceLine.tsx:375-386`) with a tighter dot — solid r≈3, halo r≈5, lower blur — so under the higher px/$ of the fitted scale it marks the head without painting into the neighbouring cell:
```ts
        ctx!.save();
        ctx!.shadowColor = 'rgba(255,45,126,0.7)';
        ctx!.shadowBlur = 6;
        ctx!.beginPath();
        ctx!.arc(lastPx, lastPy, 3, 0, Math.PI * 2);
        ctx!.fillStyle = STROKE;
        ctx!.fill();
        ctx!.restore();
```
(Drop the separate r=7 translucent halo circle.)

- [ ] **Step 3: Verify it builds + unit tests stay green**

Run: `cd games/tap-trading/ui && bunx tsc --noEmit && bun test`
Expected: no type errors (e.g., from a removed `spot` prop — fix the call site), tests PASS.

- [ ] **Step 4: Commit** — deferred.

---

## Task 6: Verify — real app + regression guard

**Files:** none (verification).

- [ ] **Step 1: Backend + UI unit suites green**

Run: `cd games/tap-trading/backend && cargo test -p tap-trading-api validation:: && cargo test -p tap-trading-pricing-engine`
Run: `cd games/tap-trading/ui && bun test && bunx tsc --noEmit`
Expected: PASS. (Integration tests need Docker — run if available, else report skipped.)

- [ ] **Step 2: Settlement regression guard — re-run the live repro harness**

The dev stack is up. Re-run `tmp/repro-settle.ts` (places real bets, correlates outcome vs raw path). Expected: every band still satisfies `server outcome == raw-path-crossed-band` (proves "what you see == what settles" held through the changes; also exercises a 10s cell end-to-end). Note: the harness's cell construction must use the 10s window (`t_close = t_open + 10_000`) — update it before re-running.

- [ ] **Step 3: Drive the real app (chrome-devtools) and confirm the UX wins**

Open the tap-ui, and confirm visually against the references:
- Line fills the frame, head near vertical center, **no edge-clamp/slope cliff** as the price drifts.
- **No floating price pill** over cells; only the small head dot.
- At a near-miss, the head dot sits **visibly outside** the band edge (does not paint into the cell) — the honesty fix.
- Place a bet on a near cell; when the price genuinely crosses, it celebrates and pays; when it only approaches, it visibly stops short.
Capture a screenshot for the before/after.

- [ ] **Step 4: Final check against the goal**

Confirm the product is better than before: settlement provably matches the visible price (regression guard green), the chart reads like the references (framed, honest, no pill), and the 10s window gives genuine touches. Report any residual (in-play-column, feed silences) as known/out-of-scope, not silently.

---

## Self-review notes
- **Spec coverage:** §4.1 window→Tasks 1–3; §4.2 auto-range→Task 4 (auto-tier variant, fixed-height rows kept — flagged for review vs continuous-zoom); §4.3 pill/dot→Task 5; §4.4 render-source→explicit decision to KEEP the EMA (sub-cell), documented in Task 3 step 3 + architecture. §5 success criteria→Task 6.
- **Parity:** confirmed window-agnostic — no fixture regen (corrects the spec's §4.1 "regenerate fixtures").
- **Residuals:** in-play column + feed silences remain out of scope (spec §6); 10s window mitigates the latter.
