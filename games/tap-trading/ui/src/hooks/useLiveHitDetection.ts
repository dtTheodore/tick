import { positionsStore } from '@/lib/positions-store';
import { tickStore } from '@/lib/tick-store';
import { useEffect } from 'react';

// One rAF loop for the whole app. The first time the raw price PATH enters a
// cell's band during its window, it stamps `struckAtMs` — the moment the cell is
// "struck". The cell celebrates off this stamp (see Cell), ahead of the
// settlement poll, so the win pops the instant the price touches.
//
// CRITICAL — this MUST mirror the server's settlement (`settlement-worker
// touch.rs`): both judge the touch on the RAW oracle price PATH (the segment
// between consecutive ticks crosses `[strikeLo, strikeHi)`). The chart line now
// draws this same raw mid (PriceLine carries no temporal EMA), so cue, line, and
// server agree by construction — the cell celebrates exactly when the visible
// line enters its band, and the server then settles WON. Judging anything that
// lags the raw path (an EMA) would reopen the "touch not 100% correct" gap.
//
// Per-tick (by `seq`), not per-frame, so the segment we test is exactly the
// server's consecutive-tick segment. WON and the payout remain
// server-authoritative; this only drives *when* the cell plays its animation.
export function useLiveHitDetection() {
  useEffect(() => {
    let raf = 0;
    let prevMid: number | null = null;
    let prevSeq = -1;
    const tick = () => {
      const t = tickStore.getSnapshot().tick;
      if (t && t.seq !== prevSeq) {
        const cur = t.mid;
        // Path segment since the previous tick. On the first tick there is no
        // segment, so fall back to the single point (matches touch.rs).
        const segLo = prevMid === null ? cur : Math.min(prevMid, cur);
        const segHi = prevMid === null ? cur : Math.max(prevMid, cur);
        const now = Date.now();
        for (const p of positionsStore.getSnapshot().values()) {
          if (p.struckAtMs !== undefined) continue;
          if (p.state === 'LOST' || p.state === 'VOIDED' || p.state === 'REJECTED') continue;
          // Gate the window on the TICK's own timestamp, exactly like the server
          // (touch.rs tests the segment iff `tick.ts_ms ∈ [t_open, t_close]`). Using
          // the client wall-clock (Date.now) instead let clock skew flip a
          // boundary touch — the cue would strike a touch the server scored
          // out-of-window → a green flash that then settles LOST (the rare
          // "celebrates but didn't reach"). `struckAtMs` stays wall-clock — it only
          // times the animation, not the win decision.
          if (t.ts_ms < p.tOpenMs || t.ts_ms > p.tCloseMs) continue;
          // Segment [segLo, segHi] intersects the half-open band [strikeLo, strikeHi)
          // — identical predicate to touch.rs `segment_intersects_band`.
          if (segHi >= p.strikeLo && segLo < p.strikeHi) {
            positionsStore.update(p.cellKey, { struckAtMs: now });
          }
        }
        prevMid = cur;
        prevSeq = t.seq;
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, []);
}
