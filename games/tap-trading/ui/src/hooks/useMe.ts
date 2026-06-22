import { useQuery } from '@tanstack/react-query';
import { useSyncExternalStore } from 'react';
import { api } from '@/lib/api';
import { demoEngine } from '@/lib/demo-engine';
import { useGameMode } from '@/lib/game-mode';

export interface MeResponse {
  account_id: number;
  external_id: string;
  balance: number;
  lifetime_points_won: number;
  tier: number;
  current_streak: number;
}

export interface HistorySettlement {
  outcome: string; // 'W' | 'L' | 'V'
  points_delta: number;
  settled_at_ms: number;
}

export interface HistoryItem {
  position_id: number;
  asset: string;
  strike_lo: number;
  strike_hi: number;
  t_open_ms: number;
  t_close_ms: number;
  stake_points: number;
  multiplier_at_tap: number;
  status: 'OPEN' | 'WON' | 'LOST' | 'VOIDED';
  created_at_ms: number;
  settlement: HistorySettlement | null;
  // Provability: present only for on-chain (USDC-mode) settlements. When
  // `proof_status === 'published'` the tap has a self-contained Walrus proof the
  // player can replay in-browser (see verify-proof.ts + VerifyDrawer). Proofs are
  // batched: one Walrus blob holds many settlements, and `proof_index` is this
  // tap's entry into the blob's `proofs` array.
  walrus_blob_id: string | null;
  proof_status: string | null;
  proof_index?: number;
}

export interface HistoryResponse {
  positions: HistoryItem[];
  next_cursor: number | null;
}

/** Live-mode balance. Disabled in demo, where balance is the local play-money
 *  engine (read directly via demoEngine in HeaderBar) and no network is touched. */
export function useMe() {
  const mode = useGameMode();
  return useQuery({
    queryKey: ['me'],
    queryFn: () => api<MeResponse>('/v1/me'),
    enabled: mode === 'live',
    refetchOnWindowFocus: true,
  });
}

/** Just the three fields the strip reads, so demo (store-backed) and live
 *  (query-backed) history return the same shape. */
export interface HistoryView {
  data: HistoryItem[] | undefined;
  isPending: boolean;
  isError: boolean;
}

/** Recent settled taps. Demo reads the local engine (reactive via its store);
 *  live unwraps the server envelope. */
export function useHistory(): HistoryView {
  const mode = useGameMode();
  const demoHistory = useSyncExternalStore(demoEngine.subscribe, demoEngine.getHistory);
  const live = useQuery({
    queryKey: ['me', 'history'],
    queryFn: async () => (await api<HistoryResponse>('/v1/me/history')).positions,
    enabled: mode === 'live',
    refetchOnWindowFocus: true,
  });
  if (mode === 'demo') return { data: demoHistory, isPending: false, isError: false };
  return { data: live.data, isPending: live.isPending, isError: live.isError };
}
