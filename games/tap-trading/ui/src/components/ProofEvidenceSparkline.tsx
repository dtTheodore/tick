import type { EvidenceTick } from '@/lib/verify-proof';

/** Prices in the blob are USD × 1e9; evidence `mid` is the raw price. */
const ORACLE_PRICE_SCALE = 1e9;
const VIEW_W = 320;
const VIEW_H = 96;

/** The settlement's tick path with the strike band overlaid — so the verdict is
 *  self-evident: a LOST path visibly stays outside the band, a WON path visibly
 *  crosses into it. Band bounds arrive in the blob's ×1e9 scale; everything is
 *  plotted against the raw `mid`, matching the touch math in verify-proof.ts.
 *
 *  `touchSeq` is the RECOMPUTED first-touch (from the active, possibly-tampered
 *  proof), so doctoring the evidence visibly moves the marker and the path. */
export function ProofEvidenceSparkline({
  ticks,
  bandLoScaled,
  bandHiScaled,
  touchSeq,
}: {
  ticks: EvidenceTick[];
  bandLoScaled: number;
  bandHiScaled: number;
  touchSeq: number | null;
}) {
  const lo = bandLoScaled / ORACLE_PRICE_SCALE;
  const hi = bandHiScaled / ORACLE_PRICE_SCALE;

  if (ticks.length < 2) {
    return (
      <div className="flex h-24 items-center justify-center rounded-[10px] border border-white/8 bg-white/[0.02] font-mono text-[11px] text-white/30">
        not enough ticks to plot
      </div>
    );
  }

  const mids = ticks.map((t) => t.mid);
  const dataMin = Math.min(lo, ...mids);
  const dataMax = Math.max(hi, ...mids);
  // Pad the domain so the path/band never kiss the frame; guard a flat domain.
  const pad = (dataMax - dataMin || Math.abs(dataMax) || 1) * 0.12;
  const yMin = dataMin - pad;
  const yMax = dataMax + pad;

  const xOf = (i: number) => (i / (ticks.length - 1)) * VIEW_W;
  const yOf = (v: number) => VIEW_H - ((v - yMin) / (yMax - yMin)) * VIEW_H;

  const path = ticks
    .map((t, i) => `${i === 0 ? 'M' : 'L'}${xOf(i).toFixed(2)},${yOf(t.mid).toFixed(2)}`)
    .join(' ');
  const bandTop = yOf(hi);
  const bandBottom = yOf(lo);

  const touchIdx = touchSeq === null ? -1 : ticks.findIndex((t) => t.seq === touchSeq);
  const touch = touchIdx >= 0 ? { x: xOf(touchIdx), y: yOf(ticks[touchIdx].mid) } : null;

  return (
    <svg
      viewBox={`0 0 ${VIEW_W} ${VIEW_H}`}
      preserveAspectRatio="none"
      role="img"
      aria-label="settlement evidence: price path versus strike band"
      className="h-24 w-full rounded-[10px] border border-white/8 bg-white/[0.02]"
    >
      {/* strike band — the zone the price had to enter to win */}
      <rect
        x={0}
        y={Math.min(bandTop, bandBottom)}
        width={VIEW_W}
        height={Math.max(Math.abs(bandBottom - bandTop), 1.5)}
        style={{ fill: 'var(--color-tick-info)', opacity: 0.16 }}
      />
      <line
        x1={0}
        x2={VIEW_W}
        y1={bandTop}
        y2={bandTop}
        style={{ stroke: 'var(--color-tick-info)', opacity: 0.45 }}
        strokeWidth={1}
        strokeDasharray="3 3"
        vectorEffect="non-scaling-stroke"
      />
      <line
        x1={0}
        x2={VIEW_W}
        y1={bandBottom}
        y2={bandBottom}
        style={{ stroke: 'var(--color-tick-info)', opacity: 0.45 }}
        strokeWidth={1}
        strokeDasharray="3 3"
        vectorEffect="non-scaling-stroke"
      />
      {/* the recorded price path */}
      <path
        d={path}
        fill="none"
        style={{ stroke: 'rgba(255,255,255,0.78)' }}
        strokeWidth={1.5}
        strokeLinejoin="round"
        strokeLinecap="round"
        vectorEffect="non-scaling-stroke"
      />
      {/* first-touch marker (only when the path crosses the band) */}
      {touch && (
        <circle
          cx={touch.x}
          cy={touch.y}
          r={3.5}
          style={{ fill: 'var(--color-tick-pink)' }}
          vectorEffect="non-scaling-stroke"
        />
      )}
    </svg>
  );
}
