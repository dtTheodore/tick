import { assetStore } from '@/lib/asset-store';
import { ASSETS, ASSET_LIST } from '@/lib/assets';
import { type WsState, tickStore } from '@/lib/tick-store';
import { useEffect, useRef, useState, useSyncExternalStore } from 'react';
import { AssetIcon } from './AssetIcon';

function FeedDot({ wsState }: { wsState: WsState }) {
  const cls =
    wsState === 'open'
      ? 'text-tick-win'
      : wsState === 'reconnecting' || wsState === 'connecting'
        ? 'text-yellow-400'
        : 'text-tick-loss';
  return (
    <span className={`text-[10px] leading-none ${cls}`} title={`price feed ${wsState}`}>
      ●
    </span>
  );
}

/**
 * Header quote selector (SUI/BTC/ETH). The trigger shows the live selected
 * asset, its price, and the feed status; the dropdown switches the quote, which
 * `asset-store` persists and `ws.ts` reconnects on. Reads the tick snapshot
 * directly — the connection lifecycle is owned by `useOracleTick` elsewhere.
 */
export function AssetSelector() {
  const asset = useSyncExternalStore(assetStore.subscribe, assetStore.getSnapshot);
  const { tick, wsState } = useSyncExternalStore(tickStore.subscribe, tickStore.getSnapshot);
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onPointer = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false);
    };
    document.addEventListener('mousedown', onPointer);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onPointer);
      document.removeEventListener('keydown', onKey);
    };
  }, [open]);

  const meta = ASSETS[asset];
  const price = tick
    ? tick.mid.toLocaleString('en-US', {
        minimumFractionDigits: meta.priceDecimals,
        maximumFractionDigits: meta.priceDecimals,
      })
    : '—';

  return (
    <div ref={rootRef} className="relative">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-haspopup="menu"
        aria-expanded={open}
        className="group flex items-center gap-2 rounded-[7px] border border-white/10 px-2 py-1 font-mono text-sm transition-colors hover:border-tick-pink/60 hover:bg-white/[0.04]"
      >
        <AssetIcon asset={asset} />
        <span className="hidden sm:inline">
          <span className="text-white/90">{meta.ticker}</span>
          <span className="text-white/40">/USD</span>
        </span>
        <span className="tabular-nums text-white">{price}</span>
        <FeedDot wsState={wsState} />
        <span className="text-[10px] text-white/30 transition-colors group-hover:text-white/60">
          ▾
        </span>
      </button>
      {open && (
        <div className="absolute left-0 top-[calc(100%+6px)] z-50 min-w-[184px] overflow-hidden rounded-[10px] border border-white/10 bg-[#121116] py-1 shadow-[0_14px_44px_rgba(0,0,0,0.6)]">
          {ASSET_LIST.map((a) => {
            const m = ASSETS[a];
            const active = a === asset;
            return (
              <button
                key={a}
                type="button"
                onClick={() => {
                  assetStore.set(a);
                  setOpen(false);
                }}
                className={`flex w-full items-center gap-2.5 px-3 py-2 text-left font-mono text-sm transition-colors ${
                  active ? 'bg-white/[0.06]' : 'hover:bg-white/[0.04]'
                }`}
              >
                <AssetIcon asset={a} />
                <span className={active ? 'text-white' : 'text-white/85'}>{m.ticker}</span>
                <span className="text-white/35">{m.name}</span>
                {active && <span className="ml-auto text-tick-pink">●</span>}
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
