import { useLiveHitDetection } from '@/hooks/useLiveHitDetection';
import { usePositionPoll } from '@/hooks/usePositionPoll';
import { demoEngine } from '@/lib/demo-engine';
import { useGameMode } from '@/lib/game-mode';
import { type CellPosition, positionsStore } from '@/lib/positions-store';
import { useEffect, useState } from 'react';

// usePositionPoll lives at cell scope today, but cells unmount when their
// window opens (5 s before settlement runs server-side), so the lock state
// would never see WON/LOST/VOIDED. This component mirrors the positions store
// at app scope and keeps a usePositionPoll alive for each LOCKED position
// until its terminal status arrives.

function PolledPosition({ p }: { p: CellPosition }) {
  usePositionPoll(p.positionId, p.cellKey, p.tCloseMs);
  return null;
}

// Demo positions have no server to poll: settle them locally just after each
// window closes. A single interval (not a poll-per-position) covers them all.
function useDemoSettlement() {
  const mode = useGameMode();
  useEffect(() => {
    if (mode !== 'demo') return;
    const id = setInterval(() => demoEngine.settleDue(Date.now()), 250);
    return () => clearInterval(id);
  }, [mode]);
}

export function PositionTracker() {
  useLiveHitDetection();
  useDemoSettlement();
  const [positions, setPositions] = useState<CellPosition[]>([]);
  useEffect(() => {
    const refresh = () => {
      // Only live positions get a server poll; demo positions settle via the
      // interval above.
      const arr = Array.from(positionsStore.getSnapshot().values()).filter(
        (p) => p.state === 'LOCKED' && p.positionId !== undefined && !p.demo,
      );
      setPositions(arr);
    };
    refresh();
    const unsubscribe = positionsStore.subscribe(refresh);
    return () => {
      unsubscribe();
    };
  }, []);
  return (
    <>
      {positions.map((p) => (
        <PolledPosition key={p.positionId} p={p} />
      ))}
    </>
  );
}
