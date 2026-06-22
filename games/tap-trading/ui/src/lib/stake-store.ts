// The player's selected stake per tap, in USDC micro-units (1e6 = $1).
//
// Bounds mirror the backend (`validation.rs` MIN/MAX_STAKE_MICRO): the server
// accepts any amount in [MIN, MAX], so the UI offers quick presets plus a custom
// value in the same range. Persisted to localStorage so the bet size survives a
// reload. Affordability (balance) is enforced server-side at tap time.
const STORAGE_KEY = 'tick.stakeMicro';

/** Quick-pick chips, in micro-USDC: $0.10 · $1 · $5 · $10. */
export const STAKE_PRESETS_MICRO = [100_000, 1_000_000, 5_000_000, 10_000_000] as const;

export const MIN_STAKE_MICRO = 100; // $0.0001 dust floor
export const MAX_STAKE_MICRO = 10_000_000; // $10 cap

// $1 — a real, legible default bet.
const DEFAULT_STAKE_MICRO = 1_000_000;

/** True if `v` is an integer micro-amount the backend will accept. */
export function isValidStake(v: number): boolean {
  return Number.isInteger(v) && v >= MIN_STAKE_MICRO && v <= MAX_STAKE_MICRO;
}

function load(): number {
  try {
    const raw = Number(localStorage.getItem(STORAGE_KEY));
    return isValidStake(raw) ? raw : DEFAULT_STAKE_MICRO;
  } catch {
    return DEFAULT_STAKE_MICRO;
  }
}

let current = load();
const listeners = new Set<() => void>();

export const stakeStore = {
  subscribe(cb: () => void) {
    listeners.add(cb);
    return () => listeners.delete(cb);
  },
  /** Current stake in micro-USDC. Read at tap time so the latest selection wins. */
  get(): number {
    return current;
  },
  set(micro: number) {
    if (!isValidStake(micro) || micro === current) return;
    current = micro;
    try {
      localStorage.setItem(STORAGE_KEY, String(micro));
    } catch {
      /* storage unavailable (private mode) — selection still applies in-memory */
    }
    for (const cb of listeners) cb();
  },
};
