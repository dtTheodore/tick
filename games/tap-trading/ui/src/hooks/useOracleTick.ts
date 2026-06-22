import { useSyncExternalStore, useEffect } from 'react';
import { tickStore } from '@/lib/tick-store';
import { connect, disconnect } from '@/lib/ws';

export function useOracleTick() {
  useEffect(() => {
    connect();
    return () => disconnect();
  }, []);
  return useSyncExternalStore(tickStore.subscribe, tickStore.getSnapshot);
}
