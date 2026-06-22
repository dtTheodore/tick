import { useEffect, useMemo, useRef, useState } from 'react';
import { usePrefersReducedMotion } from '../lib/usePrefersReducedMotion';
import { HERO_START, HERO_SYMBOL, fmtQuotePrice, priceToYFrac } from './chartMath';
import { buildMultiplierLadder } from './multiplierLadder';
import { mulberry32 } from './priceWalk';
import { SyntheticChart, type ChartSteer } from './SyntheticChart';

type Phase = 'arming' | 'live' | 'win' | 'miss';

interface Bet {
  center: number; // band mid price
  low: number; // band lower price bound
  high: number; // band upper price bound
  dir: 'up' | 'down';
  mult: number;
  away: number; // steer target on a miss round (drifts the line away)
}

interface Result {
  mult: number;
  win: boolean;
  id: number;
}

const ARM_MS = 750; // pause between rounds (the "pick a cell" beat)
const WINDOW_MS = 4200; // the live betting window
const HOLD_WIN_MS = 1350; // celebrate the win
const HOLD_MISS_MS = 850; // misses pass quicker
const BAND_HALF = 1.3; // band half-height in price units (legibility > exactness)

const fmtMult = (m: number) => (m >= 100 ? '99+×' : `${m.toFixed(1)}×`);
const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));
const lerp = (a: number, b: number, t: number) => a + (b - a) * t;

// A self-contained replica of the Tick loop for the hero: the live synthetic
// price line, an auto-placed bet band, and the win-on-touch payoff — reusing the
// game's own winflash/winpop celebration. No backend (see SyntheticChart). The
// walk is gently steered into the band on win rounds so the payoff always lands
// in front of a viewer, while the noise keeps it feeling live.
export function DemoPanel({ className = '' }: { className?: string }) {
  const reduced = usePrefersReducedMotion();
  const [price, setPrice] = useState(HERO_START);
  const [bet, setBet] = useState<Bet | null>(null);
  const [phase, setPhase] = useState<Phase>('arming');
  const [history, setHistory] = useState<Result[]>([]);

  const priceRef = useRef(HERO_START);
  const prevPriceRef = useRef(HERO_START);
  const steerRef = useRef<ChartSteer>({ target: null, strength: 0.08 });

  const ladder = useMemo(
    () => buildMultiplierLadder({ mid: HERO_START, tickSize: 1.6, rowCount: 13, colCount: 3 }),
    [],
  );

  const onPrice = (p: number) => {
    prevPriceRef.current = priceRef.current;
    priceRef.current = p;
    setPrice(p);
  };

  // The round state machine. Driven by a 50 ms interval reading the live price
  // ref; the steer ref it sets is consumed by SyntheticChart each tick. Skipped
  // entirely under reduced motion (a static composed frame renders instead).
  useEffect(() => {
    if (reduced) {
      // Static composed frame: a pending locked bet above the (frozen) line.
      // We render 'live', not 'win' — the paused canvas leaves the line near the
      // start price, so a green win band the line isn't touching would be a lie.
      const m = 8.4;
      const center = HERO_START + 6;
      setBet({ center, low: center - BAND_HALF, high: center + BAND_HALF, dir: 'up', mult: m, away: 0 });
      setPhase('live');
      setHistory([
        { mult: 8.4, win: true, id: 0 },
        { mult: 1.6, win: false, id: 1 },
        { mult: 12.4, win: true, id: 2 },
        { mult: 2.3, win: true, id: 3 },
        { mult: 1.4, win: false, id: 4 },
        { mult: 24.7, win: true, id: 5 },
      ]);
      steerRef.current = { target: null, strength: 0 };
      return;
    }

    const rng = mulberry32(0x7104);
    let p: Phase = 'arming';
    let phaseStart = performance.now();
    let current: Bet | null = null;
    let outcome = true;
    let lastWasMiss = false;
    let round = 0;
    let nextId = 0;

    const makeBet = (): Bet => {
      const up = rng() > 0.5;
      const d = 3.6 + rng() * 5.0; // 3.6..8.6 from the current price
      const here = priceRef.current;
      const center = clamp(here + (up ? 1 : -1) * d, HERO_START - 11, HERO_START + 11);
      const mult = Math.round((1.2 + d ** 1.82 * 0.45) * 10) / 10;
      return {
        center,
        low: center - BAND_HALF,
        high: center + BAND_HALF,
        dir: up ? 'up' : 'down',
        mult,
        away: clamp(here - (up ? 1 : -1) * 9, HERO_START - 12, HERO_START + 12),
      };
    };

    const id = window.setInterval(() => {
      const now = performance.now();
      const el = now - phaseStart;

      if (p === 'arming') {
        if (el >= ARM_MS) {
          // First round always wins (best first impression); never two misses
          // in a row; otherwise ~74% win — honest but clearly fun.
          outcome = round === 0 ? true : lastWasMiss ? true : rng() < 0.74;
          current = makeBet();
          steerRef.current = outcome
            ? { target: current.center, strength: 0.06 }
            : { target: current.away, strength: 0.05 };
          setBet(current);
          setPhase('live');
          p = 'live';
          phaseStart = now;
          round += 1;
        }
        return;
      }

      if (p === 'live' && current) {
        const frac = clamp(el / WINDOW_MS, 0, 1);
        if (outcome) {
          steerRef.current = { target: current.center, strength: lerp(0.06, 0.19, frac) };
          const touched = priceRef.current >= current.low && priceRef.current <= current.high;
          if (touched || el >= WINDOW_MS) {
            setPhase('win');
            p = 'win';
            phaseStart = now;
            lastWasMiss = false;
            setHistory((h) => [{ mult: current!.mult, win: true, id: nextId++ }, ...h].slice(0, 7));
          }
        } else if (el >= WINDOW_MS) {
          setPhase('miss');
          p = 'miss';
          phaseStart = now;
          lastWasMiss = true;
          setHistory((h) => [{ mult: current!.mult, win: false, id: nextId++ }, ...h].slice(0, 7));
        }
        return;
      }

      const hold = p === 'win' ? HOLD_WIN_MS : HOLD_MISS_MS;
      if ((p === 'win' || p === 'miss') && el >= hold) {
        steerRef.current = { target: null, strength: 0.08 };
        setBet(null);
        setPhase('arming');
        p = 'arming';
        phaseStart = now;
      }
    }, 50);

    return () => window.clearInterval(id);
  }, [reduced]);

  const getSteer = useRef((): ChartSteer => steerRef.current).current;

  const rising = priceRef.current >= prevPriceRef.current;
  const isWin = phase === 'win';
  const isMiss = phase === 'miss';
  const bandActive = bet && (phase === 'live' || isWin || isMiss);

  // Band geometry as % of the chart box — shares priceToYFrac with the canvas.
  const bandTop = bet ? priceToYFrac(bet.high) * 100 : 0;
  const bandHeight = bet ? (priceToYFrac(bet.low) - priceToYFrac(bet.high)) * 100 : 0;
  const midIndex = (ladder.length - 1) / 2;
  const targetRow = bet
    ? ladder.reduce(
        (best, r, i) =>
          Math.abs(r.strike - bet.center) < Math.abs(ladder[best].strike - bet.center) ? i : best,
        0,
      )
    : -1;

  return (
    <div
      className={`relative overflow-hidden rounded-2xl border border-white/10 bg-[#0b0a0e]/90 shadow-[0_40px_90px_-24px_rgba(0,0,0,0.85),0_0_0_1px_rgba(255,255,255,0.03)_inset] backdrop-blur-xl ${className}`}
      style={{ contain: 'layout paint' }}
    >
      {/* HUD header — live price + status, no naked countdown */}
      <div className="flex items-center justify-between gap-3 border-b border-white/8 bg-white/[0.015] px-4 py-3">
        <div className="flex items-center gap-2">
          <span className="flex items-center gap-1.5">
            <img
              src="/android-chrome-192x192.png"
              alt=""
              aria-hidden
              className="h-5 w-5 rounded-md ring-1 ring-white/10"
            />
            <span className="font-mono text-sm font-semibold tracking-tight text-lp-pink">Tick</span>
          </span>
          <span className="flex items-center gap-1.5 rounded-full border border-lp-green/25 bg-lp-green/5 px-2 py-0.5 font-mono text-[10px] uppercase tracking-[0.16em] text-lp-green/90">
            <span className="lp-live-dot h-1 w-1 rounded-full bg-lp-green" />
            Live
          </span>
        </div>
        <div className="flex items-center gap-2 font-mono text-xs">
          <span className="text-white/40">{HERO_SYMBOL}</span>
          <span className={`tabular-nums ${rising ? 'text-lp-green' : 'text-lp-loss'}`}>
            {fmtQuotePrice(price)}
          </span>
          <span className={`text-[10px] ${rising ? 'text-lp-green' : 'text-lp-loss'}`}>
            {rising ? '▲' : '▼'}
          </span>
        </div>
      </div>

      {/* Chart + bet band + ladder */}
      <div className="flex h-[290px] sm:h-[360px]">
        <div className="relative flex-1">
          <SyntheticChart paused={reduced} onPrice={onPrice} getSteer={getSteer} seed={1711} />

          {/* The auto-placed bet band — DOM overlay aligned to the canvas via
              the shared priceToYFrac mapping. */}
          {bandActive && bet && (
            <div
              className="pointer-events-none absolute inset-x-0 transition-colors duration-300"
              style={{ top: `${bandTop}%`, height: `${bandHeight}%` }}
            >
              <div
                className={`relative h-full w-full border-y border-dashed transition-colors duration-300 ${
                  isWin
                    ? 'border-lp-green bg-lp-green/[0.18] shadow-[inset_0_0_24px_rgba(0,255,136,0.22)]'
                    : isMiss
                      ? 'border-white/15 bg-white/[0.02]'
                      : 'border-lp-pink/80 bg-lp-pink/[0.15] shadow-[inset_0_0_22px_rgba(255,45,126,0.18)]'
                }`}
              >
                {/* Strike label, left */}
                <span
                  className={`absolute left-2 top-1/2 -translate-y-1/2 font-mono text-[10px] uppercase tracking-[0.14em] ${
                    isWin ? 'text-lp-green/90' : isMiss ? 'text-white/30' : 'text-lp-pink/80'
                  }`}
                >
                  {bet.dir === 'up' ? '▲' : '▼'} {fmtQuotePrice(bet.center)}
                </span>

                {/* Multiplier / payout badge, right — this is the "cell" that
                    fires the game's winflash on a hit. */}
                <div className="absolute right-2 top-1/2 -translate-y-1/2">
                  <span
                    key={`${phase}-${bet.center}`}
                    className={`inline-flex items-center gap-1 rounded-md border px-2 py-1 font-mono text-xs font-semibold tabular-nums ${
                      isWin
                        ? 'border-lp-green/60 bg-lp-green/15 text-lp-green'
                        : isMiss
                          ? 'border-white/15 bg-black/40 text-white/35'
                          : 'border-lp-pink/50 bg-black/50 text-lp-pink'
                    }`}
                    style={isWin && !reduced ? { animation: 'winflash 0.8s ease-out' } : undefined}
                  >
                    {isWin ? '✓ ' : isMiss ? '' : '🔒 '}
                    {fmtMult(bet.mult)}
                  </span>
                  {isWin && !reduced && (
                    <span
                      className="absolute -top-1 right-1 font-mono text-sm font-bold text-lp-green"
                      style={{ animation: 'winpop 1.1s ease-out forwards' }}
                    >
                      +{fmtMult(bet.mult)}
                    </span>
                  )}
                </div>

                {/* Tap ripple at the right edge when a bet is freshly locked. */}
                {phase === 'live' && !reduced && (
                  <span className="absolute right-3 top-1/2 flex h-2 w-2 -translate-y-1/2">
                    <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-lp-pink opacity-60" />
                    <span className="relative inline-flex h-2 w-2 rounded-full bg-lp-pink" />
                  </span>
                )}
              </div>
            </div>
          )}
        </div>

        {/* Multiplier ladder — desktop only; highlights the active strike row. */}
        <div
          className="hidden w-[150px] shrink-0 border-l border-white/8 py-1 font-mono text-[11px] tabular-nums sm:block"
          style={{
            maskImage: 'linear-gradient(to bottom, transparent, #000 14%, #000 86%, transparent)',
            WebkitMaskImage:
              'linear-gradient(to bottom, transparent, #000 14%, #000 86%, transparent)',
          }}
        >
          {ladder.map((row, i) => {
            const isTarget = i === targetRow;
            const atMoney = Math.abs(i - midIndex) < 0.5;
            return (
              <div
                key={row.strike}
                className={`flex items-center justify-between gap-2 px-2.5 py-[3px] transition-colors ${
                  isTarget
                    ? isWin
                      ? 'bg-lp-green/15'
                      : 'bg-lp-pink/12'
                    : atMoney
                      ? 'bg-white/[0.04]'
                      : ''
                }`}
              >
                <span
                  className={
                    isTarget
                      ? isWin
                        ? 'font-semibold text-lp-green'
                        : 'font-semibold text-lp-pink'
                      : 'text-white/55'
                  }
                >
                  {fmtMult(row.cols[1])}
                </span>
                <span className="text-white/25">{fmtQuotePrice(row.strike)}</span>
              </div>
            );
          })}
        </div>
      </div>

      {/* Recent results — fed live by the loop */}
      <div className="flex items-center gap-1.5 overflow-hidden border-t border-white/8 bg-white/[0.012] px-3 py-2">
        <span className="mr-1 font-mono text-[10px] uppercase tracking-[0.16em] text-white/30">
          Last
        </span>
        {(history.length ? history : [{ mult: 0, win: true, id: -1 }]).map((r) =>
          r.id === -1 ? (
            <span key="empty" className="font-mono text-[10px] text-white/25">
              …
            </span>
          ) : (
            <span
              key={r.id}
              className={`rounded border px-1.5 py-0.5 font-mono text-[10px] tabular-nums ${
                r.win
                  ? 'border-lp-green/40 text-lp-green'
                  : 'border-lp-loss/40 text-lp-loss/90'
              }`}
            >
              {fmtMult(r.mult)} {r.win ? 'W' : 'L'}
            </span>
          ),
        )}
      </div>
    </div>
  );
}
