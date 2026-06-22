import { BETA_BGK } from './constants';

/** Mirror of tap_trading_pricing_engine::bgk::apply_bgk_correction. */
export function applyBgkCorrection(
  l: number,
  h: number,
  sigmaPerSec: number,
  tauSec: number,
  m: number,
): [number, number] {
  if (m <= 0 || tauSec <= 0) return [l, h];
  const shift = BETA_BGK * sigmaPerSec * Math.sqrt(tauSec / m);
  return [l * Math.exp(-shift), h * Math.exp(shift)];
}
