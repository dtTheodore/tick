import { formatUsdc } from '@/lib/format';
import { MAX_STAKE_MICRO, STAKE_PRESETS_MICRO, isValidStake, stakeStore } from '@/lib/stake-store';
import { cn } from '@/lib/utils';
import { useState, useSyncExternalStore } from 'react';

const PRESETS = STAKE_PRESETS_MICRO as readonly number[];

const ACTIVE_CHIP =
  'border-tick-pink bg-tick-pink/15 text-white shadow-[inset_0_0_0_1px_rgba(255,45,126,0.4),0_0_10px_rgba(255,45,126,0.22)]';
const IDLE_CHIP = 'border-white/10 text-white/55 hover:border-white/25 hover:text-white/85';

// The bet-size control — the primary lever in the game, so it sits full-width
// under the header, always visible. Quick-pick chips cover common bets; the
// custom field lets a player push up to the $10 cap. The active choice reads as a
// locked-in pink chip, matching the grid's own locked-cell cue.
export function StakeSelector() {
  const selected = useSyncExternalStore(stakeStore.subscribe, stakeStore.get);
  const customActive = !PRESETS.includes(selected);
  // Local text for the custom field; seeded from a persisted custom stake.
  const [draft, setDraft] = useState(() => (customActive ? (selected / 1_000_000).toString() : ''));

  function onCustom(raw: string) {
    const cleaned = raw.replace(/[^0-9.]/g, '');
    setDraft(cleaned);
    const usd = Number.parseFloat(cleaned);
    if (!Number.isFinite(usd)) return;
    // Clamp to the cap so a fat-fingered "100" still resolves to a valid bet.
    const micro = Math.min(MAX_STAKE_MICRO, Math.round(usd * 1_000_000));
    if (isValidStake(micro)) stakeStore.set(micro);
  }

  return (
    <div className="flex items-center gap-3 border-b border-white/10 px-3 py-1.5 sm:px-4">
      <span className="hidden shrink-0 font-mono text-[10px] uppercase tracking-[0.18em] text-white/35 sm:inline">
        Stake / tap
      </span>
      <div className="flex flex-1 items-center justify-end gap-1.5 overflow-x-auto [-ms-overflow-style:none] [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
        {PRESETS.map((tier) => {
          const active = !customActive && tier === selected;
          return (
            <button
              key={tier}
              type="button"
              aria-pressed={active}
              aria-label={`Stake $${formatUsdc(tier)} USDC per tap`}
              onClick={() => {
                stakeStore.set(tier);
                setDraft('');
              }}
              className={cn(
                'shrink-0 rounded-md border px-2.5 py-1 font-mono text-xs tabular-nums transition-[color,background-color,border-color,box-shadow,transform] duration-200 active:scale-[0.94]',
                active ? ACTIVE_CHIP : IDLE_CHIP,
              )}
            >
              <span className={active ? 'text-tick-pink' : 'text-white/35'}>$</span>
              {formatUsdc(tier)}
            </button>
          );
        })}
        <label
          className={cn(
            'flex shrink-0 items-center gap-1 rounded-md border px-2 py-1 font-mono text-xs tabular-nums transition-colors',
            customActive ? ACTIVE_CHIP : IDLE_CHIP,
          )}
          title="Custom stake — up to $10"
        >
          <span className={customActive ? 'text-tick-pink' : 'text-white/35'}>$</span>
          <input
            inputMode="decimal"
            placeholder="custom"
            value={draft}
            onChange={(e) => onCustom(e.target.value)}
            aria-label="Custom USDC stake per tap"
            className="w-14 bg-transparent text-white outline-none placeholder:text-white/30"
          />
        </label>
      </div>
    </div>
  );
}
