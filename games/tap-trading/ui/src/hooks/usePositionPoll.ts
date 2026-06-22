import { useEffect } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '@/lib/api';
import { positionsStore } from '@/lib/positions-store';

interface PositionResponse {
  id: number;
  asset: string;
  status: 'OPEN' | 'WON' | 'LOST' | 'VOIDED';
  t_open_ms: number;
  t_close_ms: number;
  stake_points: number;
  multiplier_at_tap: number;
}

export function usePositionPoll(positionId: number | undefined, cellKey: string, tCloseMs: number) {
  const qc = useQueryClient();
  const q = useQuery({
    queryKey: ['position', positionId],
    enabled: positionId !== undefined,
    queryFn: () => api<PositionResponse>(`/v1/positions/${positionId}`),
    refetchInterval: (query) => {
      const data = query.state.data;
      if (data && data.status !== 'OPEN') return false;
      if (Date.now() > tCloseMs + 10_000) return false;
      return 500;
    },
  });

  useEffect(() => {
    if (!q.data) return;
    if (q.data.status === 'OPEN') return;
    const state = q.data.status === 'WON' ? 'WON' : q.data.status === 'LOST' ? 'LOST' : 'VOIDED';
    positionsStore.update(cellKey, { state, settledAtMs: Date.now() });
    qc.invalidateQueries({ queryKey: ['me'] });
    qc.invalidateQueries({ queryKey: ['me', 'history'] });
  }, [q.data, cellKey, qc]);
}
