import type { CSSProperties, ReactNode } from 'react';
import { Link } from 'react-router-dom';
import { useReveal } from './lib/useReveal';

// Fade-up on scroll-into-view. `delayMs` staggers siblings via animation-delay
// (transform/opacity only — see .lp-reveal in index.css). Reduced motion is
// handled in CSS, so this just no-ops visually there.
export function Reveal({
  children,
  className = '',
  delayMs = 0,
  as: Tag = 'div',
  variant = 'up',
  style,
}: {
  children: ReactNode;
  className?: string;
  delayMs?: number;
  as?: 'div' | 'section' | 'li' | 'article';
  variant?: 'up' | 'scale';
  style?: CSSProperties;
}) {
  const { ref, visible } = useReveal();
  const base = variant === 'scale' ? 'lp-reveal-scale' : 'lp-reveal';
  return (
    <Tag
      // @ts-expect-error polymorphic ref across the small allowed tag set
      ref={ref}
      className={`${base} ${visible ? 'is-visible' : ''} ${className}`}
      style={delayMs ? { ...style, animationDelay: `${delayMs}ms` } : style}
    >
      {children}
    </Tag>
  );
}

// Small uppercase label that sits above section titles.
export function Eyebrow({ children, tone = 'pink' }: { children: ReactNode; tone?: 'pink' | 'sui' }) {
  const color = tone === 'sui' ? 'text-lp-sui-blue' : 'text-lp-pink';
  return (
    <div className={`flex items-center gap-2 font-mono text-xs uppercase tracking-[0.22em] ${color}`}>
      <span className={`h-px w-6 ${tone === 'sui' ? 'bg-lp-sui-blue' : 'bg-lp-pink'} opacity-70`} />
      {children}
    </div>
  );
}

// The one primary action, repeated across the page. `to` defaults to the game.
export function PlayPill({
  children,
  variant = 'primary',
  to = '/play',
  href,
  className = '',
}: {
  children: ReactNode;
  variant?: 'primary' | 'ghost';
  to?: string;
  href?: string;
  className?: string;
}) {
  const base =
    'inline-flex items-center justify-center gap-2 rounded-full px-7 py-3.5 font-mono text-sm font-semibold tracking-wide transition-all duration-200 active:scale-[0.97] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-offset-2 focus-visible:ring-offset-lp-bg';
  const styles =
    variant === 'primary'
      ? 'bg-lp-pink text-black shadow-[0_0_28px_rgba(255,45,126,0.45)] hover:shadow-[0_0_42px_rgba(255,45,126,0.65)] hover:-translate-y-0.5 focus-visible:ring-lp-pink'
      : 'border border-white/15 bg-white/[0.03] text-white/90 backdrop-blur-sm hover:border-white/30 hover:bg-white/[0.06] focus-visible:ring-white/40';
  const cls = `${base} ${styles} ${className}`;
  if (href) {
    return (
      <a href={href} className={cls}>
        {children}
      </a>
    );
  }
  return (
    <Link to={to} className={cls}>
      {children}
    </Link>
  );
}
