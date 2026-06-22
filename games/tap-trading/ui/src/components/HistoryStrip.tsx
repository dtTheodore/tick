import { type HistoryItem, useHistory } from '@/hooks/useMe';
import { formatUsdcSigned } from '@/lib/format';
import { useGameMode } from '@/lib/game-mode';
import { cn } from '@/lib/utils';
import type { ReactNode } from 'react';

const MAX_ITEMS = 14;

// Tape multiplier: full 2-decimals up to 99.99×, then drop decimals so a far-cell
// 359× / 1000× chip stays the same compact width as a near-cell 1.42× one.
function formatMult(m: number): string {
  return m >= 100 ? `${Math.round(m)}×` : `${m.toFixed(2)}×`;
}

// Net result the chip shows under the multiplier. WON pays the full
// stake×multiplier (points_delta); the player's *gain* is that minus the stake
// they already put up — the same number that pops off the cell on a win. A LOSS
// forfeits the stake; VOID is a refund (no P&L); OPEN is still in flight.
function netResult(p: HistoryItem): { text: string; tone: Tone } {
  switch (p.status) {
    case 'WON': {
      const gainMicro = p.settlement
        ? p.settlement.points_delta - p.stake_points
        : Math.round(p.stake_points * (p.multiplier_at_tap - 1));
      return { text: formatUsdcSigned(gainMicro), tone: 'win' };
    }
    case 'LOST':
      return { text: formatUsdcSigned(-p.stake_points), tone: 'loss' };
    case 'VOIDED':
      return { text: 'refund', tone: 'void' };
    default:
      return { text: 'live', tone: 'open' };
  }
}

type Tone = 'win' | 'loss' | 'void' | 'open';
interface ToneStyle {
  // Border + fill; wins/open also carry a soft outer glow.
  chip: string;
  // The 3px left rail's color. Rendered as a child element (not an inset
  // box-shadow) on purpose: the verifiable chip adds Tailwind's `ring`, which is
  // ALSO a box-shadow and would override an inset-shadow rail — so proof chips
  // lost their green/red. A bg-colored child bar is immune to that.
  rail: string;
  // Headline net P&L — the one value that says win or loss.
  pnl: string;
  // The at-tap multiplier, demoted to muted metadata (the odds, not the result).
  mult: string;
  // Live pulse dot — open chips only.
  dot: string;
}

// A 3px tone-colored left rail gives the tape a scannable green/red ledger
// rhythm. Wins add a soft outer bloom to celebrate; losses stay flat and calm —
// premium, not alarming.
const TONE: Record<Tone, ToneStyle> = {
  win: {
    chip: 'border-white/10 bg-tick-win/[0.06] shadow-[0_0_12px_rgba(0,255,136,0.12)]',
    rail: 'bg-tick-win',
    pnl: 'text-tick-win',
    mult: 'text-white/55',
    dot: '',
  },
  loss: {
    chip: 'border-white/[0.07] bg-white/[0.015]',
    rail: 'bg-tick-loss/65',
    pnl: 'text-tick-loss/85',
    mult: 'text-white/40',
    dot: '',
  },
  void: {
    chip: 'border-white/10 bg-white/[0.02]',
    rail: 'bg-white/30',
    pnl: 'text-white/45',
    mult: 'text-white/35',
    dot: '',
  },
  open: {
    chip: 'border-tick-pink/40 bg-tick-pink/[0.07] shadow-[0_0_12px_rgba(255,45,126,0.12)]',
    rail: 'bg-tick-pink',
    pnl: 'text-tick-pink',
    mult: 'text-white/75',
    dot: 'bg-tick-pink shadow-[0_0_6px_rgba(255,45,126,0.85)] animate-pulse',
  },
};

// Soft fade on both scroll edges so chips dissolve in/out instead of clipping
// hard against the frame — the same treadmill cue the grid lane uses.
const EDGE_FADE =
  'linear-gradient(to right, transparent, #000 16px, #000 calc(100% - 28px), transparent)';

function StripShell({ children }: { children: ReactNode }) {
  return <div className="border-t border-white/10">{children}</div>;
}

// The two-line chip face, shared by the static (off-chain) and verifiable
// (on-chain proof) chip wrappers. Extracted so the wrapper can switch between a
// <div> and a <button> without duplicating the body.
//
// Reading order is deliberate: the multiplier sits on top as small muted
// metadata (it's the odds you took, not the outcome), and the net P&L is the
// bold colored headline below — so a high-× loss can't read as a win at a glance.
function ChipBody({
  mult,
  pnl,
  style,
  verifiable,
  teaser,
  live,
}: {
  mult: string;
  pnl: string;
  style: ToneStyle;
  verifiable: boolean;
  // Demo chips can't be proven (settled locally). A hollow, muted diamond stands
  // in for the verifiable ◆ — a disabled "this is provable in Live" affordance
  // rather than hiding the on-chain-fairness differentiator entirely.
  teaser?: boolean;
  // Open (in-flight) chip: prefix the multiplier with a pulsing pink dot.
  live?: boolean;
}) {
  return (
    <>
      {/* The green/red ledger rail. A child bar, not an inset box-shadow, so the
          verifiable chip's `ring` (also a box-shadow) can't override it. */}
      <span
        aria-hidden
        className={cn('pointer-events-none absolute inset-y-0 left-0 w-[3px]', style.rail)}
      />
      <span className="flex items-center justify-between gap-2">
        <span
          className={cn(
            'flex items-center gap-1 font-mono text-[10px] font-medium tabular-nums',
            style.mult,
          )}
        >
          {live && <span className={cn('h-1.5 w-1.5 shrink-0 rounded-full', style.dot)} />}
          {mult}
        </span>
        {verifiable && (
          <span className="text-[9px] text-tick-info" title="verifiable proof">
            ◆
          </span>
        )}
        {!verifiable && teaser && (
          <span
            className="text-[9px] text-white/25"
            title="Provable on-chain fairness — switch to Live"
          >
            ◇
          </span>
        )}
      </span>
      <span
        className={cn('font-mono text-[14px] font-semibold leading-none tabular-nums', style.pnl)}
      >
        {pnl}
      </span>
    </>
  );
}

export function HistoryStrip({ onVerify }: { onVerify?: (item: HistoryItem) => void }) {
  const h = useHistory();
  const isDemo = useGameMode() === 'demo';

  if (h.isPending) {
    return (
      <StripShell>
        <div className="flex items-center gap-1.5 px-3 py-2">
          {['s0', 's1', 's2', 's3', 's4', 's5'].map((k) => (
            <div
              key={k}
              className="h-9 w-[58px] shrink-0 animate-pulse rounded-md bg-white/[0.04]"
            />
          ))}
        </div>
      </StripShell>
    );
  }
  if (h.isError) {
    return (
      <StripShell>
        <div className="px-4 py-3 font-mono text-xs text-tick-loss/80">
          couldn’t load recent results
        </div>
      </StripShell>
    );
  }

  const items = (h.data ?? []).slice(0, MAX_ITEMS);

  return (
    <StripShell>
      <div
        className="flex items-stretch gap-1.5 overflow-x-auto px-3 py-2 [-ms-overflow-style:none] [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
        style={{ maskImage: EDGE_FADE, WebkitMaskImage: EDGE_FADE }}
      >
        <span className="hidden shrink-0 select-none items-center pr-1 font-mono text-[10px] uppercase tracking-[0.18em] text-white/25 sm:flex">
          recent
        </span>
        {items.length === 0 && (
          <div className="flex items-center font-mono text-xs text-white/40">
            no taps yet — pick a cell to start
          </div>
        )}
        {items.map((p) => {
          const { text, tone } = netResult(p);
          const c = TONE[tone];
          // A published Walrus proof makes this tap independently verifiable; the
          // chip becomes a button that opens the in-browser replay (info-blue ring
          // signals it). Off-chain (points) taps stay static.
          const verifiable = p.proof_status === 'published' && !!p.walrus_blob_id;
          const base = cn(
            'tick-hist-in relative flex shrink-0 flex-col justify-center gap-1 overflow-hidden rounded-md border py-1.5 pl-3 pr-2.5 leading-none',
            c.chip,
          );
          const chipMult = formatMult(p.multiplier_at_tap);
          return verifiable ? (
            <button
              key={p.position_id}
              type="button"
              onClick={() => onVerify?.(p)}
              title={`${p.asset} · verify on-chain proof`}
              className={cn(base, 'cursor-pointer ring-1 ring-tick-info/40 hover:ring-tick-info')}
            >
              <ChipBody
                mult={chipMult}
                pnl={text}
                style={c}
                verifiable={verifiable}
                live={tone === 'open'}
              />
            </button>
          ) : (
            <div
              key={p.position_id}
              title={`${p.asset} · ${p.strike_lo.toFixed(2)}–${p.strike_hi.toFixed(2)} · ${p.status}`}
              className={base}
            >
              <ChipBody
                mult={chipMult}
                pnl={text}
                style={c}
                verifiable={verifiable}
                teaser={isDemo}
                live={tone === 'open'}
              />
            </div>
          );
        })}
      </div>
    </StripShell>
  );
}
