import { COPY } from '../copy';
import { Eyebrow, Reveal } from '../ui';

const toneClass: Record<string, string> = {
  plain: 'text-white/75',
  pink: 'text-lp-pink',
  green: 'text-lp-green',
  sui: 'text-lp-sui-blue',
};

// A self-contained mock of the settlement proof every round publishes to Walrus.
// Makes the abstract "you can replay it" claim concrete and inspectable.
function ProofCard() {
  const { proof } = COPY.fairness;
  return (
    <div className="overflow-hidden rounded-2xl border border-lp-sui-blue/20 bg-[#0a0c12]/90 shadow-[0_40px_90px_-30px_rgba(0,0,0,0.9)] backdrop-blur-xl">
      <div className="flex items-center justify-between border-b border-white/8 bg-white/[0.02] px-4 py-3">
        <div className="flex items-center gap-2">
          <span className="flex gap-1.5">
            <span className="h-2.5 w-2.5 rounded-full bg-white/15" />
            <span className="h-2.5 w-2.5 rounded-full bg-white/15" />
            <span className="h-2.5 w-2.5 rounded-full bg-lp-sui-blue/60" />
          </span>
          <span className="font-mono text-xs text-white/55">{proof.title}</span>
        </div>
        <span className="rounded-full border border-lp-sui-blue/30 bg-lp-sui-blue/5 px-2 py-0.5 font-mono text-[10px] uppercase tracking-[0.14em] text-lp-sui-blue/90">
          {proof.tag}
        </span>
      </div>

      {/* oracle price path — the data the verifier replays */}
      <div className="border-b border-white/8 px-4 pt-4">
        <span className="font-mono text-[10px] uppercase tracking-[0.16em] text-white/30">
          oracle_path
        </span>
        <svg viewBox="0 0 320 56" className="mt-1 h-14 w-full" role="img" aria-hidden="true">
          <title>oracle path</title>
          <path
            d="M2 44 C 40 40, 70 42, 100 34 S 150 18, 188 16 S 250 22, 318 10"
            fill="none"
            stroke="#4DA2FF"
            strokeWidth="1.6"
            strokeLinecap="round"
          />
          {/* first-touch marker */}
          <line x1="188" y1="0" x2="188" y2="56" stroke="rgba(0,255,136,0.3)" strokeDasharray="3 3" />
          <circle cx="188" cy="16" r="3.4" fill="#00FF88" />
        </svg>
      </div>

      <div className="space-y-2 px-4 py-4 font-mono text-[13px]">
        {proof.fields.map((f) => (
          <div key={f.k} className="flex items-center justify-between gap-4">
            <span className="text-white/50">{f.k}</span>
            <span className={`tabular-nums ${toneClass[f.tone]}`}>{f.v}</span>
          </div>
        ))}
      </div>

      <div className="flex items-center justify-between border-t border-white/8 bg-white/[0.015] px-4 py-3 font-mono text-xs">
        <span className="text-lp-sui-blue/80">{proof.replay}</span>
        <span className="flex items-center gap-1.5 text-lp-green">
          <span className="lp-live-dot h-1.5 w-1.5 rounded-full bg-lp-green shadow-[0_0_10px_rgba(0,255,136,0.9)]" />
          ✓ {proof.verdict}
        </span>
      </div>
    </div>
  );
}

// The Sui-native credibility section — the one place Sui-blue leads (the judge
// handshake). Left = the integrity story + points; right = a replayable proof.
export function ProvablyFairSection() {
  const { id, eyebrow, title, body, points, note } = COPY.fairness;
  return (
    <section id={id} className="lp-anchor relative overflow-hidden">
      <div
        className="lp-glow-pulse pointer-events-none absolute left-1/2 top-0 h-[36rem] w-[36rem] -translate-x-1/2 rounded-full"
        style={{ background: 'radial-gradient(circle, rgba(77,162,255,0.16), transparent 64%)', filter: 'blur(90px)' }}
      />
      <div className="mx-auto grid max-w-7xl items-center gap-12 px-5 py-24 sm:px-8 sm:py-28 lg:grid-cols-2 lg:gap-16">
        <Reveal>
          <Eyebrow tone="sui">{eyebrow}</Eyebrow>
          <h2 className="mt-4 font-display text-3xl font-semibold tracking-[-0.02em] text-white sm:text-5xl">
            {title}
          </h2>
          <p className="mt-6 max-w-md text-base leading-relaxed text-white/55">{body}</p>

          <ul className="mt-8 space-y-4">
            {points.map((p, i) => (
              <li key={p.k} className="flex gap-3">
                <span className="mt-0.5 font-mono text-xs text-lp-sui-blue/70">
                  {String(i + 1).padStart(2, '0')}
                </span>
                <div>
                  <span className="font-mono text-sm font-semibold text-white">{p.k}</span>
                  <p className="mt-0.5 text-sm leading-relaxed text-white/50">{p.v}</p>
                </div>
              </li>
            ))}
          </ul>
        </Reveal>

        <Reveal variant="scale" delayMs={120}>
          <ProofCard />
        </Reveal>
      </div>

      <p className="mx-auto max-w-7xl px-5 pb-10 font-mono text-xs leading-relaxed text-white/50 sm:px-8">
        {note}
      </p>
    </section>
  );
}
