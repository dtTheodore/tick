import { describe, expect, it } from 'bun:test';
import { buildMultiplierLadder } from './multiplierLadder';

describe('buildMultiplierLadder', () => {
  const rows = buildMultiplierLadder({ mid: 1711.25, tickSize: 0.4, rowCount: 9, colCount: 4 });

  it('produces rowCount rows each with colCount multipliers and a strike', () => {
    expect(rows).toHaveLength(9);
    for (const r of rows) {
      expect(r.cols).toHaveLength(4);
      expect(typeof r.strike).toBe('number');
    }
  });

  it('strikes are evenly spaced by tickSize, descending from top', () => {
    const strikes = rows.map((r) => r.strike);
    expect(strikes[0]).toBeGreaterThan(strikes[strikes.length - 1]);
    for (let i = 1; i < strikes.length; i++) {
      expect(strikes[i - 1] - strikes[i]).toBeCloseTo(0.4, 5);
    }
  });

  it('multiplier grows as the row gets further from mid', () => {
    const nearest = rows.reduce((a, b) =>
      Math.abs(a.strike - 1711.25) < Math.abs(b.strike - 1711.25) ? a : b,
    );
    const farthest = rows[0];
    expect(farthest.cols[0]).toBeGreaterThan(nearest.cols[0]);
  });

  it('never returns a multiplier below the 1.0x floor', () => {
    for (const r of rows) for (const m of r.cols) expect(m).toBeGreaterThanOrEqual(1);
  });
});
