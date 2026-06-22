import { SuiClientProvider, WalletProvider, createNetworkConfig } from '@mysten/dapp-kit';
import { QueryClientProvider } from '@tanstack/react-query';
import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import '@mysten/dapp-kit/dist/index.css';
import App from './App';
import { chainEnv } from './lib/env';
import { queryClient } from './lib/query-client';
import './index.css';

// Public fullnode URLs (the `getFullnodeUrl` helper was dropped in @mysten/sui
// 2.0); these are the stable Sui-hosted endpoints. dapp-kit's jsonRpc client
// now requires the `network` discriminator alongside `url`.
const { networkConfig } = createNetworkConfig({
  testnet: { url: 'https://fullnode.testnet.sui.io:443', network: 'testnet' },
  mainnet: { url: 'https://fullnode.mainnet.sui.io:443', network: 'mainnet' },
  devnet: { url: 'https://fullnode.devnet.sui.io:443', network: 'devnet' },
});

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <SuiClientProvider networks={networkConfig} defaultNetwork={chainEnv.network}>
        <WalletProvider autoConnect>
          <App />
        </WalletProvider>
      </SuiClientProvider>
    </QueryClientProvider>
  </StrictMode>,
);
