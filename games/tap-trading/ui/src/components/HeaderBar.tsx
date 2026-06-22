import { useMe } from '@/hooks/useMe';
import { useOracleTick } from '@/hooks/useOracleTick';
import { demoEngine } from '@/lib/demo-engine';
import { type GameMode, gameModeStore, useGameMode } from '@/lib/game-mode';
import { positionsStore } from '@/lib/positions-store';
import { cn } from '@/lib/utils';
import { useSyncExternalStore } from 'react';
import { AssetSelector } from './AssetSelector';

export function HeaderBar({ onWalletClick }: { onWalletClick?: () => void }) {
  // Owns the price-feed connection for the header's lifetime. The tick value is
  // rendered by AssetSelector (which reads the store directly); here we only need
  // the connect/disconnect side effect.
  useOracleTick();
  const mode = useGameMode();
  const me = useMe();
  const demoBalance = useSyncExternalStore(demoEngine.subscribe, demoEngine.getBalance);
  const pendingStake = useSyncExternalStore(positionsStore.subscribe, () =>
    positionsStore.pendingStake(),
  );

  const baseBal = mode === 'demo' ? demoBalance : (me.data?.balance ?? 0);
  const displayed = baseBal - pendingStake;
  // Only live mode loads balance over the network; demo is always ready.
  const balanceLoading = mode === 'live' && me.isPending;

  // Flipping to live opens the wallet so the user can connect + deposit; flipping
  // back to demo just restores play money.
  function onPillClick() {
    if (mode === 'demo') {
      gameModeStore.set('live');
      onWalletClick?.();
    } else {
      gameModeStore.set('demo');
    }
  }

  // The wallet is real-money only — opening it forces live, so a deposit can never
  // happen "behind" a demo balance that wouldn't reflect it.
  function onBalanceClick() {
    if (mode === 'demo') gameModeStore.set('live');
    onWalletClick?.();
  }

  return (
    <header className="grid grid-cols-[auto_1fr_auto] items-center gap-2 border-b border-white/10 px-3 py-2.5 sm:px-4 sm:py-3">
      <div className="flex items-center gap-1.5 sm:gap-2">
        <img
          src="/android-chrome-192x192.png"
          alt=""
          aria-hidden
          className="h-5 w-5 rounded-md ring-1 ring-white/10 sm:h-6 sm:w-6"
        />
        <span className="bg-gradient-to-r from-white to-tick-pink bg-clip-text font-mono text-base font-semibold tracking-tight text-transparent sm:text-lg">
          Tick
        </span>
      </div>
      <div className="flex justify-center">
        <AssetSelector />
      </div>
      <div className="flex items-center justify-self-end gap-1.5 sm:gap-2">
        <ModePill mode={mode} onClick={onPillClick} />
        <button
          type="button"
          onClick={onBalanceClick}
          className="group flex items-center gap-1.5 whitespace-nowrap rounded-[7px] border border-white/10 px-2.5 py-1 font-mono text-sm tabular-nums text-white transition-colors hover:border-tick-pink/60 hover:bg-white/[0.04] sm:text-base"
          title={
            mode === 'demo'
              ? 'Demo play money — go Live to deposit real USDC'
              : 'Deposit / withdraw USDC'
          }
        >
          <span>{balanceLoading ? '…' : `${(displayed / 1_000_000).toFixed(4)}`}</span>
          <span className="text-white/40 group-hover:text-tick-pink">USDC</span>
          <span className="text-[10px] text-white/30 group-hover:text-white/60">▾</span>
        </button>
      </div>
    </header>
  );
}

/**
 * Mode switch that doubles as the play-money disclosure. In demo it reads "Demo"
 * in pink with a tooltip stating nothing is on-chain and how to go live; in live
 * it reads "Live" in green. Tapping it flips the mode (see HeaderBar.onPillClick).
 */
function ModePill({ mode, onClick }: { mode: GameMode; onClick: () => void }) {
  const demo = mode === 'demo';
  return (
    <button
      type="button"
      onClick={onClick}
      title={
        demo
          ? 'Demo — play money, local only. Nothing is on-chain. Tap to go Live with real testnet USDC + provable Walrus fairness.'
          : 'Live — real testnet USDC, settled with provable on-chain fairness. Tap to switch back to demo play money.'
      }
      className={cn(
        'group flex items-center gap-1.5 rounded-[7px] border px-2 py-1 font-mono text-[11px] font-semibold uppercase tracking-[0.16em] transition-colors',
        demo
          ? 'border-tick-pink/40 bg-tick-pink/10 text-tick-pink hover:border-tick-pink/70'
          : 'border-tick-win/40 bg-tick-win/10 text-tick-win hover:border-tick-win/70',
      )}
      aria-label={demo ? 'Demo mode — switch to live' : 'Live mode — switch to demo'}
    >
      <span
        className={cn(
          'h-1.5 w-1.5 rounded-full',
          demo ? 'animate-pulse bg-tick-pink' : 'bg-tick-win',
        )}
      />
      <span>{demo ? 'Demo' : 'Live'}</span>
      <span className="text-[9px] opacity-0 transition-opacity group-hover:opacity-70">⇄</span>
    </button>
  );
}
