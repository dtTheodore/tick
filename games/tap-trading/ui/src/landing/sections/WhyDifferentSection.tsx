import { Lock, Sigma, Wallet, Zap } from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { COPY } from '../copy';
import { Eyebrow, Reveal } from '../ui';

// Per-card presentation, indexed against COPY.why.cards (copy stays the source
// of truth). `span` drives the asymmetric bento rhythm; `feature` cards get the
// extra background flourish.
const CARD_META: { Icon: LucideIcon; span: string; feature?: boolean }[] = [
  { Icon: Sigma, span: 'lg:col-span-2', feature: true },
  { Icon: Zap, span: 'lg:col-span-1' },
  { Icon: Wallet, span: 'lg:col-span-1' },
  { Icon: Lock, span: 'lg:col-span-2', feature: true },
];

export function WhyDifferentSection() {
  const { eyebrow, title, cards } = COPY.why;
  return (
    <section className="relative overflow-hidden border-y border-white/8 bg-white/[0.012]">
      <div
        className="lp-glow-pulse pointer-events-none absolute -right-32 top-1/2 h-[30rem] w-[30rem] -translate-y-1/2 rounded-full"
        style={{ background: 'radial-gradient(circle, rgba(255,45,126,0.1), transparent 65%)', filter: 'blur(70px)' }}
      />
      <div className="mx-auto max-w-7xl px-5 py-24 sm:px-8 sm:py-28">
        <Reveal className="max-w-2xl">
          <Eyebrow>{eyebrow}</Eyebrow>
          <h2 className="mt-4 font-display text-3xl font-semibold tracking-[-0.02em] text-white sm:text-5xl">
            {title}
          </h2>
        </Reveal>

        <div className="mt-12 grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
          {cards.map((card, i) => {
            const { Icon, span, feature } = CARD_META[i];
            return (
              <Reveal
                key={card.title}
                as="article"
                delayMs={(i % 2) * 90}
                className={`lp-card group relative overflow-hidden rounded-2xl border border-white/10 bg-lp-raised/50 p-7 backdrop-blur-sm ${span}`}
              >
                {feature && (
                  <Icon
                    aria-hidden
                    className="pointer-events-none absolute -right-6 -top-6 h-40 w-40 text-lp-pink/[0.06] transition-colors duration-500 group-hover:text-lp-pink/[0.1]"
                    strokeWidth={1}
                  />
                )}
                <div className="relative flex items-center gap-3">
                  <span className="flex h-10 w-10 items-center justify-center rounded-xl border border-lp-pink/25 bg-lp-pink/10 text-lp-pink">
                    <Icon className="h-5 w-5" strokeWidth={1.8} aria-hidden />
                  </span>
                  <h3 className="font-mono text-lg font-semibold text-white">{card.title}</h3>
                </div>
                <p className="relative mt-4 max-w-md text-sm leading-relaxed text-white/55">
                  {card.body}
                </p>
              </Reveal>
            );
          })}
        </div>
      </div>
    </section>
  );
}
