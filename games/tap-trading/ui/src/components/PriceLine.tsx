import { chartAxis } from '@/lib/chart-axis';
import { tickStore } from '@/lib/tick-store';
import { useEffect, useLayoutEffect, useRef, useState } from 'react';

interface PriceLineProps {
  cellHeightPx: number;
  rowGapPx: number;
  rows: number;
  widthPx: number;
  // The grid's strike axis, so the line shares the cells' vertical mapping:
  // one strike `step` spans exactly one row slot, centered on `center`. Both the
  // line and the round columns derive their y from this single mapping
  // (chart-axis.ts), so the line is drawn at the price's TRUE grid row and can
  // never appear inside a band the price hasn't entered.
  center: number | null;
  step: number;
  // Shared time axis with the grid: "now" sits at `nowXFrac` of the width and
  // wall-clock maps to pixels at `pxPerMs`, so the line and the round columns
  // live on one timeline and the line draws straight through the cells.
  nowXFrac: number;
  pxPerMs: number;
}

// Fallback span for the first frame before width/pxPerMs are known. The real
// draw window is derived per-frame from the visible past span (nowX / pxPerMs),
// so the line fills from the now-line to the left edge at any viewport width.
const HISTORY_WINDOW_FALLBACK_MS = 30_000;
const STROKE = '#FF2D7E';
const DPR_CAP = 2;

// Read once: the only motion gated on it is the head dot's gentle pulse (a
// steady-state oscillation, not a transition). Everything else is static paint.
const PREFERS_REDUCED_MOTION =
  typeof window !== 'undefined' &&
  typeof window.matchMedia === 'function' &&
  window.matchMedia('(prefers-reduced-motion: reduce)').matches;

// The line is drawn from the RAW settled mid — the exact value the live cue
// (useLiveHitDetection) and the settlement worker (touch.rs) judge — so the curve
// IS the path the server settles. There is deliberately NO temporal EMA on the
// line: that lag is precisely what let the drawn line sit inside a band the price
// had not entered ("price crossed but didn't pay"). Calm comes from the
// monotone-cubic geometry and the eased axis, not from lagging the price; the
// feed itself is denoised upstream (aggregator BBO consensus), so the raw mid is
// already smooth.
//
// Only the vertical-axis CENTER is eased, so a ladder recenter glides instead of
// snapping a whole row. The scale (px-per-price) is NOT eased — it equals the
// cells' `slot / step`, so line and cells share one scale and a re-tier rescales
// both together (see chart-axis.ts).
const AXIS_EASE_TAU_MS = 130;

/** dt-aware EMA blend factor toward a target with time constant `tauMs`, so the
 *  axis ease stays frame-rate independent. */
const emaAlpha = (dtMs: number, tauMs: number): number => 1 - Math.exp(-dtMs / tauMs);

/** Monotone cubic — same curve d3.curveMonotoneX uses. No overshoot wobble on
 *  sharp spikes, which Catmull-Rom and cardinal splines both produce. Writes the
 *  tangents into the caller's reused `tan`/`slope` scratch (len ≥ n) to avoid
 *  allocating two Float32Arrays every frame. */
function computeMonotoneTangents(
  points: Float32Array,
  n: number,
  tan: Float32Array,
  slope: Float32Array,
): void {
  if (n < 2) return;
  for (let i = 0; i < n - 1; i++) {
    const dx = points[(i + 1) * 2] - points[i * 2];
    slope[i] = dx === 0 ? 0 : (points[(i + 1) * 2 + 1] - points[i * 2 + 1]) / dx;
  }
  tan[0] = slope[0];
  tan[n - 1] = slope[n - 2];
  for (let i = 1; i < n - 1; i++) {
    if (slope[i - 1] * slope[i] <= 0) {
      tan[i] = 0;
    } else {
      let t = (slope[i - 1] + slope[i]) / 2;
      const a = t / slope[i - 1];
      const b = t / slope[i];
      const h = Math.hypot(a, b);
      if (h > 3) t = (3 / h) * t;
      tan[i] = t;
    }
  }
}

/** Trace the monotone-cubic curve through `points` into a CanvasPath (a 2D ctx
 *  or a Path2D). Shared by the area fill and the stroke so they match exactly. */
function traceCurve(path: CanvasPath, points: Float32Array, n: number, tan: Float32Array) {
  path.moveTo(points[0], points[1]);
  for (let i = 0; i < n - 1; i++) {
    const x0 = points[i * 2];
    const y0 = points[i * 2 + 1];
    const x1 = points[(i + 1) * 2];
    const y1 = points[(i + 1) * 2 + 1];
    const dx = x1 - x0;
    path.bezierCurveTo(
      x0 + dx / 3,
      y0 + (tan[i] * dx) / 3,
      x1 - dx / 3,
      y1 - (tan[i + 1] * dx) / 3,
      x1,
      y1,
    );
  }
}

export function PriceLine({
  cellHeightPx,
  rowGapPx,
  rows,
  widthPx,
  center,
  step,
  nowXFrac,
  pxPerMs,
}: PriceLineProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [tickLabels, setTickLabels] = useState<
    Array<{ x: number; label: string; current: boolean }>
  >([]);

  const lastFrameMsRef = useRef<number | null>(null);
  const seedEpochRef = useRef(-1);
  // Eased copy of the grid's ladder center. The grid recenters by a whole step
  // every few seconds; binding the line straight to that snapped value teleports
  // it a full row in one frame. Easing toward it glides instead — and the grid
  // cells pan to the SAME eased value (chart-axis.ts), so line and cells stay
  // locked together while gliding.
  const easedCenterRef = useRef<number | null>(null);
  // Per-frame scratch reused across frames to avoid 60fps heap churn.
  const pointsBufRef = useRef<Float32Array | null>(null);
  const tanBufRef = useRef<Float32Array | null>(null);
  const slopeBufRef = useRef<Float32Array | null>(null);
  const gradientRef = useRef<{ h: number; grad: CanvasGradient } | null>(null);

  const totalHeightPx = rows * cellHeightPx + (rows - 1) * rowGapPx;

  // Live params read by the rAF loop each frame, so the loop is created ONCE and
  // never torn down on re-render.
  const params = {
    widthPx,
    totalHeightPx,
    center,
    step,
    cellHeightPx,
    rowGapPx,
    rows,
    nowXFrac,
    pxPerMs,
  };
  const paramsRef = useRef(params);
  paramsRef.current = params;

  useLayoutEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || widthPx <= 0) return;
    const dpr = Math.min(DPR_CAP, window.devicePixelRatio || 1);
    canvas.width = Math.floor(widthPx * dpr);
    canvas.height = Math.floor(totalHeightPx * dpr);
    canvas.style.width = `${widthPx}px`;
    canvas.style.height = `${totalHeightPx}px`;
    const ctx = canvas.getContext('2d');
    if (ctx) ctx.scale(dpr, dpr);
  }, [widthPx, totalHeightPx]);

  const ready = widthPx > 0;
  useEffect(() => {
    if (!ready) return;
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    let rafId = 0;
    let lastTimeLabelMs = 0;

    function frame() {
      const {
        widthPx: w,
        totalHeightPx: h,
        center: pCenter,
        step: pStep,
        cellHeightPx: cellH,
        rowGapPx: rowGap,
        rows: rowsN,
        nowXFrac: pNowXFrac,
        pxPerMs: pPxPerMs,
      } = paramsRef.current;
      const nowX = w * pNowXFrac;
      const snap = tickStore.getSnapshot();
      const trail = snap.trail;
      const latestMid = snap.tick?.mid ?? null;
      const nowMs = Date.now();
      // Draw exactly the past that's on-screen — from the now-line back to x=0.
      // A fixed window left the left edge blank whenever the viewport's past span
      // exceeded it (it always did: span ≈ 52 s at 1280 px vs a 30 s window). This
      // is the same span useVisibleCells fits the ladder step to (shared nowXFrac
      // + pxPerMs), so the line and the fit stay matched.
      const earliestTs = nowMs - (pPxPerMs > 0 ? nowX / pPxPerMs : HISTORY_WINDOW_FALLBACK_MS);
      const lastFrameMs = lastFrameMsRef.current;
      const dtMs = lastFrameMs === null ? 16 : Math.max(0, nowMs - lastFrameMs);
      lastFrameMsRef.current = nowMs;

      // A fresh history seed (first load, or reconnect after a tab-away gap) bumps
      // seedEpoch; re-anchor the eased axis so it snaps to the new center instead
      // of gliding across the hidden price gap.
      if (snap.seedEpoch !== seedEpochRef.current) {
        seedEpochRef.current = snap.seedEpoch;
        easedCenterRef.current = null;
      }

      // --- Shared vertical axis (chart-axis.ts): one mapping for line AND cells ---
      const slot = cellH + rowGap;
      const half = Math.floor(rowsN / 2);
      const targetCenter = pCenter ?? latestMid ?? 0;
      const pxPerPrice = pStep > 0 ? slot / pStep : 1;
      // Ease the center toward the ladder so a recenter glides — but SNAP if the
      // target jumped more than one step (a multi-step recenter, which happens in
      // low-σ markets where a single inter-tick move spans >2 steps). Easing
      // across a >1-step gap would drive the grid's vertical pan past one row
      // while the line uses the full offset, desyncing line from cells — the exact
      // "price crossed but didn't pay" lie. Snapping keeps |easedCenter − center|
      // ≤ step, so they stay locked; a multi-step recenter reads as a rare clean
      // jump of the line and the cells together.
      const aAxis = emaAlpha(dtMs, AXIS_EASE_TAU_MS);
      const prevEased = easedCenterRef.current;
      const maxGap = pStep > 0 ? 1.5 * pStep : Number.POSITIVE_INFINITY;
      easedCenterRef.current =
        prevEased === null || Math.abs(targetCenter - prevEased) > maxGap
          ? targetCenter
          : prevEased + (targetCenter - prevEased) * aAxis;
      const axisCenter = easedCenterRef.current;
      // Publish so the grid pans its cells onto this exact mapping. Line y and
      // cell y now derive from the same (axisCenter, pxPerPrice) → structural
      // "what you see == what settles".
      chartAxis.center = axisCenter;
      chartAxis.pxPerPrice = pxPerPrice;

      // y of the ladder-center strike = vertical center of the middle row.
      const centerY0 = (rowsN - 1 - half) * slot + cellH / 2;
      // Half-row down-shift so the line draws INSIDE the tile whose band actually
      // contains the price. A cell is drawn centered on its `strikeLo` label, but
      // its band is [strikeLo, strikeHi) — the half-row ABOVE that label. Without
      // this offset the line sits half a row high, landing in the tile ABOVE the
      // one that settles: price just under a tile's strike looked "in" it but
      // lost (false miss), and a real touch greened the tile while the dot showed
      // one row up (false hit). +slot/2 maps price p into the tile for band(p),
      // so line-in-tile ⇔ price-in-band ⇔ the cue/server win. Dot + fill + clip
      // all read this same yForPrice, so they shift together.
      const bandOffsetPx = slot / 2;
      const yForPrice = (p: number): number =>
        Math.max(-16, Math.min(h + 16, centerY0 - (p - axisCenter) * pxPerPrice + bandOffsetPx));

      // --- Build screen points from the RAW trail ---
      const capacity = Math.max(8, Math.ceil(w * 2));
      let points = pointsBufRef.current;
      let tan = tanBufRef.current;
      let slope = slopeBufRef.current;
      if (points === null || tan === null || slope === null || points.length < capacity * 2) {
        points = new Float32Array(capacity * 2);
        tan = new Float32Array(capacity);
        slope = new Float32Array(capacity);
        pointsBufRef.current = points;
        tanBufRef.current = tan;
        slopeBufRef.current = slope;
      }
      let nPoints = 0;
      let lastX = Number.NEGATIVE_INFINITY;
      for (let i = 0; i < trail.length; i++) {
        const t = trail[i];
        if (t.ts_ms < earliestTs) continue;
        // Same time→x as the grid columns: now-line + (t − now)·pxPerMs.
        const x = nowX + (t.ts_ms - nowMs) * pPxPerMs;
        if (x < -16) continue;
        const y = yForPrice(t.mid);
        if (x - lastX < 0.5) {
          points[(nPoints - 1) * 2 + 1] = (points[(nPoints - 1) * 2 + 1] + y) / 2;
          continue;
        }
        if (nPoints >= capacity) break;
        points[nPoints * 2] = x;
        points[nPoints * 2 + 1] = y;
        nPoints++;
        lastX = x;
      }
      // Anchor the head at the now-line with the latest mid, so the leading edge
      // glides to `nowX` rather than stepping left between ticks. (Trail points
      // are all ≤ now; this extends the last ~50 ms flat to the now-line.)
      if (latestMid !== null && nPoints < capacity) {
        const hy = yForPrice(latestMid);
        if (nPoints > 0 && nowX - points[(nPoints - 1) * 2] < 0.5) {
          points[(nPoints - 1) * 2] = nowX;
          points[(nPoints - 1) * 2 + 1] = hy;
        } else {
          points[nPoints * 2] = nowX;
          points[nPoints * 2 + 1] = hy;
          nPoints++;
        }
      }

      ctx!.clearRect(0, 0, w, h);

      // Faint "now" guide: the present sits here and future round columns cross it
      // as they open. Subtle and dashed so it reads as an axis, not a price cue.
      ctx!.save();
      ctx!.strokeStyle = 'rgba(255,45,126,0.16)';
      ctx!.lineWidth = 1;
      ctx!.setLineDash([3, 5]);
      ctx!.beginPath();
      ctx!.moveTo(Math.round(nowX) + 0.5, 0);
      ctx!.lineTo(Math.round(nowX) + 0.5, h);
      ctx!.stroke();
      ctx!.restore();

      if (nPoints >= 2) {
        computeMonotoneTangents(points, nPoints, tan, slope);
        const firstX = points[0];
        const lastPx = points[(nPoints - 1) * 2];
        const lastPy = points[(nPoints - 1) * 2 + 1];

        const line = new Path2D();
        traceCurve(line, points, nPoints, tan);

        // Area fill — subtle gradient from the line down to the baseline, cached
        // by height (rebuilt only on resize).
        const fill = new Path2D(line);
        fill.lineTo(lastPx, h);
        fill.lineTo(firstX, h);
        fill.closePath();
        let gradient = gradientRef.current;
        if (gradient === null || gradient.h !== h) {
          const g = ctx!.createLinearGradient(0, 0, 0, h);
          g.addColorStop(0, 'rgba(255,45,126,0.24)');
          g.addColorStop(0.55, 'rgba(255,45,126,0.06)');
          g.addColorStop(1, 'rgba(255,45,126,0)');
          gradient = { h, grad: g };
          gradientRef.current = gradient;
        }
        ctx!.fillStyle = gradient.grad;
        ctx!.fill(fill);

        // Layered neon depth in TWO passes, not the old single flat shadow — but
        // only ONE shadow-blurred stroke per frame (cost ≈ the original), so the
        // 60fps chart keeps its budget: a wide soft bloom carries the glow, then a
        // crisp un-shadowed core sits on top. The bloom is symmetric around the
        // line (it IS the line glowing), so it carries no band meaning.
        ctx!.save();
        ctx!.lineJoin = 'round';
        ctx!.lineCap = 'round';
        ctx!.strokeStyle = 'rgba(255,45,126,0.20)';
        ctx!.lineWidth = 6;
        ctx!.shadowColor = 'rgba(255,45,126,0.55)';
        ctx!.shadowBlur = 12;
        ctx!.stroke(line);
        ctx!.restore();

        // Crisp core line on top — no shadow (the bloom beneath supplies the glow).
        ctx!.save();
        ctx!.lineJoin = 'round';
        ctx!.lineCap = 'round';
        ctx!.strokeStyle = STROKE;
        ctx!.lineWidth = 2;
        ctx!.stroke(line);
        ctx!.restore();

        // Head marker — a tight glowing dot, CLIPPED to the band the latest mid
        // actually sits in, so its halo cannot bleed across a band edge and read
        // as a touch the price hasn't made. The clip rect is the on-screen extent
        // of that band on this same axis — the honest "what you see" guarantee.
        if (latestMid !== null && pStep > 0) {
          const k = Math.floor((latestMid - (pCenter ?? axisCenter)) / pStep);
          const bandLo = (pCenter ?? axisCenter) + k * pStep;
          const yTop = yForPrice(bandLo + pStep);
          const yBot = yForPrice(bandLo);
          ctx!.save();
          ctx!.beginPath();
          ctx!.rect(0, yTop, w, yBot - yTop);
          ctx!.clip();
          // Soft halo + a hot white core — both inside the band clip, so the glow
          // can never bleed across a band edge and read as a touch the price
          // hasn't made. The gentle pulse keeps the leading edge feeling alive.
          const pulse = PREFERS_REDUCED_MOTION ? 1 : 1 + 0.16 * Math.sin(nowMs / 180);
          const haloR = 15 * pulse;
          const halo = ctx!.createRadialGradient(lastPx, lastPy, 0, lastPx, lastPy, haloR);
          halo.addColorStop(0, 'rgba(255,45,126,0.5)');
          halo.addColorStop(1, 'rgba(255,45,126,0)');
          ctx!.fillStyle = halo;
          ctx!.beginPath();
          ctx!.arc(lastPx, lastPy, haloR, 0, Math.PI * 2);
          ctx!.fill();
          ctx!.shadowColor = 'rgba(255,45,126,0.9)';
          ctx!.shadowBlur = 8;
          ctx!.beginPath();
          ctx!.arc(lastPx, lastPy, 3, 0, Math.PI * 2);
          ctx!.fillStyle = '#fff';
          ctx!.fill();
          ctx!.restore();
        }
      }

      // Time tick labels at 4 Hz.
      if (nowMs - lastTimeLabelMs > 250) {
        lastTimeLabelMs = nowMs;
        const labels: Array<{ x: number; label: string; current: boolean }> = [];
        const pxPer5s = 5000 * pPxPerMs;
        const stepMs = 5000 * Math.max(1, Math.ceil(64 / Math.max(pxPer5s, 1)));
        const earliestVisibleTs = nowMs - nowX / pPxPerMs;
        // Label the future, not just the past: bettable round columns extend to
        // the right of the now-line, so the axis reads ahead of time (Pacifica) —
        // each upcoming column gets its open time. Walk to the right edge.
        const latestVisibleTs = nowMs + (w - nowX) / pPxPerMs;
        const firstTick = Math.ceil(earliestVisibleTs / stepMs) * stepMs;
        const currentSlot = Math.floor(nowMs / 5000) * 5000;
        for (let ts = firstTick; ts <= latestVisibleTs; ts += stepMs) {
          const x = nowX + (ts - nowMs) * pPxPerMs;
          if (x < 28 || x > w - 8) continue;
          const date = new Date(ts);
          const label =
            `${String(date.getHours()).padStart(2, '0')}:` +
            `${String(date.getMinutes()).padStart(2, '0')}:` +
            `${String(date.getSeconds()).padStart(2, '0')}`;
          labels.push({ x, label, current: ts === currentSlot });
        }
        setTickLabels(labels);
      }

      rafId = requestAnimationFrame(frame);
    }

    rafId = requestAnimationFrame(frame);
    return () => cancelAnimationFrame(rafId);
  }, [ready]);

  if (widthPx <= 0) return null;

  return (
    <div className="relative h-full w-full" style={{ height: totalHeightPx + 18 }}>
      <canvas
        ref={canvasRef}
        className="pointer-events-none absolute inset-0"
        style={{ width: widthPx, height: totalHeightPx }}
      />
      <div
        className="pointer-events-none absolute left-0 right-0 font-mono text-[11px] tabular-nums"
        style={{ top: totalHeightPx + 2, height: 16 }}
      >
        {tickLabels.map((t, i) => (
          <span
            key={i}
            className={
              t.current
                ? 'absolute -translate-x-1/2 font-semibold text-tick-pink'
                : 'absolute -translate-x-1/2 text-white/35'
            }
            style={{ left: t.x }}
          >
            {t.label}
          </span>
        ))}
      </div>
    </div>
  );
}
