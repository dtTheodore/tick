import { useEffect, useRef } from 'react';
import { HERO_START, HERO_WINDOW, priceToYFrac } from './chartMath';
import { createPriceWalk } from './priceWalk';

export interface ChartSteer {
  target: number | null;
  strength: number;
}

interface SyntheticChartProps {
  className?: string;
  // When true (reduced motion), render a single static composed frame instead
  // of running the animation loop.
  paused?: boolean;
  // Throttled live price, for the parent's HUD readout + round logic. Called at
  // the data-tick rate (~10 Hz) — cheap (one number), never per animation frame.
  onPrice?: (price: number) => void;
  // Read each tick to steer the walk toward the active bet band (see DemoPanel).
  // Returning { target: null } lets the walk wander freely.
  getSteer?: () => ChartSteer;
  seed?: number;
  start?: number;
}

const STROKE = '#FF2D7E';
const TICK_MS = 100; // synthetic data cadence (10 Hz), decoupled from rAF
const VISIBLE = 130; // points shown across the width

// Monotone cubic — same curve d3.curveMonotoneX / the game's PriceLine use; no
// overshoot wobble on sharp moves. Inlined here to keep the hero fully
// decoupled from the live game components (which read the oracle WS store).
function monotoneTangents(xs: number[], ys: number[]): number[] {
  const n = xs.length;
  const tan = new Array<number>(n).fill(0);
  if (n < 2) return tan;
  const slope = new Array<number>(n - 1);
  for (let i = 0; i < n - 1; i++) {
    const dx = xs[i + 1] - xs[i];
    slope[i] = dx === 0 ? 0 : (ys[i + 1] - ys[i]) / dx;
  }
  tan[0] = slope[0];
  tan[n - 1] = slope[n - 2];
  for (let i = 1; i < n - 1; i++) {
    if (slope[i - 1] * slope[i] <= 0) {
      tan[i] = 0;
    } else {
      let t = (slope[i - 1] + slope[i]) / 2;
      const h = Math.hypot(t / slope[i - 1], t / slope[i]);
      if (h > 3) t = (3 / h) * t;
      tan[i] = t;
    }
  }
  return tan;
}

function traceCurve(path: Path2D, xs: number[], ys: number[], tan: number[]) {
  path.moveTo(xs[0], ys[0]);
  for (let i = 0; i < xs.length - 1; i++) {
    const dx = xs[i + 1] - xs[i];
    path.bezierCurveTo(
      xs[i] + dx / 3,
      ys[i] + (tan[i] * dx) / 3,
      xs[i + 1] - dx / 3,
      ys[i + 1] - (tan[i + 1] * dx) / 3,
      xs[i + 1],
      ys[i + 1],
    );
  }
}

export function SyntheticChart({
  className,
  paused = false,
  onPrice,
  getSteer,
  seed = 1711,
  start = HERO_START,
}: SyntheticChartProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  // Props that change per round are read through refs so the heavy draw effect
  // never re-runs (which would reset the price walk).
  const onPriceRef = useRef(onPrice);
  onPriceRef.current = onPrice;
  const getSteerRef = useRef(getSteer);
  getSteerRef.current = getSteer;

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    const walk = createPriceWalk({ seed, start, volatility: 0.5, reversion: 0.012 });
    const prices: number[] = [];
    for (let i = 0; i < VISIBLE; i++) prices.push(walk.step()); // pre-fill the buffer

    let w = 0;
    let h = 0;
    let gradient: CanvasGradient | null = null;

    const resize = () => {
      const rect = canvas.getBoundingClientRect();
      w = Math.max(1, rect.width);
      h = Math.max(1, rect.height);
      const dpr = Math.min(1.75, window.devicePixelRatio || 1); // cap for mobile GPUs
      canvas.width = Math.floor(w * dpr);
      canvas.height = Math.floor(h * dpr);
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      const g = ctx.createLinearGradient(0, 0, 0, h);
      g.addColorStop(0, 'rgba(255,45,126,0.24)');
      g.addColorStop(0.6, 'rgba(255,45,126,0.06)');
      g.addColorStop(1, 'rgba(255,45,126,0)');
      gradient = g;
    };
    resize();
    const ro = new ResizeObserver(resize);
    ro.observe(canvas);

    const yForPrice = (p: number) => priceToYFrac(p, { ...HERO_WINDOW, start }) * h;

    const draw = (frac: number) => {
      if (!gradient) return;
      ctx.clearRect(0, 0, w, h);
      const n = prices.length;
      const dx = w / (VISIBLE - 1);
      const xs = new Array<number>(n);
      const ys = new Array<number>(n);
      for (let i = 0; i < n; i++) {
        // Newest (i = n-1) anchored near the right edge; sub-tick `frac` slides
        // the whole line left for continuous scroll between data ticks.
        xs[i] = w - (n - 1 - i + frac) * dx;
        ys[i] = yForPrice(prices[i]);
      }
      const tan = monotoneTangents(xs, ys);

      const line = new Path2D();
      traceCurve(line, xs, ys, tan);

      const fill = new Path2D(line);
      fill.lineTo(xs[n - 1], h);
      fill.lineTo(xs[0], h);
      fill.closePath();
      ctx.fillStyle = gradient;
      ctx.fill(fill);

      ctx.save();
      ctx.lineJoin = 'round';
      ctx.lineCap = 'round';
      ctx.shadowColor = 'rgba(255,45,126,0.55)';
      ctx.shadowBlur = 10;
      ctx.strokeStyle = STROKE;
      ctx.lineWidth = 1.9;
      ctx.stroke(line);
      ctx.restore();

      // Glowing head dot at the leading edge.
      ctx.save();
      ctx.shadowColor = 'rgba(255,45,126,0.8)';
      ctx.shadowBlur = 7;
      ctx.beginPath();
      ctx.arc(xs[n - 1], ys[n - 1], 3.2, 0, Math.PI * 2);
      ctx.fillStyle = STROKE;
      ctx.fill();
      ctx.restore();
    };

    if (paused) {
      draw(0);
      onPriceRef.current?.(prices[prices.length - 1]);
      return () => ro.disconnect();
    }

    let raf = 0;
    let last = -1;
    let acc = 0;
    const loop = (tNow: number) => {
      if (last < 0) last = tNow;
      const dt = Math.min(64, tNow - last);
      last = tNow;
      acc += dt;
      while (acc >= TICK_MS) {
        const steer = getSteerRef.current?.();
        walk.setTarget(steer?.target ?? null, steer?.strength ?? 0.08);
        prices.push(walk.step());
        if (prices.length > VISIBLE) prices.shift();
        acc -= TICK_MS;
        onPriceRef.current?.(prices[prices.length - 1]);
      }
      draw(acc / TICK_MS);
      raf = requestAnimationFrame(loop);
    };
    raf = requestAnimationFrame(loop);

    return () => {
      cancelAnimationFrame(raf);
      ro.disconnect();
    };
  }, [paused, seed, start]);

  return (
    <canvas
      ref={canvasRef}
      aria-hidden="true"
      className={className}
      style={{ display: 'block', width: '100%', height: '100%' }}
    />
  );
}
