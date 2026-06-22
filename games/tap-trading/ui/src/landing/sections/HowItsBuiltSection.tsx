import { Activity, Cpu, Database, Sigma } from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { COPY } from '../copy';
import { Eyebrow, Reveal } from '../ui';

const STAGE_ICONS: LucideIcon[] = [Activity, Sigma, Cpu, Database];

// Judge layer: the streaming pipeline from oracle to on-chain settlement, drawn
// as a connected rail with data "flowing" through it (blips ride the rail; see
// .lp-flow-dot in index.css). Anchored as "Built on Sui".
export function HowItsBuiltSection() {
  const { eyebrow, title, body, flow, stack } = COPY.tech;
  return (
    <section id="built" className="lp-anchor relative border-t border-white/8 bg-white/[0.012]">
      <div className="mx-auto max-w-7xl px-5 py-24 sm:px-8 sm:py-28">
        <Reveal className="max-w-2xl">
          <Eyebrow tone="sui">{eyebrow}</Eyebrow>
          <h2 className="mt-4 font-display text-3xl font-semibold tracking-[-0.02em] text-white sm:text-5xl">
            {title}
          </h2>
          <p className="mt-5 text-base text-white/55">{body}</p>
        </Reveal>

        <div className="relative mt-16">
          {/* connecting rail + flowing data blips (desktop) */}
          <div className="absolute inset-x-0 top-[22px] hidden h-px bg-gradient-to-r from-lp-sui-blue/0 via-lp-sui-blue/30 to-lp-sui-blue/0 lg:block" />
          <div className="absolute inset-x-0 top-[20px] hidden h-1 overflow-hidden lg:block">
            {[0, 1, 2].map((i) => (
              <div key={i} className="lp-flow-dot absolute inset-0" style={{ animationDelay: `${i * 1.13}s` }}>
                <span className="absolute left-0 top-1/2 h-1.5 w-1.5 -translate-y-1/2 rounded-full bg-lp-sui-blue shadow-[0_0_8px_rgba(77,162,255,0.9)]" />
              </div>
            ))}
          </div>

          <ol className="grid gap-10 lg:grid-cols-4 lg:gap-6">
            {flow.map((stage, i) => {
              const Icon = STAGE_ICONS[i];
              return (
                <Reveal as="li" key={stage.step} delayMs={i * 90} className="relative">
                  <div className="relative z-10 mb-5 flex h-11 w-11 items-center justify-center rounded-full border border-lp-sui-blue/30 bg-[#0b0d13] text-lp-sui-blue">
                    <Icon className="h-5 w-5" strokeWidth={1.8} aria-hidden />
                  </div>
                  <span className="font-mono text-[11px] uppercase tracking-[0.16em] text-lp-sui-blue/60">
                    Stage {i + 1}
                  </span>
                  <h3 className="mt-1.5 font-mono text-base font-semibold text-white">{stage.step}</h3>
                  <p className="mt-2.5 text-sm leading-relaxed text-white/50">{stage.body}</p>
                </Reveal>
              );
            })}
          </ol>
        </div>

        <Reveal className="mt-14 flex flex-wrap items-center gap-3">
          {stack.map((tech) => (
            <span
              key={tech}
              className={`rounded-full border px-4 py-1.5 font-mono text-xs ${
                tech.startsWith('Sui')
                  ? 'border-lp-sui-blue/30 bg-lp-sui-blue/5 text-lp-sui-blue/90'
                  : 'border-white/12 bg-white/[0.03] text-white/60'
              }`}
            >
              {tech}
            </span>
          ))}
        </Reveal>
      </div>
    </section>
  );
}
