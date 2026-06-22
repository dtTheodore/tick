export const CELL_DURATION_MS = 5_000;
const SECONDS_PER_YEAR = 31_557_600;

// Strike-step width as a fraction of the per-cell σ move. Wider steps push each
// row's barrier further from spot, so P_touch falls off faster row-to-row —
// giving the steep fan and the tight (≈1-cell) center floor block Pacifica
// shows, instead of a gentle fan with a wide flat middle. Grid-layout only; the
// server prices whatever cell is tapped, so this never desyncs from settlement.
const STRIKE_STEP_K = 0.6;

// Round, readable band widths. The mid-range tiers (0.3, 0.4, 0.75) give the
// auto-fit (fitStrikeStep) enough granularity to land on a calm, well-filled
// scale instead of bouncing between a too-tight (jagged) and too-loose (flat) one.
//
// The sub-cent tiers (down to 0.0001) exist for low-priced quotes like SUI
// (~$0.70). The step must track the per-cell σ move (≈0.6× it, per
// STRIKE_STEP_K) so the multiplier fan spans several rows; one 5 s σ move on a
// $0.70 asset is only ~$0.0001–0.0002. If the finest tier were 0.001, the step
// snaps ~6× too wide → every row sits >5 σ from spot → the whole grid pins to the
// 99+× cap with a single 1× center row (a cliff, not a fan). BTC/ETH steps are
// orders larger, so they never land on these tiers.
const STEP_TIERS = [
  0.0001, 0.0002, 0.0005, 0.001, 0.002, 0.005, 0.01, 0.02, 0.05, 0.1, 0.2, 0.25, 0.3, 0.4, 0.5,
  0.75, 1, 1.5, 2, 2.5, 5, 10, 20, 25, 50, 100,
];

// Tightest the auto-fit may zoom, as a fraction of spot (~0.6 bps per row). The
// floor must NOT depend on σ: `vol_annualized` is twitchy second-to-second, and
// on a calm asset (when the realized range collapses) a σ-scaled floor put the
// step under σ's control and ping-ponged the whole axis a full tier every few
// seconds. A fixed fraction keeps a calm market on a stable, comfortably-filled
// scale (calmer market → flatter line, as on Pacifica/Hyperliquid — not a frame-
// filling amplifier of $0.05 wiggles). Volatile regimes are handled by the
// realized-range `target` term, which dominates the floor when the range is wide.
const MIN_STEP_FRACTION_OF_SPOT = 6e-5;

export function nextCellOpenMs(nowMs: number): number {
  return Math.floor(nowMs / CELL_DURATION_MS + 1) * CELL_DURATION_MS;
}

/** Snap a raw step to the nearest tier so the ladder doesn't churn each tick. */
function snapStep(raw: number): number {
  if (!Number.isFinite(raw) || raw <= 0) return 0.5;
  let best = STEP_TIERS[0];
  let bestRatio = Math.max(raw / best, best / raw);
  for (const tier of STEP_TIERS) {
    const ratio = Math.max(raw / tier, tier / raw);
    if (ratio < bestRatio) {
      best = tier;
      bestRatio = ratio;
    }
  }
  return best;
}

export function calibratedStrikeStep(
  spot: number,
  sigmaAnnualized: number,
  cellDurationMs: number = CELL_DURATION_MS,
): number {
  const tauYear = cellDurationMs / 1000 / SECONDS_PER_YEAR;
  const raw = STRIKE_STEP_K * sigmaAnnualized * spot * Math.sqrt(tauYear);
  return snapStep(raw);
}

/**
 * Choose the strike step (one row = one band) so the visible price *history*
 * fills the ladder instead of clamping at an edge — the calm, framed look of a
 * top-tier chart. The ladder center already tracks spot, so the head stays
 * centered; this only sizes the amplitude.
 *
 * - `target` sizes the busier side (farther extreme from spot) to ~62% of the
 *   half-ladder, so the line fills the frame with headroom (not edge-to-edge,
 *   which over-magnifies chop into a jagged line) — the
 *   realized range, not the σ forecast, drives the zoom (that's what kept the
 *   line stranded in a thin band before).
 * - `noiseFloor` (a fixed fraction of spot, NOT σ-scaled) keeps a dead-calm
 *   range from zooming in so far that tick noise blows up to a full row — and,
 *   being σ-independent, stops the tier from tracking the twitchy vol forecast.
 * - Hysteresis around the current tier so the ladder doesn't relabel on every
 *   wiggle; we only re-fit when it is about to clamp or is badly under-filling.
 */
export function fitStrikeStep(
  spot: number,
  visibleMin: number,
  visibleMax: number,
  rows: number,
  currentStep: number | null,
): number {
  const halfRows = Math.max(1, Math.floor(rows / 2));
  const halfSpan = Math.max(Math.abs(spot - visibleMin), Math.abs(spot - visibleMax), 1e-9);
  const target = halfSpan / (halfRows * 0.62);
  const noiseFloor = spot * MIN_STEP_FRACTION_OF_SPOT;
  const wanted = snapStep(Math.max(target, noiseFloor));
  if (currentStep === null || !(currentStep > 0)) return wanted;
  const fills = halfSpan / (halfRows * currentStep); // fraction of the half-ladder used
  if (fills > 0.95 || fills < 0.3) return wanted; // about to clamp, or too much dead space
  return currentStep;
}

/** Decimals needed to represent a strike (and its axis label) at this `step`
 *  without (a) collapsing distinct rows or (b) carrying float drift. Two decimals
 *  suffice for BTC/ETH steps; sub-cent steps need three or four. Shared by
 *  `roundStrike`, `cellKey`, and the Grid's price-axis labels so all three agree. */
export function strikeDecimals(step: number): number {
  return step >= 0.01 ? 2 : step >= 0.001 ? 3 : 4;
}

/** Round a raw strike to its step's precision. Replaces a blanket `toFixed(2)`,
 *  which floored every SUI row to the same cent. */
export function roundStrike(value: number, step: number): number {
  return +value.toFixed(strikeDecimals(step));
}

/**
 * Returns `rows` strike levels centered around the current price, snapped to a
 * ladder whose step is calibrated to current σ. Asset-agnostic: the step (not a
 * fixed decimal count) sets the rounding, so a $64k BTC and a $3.50 SUI both get
 * a clean, non-degenerate ladder.
 */
export function strikeLadder(currentMid: number, rows: number, step: number): number[] {
  const center = Math.round(currentMid / step) * step;
  const half = Math.floor(rows / 2);
  const out: number[] = [];
  for (let i = -half; i < rows - half; i++) {
    out.push(roundStrike(center + i * step, step));
  }
  return out;
}

export function cellKey(strikeLo: number, strikeHi: number, tOpenMs: number): string {
  // toFixed(4) (not 2): sub-cent SUI strikes (~$0.001 steps) must not collapse to
  // the same key. The unary + strips trailing zeros so BTC/ETH keys stay tidy.
  return `${+strikeLo.toFixed(4)}-${+strikeHi.toFixed(4)}-${tOpenMs}`;
}
