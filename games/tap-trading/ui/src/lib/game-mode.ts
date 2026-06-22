import { useSyncExternalStore } from 'react';

// Demo (play-money) vs live (real testnet USDC) play mode.
//
// Demo is the default so anyone — judges included — can try the game instantly:
// local play money, no wallet, no chain, no backend money calls. Live routes
// balance, taps, and settlement through the server + on-chain custody vault.
//
// A global store (not React context) because the tap mutation reads the mode
// outside React (useTap's mutationFn) and the choice must survive reload.
export type GameMode = 'demo' | 'live';

const STORAGE_KEY = 'tick.gameMode';

function loadInitial(): GameMode {
  if (typeof localStorage === 'undefined') return 'demo';
  return localStorage.getItem(STORAGE_KEY) === 'live' ? 'live' : 'demo';
}

let current: GameMode = loadInitial();
const listeners = new Set<() => void>();

export const gameModeStore = {
  subscribe(cb: () => void) {
    listeners.add(cb);
    return () => {
      listeners.delete(cb);
    };
  },
  getSnapshot(): GameMode {
    return current;
  },
  set(mode: GameMode) {
    if (mode === current) return;
    current = mode;
    try {
      localStorage.setItem(STORAGE_KEY, mode);
    } catch {
      // private-mode / quota — selection still applies in-memory.
    }
    for (const cb of listeners) cb();
  },
};

export function useGameMode(): GameMode {
  return useSyncExternalStore(gameModeStore.subscribe, gameModeStore.getSnapshot);
}
