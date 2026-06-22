import type { Asset } from '@/pricing/types';

export interface AssetMeta {
  ticker: Asset;
  /** Full name shown in the selector dropdown. */
  name: string;
  /** Brand color for the asset's coin chip. */
  color: string;
  /** Decimals for the headline price. SUI trades sub-cent moves at ~$3, so it
   *  needs more precision than the dollar-tick majors. */
  priceDecimals: number;
}

export const ASSETS: Record<Asset, AssetMeta> = {
  SUI: { ticker: 'SUI', name: 'Sui', color: '#4da2ff', priceDecimals: 4 },
  BTC: { ticker: 'BTC', name: 'Bitcoin', color: '#f7931a', priceDecimals: 2 },
  ETH: { ticker: 'ETH', name: 'Ethereum', color: '#627eea', priceDecimals: 2 },
};

/** Selector order: SUI first (the default quote), then the majors. */
export const ASSET_LIST: Asset[] = ['SUI', 'BTC', 'ETH'];

export const DEFAULT_ASSET: Asset = 'SUI';
