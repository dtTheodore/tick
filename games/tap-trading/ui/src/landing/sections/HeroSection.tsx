import { COPY } from '../copy';
import { DemoPanel } from '../hero/DemoPanel';
import { PlayPill } from '../ui';

// Hero: the auto-playing price-grid demo is the protagonist. Desktop = copy
// left / demo right. Mobile reorders to headline → demo → subhead+CTA via grid
// template areas, so the live proof lands in the first viewport (mobile is the
// majority of traffic) instead of being pushed below a wall of copy.
export function HeroSection() {
  const { kicker, h1, sub, primary, secondary, microcopy } = COPY.hero;
  return (
    <section className="lp-grid-precise relative isolate overflow-hidden">
      {/* Ambient glows — pre-blurred, opacity-pulsed only (never blur radius). */}
      <div
        className="lp-glow-pulse pointer-events-none absolute -top-32 -right-24 -z-10 h-[42rem] w-[42rem] rounded-full"
        style={{ background: 'radial-gradient(circle, rgba(255,45,126,0.30), transparent 62%)', filter: 'blur(60px)' }}
      />
      <div
        className="lp-glow-pulse pointer-events-none absolute top-1/3 -left-40 -z-10 h-[34rem] w-[34rem] rounded-full"
        style={{ background: 'radial-gradient(circle, rgba(0,255,136,0.12), transparent 60%)', filter: 'blur(80px)', animationDelay: '2.5s' }}
      />
      <div className="lp-scanbeam -z-10" />
      <div className="lp-grain pointer-events-none absolute inset-0 -z-10" />

      <div
        className="mx-auto grid min-h-[100svh] max-w-7xl items-center gap-x-10 gap-y-7 px-5 pt-24 pb-14 [grid-template-areas:'head''demo''copy'] sm:px-8 lg:grid-cols-[1.05fr_0.95fr] lg:gap-y-8 lg:pt-32 lg:[grid-template-areas:'head_demo''copy_demo']"
      >
        {/* Kicker + headline */}
        <div className="max-w-xl [grid-area:head]">
          <div className="mb-5 inline-flex items-center gap-2 rounded-full border border-lp-pink/25 bg-lp-pink/5 px-3 py-1 font-mono text-[11px] uppercase tracking-[0.18em] text-lp-pink">
            <span className="relative flex h-1.5 w-1.5">
              <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-lp-pink opacity-75" />
              <span className="relative inline-flex h-1.5 w-1.5 rounded-full bg-lp-pink" />
            </span>
            {kicker}
          </div>

          <h1 className="font-display text-[clamp(2.5rem,7vw,5.4rem)] font-semibold leading-[0.96] tracking-[-0.03em] text-white">
            {h1.map((line, i) => (
              <span
                key={line}
                className="lp-reveal is-visible block"
                style={{ animationDelay: `${i * 90}ms` }}
              >
                {i === h1.length - 1 ? (
                  <span className="bg-gradient-to-r from-lp-pink via-lp-pink to-lp-sui-pink bg-clip-text text-transparent">
                    {line}
                  </span>
                ) : (
                  line
                )}
              </span>
            ))}
          </h1>
        </div>

        {/* Demo device */}
        <div className="relative w-full [grid-area:demo]">
          <DemoPanel />
        </div>

        {/* Subhead + CTAs */}
        <div className="max-w-xl [grid-area:copy]">
          <p className="max-w-md text-base leading-relaxed text-white/55 sm:text-lg">{sub}</p>

          <div className="mt-7 flex flex-col gap-3 sm:flex-row sm:items-center">
            <PlayPill className="lp-shine w-full sm:w-auto">{primary}</PlayPill>
            <PlayPill variant="ghost" href="#fairness" className="w-full sm:w-auto">
              {secondary}
            </PlayPill>
          </div>
          <p className="mt-4 font-mono text-xs text-white/50">{microcopy}</p>
        </div>
      </div>
    </section>
  );
}
