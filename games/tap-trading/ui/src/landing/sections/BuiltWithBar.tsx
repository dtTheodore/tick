import { COPY } from '../copy';

const toneDot: Record<string, string> = {
  sui: 'bg-lp-sui-blue',
  green: 'bg-lp-green',
  none: 'bg-white/30',
};
const toneText: Record<string, string> = {
  sui: 'text-lp-sui-blue/90',
  green: 'text-lp-green/90',
  none: 'text-white/55',
};

// One pass of the ticker. Rendered twice inside the track so the translateX(-50%)
// loop is seamless. aria-hidden on the duplicate keeps it out of the a11y tree.
function TickerRun({ ariaHidden }: { ariaHidden?: boolean }) {
  return (
    <div className="flex shrink-0 items-center" aria-hidden={ariaHidden}>
      {COPY.ticker.map((item, i) => (
        <span key={`${item.label}-${i}`} className="flex items-center">
          <span className="flex items-center gap-2 px-6 font-mono text-[13px] uppercase tracking-[0.14em]">
            <span className={`h-1.5 w-1.5 rounded-full ${toneDot[item.tone]}`} />
            <span className={toneText[item.tone]}>{item.label}</span>
          </span>
          <span className="text-white/12">◆</span>
        </span>
      ))}
    </div>
  );
}

// Honest pre-launch trust strip, styled as a market ticker tape — on-theme for a
// price-grid product. Every entry is a real fact; no fabricated metrics. The
// marquee pauses on hover and freezes under reduced motion (see index.css).
export function BuiltWithBar() {
  return (
    <div className="relative border-y border-white/8 bg-white/[0.015]">
      <div className="lp-marquee py-4">
        <div className="lp-marquee-track">
          <TickerRun />
          <TickerRun ariaHidden />
        </div>
      </div>
    </div>
  );
}
