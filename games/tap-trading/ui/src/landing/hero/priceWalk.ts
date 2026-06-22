export interface PriceWalkOptions {
  seed: number;
  start: number;
  drift?: number; // per-step drift, default 0
  volatility?: number; // step size scale, default 0.5
  reversion?: number; // pull back toward start, default 0.01
}

export interface PriceWalk {
  step(): number; // advance one tick, return new price
  current(): number; // current price without advancing
  // Steer the walk toward `target` at `strength` (0..1 of the remaining gap per
  // step). null clears it. Lets the hero loop guarantee the line reaches a bet
  // band on a win round while the noise keeps it feeling live. Default off.
  setTarget(target: number | null, strength?: number): void;
}

// Mulberry32: tiny seedable PRNG — deterministic across runs/platforms so the
// hero animation is reproducible and tests are stable (no Math.random).
export function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

// A mean-reverting random walk that feels like a live crypto mid: small noisy
// steps that drift but get pulled back toward `start`, keeping the hero chart
// lively without wandering off-screen.
export function createPriceWalk(opts: PriceWalkOptions): PriceWalk {
  const { seed, start, drift = 0, volatility = 0.5, reversion = 0.01 } = opts;
  const rng = mulberry32(seed);
  let price = start;
  let target: number | null = null;
  let targetPull = 0;
  return {
    current: () => price,
    setTarget: (t, strength = 0.08) => {
      target = t;
      targetPull = strength;
    },
    step: () => {
      const shock = (rng() - 0.5) * 2 * volatility; // uniform in [-vol, vol]
      const pull = (start - price) * reversion; // mean reversion toward start
      const steer = target == null ? 0 : (target - price) * targetPull;
      price = price + drift + pull + steer + shock;
      return price;
    },
  };
}
