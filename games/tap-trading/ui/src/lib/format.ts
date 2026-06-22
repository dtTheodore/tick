const USDC_MICRO = 1_000_000;

/**
 * Format an integer **micro-USDC** amount for display. The whole Tick economy is
 * denominated in USDC micro-units (1e6 = $1) — balances, stakes, and payouts all
 * arrive in this unit (see backend `validation.rs`), so they all format through
 * here for one consistent presentation across the header, grid, and history.
 *
 * Widens to 4 decimals only for sub-cent amounts so a micro-stake still reads as
 * a number instead of collapsing to `0.00`.
 */
export function formatUsdc(micro: number): string {
  const usd = Math.abs(micro) / USDC_MICRO;
  const maxFrac = usd > 0 && usd < 0.01 ? 4 : 2;
  return usd.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: maxFrac });
}

/** Signed variant for P&L deltas; uses U+2212 (−) to match the grid/strip glyphs. */
export function formatUsdcSigned(micro: number): string {
  const sign = micro > 0 ? '+' : micro < 0 ? '−' : '';
  return `${sign}${formatUsdc(micro)}`;
}
