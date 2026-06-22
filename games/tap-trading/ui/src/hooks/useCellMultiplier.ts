import { tickStore } from '@/lib/tick-store';
import { cellKey } from '@/lib/time';
import { InvalidSigma, InvalidSpot } from '@/pricing/errors';
import { HuiConvergenceFailure } from '@/pricing/hui';
import { computeMultiplier } from '@/pricing/multiplier';
import { type Cell, DEFAULT_PRICING_CONFIG } from '@/pricing/types';
import { useSyncExternalStore } from 'react';

interface CacheEntry {
  bucketMs: number;
  // The committed *displayed* multiplier, held across buckets so the rounding
  // deadband can suppress sub-threshold flicker (see DEADBAND below).
  shown: number | null;
}

const CACHE_CAP = 256;
const cache = new Map<string, CacheEntry>();

// Hysteresis band on the displayed value, just over half the 1-decimal display
// quantum (0.1×). The committed `shown` value only jumps to a freshly computed
// one once they differ by this much, so a multiplier hovering on a rounding
// boundary stops strobing (1.6↔1.7) while genuine drift still steps through.
const DEADBAND = 0.07;

// 10 Hz display cadence (MATH_SPEC §4.2 `display_refresh_ms = 100`). The oracle
// feed pushes at 20–50 Hz; bucketing the tick's own clock to 100 ms holds each
// cell's value steady between sub-bucket ticks, so `useSyncExternalStore`
// returns an unchanged value and React skips the re-render until the next
// boundary. The smoothed spot (see the selector) is sampled once per bucket, at
// recompute time.
function bucket(tsMs: number): number {
  return Math.floor(tsMs / 100) * 100;
}

function cachePut(ck: string, entry: CacheEntry) {
  if (cache.has(ck)) cache.delete(ck);
  cache.set(ck, entry);
  if (cache.size > CACHE_CAP) {
    const oldest = cache.keys().next().value;
    if (oldest !== undefined) cache.delete(oldest);
  }
}

export function useCellMultiplier(cell: Cell): number | null {
  const ck = cellKey(cell.strike_lo, cell.strike_hi, cell.t_open_ms);
  return useSyncExternalStore(tickStore.subscribe, () => {
    const snap = tickStore.getSnapshot();
    const tick = snap.tick;
    if (!tick || !Number.isFinite(tick.mid)) return null;

    const b = bucket(tick.ts_ms);
    const cached = cache.get(ck);
    if (cached && cached.bucketMs === b) return cached.shown;

    // Price the preview off the raw settled mid — the SAME value the chart line
    // now draws and the server settles. (The line no longer carries a temporal
    // EMA, and the feed is denoised upstream at the aggregator, so the old
    // strobe-against-a-still-line problem is gone.) The 100 ms bucket + deadband
    // below absorb any residual sub-cent flicker. σ and "now" come from the tick.
    // NB: the *locked* multiplier is the server's value at `oracle_seq_at_tap`
    // (see useTap); this is the live preview only.
    const spot = tick.mid;
    let raw: number | null;
    try {
      const oracle = {
        asset: 'ETH' as const,
        spot,
        sigma_annualized: tick.vol_annualized,
        timestamp_ms: tick.ts_ms,
      };
      const m = computeMultiplier(cell, oracle, DEFAULT_PRICING_CONFIG, tick.ts_ms);
      raw = m > 0 ? Math.round(m * 100) / 100 : null;
    } catch (e) {
      if (
        e instanceof InvalidSpot ||
        e instanceof InvalidSigma ||
        e instanceof HuiConvergenceFailure
      ) {
        raw = null;
      } else {
        throw e;
      }
    }

    // Deadband: keep the committed `shown` value unless the fresh one cleared the
    // band, suppressing boundary flicker while letting real moves step through.
    const prev = cached?.shown ?? null;
    const shown = raw === null || prev === null || Math.abs(raw - prev) >= DEADBAND ? raw : prev;
    cachePut(ck, { bucketMs: b, shown });
    return shown;
  });
}
