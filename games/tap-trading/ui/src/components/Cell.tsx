import { useCellMultiplier } from '@/hooks/useCellMultiplier';
import { useTap } from '@/hooks/useTap';
import { formatUsdcSigned } from '@/lib/format';
import { type CellPosition, positionsStore } from '@/lib/positions-store';
import { cellKey as makeCellKey } from '@/lib/time';
import { cn } from '@/lib/utils';
import type { Cell as CellType } from '@/pricing/types';
import {
  type CSSProperties,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from 'react';

// Reduced-motion is read once at module load: the celebration is the only thing
// gated on it and the OS setting doesn't change mid-session in practice. When set,
// the burst/count-up are skipped entirely and the win reads as a calm green tile.
const PREFERS_REDUCED_MOTION =
  typeof window !== 'undefined' &&
  typeof window.matchMedia === 'function' &&
  window.matchMedia('(prefers-reduced-motion: reduce)').matches;

const SPARK_COUNT = 10;

// Shockwave rings + spark burst on a win, anchored at the cell's centre. Mounted
// once (keyed off struckAtMs at the call site) so the CSS animations fire exactly
// once; spark vectors are memoised so a parent re-render can't re-scatter them.
function WinBurst() {
  const sparks = useMemo(
    () =>
      Array.from({ length: SPARK_COUNT }, (_, i) => {
        const angle = (i / SPARK_COUNT) * Math.PI * 2 + Math.random() * 0.5;
        const dist = 30 + Math.random() * 26;
        return { dx: Math.cos(angle) * dist, dy: Math.sin(angle) * dist };
      }),
    [],
  );
  return (
    <span className="pointer-events-none absolute inset-0 z-[5]" aria-hidden="true">
      <span className="tick-ring" />
      <span className="tick-ring tick-ring-2" />
      {sparks.map((s) => (
        <span
          key={`${s.dx}:${s.dy}`}
          className="tick-spark"
          style={
            {
              '--tick-dx': `${s.dx.toFixed(1)}px`,
              '--tick-dy': `${s.dy.toFixed(1)}px`,
            } as CSSProperties
          }
        />
      ))}
    </span>
  );
}

// Payout that rolls 0 → gain over ~600ms while it springs off the cell, so the
// reward reads as *earned* rather than just appearing. Mounted once per win.
function PayoutCounter({ gainMicro }: { gainMicro: number }) {
  const [shown, setShown] = useState(0);
  useEffect(() => {
    let raf = 0;
    const start = performance.now();
    const step = (t: number) => {
      const k = Math.min(1, (t - start) / 600);
      setShown(Math.round(gainMicro * (1 - (1 - k) ** 3)));
      if (k < 1) raf = requestAnimationFrame(step);
    };
    raf = requestAnimationFrame(step);
    return () => cancelAnimationFrame(raf);
  }, [gainMicro]);
  return (
    <span className="pointer-events-none absolute inset-x-0 -top-3 z-10 text-center text-[19px] font-extrabold text-tick-win drop-shadow-[0_0_10px_rgba(0,255,136,0.95)] animate-[winpop_1600ms_cubic-bezier(0.22,1,0.36,1)_forwards]">
      {formatUsdcSigned(shown)}
    </span>
  );
}

interface CellProps {
  cell: CellType;
  nowMs: number;
  heightPx: number;
  // Cell sits in the live betting band (the soonest few rounds): only these are
  // tappable. Cells whose column has already opened or closed (`live === false`)
  // can't take a fair bet — their t_open is in the past — so their idle
  // multiplier fades away as the column reaches the now-edge. A held position
  // still renders there, riding the now-edge until it settles.
  live?: boolean;
}

// A resolved bet reads for this long, then dissolves — so settled cells don't
// ride the chart on into the history line and clutter it.
const SETTLE_LINGER_MS = 1500;

// Pacifica keeps the idle grid compact at 1 decimal (5.5×, 1.6×) so the field
// stays quiet, but shows the player's locked bet at full 2-decimal precision
// (it's the number their payout rides on). Past 99 collapses to "99+×" so a
// fast-settling column doesn't fill with "1000×".
function formatMult(m: number, decimals: number): string {
  if (m >= 100) return '99+×';
  return `${m.toFixed(decimals)}×`;
}

// Tile styling per lifecycle. Branch order is load-bearing: the chartHit win
// flash pre-empts a plain server WON, and the final fallback covers REJECTED
// (a held position in none of the prior states), styled as a loss.
function cellStateClasses(
  hiddenIdle: boolean,
  position: CellPosition | undefined,
  chartHit: boolean,
  won: boolean,
): string {
  if (hiddenIdle)
    return 'pointer-events-none border-transparent bg-transparent text-white/70 opacity-0';
  if (!position)
    return 'border-transparent bg-transparent text-white/70 hover:bg-white/[0.1] hover:text-white';
  if (position.state === 'PENDING')
    return 'border-dashed border-tick-pink/80 bg-tick-pink/12 animate-pulse text-white';
  if (chartHit)
    return 'border-tick-win bg-tick-win/35 text-white shadow-[0_0_26px_rgba(0,255,136,0.6),inset_0_0_0_1px_rgba(0,255,136,0.7)]';
  if (won)
    return 'border-tick-win bg-tick-win/35 text-white shadow-[0_0_14px_rgba(0,255,136,0.35),inset_0_0_0_1px_rgba(0,255,136,0.6)]';
  if (position.state === 'LOCKED')
    return 'border-tick-pink bg-tick-pink/30 text-white shadow-[inset_0_0_0_1px_rgba(255,45,126,0.5),0_0_12px_rgba(255,45,126,0.22)]';
  if (position.state === 'LOST') return 'border-tick-loss/60 bg-tick-loss/20 text-white/70';
  if (position.state === 'VOIDED') return 'border-white/30 bg-white/[0.06] text-white/60';
  return 'border-tick-loss bg-tick-loss/40 text-white';
}

export function Cell({ cell, nowMs, heightPx, live = false }: CellProps) {
  const mult = useCellMultiplier(cell);
  const key = makeCellKey(cell.strike_lo, cell.strike_hi, cell.t_open_ms);
  const position = useSyncExternalStore(positionsStore.subscribe, () => positionsStore.get(key));
  const tap = useTap();
  const btnRef = useRef<HTMLButtonElement>(null);

  // Freeze the last bettable multiplier. When this cell crosses the now-edge
  // (`live` → false) its number fades out on the *same* element rather than
  // popping away, and shows the frozen value instead of flickering to the
  // in-play column's garbage τ<5s multiplier during the fade.
  const lastMultRef = useRef<string | null>(null);
  if (live && mult !== null) lastMultRef.current = formatMult(mult, 1);

  // No fair bet possible and nothing held here → fade to dead space, but keep the
  // button mounted (same node) so the opacity transition can run.
  const hiddenIdle = !live && !position;
  const disabled = !live || nowMs + 1000 >= cell.t_close_ms || !!position;

  // The rendered price line visibly entered this cell's band during its window —
  // the exact moment to celebrate. We fire the win off THIS (stamped by
  // useLiveHitDetection the frame it happens, ~300ms before the settlement poll),
  // not the server's WON, so the success pops when the chart hits the cell. Safe
  // to pre-empt the poll: a visible touch all but guarantees the server settles
  // WON (its raw band is a superset), and the poll reconciles USDC either way.
  const chartHit = !!position && position.struckAtMs !== undefined;
  // A server-confirmed win whose brief touch the eased line never visibly entered
  // is still a win (USDC credited) and shows green — but without the
  // celebration, because the chart didn't visibly hit it.
  const won = chartHit || position?.state === 'WON';
  // Dissolve the cell a beat after it resolves (won or lost): the result lands,
  // reads, then clears as the round scrolls left — keeping the chart-line history
  // unobstructed instead of dragging settled tiles across it.
  const resolvedAtMs = position?.struckAtMs ?? position?.settledAtMs;
  const settled = won || position?.state === 'LOST' || position?.state === 'VOIDED';
  const fadeResolved =
    settled && resolvedAtMs !== undefined && nowMs > resolvedAtMs + SETTLE_LINGER_MS;

  // Pacifica-style tile: the idle grid recedes (dim text, no border or fill) so
  // only the price line and the player's own positions read as foreground; a
  // cell only lifts on hover. The grid intentionally has no "current price"
  // cell highlight — the chart and the strike ladder don't share vertical
  // geometry (the chart pins the marker to center; the ladder uses step
  // hysteresis), so a spot-derived highlight would drift off the marker. The
  // marker pill is the price cue. Smooth transitions keep PENDING → LOCKED →
  // WON from jarring on fast settles.
  const stateClasses = cellStateClasses(hiddenIdle, position, chartHit, won);

  function handleClick() {
    if (position) {
      btnRef.current?.classList.add('shake');
      setTimeout(() => btnRef.current?.classList.remove('shake'), 150);
      return;
    }
    if (disabled) return;
    tap.mutate(cell);
  }

  const lockedLike =
    position?.state === 'LOCKED' || position?.state === 'WON' || position?.state === 'LOST';
  const lockedMult = position?.multiplierAtTap;
  const mainText = hiddenIdle
    ? (lastMultRef.current ?? '')
    : lockedLike && lockedMult !== undefined
      ? formatMult(lockedMult, 2)
      : mult === null
        ? '—'
        : formatMult(mult, 1);
  // Gain (stake × (mult − 1)) in micro-USDC — the same number the server settles.
  // The win celebration counts up to it; the stake itself is no longer crammed
  // into the tile (it's the selected stake, and rides on the count-up).
  const gainMicro =
    chartHit && position && lockedMult !== undefined
      ? Math.round(position.stake * (lockedMult - 1))
      : null;
  const struckAtMs = position?.struckAtMs;

  return (
    <button
      ref={btnRef}
      type="button"
      onClick={handleClick}
      disabled={disabled && !position}
      style={{ height: heightPx }}
      className={cn(
        'group relative flex flex-col items-center justify-center rounded-[6px] border font-sans text-white/85',
        'transition-[background-color,border-color,box-shadow,color,opacity] duration-300 ease-out',
        stateClasses,
        chartHit && 'z-[5]',
        chartHit &&
          !PREFERS_REDUCED_MOTION &&
          'animate-[winflash_780ms_cubic-bezier(0.22,1,0.36,1)]',
        position?.state === 'LOCKED' && !won && !PREFERS_REDUCED_MOTION && 'tick-locked-pulse',
        fadeResolved && 'opacity-0',
        disabled ? 'cursor-default' : 'cursor-pointer',
      )}
    >
      <span
        className={cn(
          'leading-none tabular-nums',
          lockedLike ? 'text-[13px] font-semibold' : 'text-[12px] font-normal',
        )}
      >
        {mainText}
      </span>
      {chartHit && !PREFERS_REDUCED_MOTION && <WinBurst key={`burst-${struckAtMs}`} />}
      {gainMicro !== null &&
        (PREFERS_REDUCED_MOTION ? (
          <span className="pointer-events-none absolute inset-x-0 -top-3 z-10 text-center text-[17px] font-extrabold text-tick-win drop-shadow-[0_0_8px_rgba(0,255,136,0.95)]">
            {formatUsdcSigned(gainMicro)}
          </span>
        ) : (
          <PayoutCounter key={`pay-${struckAtMs}`} gainMicro={gainMicro} />
        ))}
    </button>
  );
}
