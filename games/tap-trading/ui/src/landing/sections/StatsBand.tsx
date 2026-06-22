import { COPY } from '../copy';
import { useCountUp } from '../lib/useCountUp';

function Stat({ value, suffix, label }: { value: number; suffix: string; label: string }) {
  const { ref, value: v } = useCountUp(value);
  return (
    <div className="flex flex-col items-center px-4 text-center sm:items-start sm:text-left">
      <span
        ref={ref}
        className="font-mono text-4xl font-semibold tabular-nums tracking-tight text-white sm:text-5xl"
      >
        {Math.round(v)}
        <span className="text-lp-pink">{suffix}</span>
      </span>
      <span className="mt-2 font-mono text-[11px] uppercase tracking-[0.16em] text-white/55">
        {label}
      </span>
    </div>
  );
}

// A tight "by the numbers" interlude between the pitch and the deep tech. All
// values are real engine parameters (see copy.ts), surfaced as count-up stats.
export function StatsBand() {
  return (
    <section className="lp-grid-precise relative border-b border-white/8">
      <div className="mx-auto grid max-w-7xl grid-cols-2 gap-y-10 px-5 py-16 sm:px-8 lg:grid-cols-4 lg:divide-x lg:divide-white/8">
        {COPY.stats.map((s) => (
          <Stat key={s.label} value={s.value} suffix={s.suffix} label={s.label} />
        ))}
      </div>
    </section>
  );
}
