import { assetStore } from './asset-store';
import { env } from './env';
import { tickStore, type OracleTick } from './tick-store';
import { positionsStore } from './positions-store';
import { queryClient } from './query-client';

let socket: WebSocket | null = null;
let reconnectAttempts = 0;
let reconnectTimer: ReturnType<typeof setTimeout> | null = null;

function backoffMs(): number {
  return Math.min(4000, 250 * 2 ** reconnectAttempts);
}

function scheduleReconnect() {
  if (reconnectTimer) return;
  tickStore.setWsState('reconnecting');
  reconnectTimer = setTimeout(() => {
    reconnectTimer = null;
    reconnectAttempts += 1;
    connect();
  }, backoffMs());
}

function parseTick(msg: unknown): OracleTick | null {
  if (!msg || typeof msg !== 'object') return null;
  const m = msg as Record<string, unknown>;
  if (m.type !== 'tick') return null;
  if (
    typeof m.asset !== 'string' ||
    typeof m.run_id !== 'number' ||
    typeof m.seq !== 'number' ||
    typeof m.ts_ms !== 'number' ||
    typeof m.mid !== 'number' ||
    typeof m.vol_annualized !== 'number' ||
    typeof m.source_count !== 'number'
  ) {
    return null;
  }
  // The aggregator multiplexes all assets on one stream with no server-side
  // filter, so we keep only the selected quote here. Switching asset (asset-store)
  // forces a reconnect below, which reseeds history for the new asset.
  if (m.asset !== assetStore.getSnapshot()) return null;
  return {
    asset: m.asset as OracleTick['asset'],
    run_id: m.run_id,
    seq: m.seq,
    ts_ms: m.ts_ms,
    mid: m.mid,
    vol_annualized: m.vol_annualized,
    source_count: m.source_count,
  };
}

export function connect() {
  if (socket && (socket.readyState === WebSocket.OPEN || socket.readyState === WebSocket.CONNECTING)) {
    return;
  }
  tickStore.setWsState('connecting');
  socket = new WebSocket(env.wsUrl);
  socket.onopen = () => {
    reconnectAttempts = 0;
    tickStore.reset();
  };
  socket.onmessage = (ev) => {
    try {
      const msg = JSON.parse(ev.data as string);
      // Connect-time history backfill: seed the chart with the real recent
      // price shape so a fresh load shows the prior line, not a flat seed.
      if (msg && typeof msg === 'object' && (msg as { type?: unknown }).type === 'history') {
        const raw = (msg as { ticks?: unknown }).ticks;
        if (Array.isArray(raw)) {
          const sel = assetStore.getSnapshot();
          const points = raw
            .filter((t): t is { asset: string; ts_ms: number; mid: number } =>
              !!t && typeof t === 'object' && (t as { asset?: unknown }).asset === sel)
            .map((t) => ({ ts_ms: t.ts_ms, mid: t.mid }));
          tickStore.seedTrail(points);
        }
        return;
      }
      const tick = parseTick(msg);
      if (tick) tickStore.push(tick);
      // status and heartbeat frames are observed but not consumed in v0.
    } catch {
      // drop garbled frames
    }
  };
  socket.onclose = () => {
    scheduleReconnect();
  };
  socket.onerror = () => {
    socket?.close();
  };
}

export function disconnect() {
  if (reconnectTimer) clearTimeout(reconnectTimer);
  reconnectTimer = null;
  socket?.close();
  socket = null;
  tickStore.setWsState('closed');
}

// Reconnect when the quote asset changes: the new connection's onopen resets the
// store and its history frame reseeds the trail for the new asset, the same path
// the visibility handler uses. parseTick + the history filter already key off the
// live selection, so the socket comes back showing only the new asset.
assetStore.subscribe(() => {
  reconnectAttempts = 0;
  disconnect();
  connect();
});

if (typeof document !== 'undefined') {
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState !== 'visible') return;
    // 1. Force reconnect; the new onopen resets tickStore.
    disconnect();
    connect();
    // 2. Refetch /v1/me so balance is current after the tab was idle.
    queryClient.invalidateQueries({ queryKey: ['me'] });
    queryClient.invalidateQueries({ queryKey: ['me', 'history'] });
    // 3 & 4. Re-poke each LOCKED position once; drop stale PENDINGs.
    const ps = positionsStore.getSnapshot();
    ps.forEach((p, k) => {
      if (p.state === 'LOCKED' && p.positionId !== undefined) {
        queryClient.invalidateQueries({ queryKey: ['position', p.positionId] });
      }
      if (p.state === 'PENDING' && Date.now() > p.tCloseMs + 10_000) {
        positionsStore.update(k, { state: 'REJECTED', rejectReason: 'visibility_timeout' });
        setTimeout(() => positionsStore.delete(k), 800);
      }
    });
  });
}
