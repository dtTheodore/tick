import { COPY } from '../copy';
import { Eyebrow, Reveal } from '../ui';
import { STEP_VISUALS } from './StepVisuals';

// The 3-step loop as a connected stepper: each step pairs a mini-diagram (the
// game's real visuals in miniature) with copy, joined by directional connectors.
export function HowItWorksSection() {
  const { id, eyebrow, title, steps } = COPY.how;
  return (
    <section id={id} className="lp-anchor relative mx-auto max-w-7xl px-5 py-24 sm:px-8 sm:py-28">
      <div className="flex flex-col gap-5 sm:flex-row sm:items-end sm:justify-between">
        <Reveal className="max-w-2xl">
          <Eyebrow>{eyebrow}</Eyebrow>
          <h2 className="mt-4 font-display text-3xl font-semibold tracking-[-0.02em] text-white sm:text-5xl">
            {title}
          </h2>
        </Reveal>
        <Reveal className="max-w-xs text-sm leading-relaxed text-white/50" delayMs={120}>
          One round is five seconds. Pick, lock, and the line decides — then it
          all starts again.
        </Reveal>
      </div>

      <div className="mt-14 flex flex-col items-stretch gap-4 lg:flex-row lg:gap-0">
        {steps.map((step, i) => {
          const Visual = STEP_VISUALS[i];
          return (
            <div key={step.n} className="contents">
              <Reveal
                as="article"
                delayMs={i * 110}
                className="lp-card group flex-1 rounded-2xl border border-white/10 bg-lp-raised/40 p-6"
              >
                <div className="flex h-[148px] items-center justify-center rounded-xl border border-white/8 bg-[#0c0b10] p-3">
                  {Visual && <Visual />}
                </div>
                <div className="mt-6 flex items-center gap-3">
                  <span className="font-mono text-2xl font-semibold text-lp-pink/30 transition-colors group-hover:text-lp-pink/70">
                    {step.n}
                  </span>
                  <h3 className="font-mono text-xl font-semibold text-white">{step.title}</h3>
                </div>
                <p className="mt-3 text-sm leading-relaxed text-white/55">{step.body}</p>
              </Reveal>

              {i < steps.length - 1 && (
                <div
                  aria-hidden="true"
                  className="flex items-center justify-center py-1 text-white/20 lg:px-2"
                >
                  <span className="font-mono text-lg lg:hidden">↓</span>
                  <span className="hidden font-mono text-lg lg:inline">→</span>
                </div>
              )}
            </div>
          );
        })}
      </div>
    </section>
  );
}
