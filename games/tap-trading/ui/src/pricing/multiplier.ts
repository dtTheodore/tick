import { EPSILON, SECONDS_PER_YEAR } from './constants';
import { erfc } from './erfc';
import { InvalidSpot, InvalidSigma } from './errors';
import { type Cell, type OracleState, type PricingConfig } from './types';

const SQRT_2 = Math.sqrt(2);

// Fixed Gauss-weighted grid for the t_open spot integral. MUST be byte-identical
// to the Rust port (`multiplier.rs`). Midpoint rule over a ±6σ-truncated normal;
// the 1/√(2π) normalisation cancels in `acc / wsum`.
const Z_LO = -6.0;
const Z_HI = 6.0;
const Z_STEPS = 64;

/** Standard normal CDF Φ(x) = ½·erfc(−x/√2). Mirrors `multiplier::normal_cdf`. */
function normalCdf(x: number): number {
  return 0.5 * erfc(-x / SQRT_2);
}

/**
 * Reflection-principle first-passage touch probability for OTM regimes.
 * Mirrors `tap_trading_pricing_engine::first_passage_touch_prob`
 * (multiplier.rs:77-84).
 *   P(touch ≤ τ) = 2·Φ(−|ln(barrier/s₀)| / (σ_sec · √τ))
 */
export function firstPassageTouchProb(
  s0: number, barrier: number, sigmaPerSec: number, tauSec: number,
): number {
  if (sigmaPerSec <= 0 || tauSec <= 0) return 0;
  const b = Math.abs(Math.log(barrier / s0));
  const v = sigmaPerSec * Math.sqrt(tauSec);
  return Math.max(0, Math.min(1, 2 * normalCdf(-b / v)));
}

/**
 * Continuous-monitoring touch probability of band `[lo, hi)` over `tauSec` from
 * start `s`. In-band → 1.0; else first-passage to the near edge only (far edge /
 * width are irrelevant under first-touch). Mirrors Rust `touch_prob_from`.
 */
function touchProbFrom(s: number, lo: number, hi: number, sigmaPerSec: number, tauSec: number): number {
  if (s >= lo && s < hi) return 1;
  if (sigmaPerSec <= 0 || tauSec <= 0) return 0;
  const barrier = s < lo ? lo : hi;
  return Math.max(0, Math.min(1, firstPassageTouchProb(s, barrier, sigmaPerSec, tauSec)));
}

/**
 * Mirror of `tap_trading_pricing_engine::compute_p_touch` (multiplier.rs).
 *
 * Probability the continuous price path enters `[lo, hi)` during the FUTURE
 * window `[t_open, t_close]`. The price at t_open is random — `S(t_open) =
 * S₀·exp(σ_o·Z − σ_o²/2)`, `σ_o = σ_sec·√τ_open` — so we integrate the
 * continuous touch prob over that distribution (fixed ±6σ midpoint grid,
 * identical to Rust). No BGK (settlement is continuous-path). For τ_open = 0 the
 * integral collapses to a single evaluation (and keeps fixture parity exact).
 */
export function computePTouch(
  cell: Cell, oracle: OracleState, cfg: PricingConfig, nowMs: number,
): number {
  if (!(Number.isFinite(oracle.spot) && oracle.spot > 0)) {
    throw new InvalidSpot(oracle.spot);
  }
  if (!(Number.isFinite(oracle.sigma_annualized) && oracle.sigma_annualized >= 0)) {
    throw new InvalidSigma(oracle.sigma_annualized);
  }
  if (cell.t_close_ms <= nowMs) return 0;

  const sigmaPerSec = (oracle.sigma_annualized / Math.sqrt(SECONDS_PER_YEAR)) * cfg.jump_buffer;
  const tOpenEff = Math.max(cell.t_open_ms, nowMs);
  const tauOpenSec = (tOpenEff - nowMs) / 1000;
  const tauWinSec = (cell.t_close_ms - tOpenEff) / 1000;

  if (tauOpenSec <= 0) {
    return touchProbFrom(oracle.spot, cell.strike_lo, cell.strike_hi, sigmaPerSec, tauWinSec);
  }

  const sigmaOpen = sigmaPerSec * Math.sqrt(tauOpenSec);
  const dz = (Z_HI - Z_LO) / Z_STEPS;
  let acc = 0;
  let wsum = 0;
  for (let k = 0; k < Z_STEPS; k++) {
    const z = Z_LO + (k + 0.5) * dz;
    const w = Math.exp(-0.5 * z * z);
    const sOpen = oracle.spot * Math.exp(sigmaOpen * z - 0.5 * sigmaOpen * sigmaOpen);
    acc += w * touchProbFrom(sOpen, cell.strike_lo, cell.strike_hi, sigmaPerSec, tauWinSec);
    wsum += w;
  }
  return Math.max(0, Math.min(1, acc / wsum));
}

/**
 * Mirror of `tap_trading_pricing_engine::compute_multiplier` (multiplier.rs:95-115).
 *
 * Order of operations matches Rust exactly:
 *   1. closed cell (t_close <= now) → 0.0   (does NOT validate oracle)
 *   2. compute τ, floor
 *   3. computePTouch validates spot/sigma — throws InvalidSpot / InvalidSigma
 *   4. raw = p_touch < EPSILON ? cap : (1 - house_margin) / p_touch
 *   5. clamp into [floor, cap]
 */
export function computeMultiplier(
  cell: Cell, oracle: OracleState, cfg: PricingConfig, nowMs: number,
): number {
  if (cell.t_close_ms <= nowMs) return 0;

  const tauCloseSec = (cell.t_close_ms - nowMs) / 1000;
  const floor = cfg.floor_a + cfg.floor_b * tauCloseSec;

  const pTouch = computePTouch(cell, oracle, cfg, nowMs);
  const raw = pTouch < EPSILON
    ? cfg.multiplier_cap
    : (1 - cfg.house_margin) / pTouch;

  return Math.min(cfg.multiplier_cap, Math.max(floor, raw));
}
