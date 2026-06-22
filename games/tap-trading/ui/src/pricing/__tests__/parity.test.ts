import { test, expect } from 'bun:test';
import fixtures from '../../../tests/fixtures/parity.json' with { type: 'json' };
import { computeMultiplier } from '..';
import { InvalidSpot, InvalidSigma } from '../errors';
import { DEFAULT_PRICING_CONFIG, type Cell, type OracleState } from '../types';

type Case = {
  input: { s0: number; lo: number; hi: number; sigma: number; tau_sec: number; now_ms: number };
  multiplier: number | null;
  stage: 'ok' | 'boundary_zero' | 'invalid_input';
  error_kind: 'InvalidSpot' | 'InvalidSigma' | 'other' | null;
};

const TOLERANCE = 1e-6;

const ERROR_CLASSES = { InvalidSpot, InvalidSigma } as const;

(fixtures as Case[]).forEach((c, idx) => {
  test(`case ${idx} stage=${c.stage}${c.error_kind ? ` kind=${c.error_kind}` : ''}`, () => {
    const cell: Cell = {
      asset: 'ETH',
      strike_lo: c.input.lo,
      strike_hi: c.input.hi,
      t_open_ms: c.input.now_ms,
      t_close_ms: c.input.tau_sec > 0
        ? c.input.now_ms + Math.round(c.input.tau_sec * 1000)
        : c.input.now_ms,
    };
    const oracle: OracleState = {
      asset: 'ETH',
      spot: c.input.s0,
      sigma_annualized: c.input.sigma,
      timestamp_ms: c.input.now_ms,
    };
    const call = () => computeMultiplier(cell, oracle, DEFAULT_PRICING_CONFIG, c.input.now_ms);

    if (c.stage === 'ok') {
      const got = call();
      expect(Math.abs(got - c.multiplier!)).toBeLessThan(TOLERANCE);
    } else if (c.stage === 'boundary_zero') {
      expect(call()).toBe(0);
    } else if (c.stage === 'invalid_input') {
      const cls = c.error_kind && c.error_kind in ERROR_CLASSES
        ? ERROR_CLASSES[c.error_kind as keyof typeof ERROR_CLASSES]
        : Error;
      expect(call).toThrow(cls as ErrorConstructor);
    }
  });
});
