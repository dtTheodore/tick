import { expect, test } from 'bun:test';
import { cellKey, fitStrikeStep, nextCellOpenMs, strikeLadder } from '../time';

test('nextCellOpenMs aligns to 5s boundary', () => {
  expect(nextCellOpenMs(1_000_000_001)).toBe(1_000_005_000);
  expect(nextCellOpenMs(1_000_005_000)).toBe(1_000_010_000);
});

test('strikeLadder steps by the supplied step', () => {
  const rows = strikeLadder(3812.25, 6, 0.5);
  expect(rows.length).toBe(6);
  expect(rows[1] - rows[0]).toBeCloseTo(0.5, 10);
});

test('cellKey is stable for identical inputs', () => {
  expect(cellKey(3812, 3812.5, 1_000_000)).toBe(cellKey(3812, 3812.5, 1_000_000));
});

test('fitStrikeStep fits the busy side into the ladder', () => {
  // spot 1000, history 1000..1030 over 15 rows (half=7). Half-span 30 →
  // target 30/(7*0.62)≈6.9 → snaps to tier 5.
  expect(fitStrikeStep(1000, 1000, 1030, 15, null)).toBe(5);
});

test('fitStrikeStep keeps the current tier within the hysteresis band', () => {
  // half-span 20, current step 5 → fills 20/(7*5)=0.57 (between 0.3 and 0.95) → unchanged.
  expect(fitStrikeStep(1000, 1000, 1020, 15, 5)).toBe(5);
});

test('fitStrikeStep zooms out when the history would clamp', () => {
  // half-span 70, current step 5 → fills 70/35=2.0 (>0.95) → re-fit larger.
  expect(fitStrikeStep(1000, 930, 1000, 15, 5)).toBeGreaterThan(5);
});

test('fitStrikeStep floors at a fixed fraction of spot, not below', () => {
  // Near-zero history range: the fit wants to zoom way in, but the σ-INDEPENDENT
  // floor (spot × 6e-5 = 0.06) keeps it off the finest tiers — snapping to 0.05 —
  // so a calm market stays on a stable scale instead of tracking the vol forecast.
  expect(fitStrikeStep(1000, 999.99, 1000.01, 15, null)).toBe(0.05);
});
