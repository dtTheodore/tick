import { COPY } from '../copy';
import { PlayPill, Reveal } from '../ui';

// The closer: one repeated CTA inside a glowing glass panel, with a last pass of
// the trust chips. Single-CTA by design — it converts better.
export function FinalCtaSection() {
  const { title, sub, primary, chips } = COPY.finalCta;
  return (
    <section className="relative overflow-hidden px-5 py-24 sm:px-8 sm:py-28">
      <Reveal
        variant="scale"
        className="lp-grid-precise relative mx-auto flex max-w-4xl flex-col items-center overflow-hidden rounded-3xl border border-white/10 bg-lp-raised/40 px-6 py-20 text-center backdrop-blur-sm sm:py-24"
      >
        <div
          className="lp-glow-pulse pointer-events-none absolute left-1/2 top-1/2 h-[34rem] w-[34rem] -translate-x-1/2 -translate-y-1/2 rounded-full"
          style={{ background: 'radial-gradient(circle, rgba(255,45,126,0.22), transparent 60%)', filter: 'blur(90px)' }}
        />
        <div className="lp-grain pointer-events-none absolute inset-0" />

        <h2 className="relative font-display text-4xl font-semibold leading-[1.02] tracking-[-0.03em] text-white sm:text-6xl">
          {title}
        </h2>
        <p className="relative mt-5 max-w-lg text-base text-white/55 sm:text-lg">{sub}</p>

        <PlayPill className="lp-shine relative mt-9 !px-10 !py-4 text-base">{primary}</PlayPill>

        <div className="relative mt-8 flex flex-wrap items-center justify-center gap-x-5 gap-y-2 font-mono text-[11px] uppercase tracking-[0.16em] text-white/55">
          {chips.map((c) => (
            <span key={c} className="flex items-center gap-1.5">
              <span className="h-1 w-1 rounded-full bg-lp-green" />
              {c}
            </span>
          ))}
        </div>
      </Reveal>
    </section>
  );
}
