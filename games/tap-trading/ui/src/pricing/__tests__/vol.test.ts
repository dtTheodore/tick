import { test, expect } from 'bun:test';
import { nextVol, INITIAL_VOL_STATE, InvalidLambda, InvalidLogReturn, type VolState } from '../vol';
import { SECONDS_PER_YEAR } from '../constants';

const YEARLY_ROOT = Math.sqrt(SECONDS_PER_YEAR);

test('first observation seeds σ as |r| · √yearly and rawVar as r²', () => {
  const r = 0.001;
  const next = nextVol(INITIAL_VOL_STATE, r, 0.94);
  const expectedSigma = Math.abs(r) * YEARLY_ROOT;
  expect(Math.abs(next.sigmaAnnualized - expectedSigma)).toBeLessThan(1e-6);
  expect(Math.abs(next.rawVarianceTickSq - r * r)).toBeLessThan(1e-12);
  expect(next.lastLogReturn).toBe(r);
});

test('λ = 1 throws InvalidLambda', () => {
  expect(() => nextVol(INITIAL_VOL_STATE, 0.01, 1.0)).toThrow(InvalidLambda);
});

test('λ outside [0, 1) throws InvalidLambda', () => {
  expect(() => nextVol(INITIAL_VOL_STATE, 0.01, -0.1)).toThrow(InvalidLambda);
  expect(() => nextVol(INITIAL_VOL_STATE, 0.01, 1.5)).toThrow(InvalidLambda);
});

test('NaN log return throws InvalidLogReturn (must surface — caller pauses taps)', () => {
  const s: VolState = { rawVarianceTickSq: 1e-8, sigmaAnnualized: 0.5, lastLogReturn: 0.001 };
  expect(() => nextVol(s, Number.NaN, 0.94)).toThrow(InvalidLogReturn);
});

test('spike (|r| > 5·prev_per_sec) bumps σ but does NOT corrupt rawVariance', () => {
  // Seed with a low-vol regime so the spike threshold is small.
  let s: VolState = INITIAL_VOL_STATE;
  for (let i = 0; i < 50; i++) s = nextVol(s, 0.0001, 0.94);
  const baselineRawVar = s.rawVarianceTickSq;
  const prevPerSec = s.sigmaAnnualized / YEARLY_ROOT;
  const spike = 10 * prevPerSec;
  const after = nextVol(s, spike, 0.94);
  // σ is bumped to at least |spike| · √yearly
  const expectedSigmaFloor = Math.abs(spike) * YEARLY_ROOT;
  expect(after.sigmaAnnualized).toBeGreaterThanOrEqual(expectedSigmaFloor - 1e-9);
  // rawVariance is the pure EWMA — bounded by λ·baseline + (1-λ)·spike²
  const expectedRawVar = 0.94 * baselineRawVar + 0.06 * spike * spike;
  expect(Math.abs(after.rawVarianceTickSq - expectedRawVar)).toBeLessThan(1e-12);
});

test('post-spike, next quiet tick does NOT inherit the bumped σ in rawVariance', () => {
  // Regression for the "bumped σ feeds back into EWMA" bug.
  let s: VolState = INITIAL_VOL_STATE;
  for (let i = 0; i < 50; i++) s = nextVol(s, 0.0001, 0.94);
  const rawBeforeSpike = s.rawVarianceTickSq;
  const prevPerSec = s.sigmaAnnualized / YEARLY_ROOT;
  // Spike
  s = nextVol(s, 10 * prevPerSec, 0.94);
  const rawAfterSpike = s.rawVarianceTickSq;
  // Now several quiet ticks. rawVariance should decay back toward the baseline,
  // not stay elevated as it would if σ_bumped had been written back.
  for (let i = 0; i < 100; i++) s = nextVol(s, 0.0001, 0.94);
  // After 100 ticks at λ=0.94, the spike contribution has decayed by 0.94^100 ≈ 2e-3.
  expect(s.rawVarianceTickSq).toBeLessThan(rawAfterSpike);
  // And rawVariance has returned within ~5% of the pre-spike steady state.
  expect(Math.abs(s.rawVarianceTickSq - rawBeforeSpike) / rawBeforeSpike).toBeLessThan(0.05);
});

test('non-spike return leaves σ near EWMA path (no bump)', () => {
  let s: VolState = INITIAL_VOL_STATE;
  for (let i = 0; i < 50; i++) s = nextVol(s, 0.0001, 0.94);
  const prevPerSec = s.sigmaAnnualized / YEARLY_ROOT;
  const small = 0.5 * prevPerSec;
  const next = nextVol(s, small, 0.94);
  expect(Math.abs(next.sigmaAnnualized - s.sigmaAnnualized) / s.sigmaAnnualized)
    .toBeLessThan(0.5);
});
