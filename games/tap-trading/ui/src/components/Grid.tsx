import { useOracleTick } from '@/hooks/useOracleTick';
import { useVisibleCells } from '@/hooks/useVisibleCells';
import { chartAxis } from '@/lib/chart-axis';
import { CELL_DURATION_MS, strikeDecimals } from '@/lib/time';
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { Cell } from './Cell';
import { PriceLine } from './PriceLine';

const ROW_GAP_PX = 4;
const COL_GAP_PX = 4;
const LABEL_WIDTH_PX = 72;
const ROW_MIN_PX = 40;
const ROW_MAX_PX = 76;
// Where "now" sits across the chart. Price history (the line) fills the left;
// future bettable rounds fill the right. Rounds scroll left through this line,
// and the price line draws straight through each cell as it crosses — so a bet
// cell lights up the instant the line is inside it (the Pacifica feel).
const NOW_FRAC = 0.62;
// Stop offering a column this long before it opens. Two reasons: (1) its center
// cell's multiplier collapses toward ~1.0× as it nears the now-line (in-band,
// window opening now) — a dead bet that should fade out before the now-edge, not
// linger as a "1×" tile; (2) anti-exploit — a player must not be able to snipe
// the nearest cell at the last second for a near-safe bet. Locking ~3 s ahead
// keeps the soonest bettable cell close enough to feel tappable while still
// carrying a few seconds of unobserved drift before its window opens, so it
// stays genuinely risky (and fairly priced).
const NEAR_COLUMN_LOCK_MS = 3_000;

function readSlotWidth() {
  // Wider columns stretch the time axis (pxPerMs = slotWidth / CELL_DURATION_MS),
  // so the price line spreads horizontally and reads at a gentler slope — without
  // smoothing it (no added lag, settlement/cue logic untouched). Also yields a
  // slightly narrower grid (fewer columns across the same width).
  const isDesktop = typeof window !== 'undefined' && window.innerWidth >= 1024;
  return isDesktop ? 70 : 56;
}

/** Row height that makes the strike ladder fill the chart's available height —
 *  Pacifica's grid is space-filling, not a short band stranded at the top. */
function rowHeightFor(areaHeightPx: number, rows: number): number {
  if (areaHeightPx <= 0) return 44;
  const usable = areaHeightPx - 18 - (rows - 1) * ROW_GAP_PX;
  return Math.max(ROW_MIN_PX, Math.min(ROW_MAX_PX, Math.floor(usable / rows)));
}

interface ColumnProps {
  cells: Array<import('@/pricing/types').Cell>;
  keys: string[];
  cellHeightPx: number;
  slotWidthPx: number;
  nowMs: number;
  // The round is still in the future (t_open > now): tappable, shows live
  // multipliers. Once it crosses the now-line it stops being bettable; the price
  // line then draws through it and any held bet settles in place.
  bettable: boolean;
}

/** A single round's column of strike cells (highest strike on top). X position
 *  is driven imperatively by Grid's rAF — never via React style — so the scroll
 *  stays at 60 fps and multiplier re-renders can't fight the transform. */
function Column({ cells, keys, cellHeightPx, slotWidthPx, nowMs, bettable }: ColumnProps) {
  return (
    <div
      className="flex flex-col"
      style={{ gap: ROW_GAP_PX, width: slotWidthPx, paddingRight: COL_GAP_PX }}
    >
      {cells.map((_, r) => {
        const rr = cells.length - 1 - r;
        return (
          <Cell
            key={keys[rr]}
            cell={cells[rr]}
            nowMs={nowMs}
            heightPx={cellHeightPx}
            live={bettable}
          />
        );
      })}
    </div>
  );
}

export function Grid() {
  const { tick } = useOracleTick();
  const [chartWidth, setChartWidth] = useState(0);
  const [slotWidthPx, setSlotWidthPx] = useState(readSlotWidth);
  // One round spans `slotWidthPx`, so wall-clock → px at this rate. "now" sits at
  // NOW_FRAC of the width, so the on-screen past spans this many ms. The price
  // line draws back to exactly this span AND the ladder step fits to it, so the
  // line fills from the now-line to the left edge and uses the full height — no
  // dead band on either axis, at any viewport width.
  const pxPerMs = slotWidthPx / CELL_DURATION_MS;
  const visiblePastMs =
    pxPerMs > 0 && chartWidth > 0 ? (NOW_FRAC * chartWidth) / pxPerMs : 30_000;
  const grid = useVisibleCells(tick?.mid ?? null, visiblePastMs);
  const areaRef = useRef<HTMLDivElement>(null);
  const chartRef = useRef<HTMLDivElement>(null);
  const laneRef = useRef<HTMLDivElement>(null);
  // Inner wrappers that carry the vertical pan (translateY). Columns/labels lay
  // out statically around the snapped ladder center; this wrapper glides them to
  // the line's eased center (chart-axis.ts) so the grid pans smoothly AND stays
  // pixel-locked to the price line through every recenter (the treadmill).
  const laneInnerRef = useRef<HTMLDivElement>(null);
  const labelsInnerRef = useRef<HTMLDivElement>(null);
  const [areaHeight, setAreaHeight] = useState(0);

  useLayoutEffect(() => {
    const measure = () => {
      if (chartRef.current) setChartWidth(chartRef.current.clientWidth);
      if (areaRef.current) setAreaHeight(areaRef.current.clientHeight);
      setSlotWidthPx(readSlotWidth());
    };
    // Measure synchronously on mount so the chart and row height are correct on
    // the first paint; don't wait for the observer's first async callback.
    measure();
    const observer = new ResizeObserver(measure);
    if (chartRef.current) observer.observe(chartRef.current);
    if (areaRef.current) observer.observe(areaRef.current);
    return () => observer.disconnect();
  }, []);

  const cellHeightPx = rowHeightFor(areaHeight, grid.rows);
  const totalHeightPx = grid.rows * cellHeightPx + (grid.rows - 1) * ROW_GAP_PX;
  const labelWidthPx = slotWidthPx >= 60 ? LABEL_WIDTH_PX : 52;
  // Sparse milestone price labels (~5 across the ladder), pinned to round levels
  // = multiples of `labelStep`. Decimals follow the step so sub-cent SUI strikes
  // (~$0.001) read with enough precision while BTC/ETH stay at cents.
  const labelEvery = Math.max(1, Math.round(grid.rows / 5));
  const labelStep = grid.step * labelEvery;
  const labelDecimals = strikeDecimals(grid.step);

  // Read by the rAF loop; kept in refs so the loop never restarts on resize.
  const chartWidthRef = useRef(chartWidth);
  chartWidthRef.current = chartWidth;
  const pxPerMsRef = useRef(pxPerMs);
  pxPerMsRef.current = pxPerMs;
  // The static layout anchors on the snapped ladder center; the rAF pans toward
  // the line's eased center. One row slot = the clamp bound (the pan never needs
  // to exceed one row between recenters).
  const gridCenterRef = useRef(grid.center);
  gridCenterRef.current = grid.center;
  const slotRef = useRef(cellHeightPx + ROW_GAP_PX);
  slotRef.current = cellHeightPx + ROW_GAP_PX;

  // Map each round to its x by wall-clock: x = now-line + (t_open − now)·pxPerMs.
  // As `now` advances every frame, columns glide continuously left; a future
  // round crosses the now-line exactly when it opens, which is when the price
  // line begins drawing through it. Continuous (not stepped) so the line and the
  // cells stay locked to the same timeline.
  const positionColumns = useCallback(() => {
    const inner = laneInnerRef.current;
    if (!inner) return;
    const now = Date.now();
    const nowX = chartWidthRef.current * NOW_FRAC;
    const ppm = pxPerMsRef.current;
    for (let i = 0; i < inner.children.length; i++) {
      const child = inner.children[i] as HTMLElement;
      const tOpen = Number(child.dataset.topen);
      if (!tOpen) continue;
      child.style.transform = `translate3d(${nowX + (tOpen - now) * ppm}px,0,0)`;
    }
    // Vertical pan: glide the cells (and labels) to the line's eased center so
    // they stay pixel-locked to the price line. cellY(p) ≡ lineY(p) by
    // construction — the "crossed but didn't pay" lie can't happen.
    const center = gridCenterRef.current;
    const slot = slotRef.current;
    let vpan = 0;
    if (center !== null && chartAxis.center !== null && chartAxis.pxPerPrice > 0) {
      vpan = (chartAxis.center - center) * chartAxis.pxPerPrice;
      // Clamp to TWO rows, not one: a fast tick can drift >1 step between
      // recenter checks, so the snapped ladder center can jump two rows at once;
      // clamping at one row would leave the grid a row off the line until the
      // eased center caught up (a visible desync on a quick move).
      const maxPan = 2 * slot;
      if (vpan > maxPan) vpan = maxPan;
      else if (vpan < -maxPan) vpan = -maxPan;
    }
    inner.style.transform = `translate3d(0,${vpan}px,0)`;
    if (labelsInnerRef.current) {
      labelsInnerRef.current.style.transform = `translate3d(0,${vpan}px,0)`;
    }
  }, []);

  // Position synchronously after each render so newly-mounted columns never
  // flash at x=0 before the next frame.
  useLayoutEffect(() => {
    positionColumns();
  });

  useEffect(() => {
    let raf = 0;
    const loop = () => {
      positionColumns();
      raf = requestAnimationFrame(loop);
    };
    raf = requestAnimationFrame(loop);
    return () => cancelAnimationFrame(raf);
  }, [positionColumns]);

  return (
    <div className="flex flex-1 gap-2 p-3">
      <div ref={areaRef} className="relative flex-1" style={{ minHeight: totalHeightPx + 18 }}>
        <div className="relative flex h-full gap-2">
          {/* Unified chart + grid: the price line (canvas, behind) and the round
              columns (overlay, in front but translucent) share one time×price
              space, so the line draws straight through the cells. */}
          <div
            ref={chartRef}
            className="relative min-w-0 flex-1"
            style={{ height: totalHeightPx + 18 }}
          >
            <PriceLine
              cellHeightPx={cellHeightPx}
              rowGapPx={ROW_GAP_PX}
              rows={grid.rows}
              widthPx={chartWidth}
              center={grid.center}
              step={grid.step}
              nowXFrac={NOW_FRAC}
              pxPerMs={pxPerMs}
            />
            {/* Round columns, positioned by t_open along the same timeline as the
                line. Translucent cells let the line show through; clipped + edge-
                faded so rounds dissolve as they scroll off either side. */}
            <div
              ref={laneRef}
              className="absolute left-0 top-0 z-[1] overflow-hidden"
              style={{
                width: chartWidth,
                height: totalHeightPx,
                maskImage:
                  'linear-gradient(to right, transparent 0, #000 12px, #000 calc(100% - 12px), transparent 100%)',
                WebkitMaskImage:
                  'linear-gradient(to right, transparent 0, #000 12px, #000 calc(100% - 12px), transparent 100%)',
              }}
            >
              <div ref={laneInnerRef} className="absolute inset-0 will-change-transform">
                {grid.columns.map((col, c) => {
                  const tOpen = col[0].t_open_ms;
                  return (
                    <div
                      key={tOpen}
                      data-topen={tOpen}
                      className="absolute left-0 top-0 will-change-transform"
                      style={{ width: slotWidthPx, height: totalHeightPx }}
                    >
                      <Column
                        cells={col}
                        keys={grid.columnKeys[c]}
                        cellHeightPx={cellHeightPx}
                        slotWidthPx={slotWidthPx}
                        nowMs={grid.nowMs}
                        bettable={tOpen > grid.nowMs + NEAR_COLUMN_LOCK_MS}
                      />
                    </div>
                  );
                })}
              </div>
            </div>
          </div>
          <div
            className="relative overflow-hidden font-sans text-[12px] font-normal text-white/40 tabular-nums"
            style={{ width: labelWidthPx, height: totalHeightPx }}
          >
            <div
              ref={labelsInnerRef}
              className="flex flex-col will-change-transform"
              style={{ gap: ROW_GAP_PX }}
            >
              {grid.ladder.map((_, r) => {
                const rr = grid.ladder.length - 1 - r;
                const value = grid.ladder[rr];
                // Pacifica shows the strike ladder bare (no "$"), muted, AND only
                // at sparse milestone levels — not one label per row. We label
                // rows whose price is a multiple of `labelStep` (a few row-steps),
                // so labels pin to round price gridlines and scroll with the line,
                // leaving the rest of the axis clean.
                const isMilestone =
                  labelStep > 0 &&
                  Math.abs(value / labelStep - Math.round(value / labelStep)) < 1e-6;
                const formatted = !isMilestone
                  ? ''
                  : value >= 1000
                    ? value.toLocaleString('en-US', {
                        minimumFractionDigits: labelDecimals,
                        maximumFractionDigits: labelDecimals,
                      })
                    : value.toFixed(labelDecimals);
                // Positional key, never the value: the ladder is a fixed-length
                // axis whose numbers change in place as spot drifts. Keying by
                // value collides on first paint (all rows 0 until the first tick)
                // and React stacks a second ladder, doubling the column height.
                return (
                  <div key={rr} className="flex items-center pl-2" style={{ height: cellHeightPx }}>
                    {formatted}
                  </div>
                );
              })}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
