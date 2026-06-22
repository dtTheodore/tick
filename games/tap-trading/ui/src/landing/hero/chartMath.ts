// Shared price→position geometry for the hero demo. The canvas line
// (SyntheticChart) and the DOM bet-band overlay (DemoPanel) both map prices
// through this single function, so a band drawn over the canvas lines up
// exactly with the curve — no duplicated, drifting math.

export const HERO_START = 1711.25;

export interface ChartWindow {
  start: number; // price centered vertically
  halfRange: number; // price half-window mapped top→bottom (fixed = no jitter)
  padY: number; // vertical padding as a fraction of height
}

export const HERO_WINDOW: ChartWindow = {
  start: HERO_START,
  halfRange: 16,
  padY: 0.16,
};

// Returns the vertical position of `price` as a fraction of chart height
// (0 = top = high price, 1 = bottom = low price), clamped to the window.
export function priceToYFrac(price: number, win: ChartWindow = HERO_WINDOW): number {
  const norm = (price - (win.start - win.halfRange)) / (2 * win.halfRange);
  const clamped = Math.min(1, Math.max(0, norm));
  return win.padY + (1 - clamped) * (1 - 2 * win.padY);
}

// ── Quote display ──────────────────────────────────────────────────────────
// The demo quotes SUI/USD — the product's default asset (see lib/assets.ts),
// which trades near $3 in sub-cent moves. The price geometry above is calibrated
// in abstract units around HERO_START, so displayed prices are remapped into
// SUI's band at 4-decimal precision: only the shown numbers change, never the
// curve, bands, ladder spacing, or multipliers.
export const HERO_SYMBOL = 'SUI/USD';
const QUOTE_MID = 3.42;
const QUOTE_SCALE = 0.002; // abstract-$ → SUI-$; the ±16u window maps to ±$0.032
const QUOTE_DECIMALS = 4;

export function fmtQuotePrice(internal: number): string {
  const quote = QUOTE_MID + (internal - HERO_START) * QUOTE_SCALE;
  return quote.toLocaleString('en-US', {
    minimumFractionDigits: QUOTE_DECIMALS,
    maximumFractionDigits: QUOTE_DECIMALS,
  });
}
