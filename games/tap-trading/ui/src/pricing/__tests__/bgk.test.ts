import { test, expect } from 'bun:test';
import { applyBgkCorrection } from '../bgk';

test('corrected band widens on lower side and upper side', () => {
  const [lc, hc] = applyBgkCorrection(100, 110, 0.01, 5, 100);
  expect(lc).toBeLessThan(100);
  expect(hc).toBeGreaterThan(110);
});

test('m <= 0 leaves band unchanged (mirrors Rust early return)', () => {
  expect(applyBgkCorrection(100, 110, 0.01, 5, 0)).toEqual([100, 110]);
});

test('tau <= 0 leaves band unchanged', () => {
  expect(applyBgkCorrection(100, 110, 0.01, 0, 100)).toEqual([100, 110]);
});

test('larger sigma shifts bands further out', () => {
  const [l1, h1] = applyBgkCorrection(100, 110, 0.001, 5, 100);
  const [l2, h2] = applyBgkCorrection(100, 110, 0.01, 5, 100);
  expect(l2).toBeLessThan(l1);
  expect(h2).toBeGreaterThan(h1);
});

test('shift is symmetric in log space', () => {
  const [lc, hc] = applyBgkCorrection(99, 101, 0.001, 5, 100);
  const up = Math.log(hc / 101);
  const down = Math.log(99 / lc);
  expect(Math.abs(up - down)).toBeLessThan(1e-12);
});
