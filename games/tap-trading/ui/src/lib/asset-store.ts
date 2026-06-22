import type { Asset } from '@/pricing/types';
import { ASSET_LIST, DEFAULT_ASSET } from './assets';

const STORAGE_KEY = 'tick.selectedAsset';

function loadInitial(): Asset {
  if (typeof localStorage === 'undefined') return DEFAULT_ASSET;
  const stored = localStorage.getItem(STORAGE_KEY);
  return stored && (ASSET_LIST as string[]).includes(stored) ? (stored as Asset) : DEFAULT_ASSET;
}

let current: Asset = loadInitial();
const listeners = new Set<() => void>();

/**
 * The selected quote asset — the single price feed the whole UI tracks. One
 * global store (not React context) because the module-level WS layer (`ws.ts`)
 * reads it on every tick to filter the multiplexed stream, and subscribes to it
 * to reconnect + reseed history when the user switches. React reads it via
 * `useSyncExternalStore`. Persisted so the choice survives reload.
 */
export const assetStore = {
  subscribe(cb: () => void) {
    listeners.add(cb);
    return () => {
      listeners.delete(cb);
    };
  },
  getSnapshot(): Asset {
    return current;
  },
  set(asset: Asset) {
    if (asset === current) return;
    current = asset;
    try {
      localStorage.setItem(STORAGE_KEY, asset);
    } catch {
      // private-mode / quota — selection still works for the session.
    }
    for (const cb of listeners) cb();
  },
};
