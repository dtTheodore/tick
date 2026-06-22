import { assetStore } from '@/lib/asset-store';
import { positionsStore } from '@/lib/positions-store';
import { tickStore } from '@/lib/tick-store';
import { CELL_DURATION_MS, cellKey, fitStrikeStep, roundStrike, strikeLadder } from '@/lib/time';
import type { Cell } from '@/pricing/types';
import { useEffect, useRef, useState } from 'react';

// Pacifica mechanic: rounds live on the chart's own timeline. Columns are keyed
// by t_open and flow leftward over wall-clock time. FUTURE rounds (t_open > now)
// sit right of the now-line and are bettable; as a round crosses the now-line the
// price line draws straight through it, and PAST rounds keep scrolling left
// (showing their settled result) until they leave the viewport. We emit a fixed
// band of past + future columns; Grid positions each by time and clips the ones
// off-screen, so the exact count only needs to exceed the widest viewport.
// Widen the visible price band (more strike rows ⇒ larger ROWS·step range, which
// also scales with σ through `step`) so the price line uses a comfortable
// fraction of the height instead of shooting across it. This is what makes the
// line read calm WITHOUT lagging it off the real price (see PriceLine LINE_TAU).
const ROWS = 15;
// How many rounds to keep mounted on each side of the now-line. Past must outlast
// the time a settled round takes to scroll off the left of the widest chart;
// future must fill the bettable region to the right of the now-line.
const PAST_COLS = 8;
const FUTURE_COLS = 8;
const FALLBACK_STEP = 0.5;

interface VisibleGrid {
  columns: Cell[][]; // outer = time slots (oldest→future), inner = strike rows
  columnKeys: string[][];
  ladder: number[];
  step: number; // strike-band width; one grid row = one step (price→y unit)
  center: number | null; // ladder center strike (ladder[half]); chart shares this axis
  rows: number;
  nowMs: number;
}

// `historyHorizonMs` is the on-screen past span (from Grid). The ladder step is
// fitted to exactly this window so it sizes the line we actually draw — fit it to
// less and old history clamps flat at an edge; fit to more and the line shrinks
// into a thin band.
export function useVisibleCells(currentMid: number | null, historyHorizonMs: number): VisibleGrid {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    // The per-frame slide is a CSS transform (see Grid); this interval only
    // drives which columns exist (changes once per CELL boundary) and the
    // multiplier-refresh cadence. 200 ms is plenty.
    const id = setInterval(() => setNow(Date.now()), 200);
    return () => clearInterval(id);
  }, []);
  const snap = tickStore.getSnapshot();
  const asset = assetStore.getSnapshot();

  // Ladder state persists across renders via refs. On a history reseed
  // (seedEpoch bump — fresh load, tab-away reconnect, OR an asset switch) it
  // must reset: a $20 BTC step and a $64k center are nonsense for a $3.50 SUI,
  // and the re-tier/recenter hysteresis would hold the stale scale for seconds.
  // Clearing the refs makes the first tick of the new feed re-fit from scratch.
  const appliedStepRef = useRef<number | null>(null);
  const pendingTierRef = useRef<{ step: number; since: number } | null>(null);
  const ladderCenterRef = useRef<number | null>(null);
  const seedEpochRef = useRef(snap.seedEpoch);
  if (snap.seedEpoch !== seedEpochRef.current) {
    seedEpochRef.current = snap.seedEpoch;
    appliedStepRef.current = null;
    pendingTierRef.current = null;
    ladderCenterRef.current = null;
  }

  // Visible price extremes over the chart's history window, so the ladder step
  // fits the drawn line into the frame (see fitStrikeStep) instead of letting it
  // clamp at an edge. Bounded work: the trail is capped (≤ ~3000 points).
  const cutoff = Date.now() - historyHorizonMs;
  let vMin = currentMid ?? 0;
  let vMax = currentMid ?? 0;
  for (const p of snap.trail) {
    if (p.ts_ms < cutoff) continue;
    if (p.mid < vMin) vMin = p.mid;
    if (p.mid > vMax) vMax = p.mid;
  }

  // `fitStrikeStep` picks the *wanted* tier from the realized range with a fixed
  // (σ-independent) floor, so a calm market sits on a stable scale. On top of its
  // spatial hysteresis we add ASYMMETRIC TEMPORAL hysteresis before ADOPTING a
  // re-tier, because a re-tier is an instant ×2 rescale of the whole view —
  // jarring if it fires on a transient. So a brief range spike out-and-back
  // doesn't snap the axis:
  //   - zoom OUT (bigger step) after only a short hold — a real move must not
  //     sit clipped at the chart edge waiting for the axis to widen;
  //   - zoom IN (smaller step) only after a long sustained-calm hold — zooming
  //     in is cosmetic, and being eager is what makes the axis flap.
  const RETIER_OUT_HOLD_MS = 800;
  const RETIER_IN_HOLD_MS = 5_000;
  // Freeze the tier while a bet is held. A re-tier changes the ladder's band
  // boundaries (strikes snap to multiples of the new step), so a held position's
  // cell — keyed by its exact [strikeLo, strikeHi) — is no longer in the ladder
  // and orphans: no rendered Cell carries that cellKey, so its WIN can't flash and
  // its result can't show ("price reaches the cell but nothing happens"). Holding
  // the scale until every position settles keeps held bands valid; a single
  // betting session shares one frozen step, so concurrent holds never desync.
  const hasOpenPosition = (() => {
    for (const p of positionsStore.getSnapshot().values()) {
      if (p.state === 'LOCKED' || p.state === 'PENDING') return true;
    }
    return false;
  })();
  if (currentMid !== null) {
    const wanted = fitStrikeStep(currentMid, vMin, vMax, ROWS, appliedStepRef.current);
    const applied = appliedStepRef.current;
    if (applied === null || wanted === applied) {
      appliedStepRef.current = wanted;
      pendingTierRef.current = null;
    } else if (hasOpenPosition) {
      // A re-tier is wanted but a bet is live — defer it; re-evaluate once flat.
      pendingTierRef.current = null;
    } else {
      const holdMs = wanted > applied ? RETIER_OUT_HOLD_MS : RETIER_IN_HOLD_MS;
      if (pendingTierRef.current?.step !== wanted) {
        pendingTierRef.current = { step: wanted, since: now };
      } else if (now - pendingTierRef.current.since >= holdMs) {
        appliedStepRef.current = wanted;
        pendingTierRef.current = null;
      }
    }
  }
  const step = appliedStepRef.current ?? FALLBACK_STEP;

  // Ladder hysteresis: lock the center until spot drifts more than one full step.
  if (currentMid !== null) {
    const cur = ladderCenterRef.current;
    if (cur === null || Math.abs(currentMid - cur) > step) {
      ladderCenterRef.current = Math.round(currentMid / step) * step;
    }
  }
  const stableCenter = ladderCenterRef.current ?? currentMid;
  const ladder =
    stableCenter !== null ? strikeLadder(stableCenter, ROWS, step) : Array(ROWS).fill(0);

  // liveOpenMs = t_open of the in-play round (the one currently straddling the
  // now-line). c < 0 are past/settled rounds scrolling off the left; c = 0 is in
  // play; c ≥ 1 are future bettable rounds to the right of the now-line.
  const liveOpenMs = Math.floor(now / CELL_DURATION_MS) * CELL_DURATION_MS;

  const columns: Cell[][] = [];
  const columnKeys: string[][] = [];
  for (let c = -PAST_COLS; c <= FUTURE_COLS; c++) {
    const tOpen = liveOpenMs + c * CELL_DURATION_MS;
    const tClose = tOpen + CELL_DURATION_MS;
    const colCells: Cell[] = [];
    const colKeys: string[] = [];
    for (let r = 0; r < ROWS; r++) {
      const strikeLo = ladder[r];
      // Round the upper band edge to the step's precision too — `lo + step`
      // reintroduces float drift that would otherwise desync the cellKey.
      const strikeHi = roundStrike(ladder[r] + step, step);
      colCells.push({
        asset,
        strike_lo: strikeLo,
        strike_hi: strikeHi,
        t_open_ms: tOpen,
        t_close_ms: tClose,
      });
      colKeys.push(currentMid !== null ? cellKey(strikeLo, strikeHi, tOpen) : `loading-${c}-${r}`);
    }
    columns.push(colCells);
    columnKeys.push(colKeys);
  }
  return {
    columns,
    columnKeys,
    ladder,
    step,
    center: stableCenter,
    rows: ROWS,
    nowMs: now,
  };
}
