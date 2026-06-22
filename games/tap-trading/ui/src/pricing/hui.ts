export class InvalidTerms extends Error {
  constructor() { super('hui_no_touch: terms must be > 0'); }
}
export class InvalidBand extends Error {
  constructor(public l: number, public h: number) {
    super(`hui_no_touch: invalid band l=${l} h=${h}`);
  }
}
export class HuiConvergenceFailure extends Error {
  constructor(public lastTermMag: number, public terms: number) {
    super(`hui series failed to converge: last_term_mag=${lastTermMag} terms=${terms}`);
  }
}

const CONVERGENCE_TOLERANCE = 1e-4;

export function huiNoTouch(
  s0: number, l: number, h: number, sigmaPerSec: number, tauSec: number, terms: number,
): number {
  if (terms === 0) throw new InvalidTerms();
  if (!(l > 0 && h > l
        && Number.isFinite(s0) && Number.isFinite(sigmaPerSec) && Number.isFinite(tauSec))) {
    throw new InvalidBand(l, h);
  }
  if (tauSec <= 0) return 1.0;
  if (s0 <= l || s0 >= h) return 0.0;

  const alpha = 0.5;
  const beta = -0.25;
  const z = Math.log(h / l);
  const logS0OverL = Math.log(s0 / l);
  const s0OverLAlpha = Math.pow(s0 / l, alpha);
  const s0OverHAlpha = Math.pow(s0 / h, alpha);

  let sum = 0;
  let lastTermMag = Infinity;

  for (let n = 1; n <= terms; n++) {
    const piNOverZ = (Math.PI * n) / z;
    const sign = n % 2 === 0 ? 1 : -1;
    const numerator = ((2 * Math.PI * n) / (z * z)) * (s0OverLAlpha - sign * s0OverHAlpha);
    const denominator = alpha * alpha + piNOverZ * piNOverZ;
    const sinTerm = Math.sin(piNOverZ * logS0OverL);
    const expArg = -0.5 * (piNOverZ * piNOverZ - beta) * sigmaPerSec * sigmaPerSec * tauSec;
    const expTerm = Math.exp(expArg);

    const term = (numerator / denominator) * sinTerm * expTerm;
    sum += term;
    lastTermMag = Math.abs(term);

    if (lastTermMag < CONVERGENCE_TOLERANCE && n >= 3) {
      return Math.max(0, Math.min(1, sum));
    }
  }

  throw new HuiConvergenceFailure(lastTermMag, terms);
}
