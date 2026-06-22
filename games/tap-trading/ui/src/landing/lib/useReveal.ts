import { useEffect, useRef, useState } from 'react';

// Adds `is-visible` once the element scrolls into view (fires once, then
// disconnects). Pair with the `.lp-reveal` CSS class for a compositor-only
// fade-up. No animation library needed.
export function useReveal<T extends HTMLElement = HTMLDivElement>(
  rootMargin = '0px 0px -10% 0px',
) {
  const ref = useRef<T>(null);
  const [visible, setVisible] = useState(false);
  useEffect(() => {
    const el = ref.current;
    if (!el || visible) return;
    const obs = new IntersectionObserver(
      (entries) => {
        for (const e of entries) {
          if (e.isIntersecting) {
            setVisible(true);
            obs.disconnect();
          }
        }
      },
      { rootMargin, threshold: 0.1 },
    );
    obs.observe(el);
    return () => obs.disconnect();
  }, [rootMargin, visible]);
  return { ref, visible };
}
