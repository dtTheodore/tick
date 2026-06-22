export type LifecycleState = 'PENDING' | 'LOCKED' | 'WON' | 'LOST' | 'VOIDED' | 'REJECTED';

export interface CellPosition {
  cellKey: string;
  clientRequestId: string;
  positionId?: number;
  stake: number;
  state: LifecycleState;
  multiplierAtTap?: number;
  rejectReason?: string;
  settledAtMs?: number;
  tOpenMs: number;
  tCloseMs: number;
  // The cell's strike band, kept so client-side hit detection can light the cell
  // the instant the raw price PATH enters [strikeLo, strikeHi) during its window
  // (see useLiveHitDetection — same raw-path predicate the server settles on).
  // Server settlement remains the sole authority for WON/payout.
  strikeLo: number;
  strikeHi: number;
  // Wall-clock of the first detected live touch. Visual cue only; never a payout.
  struckAtMs?: number;
  // Quote asset at tap time. Carried so demo (play-money) settlement can build a
  // faithful local history item; live history comes from the server, which
  // already knows the asset.
  asset?: string;
  // True when opened in demo mode: settles locally via demo-engine instead of the
  // server poll. Keeps demo and live positions cleanly separated in the shared
  // store across a mid-session mode flip.
  demo?: boolean;
}

const positions = new Map<string, CellPosition>();
const listeners = new Set<() => void>();
function emit() {
  for (const cb of listeners) cb();
}

export const positionsStore = {
  subscribe(cb: () => void) {
    listeners.add(cb);
    return () => listeners.delete(cb);
  },
  getSnapshot(): ReadonlyMap<string, CellPosition> {
    return positions;
  },
  get(cellKey: string): CellPosition | undefined {
    return positions.get(cellKey);
  },
  set(cellKey: string, p: CellPosition) {
    positions.set(cellKey, p);
    emit();
  },
  update(cellKey: string, patch: Partial<CellPosition>) {
    const cur = positions.get(cellKey);
    if (!cur) return;
    positions.set(cellKey, { ...cur, ...patch });
    emit();
  },
  delete(cellKey: string) {
    positions.delete(cellKey);
    emit();
  },
  pendingStake(): number {
    let s = 0;
    for (const p of positions.values()) {
      if (p.state === 'PENDING') s += p.stake;
    }
    return s;
  },
};
