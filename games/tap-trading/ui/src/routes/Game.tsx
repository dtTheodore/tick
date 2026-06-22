import type { HistoryItem } from '@/hooks/useMe';
import { useState } from 'react';
import { DebugOverlay } from '../components/DebugOverlay';
import { Grid } from '../components/Grid';
import { HeaderBar } from '../components/HeaderBar';
import { HistoryStrip } from '../components/HistoryStrip';
import { PositionTracker } from '../components/PositionTracker';
import { StakeSelector } from '../components/StakeSelector';
import { Toaster } from '../components/Toaster';
import { VerifyDrawer } from '../components/VerifyDrawer';
import { WalletDrawer } from '../components/WalletDrawer';

export function Game() {
  const [walletOpen, setWalletOpen] = useState(false);
  const [verifyItem, setVerifyItem] = useState<HistoryItem | null>(null);
  return (
    <main className="tick-play flex h-full flex-col">
      <HeaderBar onWalletClick={() => setWalletOpen(true)} />
      <StakeSelector />
      <Grid />
      <HistoryStrip onVerify={setVerifyItem} />
      <Toaster />
      <DebugOverlay />
      <PositionTracker />
      <WalletDrawer open={walletOpen} onClose={() => setWalletOpen(false)} />
      <VerifyDrawer item={verifyItem} onClose={() => setVerifyItem(null)} />
    </main>
  );
}
