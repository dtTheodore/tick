function required(name: string): string {
  const v = (import.meta.env as Record<string, string | undefined>)[name];
  if (!v) throw new Error(`${name} is required (set in .env.local via sync-service-envs.sh)`);
  return v;
}

function optional(name: string): string | undefined {
  return (import.meta.env as Record<string, string | undefined>)[name] || undefined;
}

export const env = {
  apiUrl: required('VITE_TAP_API_URL'),
  wsUrl: required('VITE_TAP_API_WS_URL'),
};

/** On-chain USDC deposit/withdraw config. Undefined fields disable the wallet
 *  flow gracefully (the game still runs without an in-app funding path). */
export const chainEnv = {
  network: (optional('VITE_SUI_NETWORK') ?? 'testnet') as 'testnet' | 'mainnet' | 'devnet',
  vaultPkg: optional('VITE_TICK_VAULT_PKG'),
  usdcType: optional('VITE_TICK_USDC_TYPE'),
  custodyPb: optional('VITE_TICK_CUSTODY_PB'),
};

export const walletConfigured = Boolean(
  chainEnv.vaultPkg && chainEnv.usdcType && chainEnv.custodyPb,
);
