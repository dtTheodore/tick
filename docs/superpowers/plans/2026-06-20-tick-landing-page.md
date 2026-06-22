# Tick Landing Page Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Visual components MUST be built with the `frontend-design:frontend-design` skill.

**Goal:** Add a premium, responsive marketing landing page at `/` for the Tick game (showcased at Sui Overflow 2026), with the existing game moved to `/play` and rendered verbatim.

**Architecture:** Add `react-router-dom` to the existing `games/tap-trading/ui` (React 19 + Vite 7 + Tailwind v4). `App.tsx` becomes a router: `/` → `LandingPage`, `/play` → `Game` (today's `App.tsx` body, moved verbatim). The hero recreates the game's signature glowing price line + multiplier ladder as a **self-contained synthetic component** (seeded price walk, no backend/WebSocket). Landing-scoped design tokens extend `index.css`; pure logic (price walk, multiplier ladder) is unit-tested; the whole page is verified in-browser via chrome-devtools.

**Tech Stack:** React 19, Vite 7, TypeScript, Tailwind CSS v4, `react-router-dom`, `bun test`, Space Grotesk + Inter (already installed), Canvas 2D.

**Spec:** `docs/superpowers/specs/2026-06-20-tick-landing-page-design.md`

**Working dir for all paths below:** `games/tap-trading/ui/`

---

## File structure

**Create:**
- `src/routes/Game.tsx` — the game, moved verbatim from current `App.tsx`.
- `src/landing/LandingPage.tsx` — composes all sections.
- `src/landing/sections/LandingNav.tsx` — sticky nav.
- `src/landing/sections/HeroSection.tsx` — hero copy + CTA + synthetic demo.
- `src/landing/sections/BuiltWithBar.tsx` — Sui/Walrus/Pyth trust bar.
- `src/landing/sections/HowItWorksSection.tsx` — 3 steps.
- `src/landing/sections/WhyDifferentSection.tsx` — feature cards.
- `src/landing/sections/ProvablyFairSection.tsx` — on-chain/Walrus integrity (Sui-blue).
- `src/landing/sections/HowItsBuiltSection.tsx` — architecture for judges.
- `src/landing/sections/FinalCtaSection.tsx` — repeat CTA.
- `src/landing/sections/LandingFooter.tsx` — footer.
- `src/landing/hero/SyntheticChart.tsx` — canvas hero (line + glow + fill).
- `src/landing/hero/priceWalk.ts` — pure seeded price-walk stepper.
- `src/landing/hero/priceWalk.test.ts` — unit tests.
- `src/landing/hero/multiplierLadder.ts` — pure synthetic multiplier ladder.
- `src/landing/hero/multiplierLadder.test.ts` — unit tests.
- `src/landing/lib/useReveal.ts` — IntersectionObserver scroll-reveal hook.
- `src/landing/lib/usePrefersReducedMotion.ts` — reduced-motion media-query hook.
- `src/landing/lib/usePrefersReducedMotion.test.ts` — unit test.
- `src/landing/copy.ts` — centralized landing copy strings.
- `public/og-image.png` — 1200×630 social preview (composed from the hero).

**Modify:**
- `package.json` — add `react-router-dom`.
- `src/App.tsx` — becomes the router.
- `src/index.css` — add `--color-lp-*` tokens + landing keyframes/utilities.
- `index.html` — OG/Twitter meta tags.

**Untouched (must not change behavior):** everything under `src/components/`, `src/hooks/`, `src/lib/` (except new files), `src/pricing/`, `src/main.tsx`.

---

## Task 1: Add routing, move game to `/play` (parity-preserving)

**Files:**
- Modify: `package.json`, `src/App.tsx`
- Create: `src/routes/Game.tsx`, `src/landing/LandingPage.tsx` (temporary stub)

- [ ] **Step 1: Install router**

Run: `cd games/tap-trading/ui && bun add react-router-dom`
Expected: `react-router-dom` appears in `package.json` dependencies; `bun.lockb` updated.

- [ ] **Step 2: Move the game verbatim into `Game.tsx`**

Create `src/routes/Game.tsx` with the current `App.tsx` body, unchanged except the export name and import depth (`./` → `../`):

```tsx
import { HeaderBar } from '../components/HeaderBar';
import { Grid } from '../components/Grid';
import { HistoryStrip } from '../components/HistoryStrip';
import { Toaster } from '../components/Toaster';
import { DebugOverlay } from '../components/DebugOverlay';
import { PositionTracker } from '../components/PositionTracker';

export function Game() {
  return (
    <main className="flex h-full flex-col">
      <HeaderBar />
      <Grid />
      <HistoryStrip />
      <Toaster />
      <DebugOverlay />
      <PositionTracker />
    </main>
  );
}
```

- [ ] **Step 3: Temporary landing stub** (replaced in Task 15)

Create `src/landing/LandingPage.tsx`:

```tsx
export function LandingPage() {
  return <div className="grid h-full place-items-center text-tick-pink font-mono">Tick landing — coming up</div>;
}
```

- [ ] **Step 4: Rewrite `App.tsx` as the router**

```tsx
import { BrowserRouter, Routes, Route } from 'react-router-dom';
import { Game } from './routes/Game';
import { LandingPage } from './landing/LandingPage';

export default function App() {
  return (
    <BrowserRouter>
      <Routes>
        <Route path="/" element={<LandingPage />} />
        <Route path="/play" element={<Game />} />
      </Routes>
    </BrowserRouter>
  );
}
```

- [ ] **Step 5: Verify build + types**

Run: `bun run typecheck`
Expected: PASS, no errors.

- [ ] **Step 6: Verify in browser (parity gate)**

With the dev server on `:5231`: navigate to `/play`. Expected: the game renders **identically** to before (header, grid, price line, history strip). Navigate to `/`: the stub renders. Zero new console errors beyond pre-existing backend-connection warnings.

- [ ] **Step 7: Commit**

```bash
git add -A && git commit -m "feat(tick): route game to /play, add landing shell"
```

---

## Task 2: Landing design tokens + base utilities

**Files:**
- Modify: `src/index.css`

- [ ] **Step 1: Add landing tokens + keyframes** (append to `index.css`; do NOT alter existing `@theme` game tokens)

Add the landing colors into the existing `@theme` block (so they become Tailwind utilities `bg-lp-*`, `text-lp-*`, `border-lp-*`):

```css
@theme {
  /* ...existing tick tokens stay... */
  --color-lp-bg: #0a0a0c;
  --color-lp-raised: #121116;
  --color-lp-pink: #ff2d7e;
  --color-lp-green: #00ff88;
  --color-lp-sui-blue: #4da2ff;
  --color-lp-sui-pink: #fe8bc2;
}
```

Then append landing-scoped helpers (after the existing keyframes):

```css
/* Landing: scroll-reveal — transform/opacity only (compositor-safe). */
@keyframes lp-reveal-up {
  from { opacity: 0; transform: translateY(24px); }
  to   { opacity: 1; transform: translateY(0); }
}
.lp-reveal { opacity: 0; }
.lp-reveal.is-visible {
  animation: lp-reveal-up 640ms cubic-bezier(0.16, 1, 0.3, 1) forwards;
}

/* Landing: ambient glow pulse — animate OPACITY of a pre-blurred layer only. */
@keyframes lp-glow-pulse {
  0%, 100% { opacity: 0.55; }
  50%      { opacity: 0.8; }
}

/* Landing: faint glowing 1px price-grid background (Cetus technique). */
.lp-grid-bg {
  background-image:
    linear-gradient(to right, rgba(255, 45, 126, 0.06) 1px, transparent 1px),
    linear-gradient(to bottom, rgba(255, 45, 126, 0.05) 1px, transparent 1px);
  background-size: 48px 48px;
}

/* Respect reduced motion globally for landing animations. */
@media (prefers-reduced-motion: reduce) {
  .lp-reveal { opacity: 1 !important; animation: none !important; }
  .lp-reveal.is-visible { animation: none !important; }
}
```

- [ ] **Step 2: Verify build**

Run: `bun run build`
Expected: PASS. (Tailwind v4 compiles the new `--color-lp-*` into utilities.)

- [ ] **Step 3: Commit**

```bash
git add src/index.css && git commit -m "feat(tick): add landing design tokens and motion utilities"
```

---

## Task 3: Synthetic price-walk generator (TDD, pure logic)

A seeded random walk that produces a smoothed price series. Deterministic given a seed (so tests are stable and `Math.random` is avoided in the hot path). Mirrors the game's mean-reverting-ish ETH price feel.

**Files:**
- Create: `src/landing/hero/priceWalk.ts`, `src/landing/hero/priceWalk.test.ts`

- [ ] **Step 1: Write failing tests**

```ts
import { describe, expect, it } from 'bun:test';
import { createPriceWalk } from './priceWalk';

describe('createPriceWalk', () => {
  it('is deterministic for a given seed', () => {
    const a = createPriceWalk({ seed: 42, start: 1700 });
    const b = createPriceWalk({ seed: 42, start: 1700 });
    const seriesA = Array.from({ length: 50 }, () => a.step());
    const seriesB = Array.from({ length: 50 }, () => b.step());
    expect(seriesA).toEqual(seriesB);
  });

  it('starts at the provided start price', () => {
    const w = createPriceWalk({ seed: 1, start: 1700 });
    expect(w.current()).toBeCloseTo(1700, 5);
  });

  it('stays within a bounded band around start (mean-reverting)', () => {
    const w = createPriceWalk({ seed: 7, start: 1700, drift: 0, volatility: 0.4 });
    let min = Infinity, max = -Infinity;
    for (let i = 0; i < 2000; i++) {
      const p = w.step();
      min = Math.min(min, p);
      max = Math.max(max, p);
    }
    // mean reversion keeps it within ~5% of start over a long run
    expect(min).toBeGreaterThan(1700 * 0.9);
    expect(max).toBeLessThan(1700 * 1.1);
  });

  it('produces different series for different seeds', () => {
    const a = Array.from({ length: 20 }, ((w) => () => w.step())(createPriceWalk({ seed: 1, start: 1700 })));
    const b = Array.from({ length: 20 }, ((w) => () => w.step())(createPriceWalk({ seed: 2, start: 1700 })));
    expect(a).not.toEqual(b);
  });
});
```

- [ ] **Step 2: Run tests, verify they fail**

Run: `bun test src/landing/hero/priceWalk.test.ts`
Expected: FAIL ("Cannot find module './priceWalk'").

- [ ] **Step 3: Implement**

```ts
export interface PriceWalkOptions {
  seed: number;
  start: number;
  drift?: number;      // per-step drift, default 0
  volatility?: number; // step size scale, default 0.5
  reversion?: number;  // pull back toward start, default 0.01
}

export interface PriceWalk {
  step(): number;    // advance one tick, return new price
  current(): number; // current price without advancing
}

// Mulberry32: tiny, fast, seedable PRNG — deterministic across runs/platforms.
function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

export function createPriceWalk(opts: PriceWalkOptions): PriceWalk {
  const { seed, start, drift = 0, volatility = 0.5, reversion = 0.01 } = opts;
  const rng = mulberry32(seed);
  let price = start;
  return {
    current: () => price,
    step: () => {
      const shock = (rng() - 0.5) * 2 * volatility; // uniform in [-vol, vol]
      const pull = (start - price) * reversion;     // mean reversion toward start
      price = price + drift + pull + shock;
      return price;
    },
  };
}
```

- [ ] **Step 4: Run tests, verify pass**

Run: `bun test src/landing/hero/priceWalk.test.ts`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add src/landing/hero/priceWalk.ts src/landing/hero/priceWalk.test.ts
git commit -m "feat(tick): add seeded synthetic price-walk generator"
```

---

## Task 4: Synthetic multiplier ladder (TDD, pure logic)

Given a mid price + tick size, produce a ladder of strike rows with multipliers that mirror the real game: low (~1.5–3×) near the price, rising sharply far from it. This is for **display only** — not the real pricing engine.

**Files:**
- Create: `src/landing/hero/multiplierLadder.ts`, `src/landing/hero/multiplierLadder.test.ts`

- [ ] **Step 1: Write failing tests**

```ts
import { describe, expect, it } from 'bun:test';
import { buildMultiplierLadder } from './multiplierLadder';

describe('buildMultiplierLadder', () => {
  const rows = buildMultiplierLadder({ mid: 1711.25, tickSize: 0.4, rowCount: 9, colCount: 4 });

  it('produces rowCount rows each with colCount multipliers and a strike', () => {
    expect(rows).toHaveLength(9);
    for (const r of rows) {
      expect(r.cols).toHaveLength(4);
      expect(typeof r.strike).toBe('number');
    }
  });

  it('strikes are evenly spaced by tickSize and centered on mid', () => {
    const strikes = rows.map((r) => r.strike);
    expect(strikes[0]).toBeGreaterThan(strikes[strikes.length - 1]); // top row = highest price
    for (let i = 1; i < strikes.length; i++) {
      expect(strikes[i - 1] - strikes[i]).toBeCloseTo(0.4, 5);
    }
  });

  it('multiplier grows as the row gets further from mid', () => {
    const nearest = rows.reduce((a, b) =>
      Math.abs(a.strike - 1711.25) < Math.abs(b.strike - 1711.25) ? a : b);
    const farthest = rows[0];
    expect(farthest.cols[0]).toBeGreaterThan(nearest.cols[0]);
  });

  it('never returns a multiplier below the 1.0x floor', () => {
    for (const r of rows) for (const m of r.cols) expect(m).toBeGreaterThanOrEqual(1);
  });
});
```

- [ ] **Step 2: Run tests, verify fail**

Run: `bun test src/landing/hero/multiplierLadder.test.ts`
Expected: FAIL ("Cannot find module './multiplierLadder'").

- [ ] **Step 3: Implement**

```ts
export interface LadderRow {
  strike: number;
  cols: number[]; // one display multiplier per future time column
}

export interface LadderOptions {
  mid: number;
  tickSize: number;
  rowCount: number;
  colCount: number;
}

// Display-only multiplier curve: ~1.5x at the money, growing with distance,
// nearer time columns slightly higher. NOT the real pricing engine.
export function buildMultiplierLadder(opts: LadderOptions): LadderRow[] {
  const { mid, tickSize, rowCount, colCount } = opts;
  const mids = (rowCount - 1) / 2;
  const rows: LadderRow[] = [];
  for (let i = 0; i < rowCount; i++) {
    const offsetTicks = mids - i; // top row positive (above mid)
    const strike = mid + offsetTicks * tickSize;
    const distance = Math.abs(offsetTicks);
    const cols: number[] = [];
    for (let c = 0; c < colCount; c++) {
      // base grows ~exponentially with distance; nearer columns (small c) richer.
      const base = 1.5 + Math.pow(distance, 1.9) * 0.55;
      const timeBoost = 1 + (colCount - 1 - c) * 0.06;
      const m = Math.max(1, base * timeBoost);
      cols.push(Math.round(m * 10) / 10);
    }
    rows.push({ strike: Math.round(strike * 100) / 100, cols });
  }
  return rows;
}
```

- [ ] **Step 4: Run tests, verify pass**

Run: `bun test src/landing/hero/multiplierLadder.test.ts`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add src/landing/hero/multiplierLadder.ts src/landing/hero/multiplierLadder.test.ts
git commit -m "feat(tick): add synthetic multiplier ladder for hero demo"
```

---

## Task 5: Reveal + reduced-motion hooks

**Files:**
- Create: `src/landing/lib/useReveal.ts`, `src/landing/lib/usePrefersReducedMotion.ts`, `src/landing/lib/usePrefersReducedMotion.test.ts`

- [ ] **Step 1: Write failing test for reduced-motion hook**

```ts
import { describe, expect, it, mock } from 'bun:test';

describe('prefersReducedMotion', () => {
  it('reads the (prefers-reduced-motion: reduce) media query', async () => {
    const calls: string[] = [];
    // @ts-expect-error test shim
    globalThis.window = {
      matchMedia: (q: string) => {
        calls.push(q);
        return { matches: true, addEventListener() {}, removeEventListener() {} };
      },
    };
    const { readPrefersReducedMotion } = await import('./usePrefersReducedMotion');
    expect(readPrefersReducedMotion()).toBe(true);
    expect(calls[0]).toContain('prefers-reduced-motion');
  });
});
```

- [ ] **Step 2: Run, verify fail**

Run: `bun test src/landing/lib/usePrefersReducedMotion.test.ts`
Expected: FAIL (module not found / export missing).

- [ ] **Step 3: Implement both hooks**

`src/landing/lib/usePrefersReducedMotion.ts`:

```ts
import { useEffect, useState } from 'react';

const QUERY = '(prefers-reduced-motion: reduce)';

export function readPrefersReducedMotion(): boolean {
  if (typeof window === 'undefined' || !window.matchMedia) return false;
  return window.matchMedia(QUERY).matches;
}

export function usePrefersReducedMotion(): boolean {
  const [reduced, setReduced] = useState(readPrefersReducedMotion);
  useEffect(() => {
    const mq = window.matchMedia(QUERY);
    const onChange = () => setReduced(mq.matches);
    mq.addEventListener('change', onChange);
    return () => mq.removeEventListener('change', onChange);
  }, []);
  return reduced;
}
```

`src/landing/lib/useReveal.ts`:

```ts
import { useEffect, useRef, useState } from 'react';

// Adds `is-visible` when the element scrolls into view (once). Pair with the
// `.lp-reveal` class. Uses IntersectionObserver; no animation lib needed.
export function useReveal<T extends HTMLElement = HTMLDivElement>(rootMargin = '0px 0px -10% 0px') {
  const ref = useRef<T>(null);
  const [visible, setVisible] = useState(false);
  useEffect(() => {
    const el = ref.current;
    if (!el || visible) return;
    const obs = new IntersectionObserver(
      (entries) => {
        for (const e of entries) {
          if (e.isIntersecting) {
            setVisible(true);
            obs.disconnect();
          }
        }
      },
      { rootMargin, threshold: 0.1 },
    );
    obs.observe(el);
    return () => obs.disconnect();
  }, [rootMargin, visible]);
  return { ref, visible };
}
```

- [ ] **Step 4: Run test, verify pass**

Run: `bun test src/landing/lib/usePrefersReducedMotion.test.ts`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/landing/lib && git commit -m "feat(tick): add scroll-reveal and reduced-motion hooks"
```

---

## Task 6: Centralized copy

**Files:**
- Create: `src/landing/copy.ts`

- [ ] **Step 1: Write the copy module** (single source of truth; honors accuracy guardrails in the spec)

```ts
export const COPY = {
  brand: 'Tick',
  nav: [
    { label: 'How it works', href: '#how' },
    { label: 'Fairness', href: '#fairness' },
    { label: 'Built on Sui', href: '#built' },
    { label: 'Tech', href: '#tech' },
  ],
  hero: {
    kicker: 'Tap. Watch. Earn points.',
    h1: 'Tap the chart. Win the next five seconds.',
    sub: 'Tick turns live crypto prices into a tap game. Every multiplier comes from real options math — not RNG — and every settlement is provably fair, anchored on Sui and Walrus.',
    primary: 'Play Tick',
    secondary: "See how it's fair",
    microcopy: 'Points to play · No deposit · Provably fair on Sui',
  },
  builtWith: { label: 'Built with', items: ['Sui', 'Walrus', 'Pyth'], badge: 'Sui Overflow 2026' },
  how: {
    title: 'Three taps to your first win',
    steps: [
      { n: '01', title: 'Pick a cell', body: 'Each cell is a price band over the next few seconds. Near the line pays less but hits more often; far out pays big.' },
      { n: '02', title: 'Tap to lock', body: 'Your multiplier freezes the instant you tap — recorded and never re-priced. What you see is what you win.' },
      { n: '03', title: 'Watch & win', body: 'If the live price touches your band before the window closes, you win on first touch. No waiting for expiry.' },
    ],
  },
  why: {
    title: "Why Tick isn't another tap-to-earn",
    cards: [
      { title: 'Real math, not RNG', body: 'Multipliers come from published options-pricing formulas (Hui + Broadie-Glasserman-Kou), driven by live Pyth oracle data. Your chart-reading actually matters.' },
      { title: '5-second feedback loop', body: 'Faster than perps, faster than prediction markets. Tap, watch the line, win or lose — then go again.' },
      { title: 'No money at risk', body: 'Play with points. A finite balance keeps every tap meaningful, with a credible path to a future token — no deposit, no liquidation.' },
      { title: 'Locked multipliers', body: 'The number you tap is the number you’re paid. It’s recorded on-chain at tap time and re-read at settlement — never recomputed.' },
    ],
  },
  fairness: {
    title: 'Provably fair, settled on Sui',
    body: 'Tick is built so you never have to trust the UI. The multiplier you lock is written into an on-chain position. Player stakes sit in an on-chain USDC vault with exposure caps enforced in Move. And every settlement publishes a self-contained proof to Walrus — the full oracle price path, the locked multiplier, and the outcome — that anyone can replay to confirm the result.',
    points: [
      { k: 'Lock-at-tap on Sui', v: 'Your multiplier is stored in basis points in the position object — immutable after tap.' },
      { k: 'On-chain vault custody', v: 'A Move GameVault holds stakes with per-cell, directional, and treasury exposure caps.' },
      { k: 'Walrus-anchored proofs', v: 'Each settlement’s oracle path + outcome is stored immutably and is independently replayable.' },
      { k: 'Pure WASM verifier', v: 'A dependency-free verifier replays any proof client-side using the exact server pricing code.' },
    ],
    note: 'On-chain vault + Walrus proof flow implemented and verified end-to-end on Sui testnet (USDC settlement mode). Default play is points-only.',
  },
  tech: {
    title: 'How it’s built',
    body: 'A streaming pipeline from oracle to on-chain settlement.',
    flow: [
      { step: 'Oracle aggregator', body: 'Pyth Hermes + 3 CEX feeds → freshness-filtered median + EWMA smoothing, broadcast at 20 Hz with a 120s replay ring buffer.' },
      { step: 'Pricing engine', body: 'Hui continuous-barrier option pricing with the Broadie-Glasserman-Kou discrete-monitoring correction. QuantLib-parity tested.' },
      { step: 'Settlement worker', body: 'In-memory first-touch detection over a continuous price path; idempotent dual sink to Postgres (points) or Sui + Walrus (USDC).' },
      { step: 'Sui + Walrus', body: 'tick_vault Move package settles via capability-gated PTBs; proof blobs anchored to Walrus with an on-chain ProofAnchored event.' },
    ],
    stack: ['Sui Move', 'Walrus', 'Rust', 'zkLogin / Enoki', 'React'],
  },
  finalCta: {
    title: 'The next five seconds are yours.',
    sub: 'No wallet seed phrase. No deposit. Just tap.',
    primary: 'Play Tick',
  },
  footer: {
    tagline: 'Tap. Watch. Earn points.',
    builtFor: 'Built for Sui Overflow 2026',
    links: [
      { label: 'Docs', href: '#' },
      { label: 'GitHub', href: '#' },
    ],
    note: 'Points-only in v1. No real-money play. Multipliers and settlements are provably fair.',
  },
} as const;
```

- [ ] **Step 2: Typecheck**

Run: `bun run typecheck`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src/landing/copy.ts && git commit -m "feat(tick): add landing copy module"
```

---

## Task 7: SyntheticChart canvas hero  — USE `frontend-design` skill

The signature visual: a glowing hot-pink monotone-cubic price line with pink→transparent gradient fill, head dot, over the tinted-black canvas. Driven by `createPriceWalk`. **No backend.**

**Files:**
- Create: `src/landing/hero/SyntheticChart.tsx`

**Component contract:**
```tsx
interface SyntheticChartProps {
  width?: number;      // CSS px; defaults to container
  height?: number;
  className?: string;
  paused?: boolean;    // when true (reduced motion), render one static composed frame
}
export function SyntheticChart(props: SyntheticChartProps): JSX.Element;
```

**Implementation requirements (must hold):**
- [ ] Canvas 2D. On mount, build a `createPriceWalk({ seed: 1711, start: 1711, volatility: 0.45 })` and seed a rolling buffer of ~120 points.
- [ ] Render loop via `requestAnimationFrame`; advance the walk ~10×/sec (decouple visual rAF from data cadence with a time accumulator). Each frame: shift buffer, draw.
- [ ] Drawing mirrors `src/components/PriceLine.tsx`: monotone-cubic spline path, stroke `#FF2D7E` ~`1.75px`, soft glow (`shadowBlur` set once per draw, NOT animated), pink→transparent vertical gradient fill under the line, a 3px head dot.
- [ ] **DPR cap:** `const dpr = Math.min(1.5, window.devicePixelRatio || 1)`; size backing store accordingly; never exceed.
- [ ] **Perf:** only the canvas repaints; no React state per frame (use refs). `cancelAnimationFrame` on unmount. Wrap in a container with `contain: layout`.
- [ ] **Reduced motion:** if `paused`, do not start the rAF loop — draw a single representative frame once (pre-step the walk ~120 times into the buffer, draw, stop).
- [ ] Resize-aware: observe container size with `ResizeObserver`; recompute on change.

- [ ] **Step: Build it with the frontend-design skill**, then verify in-browser at `/` that the line animates smoothly and is GPU-light (no layout thrash). Verify `prefers-reduced-motion` (DevTools emulate) freezes it to a static frame.

- [ ] **Commit**

```bash
git add src/landing/hero/SyntheticChart.tsx
git commit -m "feat(tick): add self-contained synthetic price-line hero canvas"
```

---

## Task 8: Hero section — USE `frontend-design` skill

**Files:**
- Create: `src/landing/sections/HeroSection.tsx`

**Spec:**
- [ ] Full-viewport-height hero. Background: `lp-grid-bg` faint grid + one pre-blurred ambient glow shape (`filter: blur(140px)`, pink/green, opacity-pulsed via `lp-glow-pulse`, disabled under reduced motion) + subtle grain overlay.
- [ ] Left/copy column: kicker (`COPY.hero.kicker`, mono, pink), H1 (`COPY.hero.h1`, Space Grotesk, `clamp(40px, 7vw, 104px)`, weight 600, `-0.04em`), subhead (Inter, muted), CTA row: primary solid-pink pill `Play Tick` → `<Link to="/play">`, ghost secondary `See how it's fair` → `#fairness`; microcopy under buttons.
- [ ] Right/visual column: a framed "device"/panel showing `<SyntheticChart/>` + a synthetic HUD overlay — header (`Tick · ETH/USD · {live synthetic price}` + a looping `NEXT 0:0X` round timer), the right-side multiplier ladder from `buildMultiplierLadder`, and a small win/loss history strip (static sample). Numbers in Space Grotesk, tabular-nums.
- [ ] **Responsive:** desktop = two columns (copy left, panel right); mobile/tablet = stacked, copy first, panel full-width below; primary CTA full-width on mobile. No horizontal overflow at 390px.
- [ ] Pass `paused={usePrefersReducedMotion()}` to `SyntheticChart`.

- [ ] **Build with frontend-design skill**, verify in-browser at 390/768/1440.

- [ ] **Commit**: `feat(tick): build hero section with synthetic demo panel`

---

## Task 9: Landing nav — USE `frontend-design` skill

**Files:**
- Create: `src/landing/sections/LandingNav.tsx`

**Spec:**
- [ ] Sticky top, slim, glass/blur background that gains a hairline `border-lp-border` + slight opacity after scroll (`useReveal` or a small scroll listener). Tick wordmark left (mono, pink). Center/right: `COPY.nav` anchor links (hidden on mobile, shown ≥`md`). Right: `Play` pill → `/play`.
- [ ] Mobile: wordmark + `Play` pill only (drop the anchor list, or a simple disclosure — keep minimal; YAGNI on a full drawer).
- [ ] Anchor links smooth-scroll (`scroll-behavior: smooth` on `html`, or `scrollIntoView`).

- [ ] **Build + verify anchors scroll**, then **commit**: `feat(tick): build sticky landing nav`

---

## Task 10: Built-with bar + How it works — USE `frontend-design` skill

**Files:**
- Create: `src/landing/sections/BuiltWithBar.tsx`, `src/landing/sections/HowItWorksSection.tsx`

**BuiltWithBar spec:**
- [ ] Thin band under the hero: "Built with" + `Sui` (in `text-lp-sui-blue`), `Walrus`, `Pyth` as styled text/chips + a `Sui Overflow 2026` badge + a "Provably fair" badge. Honest — no fake metrics. `lp-reveal`.

**HowItWorksSection spec (`id="how"`):**
- [ ] Section title `COPY.how.title`. Three step cards from `COPY.how.steps`, each: big mono step number, title (Space Grotesk), body (Inter). Staggered `lp-reveal` (stagger via incremental `animation-delay` only when visible). Responsive: 3-up desktop, 1-up mobile.

- [ ] **Build + verify**, **commit**: `feat(tick): build built-with bar and how-it-works`

---

## Task 11: Why-different + Provably-fair — USE `frontend-design` skill

**Files:**
- Create: `src/landing/sections/WhyDifferentSection.tsx`, `src/landing/sections/ProvablyFairSection.tsx`

**WhyDifferentSection spec:**
- [ ] Title `COPY.why.title`. Four feature cards from `COPY.why.cards` (`lp-raised` surface, hairline border, subtle hover lift via `transform` only). 2×2 desktop, 1-col mobile. `lp-reveal`.

**ProvablyFairSection spec (`id="fairness"`):**
- [ ] **This is the Sui-blue section.** Title `COPY.fairness.title`, lead `COPY.fairness.body`. Four key/value rows from `COPY.fairness.points` with Sui-blue accents/icons. Render `COPY.fairness.note` as a small, honest caption. Visual cue suggesting a "proof" (e.g., a stylized proof/verify chip with `Valid` in win-green). `lp-reveal`.

- [ ] **Build + verify**, **commit**: `feat(tick): build why-different and provably-fair sections`

---

## Task 12: How-it's-built + Final CTA + Footer — USE `frontend-design` skill

**Files:**
- Create: `src/landing/sections/HowItsBuiltSection.tsx`, `src/landing/sections/FinalCtaSection.tsx`, `src/landing/sections/LandingFooter.tsx`

**HowItsBuiltSection spec (`id="built"` and `id="tech"` anchor target — put both anchors here or split; place `id="built"` on the BuiltWith/this section and `id="tech"` here):**
- [ ] Title `COPY.tech.title` + lead. A 4-stage horizontal/stepped flow from `COPY.tech.flow` (Oracle → Pricing → Settlement → Sui+Walrus) with connectors; mobile = vertical. Stack badges `COPY.tech.stack` as chips (Sui Move + Sui-blue tint). `lp-reveal`.

**FinalCtaSection spec:**
- [ ] Centered: `COPY.finalCta.title` (large), sub, single solid-pink `Play Tick` pill → `/play`. Ambient glow echo. `lp-reveal`.

**LandingFooter spec:**
- [ ] Tagline, "Built for Sui Overflow 2026", links from `COPY.footer.links`, the honesty note, copyright. Hairline top border.

- [ ] **Build + verify**, **commit**: `feat(tick): build tech, final-CTA, and footer sections`

---

## Task 13: Compose LandingPage + wire `id` anchors

**Files:**
- Modify: `src/landing/LandingPage.tsx`

- [ ] **Step 1: Replace the stub** with the composed page:

```tsx
import { LandingNav } from './sections/LandingNav';
import { HeroSection } from './sections/HeroSection';
import { BuiltWithBar } from './sections/BuiltWithBar';
import { HowItWorksSection } from './sections/HowItWorksSection';
import { WhyDifferentSection } from './sections/WhyDifferentSection';
import { ProvablyFairSection } from './sections/ProvablyFairSection';
import { HowItsBuiltSection } from './sections/HowItsBuiltSection';
import { FinalCtaSection } from './sections/FinalCtaSection';
import { LandingFooter } from './sections/LandingFooter';

export function LandingPage() {
  return (
    <div className="min-h-full overflow-x-hidden bg-lp-bg text-white">
      <LandingNav />
      <main>
        <HeroSection />
        <BuiltWithBar />
        <HowItWorksSection />
        <WhyDifferentSection />
        <ProvablyFairSection />
        <HowItsBuiltSection />
        <FinalCtaSection />
      </main>
      <LandingFooter />
    </div>
  );
}
```

- [ ] **Step 2:** Confirm each section root has the right `id` for nav anchors (`#how`, `#fairness`, `#built`, `#tech`). Add `scroll-margin-top` to anchored sections so the sticky nav doesn't overlap targets.

- [ ] **Step 3: Verify** full page scrolls, all nav anchors land correctly, no horizontal scroll at 390px.

- [ ] **Step 4: Commit**: `feat(tick): compose landing page from sections`

---

## Task 14: OG / social meta + favicon title

**Files:**
- Modify: `index.html`
- Create: `public/og-image.png`

- [ ] **Step 1:** Compose a 1200×630 `og-image.png` (screenshot the hero panel via chrome-devtools at that viewport, or design a static frame). Save to `public/og-image.png`.

- [ ] **Step 2:** Update `<head>` in `index.html` (keep existing favicon, theme-color, viewport):

```html
<title>Tick — Tap the chart. Win the next five seconds.</title>
<meta name="description" content="Tick turns live crypto prices into a tap game. Real options-math multipliers, provably fair settlements on Sui and Walrus. Points to play — no deposit." />
<meta property="og:type" content="website" />
<meta property="og:title" content="Tick — Tap the chart. Win the next five seconds." />
<meta property="og:description" content="A provably-fair tap-trading game on Sui. Real math, not RNG. Built for Sui Overflow 2026." />
<meta property="og:image" content="/og-image.png" />
<meta name="twitter:card" content="summary_large_image" />
<meta name="twitter:title" content="Tick — Tap the chart. Win the next five seconds." />
<meta name="twitter:description" content="A provably-fair tap-trading game on Sui. Built for Sui Overflow 2026." />
<meta name="twitter:image" content="/og-image.png" />
```

- [ ] **Step 3: Verify** `bun run build` succeeds and the title shows in the tab.

- [ ] **Step 4: Commit**: `feat(tick): add social/OG meta tags and preview image`

---

## Task 15: Final verification gate (build + typecheck + tests)

- [ ] **Step 1:** `bun test` — Expected: all unit tests pass (priceWalk, multiplierLadder, prefersReducedMotion).
- [ ] **Step 2:** `bun run typecheck` — Expected: PASS.
- [ ] **Step 3:** `bun run build` — Expected: PASS, no errors.
- [ ] **Step 4: Commit** any fixes: `fix(tick): resolve build/type issues for landing`

---

## Task 16: In-browser E2E verification (chrome-devtools)

Verify against the spec's success criteria. Record evidence (screenshots/console). Treat any failure as a bug to fix, not skip.

- [ ] `/` renders the landing with **zero console errors** (check `list_console_messages`).
- [ ] `/play` renders the game **verbatim** (compare to the baseline screenshot).
- [ ] Responsive at **390 / 768 / 1440** (`resize_page` / `emulate`): no horizontal scroll, readable type, mobile CTA full-width.
- [ ] **All nav anchors** scroll to their sections; **all CTAs** navigate to `/play` (click + assert URL).
- [ ] **Fonts**: headline renders in Space Grotesk (no fallback flash).
- [ ] **`prefers-reduced-motion`** (emulate): reveals off, hero frozen.
- [ ] **Lighthouse** (`lighthouse_audit`): accessibility ≥ 90; note performance.
- [ ] **OG meta** present in DOM `<head>`.
- [ ] Fix anything that fails, re-verify, then mark the tracked E2E task complete.

---

## Self-review notes

- **Spec coverage:** §2.1 routing → T1; §2.2 synthetic hero → T3,T7,T8; §3.1 tokens → T2; §3.2 type → T2/T8; §3.3 atmosphere → T2/T8; §3.4 motion+perf → T2,T5,T7; §4 sections → T8–T13; §5 copy → T6; §7 success criteria → T15,T16; OG → T14. No gaps.
- **Type consistency:** `createPriceWalk`/`PriceWalk` (T3), `buildMultiplierLadder`/`LadderRow` (T4), `usePrefersReducedMotion`/`readPrefersReducedMotion` (T5), `useReveal` (T5), `COPY` (T6), `SyntheticChart` props (T7) — all referenced consistently downstream.
- **No placeholders:** pure-logic + routing + hooks + copy + meta have complete code; visual sections have complete structural/copy specs delegated to the frontend-design skill (correct altitude for visual work; the browser is their test).
