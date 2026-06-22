import { useEffect, useRef, useState } from 'react';
import { usePrefersReducedMotion } from './usePrefersReducedMotion';

// Eases a number from 0 → `target` once the element scrolls into view (fires
// once). Under reduced motion it snaps straight to the target. The caller owns
// formatting (decimals, suffixes) so this stays a pure numeric ramp.
export function useCountUp(target: number, durationMs = 1400) {
  const reduced = usePrefersReducedMotion();
  const ref = useRef<HTMLSpanElement>(null);
  const [value, setValue] = useState(0);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    if (reduced) {
      setValue(target);
      return;
    }
    let raf = 0;
    let start = -1;
    const run = () => {
      const tick = (t: number) => {
        if (start < 0) start = t;
        const p = Math.min(1, (t - start) / durationMs);
        // easeOutExpo — fast then settles, reads as a counter spinning to rest.
        const eased = p === 1 ? 1 : 1 - 2 ** (-10 * p);
        setValue(target * eased);
        if (p < 1) raf = requestAnimationFrame(tick);
      };
      raf = requestAnimationFrame(tick);
    };
    const obs = new IntersectionObserver(
      (entries) => {
        for (const e of entries) {
          if (e.isIntersecting) {
            run();
            obs.disconnect();
          }
        }
      },
      { threshold: 0.4 },
    );
    obs.observe(el);
    return () => {
      cancelAnimationFrame(raf);
      obs.disconnect();
    };
  }, [target, durationMs, reduced]);

  return { ref, value };
}
