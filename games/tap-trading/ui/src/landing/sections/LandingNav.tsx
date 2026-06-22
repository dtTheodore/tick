import { Menu, X } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { COPY } from '../copy';
import { PlayPill } from '../ui';

// Sticky top nav. Transparent over the hero, gains a glass background + hairline
// once scrolled. A scroll-progress bar rides the very top; on mobile the anchors
// collapse behind a menu toggle.
export function LandingNav() {
  const [scrolled, setScrolled] = useState(false);
  const [open, setOpen] = useState(false);
  const progressRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const onScroll = () => {
      const y = window.scrollY;
      setScrolled(y > 16);
      const max = document.documentElement.scrollHeight - window.innerHeight;
      if (progressRef.current) {
        progressRef.current.style.transform = `scaleX(${max > 0 ? y / max : 0})`;
      }
    };
    onScroll();
    window.addEventListener('scroll', onScroll, { passive: true });
    return () => window.removeEventListener('scroll', onScroll);
  }, []);

  return (
    <header
      className={`fixed inset-x-0 top-0 z-50 transition-colors duration-300 ${
        scrolled || open ? 'border-b border-white/8 bg-lp-bg/80 backdrop-blur-xl' : 'border-b border-transparent'
      }`}
    >
      {/* scroll progress */}
      <div
        ref={progressRef}
        className="lp-progress absolute inset-x-0 top-0 h-0.5 origin-left scale-x-0 bg-gradient-to-r from-lp-pink to-lp-sui-pink"
      />

      <nav className="mx-auto flex h-16 max-w-7xl items-center justify-between px-5 sm:px-8">
        <a href="#top" className="flex items-center gap-2">
          <img
            src="/android-chrome-192x192.png"
            alt=""
            aria-hidden
            className="h-7 w-7 rounded-md ring-1 ring-white/10"
          />
          <span className="font-mono text-lg font-semibold tracking-tight text-lp-pink">
            {COPY.brand}
            <span className="text-white/30">.</span>
          </span>
        </a>

        <div className="hidden items-center gap-8 md:flex">
          {COPY.nav.map((item) => (
            <a
              key={item.href}
              href={item.href}
              className="font-mono text-sm text-white/55 transition-colors hover:text-white"
            >
              {item.label}
            </a>
          ))}
        </div>

        <div className="flex items-center gap-2">
          <PlayPill className="lp-shine !px-5 !py-2 text-xs">{COPY.hero.primary}</PlayPill>
          <button
            type="button"
            aria-label={open ? 'Close menu' : 'Open menu'}
            aria-expanded={open}
            onClick={() => setOpen((v) => !v)}
            className="flex h-9 w-9 items-center justify-center rounded-lg border border-white/12 text-white/70 transition-colors hover:text-white md:hidden"
          >
            {open ? <X className="h-4 w-4" /> : <Menu className="h-4 w-4" />}
          </button>
        </div>
      </nav>

      {/* mobile dropdown */}
      {open && (
        <div className="border-t border-white/8 px-5 pb-5 pt-2 md:hidden">
          <div className="flex flex-col">
            {COPY.nav.map((item) => (
              <a
                key={item.href}
                href={item.href}
                onClick={() => setOpen(false)}
                className="border-b border-white/5 py-3 font-mono text-sm text-white/70 transition-colors hover:text-white"
              >
                {item.label}
              </a>
            ))}
          </div>
        </div>
      )}
    </header>
  );
}
