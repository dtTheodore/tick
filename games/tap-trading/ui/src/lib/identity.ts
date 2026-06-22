const KEY = 'tick.accountId';

export function getAccountId(): string {
  const existing = localStorage.getItem(KEY);
  if (existing) return existing;
  const fresh = crypto.randomUUID();
  localStorage.setItem(KEY, fresh);
  return fresh;
}

export function resetAccountId(): void {
  localStorage.removeItem(KEY);
}

// Deposit is two-phase: the wallet commits USDC on-chain, then the API credits
// the off-chain balance from the digest. If phase two never lands (the credit
// POST is dropped — wallet popup steals focus, tab closes, network blips), the
// funds sit in the vault uncredited. We persist the digest the instant phase one
// succeeds so the credit can always be replayed (the API is idempotent by
// digest) — on reopen and before any new signing.
const PENDING_DEPOSIT_KEY = 'tick.pendingDeposit';

export interface PendingDeposit {
  digest: string;
  micro: number;
}

export function getPendingDeposit(): PendingDeposit | null {
  const raw = localStorage.getItem(PENDING_DEPOSIT_KEY);
  if (!raw) return null;
  try {
    const v = JSON.parse(raw) as PendingDeposit;
    return typeof v?.digest === 'string' && v.digest.length > 0 ? v : null;
  } catch {
    return null;
  }
}

export function setPendingDeposit(d: PendingDeposit): void {
  localStorage.setItem(PENDING_DEPOSIT_KEY, JSON.stringify(d));
}

export function clearPendingDeposit(): void {
  localStorage.removeItem(PENDING_DEPOSIT_KEY);
}
