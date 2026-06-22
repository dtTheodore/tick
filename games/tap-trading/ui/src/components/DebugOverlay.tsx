import { useSyncExternalStore, useState, useEffect } from 'react';
import { useMe } from '@/hooks/useMe';
import { positionsStore, type CellPosition } from '@/lib/positions-store';
import { tickStore } from '@/lib/tick-store';

export function DebugOverlay() {
  const enabled =
    typeof window !== 'undefined' &&
    new URLSearchParams(window.location.search).get('debug') === '1';
  const snap = useSyncExternalStore(tickStore.subscribe, tickStore.getSnapshot);
  // useSyncExternalStore requires getSnapshot to return a stable reference when
  // the store hasn't changed. positionsStore returns a mutable Map (same ref always),
  // so we use useState+useEffect to safely derive an array snapshot on each emit.
  const [positions, setPositions] = useState<Array<[string, CellPosition]>>([]);
  useEffect(() => {
    const refresh = () => setPositions(Array.from(positionsStore.getSnapshot().entries()));
    refresh();
    const unsubscribe = positionsStore.subscribe(refresh);
    return () => {
      unsubscribe();
    };
  }, []);
  const me = useMe();

  if (!enabled) return null;

  return (
    <div className="fixed bottom-12 left-2 z-40 max-w-xs rounded border border-white/20 bg-black/70 p-3 font-mono text-[10px] text-white/80">
      <div>
        ws: {snap.wsState} · age{' '}
        {snap.wsLastEventMs ? Date.now() - snap.wsLastEventMs : 0}ms
      </div>
      <div>σ: {snap.tick ? snap.tick.vol_annualized.toFixed(4) : '—'}</div>
      <div>
        tick:{' '}
        {snap.tick
          ? `seq=${snap.tick.seq} mid=${snap.tick.mid.toFixed(2)}`
          : '—'}
      </div>
      <div>
        server bal: {me.data?.balance ?? '—'} · pending:{' '}
        {positionsStore.pendingStake()}
      </div>
      <div className="mt-1 border-t border-white/10 pt-1">
        positions ({positions.length}):
      </div>
      {positions.map(([k, p]) => (
        <div key={k}>
          {p.state} · {k}
        </div>
      ))}
    </div>
  );
}
