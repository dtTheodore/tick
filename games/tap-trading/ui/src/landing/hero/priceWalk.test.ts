import { describe, expect, it } from 'bun:test';
import { createPriceWalk } from './priceWalk';

describe('createPriceWalk', () => {
  it('is deterministic for a given seed', () => {
    const a = createPriceWalk({ seed: 42, start: 1700 });
    const b = createPriceWalk({ seed: 42, start: 1700 });
    const seriesA = Array.from({ length: 50 }, () => a.step());
    const seriesB = Array.from({ length: 50 }, () => b.step());
    expect(seriesA).toEqual(seriesB);
  });

  it('starts at the provided start price', () => {
    const w = createPriceWalk({ seed: 1, start: 1700 });
    expect(w.current()).toBeCloseTo(1700, 5);
  });

  it('stays within a bounded band around start (mean-reverting)', () => {
    const w = createPriceWalk({ seed: 7, start: 1700, drift: 0, volatility: 0.4 });
    let min = Infinity;
    let max = -Infinity;
    for (let i = 0; i < 2000; i++) {
      const p = w.step();
      min = Math.min(min, p);
      max = Math.max(max, p);
    }
    expect(min).toBeGreaterThan(1700 * 0.9);
    expect(max).toBeLessThan(1700 * 1.1);
  });

  it('produces different series for different seeds', () => {
    const w1 = createPriceWalk({ seed: 1, start: 1700 });
    const w2 = createPriceWalk({ seed: 2, start: 1700 });
    const a = Array.from({ length: 20 }, () => w1.step());
    const b = Array.from({ length: 20 }, () => w2.step());
    expect(a).not.toEqual(b);
  });
});
