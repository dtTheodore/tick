import { COPY } from '../copy';

export function LandingFooter() {
  const { tagline, builtFor, links, note } = COPY.footer;
  return (
    <footer className="relative overflow-hidden border-t border-white/8 bg-lp-bg">
      <div className="mx-auto max-w-7xl px-5 py-14 sm:px-8">
        <div className="grid gap-10 sm:grid-cols-2 lg:grid-cols-[1.4fr_1fr_1fr]">
          {/* Brand */}
          <div>
            <span className="flex items-center gap-2.5">
              <img
                src="/android-chrome-192x192.png"
                alt=""
                aria-hidden
                className="h-8 w-8 rounded-md ring-1 ring-white/10"
              />
              <span className="font-mono text-2xl font-semibold tracking-tight text-lp-pink">
                {COPY.brand}
                <span className="text-white/30">.</span>
              </span>
            </span>
            <p className="mt-3 max-w-xs font-mono text-xs leading-relaxed text-white/55">{tagline}</p>
            <span className="mt-5 inline-block rounded-full border border-lp-sui-blue/25 bg-lp-sui-blue/5 px-3 py-1 font-mono text-xs text-lp-sui-blue/80">
              {builtFor}
            </span>
          </div>

          {/* Explore (page anchors) */}
          <div>
            <span className="font-mono text-[11px] uppercase tracking-[0.18em] text-white/50">
              Explore
            </span>
            <ul className="mt-4 space-y-2.5">
              {COPY.nav.map((item) => (
                <li key={item.href}>
                  <a
                    href={item.href}
                    className="font-mono text-sm text-white/55 transition-colors hover:text-white"
                  >
                    {item.label}
                  </a>
                </li>
              ))}
            </ul>
          </div>

          {/* Resources */}
          <div>
            <span className="font-mono text-[11px] uppercase tracking-[0.18em] text-white/50">
              Resources
            </span>
            <ul className="mt-4 space-y-2.5">
              {links.map((link) => (
                <li key={link.label}>
                  <a
                    href={link.href}
                    className="font-mono text-sm text-white/55 transition-colors hover:text-white"
                  >
                    {link.label}
                  </a>
                </li>
              ))}
            </ul>
          </div>
        </div>

        <p className="mt-12 border-t border-white/5 pt-6 font-mono text-xs text-white/50">{note}</p>
      </div>
    </footer>
  );
}
