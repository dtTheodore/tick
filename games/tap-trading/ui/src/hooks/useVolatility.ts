import { useSyncExternalStore } from 'react';
import { tickStore } from '@/lib/tick-store';

export function useVolatility(): number {
  return useSyncExternalStore(
    tickStore.subscribe,
    () => tickStore.getSnapshot().tick?.vol_annualized ?? 0,
  );
}
