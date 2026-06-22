import { SECONDS_PER_YEAR } from './constants';

export class InvalidLambda extends Error {
  constructor(public lambda: number) { super(`invalid lambda: ${lambda}`); }
}
export class InvalidLogReturn extends Error {
  constructor(public value: number) { super(`invalid log return: ${value}`); }
}
/** Mirrors PricingError::InsufficientHistory — empty log-return slice. */
export class InsufficientHistory extends Error {
  constructor() { super('insufficient history'); }
}

export interface VolState {
  /** Accumulator — feeds the next EWMA step. Bumping does NOT touch this. */
  rawVarianceTickSq: number;
  /** What consumers read; may be bumped above raw on spikes. */
  sigmaAnnualized: number;
  lastLogReturn: number | null;
}

const YEARLY_ROOT = Math.sqrt(SECONDS_PER_YEAR);

/**
 * Streaming-style port of `tap_trading_pricing_engine::vol`. Two-step:
 *   1. EWMA on raw variance:
 *        cold-start (lastLogReturn === null): rawVar := r²
 *        steady:                                rawVar := λ·rawVar_prev + (1 − λ)·r²
 *      `rawAnnual = √rawVar · √SECONDS_PER_YEAR`.
 *   2. jump_adjusted_sigma: if `|r| > 5 · (prev_σ / √yearly)`, bump the
 *      displayed σ to `max(rawAnnual, |r| · √yearly)`. The raw variance is
 *      NOT bumped — that's the parity-critical detail.
 *
 * Throws on bad inputs (mirrors PricingError::InvalidLambda / InvalidLogReturn).
 */
export function nextVol(prev: VolState, newLogReturn: number, lambda: number): VolState {
  if (!(lambda >= 0 && lambda < 1)) throw new InvalidLambda(lambda);
  if (!Number.isFinite(newLogReturn)) throw new InvalidLogReturn(newLogReturn);

  const r2 = newLogReturn * newLogReturn;
  const rawVarianceTickSq = prev.lastLogReturn === null
    ? r2
    : lambda * prev.rawVarianceTickSq + (1 - lambda) * r2;

  const rawAnnual = Math.sqrt(Math.max(0, rawVarianceTickSq)) * YEARLY_ROOT;

  // jump_adjusted_sigma — read prev displayed σ for the spike threshold,
  // emit max(raw, bumped) for the displayed value. Does NOT touch rawVariance.
  const isSpike = prev.sigmaAnnualized > 0
    && Math.abs(newLogReturn) > 5 * (prev.sigmaAnnualized / YEARLY_ROOT);
  const bumped = isSpike ? Math.abs(newLogReturn) * YEARLY_ROOT : 0;
  const sigmaAnnualized = Math.max(rawAnnual, bumped);

  return { rawVarianceTickSq, sigmaAnnualized, lastLogReturn: newLogReturn };
}

export const INITIAL_VOL_STATE: VolState = {
  rawVarianceTickSq: 0,
  sigmaAnnualized: 0,
  lastLogReturn: null,
};
