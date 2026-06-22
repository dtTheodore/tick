import { ASSETS } from '@/lib/assets';
import type { Asset } from '@/pricing/types';

/** A small brand-colored coin chip. Inline glyph/SVG only — no network fetch —
 *  so it paints instantly and needs no asset pipeline or CSP allowance. */
export function AssetIcon({ asset, size = 18 }: { asset: Asset; size?: number }) {
  const { color } = ASSETS[asset];
  return (
    <span
      aria-hidden
      className="inline-flex shrink-0 items-center justify-center rounded-full"
      style={{ width: size, height: size, background: color }}
    >
      {asset === 'SUI' ? (
        <svg width={size * 0.62} height={size * 0.62} viewBox="0 0 24 24" fill="none">
          <title>Sui</title>
          {/* Sui's water-drop mark, simplified. */}
          <path d="M12 3c0 0 6.5 7.2 6.5 11.5a6.5 6.5 0 1 1-13 0C5.5 10.2 12 3 12 3Z" fill="#fff" />
        </svg>
      ) : (
        <span
          className="font-mono font-bold leading-none text-white"
          style={{ fontSize: size * 0.62 }}
        >
          {asset === 'BTC' ? '₿' : 'Ξ'}
        </span>
      )}
    </span>
  );
}
