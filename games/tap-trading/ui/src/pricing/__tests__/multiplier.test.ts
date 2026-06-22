import { test, expect } from 'bun:test';
import { computeMultiplier, computePTouch, firstPassageTouchProb } from '../multiplier';
import { InvalidSpot, InvalidSigma } from '../errors';
import { DEFAULT_PRICING_CONFIG, type Cell, type OracleState } from '../types';

const eth: Cell = {
  asset: 'ETH',
  strike_lo: 3812,
  strike_hi: 3812.5,
  t_open_ms: 1_000_000,
  t_close_ms: 1_005_000,
};

const oracle: OracleState = {
  asset: 'ETH',
  spot: 3812.25,
  sigma_annualized: 0.80,
  timestamp_ms: 1_000_000,
};

test('closed cell (t_close <= now) returns 0.0 without validating oracle', () => {
  expect(computeMultiplier(eth, oracle, DEFAULT_PRICING_CONFIG, eth.t_close_ms)).toBe(0);
  expect(computeMultiplier(eth, oracle, DEFAULT_PRICING_CONFIG, eth.t_close_ms + 5_000)).toBe(0);
});

test('NaN spot throws InvalidSpot (must surface — caller pauses taps)', () => {
  const bad = { ...oracle, spot: Number.NaN };
  expect(() => computeMultiplier(eth, bad, DEFAULT_PRICING_CONFIG, 1_000_000))
    .toThrow(InvalidSpot);
});

test('non-positive spot throws InvalidSpot', () => {
  expect(() => computeMultiplier(eth, { ...oracle, spot: 0 }, DEFAULT_PRICING_CONFIG, 1_000_000))
    .toThrow(InvalidSpot);
  expect(() => computeMultiplier(eth, { ...oracle, spot: -1 }, DEFAULT_PRICING_CONFIG, 1_000_000))
    .toThrow(InvalidSpot);
});

test('NaN sigma throws InvalidSigma', () => {
  const bad = { ...oracle, sigma_annualized: Number.NaN };
  expect(() => computeMultiplier(eth, bad, DEFAULT_PRICING_CONFIG, 1_000_000))
    .toThrow(InvalidSigma);
});

test('negative sigma throws InvalidSigma', () => {
  const bad = { ...oracle, sigma_annualized: -0.1 };
  expect(() => computeMultiplier(eth, bad, DEFAULT_PRICING_CONFIG, 1_000_000))
    .toThrow(InvalidSigma);
});

// In-band at open (t_open = now, τ_open = 0): p_touch = 1.0, raw = (1−0.03) =
// 0.97, clamped UP to the flat 1.0× floor — regardless of window length. The old
// τ-growing incentive floor (1.625× / 2.25×) was the "too generous" leak; it's
// gone. Mirrors Rust `in_band_cell_at_open_pays_flat_floor`.
test('in-band cell at open pays the flat 1.0 floor at any window', () => {
  const m5 = computeMultiplier(eth, oracle, DEFAULT_PRICING_CONFIG, 1_000_000);
  expect(Math.abs(m5 - 1.0)).toBeLessThan(0.001);
  const cell30 = { ...eth, t_close_ms: 1_030_000 };
  const m30 = computeMultiplier(cell30, oracle, DEFAULT_PRICING_CONFIG, 1_000_000);
  expect(Math.abs(m30 - 1.0)).toBeLessThan(0.001);
});

// The fix's crux: the SAME band as a FUTURE column (τ_open > 0) is uncertain
// (spot may drift out before it opens) → fair multiplier naturally > 1, no floor.
test('future in-band column prices above the floor', () => {
  const future = { ...eth, t_open_ms: 1_005_000, t_close_ms: 1_010_000 };
  const m = computeMultiplier(future, oracle, DEFAULT_PRICING_CONFIG, 1_000_000);
  expect(m).toBeGreaterThan(1.0);
});

// OTM regression: deep OTM cells must pay > 10×
test('deep OTM below band pays high multiplier (first-passage path)', () => {
  const otm = { ...eth, strike_lo: 4000.0, strike_hi: 4001.0 };
  const m = computeMultiplier(otm, oracle, DEFAULT_PRICING_CONFIG, 1_000_000);
  expect(m).toBeGreaterThan(10);
  expect(m).toBeLessThanOrEqual(DEFAULT_PRICING_CONFIG.multiplier_cap);
});

test('deep OTM above band pays high multiplier (first-passage path)', () => {
  const otm = { ...eth, strike_lo: 3700.0, strike_hi: 3701.0 };
  const m = computeMultiplier(otm, oracle, DEFAULT_PRICING_CONFIG, 1_000_000);
  expect(m).toBeGreaterThan(10);
  expect(m).toBeLessThanOrEqual(DEFAULT_PRICING_CONFIG.multiplier_cap);
});

test('extreme OTM caps at multiplier_cap', () => {
  const extreme = { ...eth, strike_lo: 100_000, strike_hi: 100_001 };
  const m = computeMultiplier(
    extreme,
    { ...oracle, sigma_annualized: 0.30 },
    DEFAULT_PRICING_CONFIG,
    1_000_000,
  );
  expect(m).toBe(DEFAULT_PRICING_CONFIG.multiplier_cap);
});

test('p_touch monotone decreases as OTM distance grows', () => {
  let prev: number | null = null;
  for (const offset of [10, 50, 200, 500]) {
    const cell = { ...eth, strike_lo: 3812.0 + offset, strike_hi: 3812.5 + offset };
    const p = computePTouch(cell, oracle, DEFAULT_PRICING_CONFIG, 1_000_000);
    if (prev !== null) expect(p).toBeLessThanOrEqual(prev + 1e-9);
    prev = p;
  }
});

test('firstPassageTouchProb returns 0 for σ_per_sec <= 0 or τ <= 0', () => {
  expect(firstPassageTouchProb(100, 105, 0, 5)).toBe(0);
  expect(firstPassageTouchProb(100, 105, 0.001, 0)).toBe(0);
});
