export type Asset = 'ETH' | 'BTC' | 'SUI';

export interface Cell {
  asset: Asset;
  strike_lo: number;
  strike_hi: number;
  t_open_ms: number;
  t_close_ms: number;
}

export interface OracleState {
  asset: Asset;
  spot: number;
  sigma_annualized: number;
  timestamp_ms: number;
}

/** Mirrors `tap_trading_pricing_engine::types::PricingConfig`. */
export interface PricingConfig {
  house_margin: number;
  jump_buffer: number;
  tick_period_seconds: number;
  floor_a: number;
  floor_b: number;
  multiplier_cap: number;
}

/** Matches `PricingConfig::default()` in pricing-engine/src/types.rs.
 *  Kept in lockstep with the Rust default — divergence > 3% would trip the
 *  server drift gate and reject every tap. */
export const DEFAULT_PRICING_CONFIG: PricingConfig = {
  // 3% house edge (RTP 97%), uniform across all cells; see Rust default for the
  // rationale. EV = −house_margin on every cell.
  house_margin: 0.03,
  // No σ inflation — window-aware pricing + continuous-path settlement price at
  // fair σ (BGK discrete-monitoring correction removed with it).
  jump_buffer: 1.0,
  tick_period_seconds: 0.05,
  // Flat 1.0× minimum (a tap never returns less than the stake). The old
  // τ-growing floor paid near cells above fair value — the "too generous" leak.
  // Window-aware pricing makes future near cells uncertain (p < 1), so their
  // fair multiplier is naturally > 1 without a floor.
  floor_a: 1.0,
  floor_b: 0.0,
  multiplier_cap: 1000.0,
};

/** Hui series truncation cap per spec §7.1. Not part of PricingConfig. */
export const HUI_TERMS = 20;
