export interface OracleTick {
  asset: 'ETH' | 'BTC' | 'SUI';
  run_id: number;
  seq: number;
  ts_ms: number;
  mid: number;
  vol_annualized: number;
  source_count: number;
}

export type WsState = 'connecting' | 'open' | 'reconnecting' | 'closed';

interface Snapshot {
  tick: OracleTick | null;
  wsState: WsState;
  wsLastEventMs: number;
  trail: Array<{ ts_ms: number; mid: number }>;
  // Bumped every time the trail is re-seeded from a fresh connect-time history
  // snapshot. Lets the chart discard a buffer left stale by a tab-away gap and
  // re-seed from the new history, instead of stitching across the hole.
  seedEpoch: number;
}

// Hold enough history to fill the chart's visible past at any viewport width
// (≈106 s on an ultrawide) plus live-accumulation headroom. The drawn line and
// the vertical fit both window down to the on-screen past, so this only bounds
// the buffer. Count cap keeps a high-rate feed from growing it unbounded
// (~20 ETH ticks/s × 150 s ≈ 3000).
const TRAIL_WINDOW_MS = 150_000;
const TRAIL_MAX = 3_000;

type TrailPoint = { ts_ms: number; mid: number };

/** Drop points older than the window (by wall clock) and cap the count. Bounding
 *  by wall-clock age means a tab reopened minutes later never renders stale
 *  prices as "now" — old points fall outside the window and are discarded. */
function boundTrail(trail: TrailPoint[]): TrailPoint[] {
  const cutoff = Date.now() - TRAIL_WINDOW_MS;
  let i = 0;
  while (i < trail.length && trail[i].ts_ms < cutoff) i++;
  let out = i > 0 ? trail.slice(i) : trail;
  if (out.length > TRAIL_MAX) out = out.slice(out.length - TRAIL_MAX);
  return out;
}

let snapshot: Snapshot = {
  tick: null,
  wsState: 'closed',
  wsLastEventMs: 0,
  trail: [],
  seedEpoch: 0,
};

const listeners = new Set<() => void>();

function emit() {
  listeners.forEach((cb) => cb());
}

export const tickStore = {
  subscribe(cb: () => void) {
    listeners.add(cb);
    return () => {
      listeners.delete(cb);
    };
  },
  getSnapshot(): Snapshot {
    return snapshot;
  },
  push(tick: OracleTick) {
    const last = snapshot.trail[snapshot.trail.length - 1];
    // De-dup the seam: a tick already present in the connect-time history
    // snapshot can also arrive on the live stream once. Skip the redundant
    // point rather than drawing a zero-width segment.
    const trail = last && last.ts_ms >= tick.ts_ms
      ? snapshot.trail
      : boundTrail([...snapshot.trail, { ts_ms: tick.ts_ms, mid: tick.mid }]);
    snapshot = { tick, wsState: 'open', wsLastEventMs: Date.now(), trail, seedEpoch: snapshot.seedEpoch };
    emit();
  },
  /** Seed the trail from the server's connect-time history snapshot so the
   *  chart paints its real recent shape on load — for any client, not just
   *  ones with a warm localStorage. Replaces the trail (called right after a
   *  fresh connect resets it); the next live tick appends. */
  seedTrail(points: TrailPoint[]) {
    const sorted = points
      .filter((p) => Number.isFinite(p.ts_ms) && Number.isFinite(p.mid))
      .sort((a, b) => a.ts_ms - b.ts_ms);
    snapshot = { ...snapshot, trail: boundTrail(sorted), seedEpoch: snapshot.seedEpoch + 1 };
    emit();
  },
  setWsState(wsState: WsState) {
    snapshot = { ...snapshot, wsState };
    emit();
  },
  reset() {
    snapshot = { ...snapshot, tick: null, trail: [] };
    emit();
  },
};
