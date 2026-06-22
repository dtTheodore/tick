import { test, expect } from 'bun:test';
import { huiNoTouch, InvalidTerms, InvalidBand } from '../hui';

test('zero terms throws InvalidTerms', () => {
  expect(() => huiNoTouch(105, 100, 110, 0.01, 5, 0)).toThrow(InvalidTerms);
});

test('degenerate band throws InvalidBand', () => {
  expect(() => huiNoTouch(100, 101, 99, 0.01, 5, 20)).toThrow(InvalidBand);
});

test('tau <= 0 returns 1.0 (degenerate window)', () => {
  expect(huiNoTouch(105, 100, 110, 0.01, 0, 20)).toBe(1.0);
});

test('s0 at lower band returns 0', () => {
  expect(huiNoTouch(100, 100, 110, 0.01, 5, 20)).toBe(0);
});

test('s0 at upper band returns 0', () => {
  expect(huiNoTouch(110, 100, 110, 0.01, 5, 20)).toBe(0);
});

// Tick-scale band: Δ$0.5 on $3812 ETH ≈ 0.013% width — matches the Rust
// unit test `typical_tick_band_001pct_5s_low_vol_no_touch_near_one`.
// The hui series only converges for narrow bands (0.01–0.05% of spot);
// wide bands (10%+) fail at 20 terms per spec §7.1 + Rust docs.
test('s0 mid-band with realistic vol returns value in [0,1] (tick-scale band)', () => {
  const s = 3812.25;
  const sigmaPerSec = 0.30 / Math.sqrt(31_557_600); // 30% annualized
  const p = huiNoTouch(s, s - 0.25, s + 0.25, sigmaPerSec, 5, 20);
  expect(p).toBeGreaterThanOrEqual(0);
  expect(p).toBeLessThanOrEqual(1.0);
});

test('result is always in [0, 1] (tick-scale band)', () => {
  for (let i = 0; i < 50; i++) {
    const sigma = 0.30 / Math.sqrt(31_557_600); // realistic per-sec σ
    const tau = 0.5 + Math.random() * 30;
    const s = 3812.25;
    const p = huiNoTouch(s, s - 0.25, s + 0.25, sigma, tau, 20);
    expect(p).toBeGreaterThanOrEqual(0);
    expect(p).toBeLessThanOrEqual(1);
  }
});
