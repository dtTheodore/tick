import { ApiError, api } from '@/lib/api';
import { chainEnv } from '@/lib/env';
import {
  type PendingDeposit,
  clearPendingDeposit,
  getPendingDeposit,
  setPendingDeposit,
} from '@/lib/identity';
import { toast } from '@/lib/toast';
import {
  ConnectModal,
  useCurrentAccount,
  useDisconnectWallet,
  useSignAndExecuteTransaction,
} from '@mysten/dapp-kit';
import { Transaction, coinWithBalance } from '@mysten/sui/transactions';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useCallback, useEffect, useState } from 'react';

// USDC has 6 decimals; balances and stakes are integer micro-units everywhere.
const USDC_DECIMALS = 6;
const MICRO = 10 ** USDC_DECIMALS;

interface MeResponse {
  balance: number; // micro-USDC
}

type Mode = 'deposit' | 'withdraw';
type Phase = 'idle' | 'signing' | 'confirming' | 'done' | 'error';

function fmtUsdc(micro: number): string {
  return (micro / MICRO).toLocaleString(undefined, {
    minimumFractionDigits: 2,
    maximumFractionDigits: 4,
  });
}

function short(addr: string): string {
  return `${addr.slice(0, 6)}…${addr.slice(-4)}`;
}

/**
 * Deposit / withdraw real USDC against the on-chain custody vault.
 *
 * Deposit: the connected wallet signs ONE `vault::deposit` into the custody
 * PlayerBalance, then we post the digest to the API, which verifies it on-chain
 * and credits the off-chain balance. Play itself never signs. Withdraw debits
 * the balance server-side and the operator releases USDC back to this wallet.
 */
export function WalletDrawer({ open, onClose }: { open: boolean; onClose: () => void }) {
  const account = useCurrentAccount();
  const { mutateAsync: disconnect } = useDisconnectWallet();
  const { mutateAsync: signAndExecute } = useSignAndExecuteTransaction();
  const qc = useQueryClient();

  const [mode, setMode] = useState<Mode>('deposit');
  const [amount, setAmount] = useState('');
  const [phase, setPhase] = useState<Phase>('idle');
  const [note, setNote] = useState<string | null>(null);
  const [pending, setPending] = useState<PendingDeposit | null>(null);

  const me = useQuery({
    queryKey: ['me-balance'],
    queryFn: () => api<MeResponse>('/v1/me'),
    enabled: open,
    refetchInterval: open ? 4000 : false,
  });
  const balanceMicro = me.data?.balance ?? 0;

  function reset() {
    setPhase('idle');
    setNote(null);
    setAmount('');
  }

  const refresh = useCallback(async () => {
    await qc.invalidateQueries({ queryKey: ['me-balance'] });
    await qc.invalidateQueries({ queryKey: ['me'] });
  }, [qc]);

  // Replay the off-chain credit for a deposit already committed on-chain. The
  // API is idempotent by digest, so retrying is always safe; a just-executed
  // deposit can briefly be unindexed on the API's fullnode (propagation lag →
  // deposit_unverifiable), so we back off and retry rather than strand funds.
  const creditDeposit = useCallback(
    async (digest: string, depositMicro: number) => {
      setPhase('confirming');
      setNote(null);
      const backoffMs = [0, 1500, 3000, 5000, 8000];
      let lastErr: unknown;
      for (const wait of backoffMs) {
        if (wait) await new Promise((r) => setTimeout(r, wait));
        try {
          const credited = await api<{ credited_micro: number; balance: number }>('/v1/deposit', {
            method: 'POST',
            body: { tx_digest: digest },
          });
          clearPendingDeposit();
          setPending(null);
          await refresh();
          setPhase('done');
          const amt = credited.credited_micro || depositMicro;
          setNote(`+${fmtUsdc(amt)} USDC credited`);
          toast.info(`Deposited ${fmtUsdc(amt)} USDC`);
          return;
        } catch (e) {
          lastErr = e;
        }
      }
      // Inline retries exhausted — the digest stays persisted, so reopening the
      // panel (or tapping "Finish crediting") replays it. Funds are never lost.
      console.error('[deposit] credit failed; digest kept for retry', { digest, error: lastErr });
      setPhase('error');
      setNote(creditError());
    },
    [refresh],
  );

  // On open: clear transient state, then surface and auto-retry any deposit that
  // committed on-chain but never got credited (dropped POST, closed tab, …).
  useEffect(() => {
    if (!open) return;
    setPhase('idle');
    setNote(null);
    setAmount('');
    const p = getPendingDeposit();
    setPending(p);
    if (p) void creditDeposit(p.digest, p.micro);
  }, [open, creditDeposit]);

  const micro = Math.round(Number.parseFloat(amount || '0') * MICRO);
  const busy = phase === 'signing' || phase === 'confirming';
  const hasPending = mode === 'deposit' && !!pending;
  const canSubmit =
    !!account &&
    !busy &&
    (hasPending || (micro > 0 && (mode === 'withdraw' ? micro <= balanceMicro : true)));

  async function onDeposit() {
    if (!chainEnv.vaultPkg || !chainEnv.usdcType || !chainEnv.custodyPb) return;
    // Never sign a fresh deposit while an earlier one is uncredited — that would
    // strand a second on-chain deposit. Finish the pending credit first.
    const existing = getPendingDeposit();
    if (existing) {
      await creditDeposit(existing.digest, existing.micro);
      return;
    }
    setPhase('signing');
    setNote(null);
    try {
      const tx = new Transaction();
      tx.moveCall({
        target: `${chainEnv.vaultPkg}::vault::deposit`,
        typeArguments: [chainEnv.usdcType],
        arguments: [
          tx.object(chainEnv.custodyPb),
          coinWithBalance({ type: chainEnv.usdcType, balance: BigInt(micro) }),
        ],
      });
      const res = await signAndExecute({ transaction: tx });
      // Phase one is on-chain now — persist the digest BEFORE crediting so a
      // dropped credit POST can always be replayed instead of losing the funds.
      const p = { digest: res.digest, micro };
      setPendingDeposit(p);
      setPending(p);
      await creditDeposit(res.digest, micro);
    } catch (e) {
      // Build/sign failure (user rejected, or no USDC of the configured type) —
      // nothing committed on-chain, so there is nothing to recover.
      console.error('[deposit] sign/execute failed', e);
      setPhase('error');
      setNote(depositError(e));
    }
  }

  async function onWithdraw() {
    setPhase('confirming');
    setNote(null);
    try {
      const res = await api<{ tx_digest: string; balance: number }>('/v1/withdraw', {
        method: 'POST',
        body: { amount_micro: micro },
      });
      await refresh();
      setPhase('done');
      setNote(`Sent · ${res.tx_digest.slice(0, 10)}…`);
      toast.info(`Withdrew ${fmtUsdc(micro)} USDC`);
    } catch (e) {
      setPhase('error');
      setNote(withdrawError(e));
    }
  }

  return (
    <>
      {/* scrim — a button so keyboard users can dismiss it too */}
      <button
        type="button"
        aria-label="Close wallet"
        tabIndex={open ? 0 : -1}
        className={`fixed inset-0 z-40 cursor-default bg-black/60 backdrop-blur-[2px] transition-opacity duration-300 ${
          open ? 'opacity-100' : 'pointer-events-none opacity-0'
        }`}
        onClick={onClose}
      />
      {/* drawer */}
      <aside
        className={`fixed right-0 top-0 z-50 flex h-full w-full max-w-[380px] flex-col border-l border-white/10 bg-[#0b0b0e] shadow-[-24px_0_60px_rgba(0,0,0,0.6)] transition-transform duration-300 ease-out ${
          open ? 'translate-x-0' : 'translate-x-full'
        }`}
      >
        {/* header */}
        <div className="flex items-center justify-between border-b border-white/10 px-5 py-4">
          <div className="font-mono text-sm uppercase tracking-[0.2em] text-white/60">Wallet</div>
          <button
            type="button"
            onClick={onClose}
            className="text-white/40 transition-colors hover:text-white"
            aria-label="Close wallet"
          >
            ✕
          </button>
        </div>

        {!account ? (
          <div className="flex flex-1 flex-col items-center justify-center gap-5 px-6 text-center">
            <div className="font-mono text-[13px] leading-relaxed text-white/50">
              Connect a Sui wallet to deposit USDC. You sign once to fund — then every tap is
              instant, with <span className="text-white/80">no per-bet signing.</span>
            </div>
            <ConnectModal
              trigger={
                <button
                  type="button"
                  className="rounded-[8px] bg-tick-pink px-6 py-3 font-mono text-sm font-semibold text-white shadow-[0_0_24px_rgba(255,45,126,0.45)] transition-transform hover:scale-[1.03] active:scale-95"
                >
                  Connect Wallet
                </button>
              }
            />
          </div>
        ) : (
          <div className="flex flex-1 flex-col px-5 py-5">
            {/* connected address + balance */}
            <div className="rounded-[10px] border border-white/10 bg-white/[0.03] p-4">
              <div className="flex items-center justify-between">
                <span className="font-mono text-[11px] uppercase tracking-wider text-white/40">
                  Playable balance
                </span>
                <button
                  type="button"
                  onClick={() => disconnect()}
                  className="font-mono text-[11px] text-white/40 transition-colors hover:text-tick-pink"
                  title={account.address}
                >
                  {short(account.address)} · disconnect
                </button>
              </div>
              <div className="mt-1.5 font-mono text-3xl font-semibold tabular-nums text-white">
                {fmtUsdc(balanceMicro)} <span className="text-base text-white/40">USDC</span>
              </div>
            </div>

            {/* tabs */}
            <div className="mt-5 grid grid-cols-2 rounded-[8px] border border-white/10 p-1">
              {(['deposit', 'withdraw'] as Mode[]).map((m) => (
                <button
                  key={m}
                  type="button"
                  onClick={() => {
                    setMode(m);
                    reset();
                  }}
                  className={`rounded-[6px] py-2 font-mono text-xs font-semibold uppercase tracking-wider transition-colors ${
                    mode === m ? 'bg-tick-pink/20 text-white' : 'text-white/40 hover:text-white/70'
                  }`}
                >
                  {m}
                </button>
              ))}
            </div>

            {/* pending-credit recovery — a deposit is on-chain but uncredited */}
            {hasPending && (
              <div className="mt-5 rounded-[10px] border border-tick-pink/40 bg-tick-pink/10 px-4 py-3 font-mono text-[11px] leading-relaxed text-white/70">
                A deposit of <span className="text-white">{fmtUsdc(pending?.micro ?? 0)} USDC</span>{' '}
                is confirmed on-chain but not yet credited. Your funds are safe — tap{' '}
                <span className="text-white">Finish crediting</span> to complete it.
              </div>
            )}

            {/* amount input — hidden while finishing a pending credit */}
            {!hasPending && (
              <label className="mt-5 block">
                <span className="font-mono text-[11px] uppercase tracking-wider text-white/40">
                  Amount
                </span>
                <div className="mt-1.5 flex items-center gap-2 rounded-[10px] border border-white/10 bg-black/40 px-4 py-3 focus-within:border-tick-pink/60">
                  <input
                    inputMode="decimal"
                    placeholder="0.00"
                    value={amount}
                    onChange={(e) => setAmount(e.target.value.replace(/[^0-9.]/g, ''))}
                    className="w-full bg-transparent font-mono text-2xl tabular-nums text-white outline-none placeholder:text-white/20"
                  />
                  <span className="font-mono text-sm text-white/40">USDC</span>
                  {mode === 'withdraw' && (
                    <button
                      type="button"
                      onClick={() => setAmount(String(balanceMicro / MICRO))}
                      className="rounded-[5px] border border-white/15 px-2 py-1 font-mono text-[10px] uppercase text-white/50 hover:text-white"
                    >
                      max
                    </button>
                  )}
                </div>
              </label>
            )}

            {/* submit */}
            <button
              type="button"
              disabled={!canSubmit}
              onClick={mode === 'deposit' ? onDeposit : onWithdraw}
              className="mt-5 rounded-[10px] bg-tick-pink py-3.5 font-mono text-sm font-bold uppercase tracking-wider text-white shadow-[0_0_24px_rgba(255,45,126,0.35)] transition-all hover:scale-[1.02] active:scale-95 disabled:cursor-not-allowed disabled:bg-white/10 disabled:text-white/30 disabled:shadow-none"
            >
              {busy
                ? phase === 'signing'
                  ? 'Sign in wallet…'
                  : 'Confirming…'
                : mode === 'deposit'
                  ? hasPending
                    ? 'Finish crediting'
                    : 'Deposit USDC'
                  : 'Withdraw USDC'}
            </button>

            {/* status */}
            {note && (
              <div
                className={`mt-3 font-mono text-[12px] ${
                  phase === 'error' ? 'text-tick-loss' : 'text-tick-win'
                }`}
              >
                {note}
              </div>
            )}

            <div className="mt-auto pt-6 font-mono text-[10px] leading-relaxed text-white/25">
              Real USDC on Sui {chainEnv.network}. Deposit signs one on-chain tx into the audited
              vault; play settles off-chain instantly; withdraw returns USDC to this wallet.
            </div>
          </div>
        )}
      </aside>
    </>
  );
}

// Sign/build-phase failures only — nothing committed on-chain. coinWithBalance
// throws "Insufficient balance of <type>…" when the wallet holds none of the
// configured USDC, which is the most common real-world stumble.
function depositError(e: unknown): string {
  if (e instanceof Error && /reject|cancel|denied/i.test(e.message)) return 'Signing cancelled.';
  if (e instanceof Error && /insufficient balance of/i.test(e.message))
    return 'This wallet holds no USDC of the supported type — fund it at faucet.circle.com (Sui testnet).';
  return 'Could not sign the deposit — see console.';
}

// Credit-phase failure — the deposit is already on-chain, so funds are safe and
// the digest is kept for retry. Reassure and point at the recovery action.
function creditError(): string {
  return 'Deposited on-chain ✓ — crediting will finish on retry. Reopen this panel if your balance is not updated.';
}

function withdrawError(e: unknown): string {
  if (e instanceof ApiError && e.code === 'insufficient_balance') return 'Not enough balance.';
  if (e instanceof ApiError && e.code === 'no_withdraw_address')
    return 'Deposit once first to link your wallet.';
  return 'Withdraw failed — see console.';
}
