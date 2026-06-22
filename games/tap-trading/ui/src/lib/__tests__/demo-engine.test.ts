import { beforeEach, expect, test } from 'bun:test';
import type { CellPosition, LifecycleState } from '../positions-store';

// Mock localStorage BEFORE importing the engine: it reads the persisted balance
// at module load, and we want a clean 100-USDC start (empty store).
const storage = new Map<string, string>();
(globalThis as { localStorage?: Storage }).localStorage = {
  getItem: (k: string) => storage.get(k) ?? null,
  setItem: (k: string, v: string) => {
    storage.set(k, v);
  },
  removeItem: (k: string) => {
    storage.delete(k);
  },
  clear: () => storage.clear(),
  key: () => null,
  length: 0,
} as Storage;

const { demoEngine, DemoInsufficientBalance } = await import('../demo-engine');
const { positionsStore } = await import('../positions-store');

const USDC = 1_000_000;
const BALANCE_KEY = 'tick.demoBalanceMicro';

// A LOCKED demo position as it exists right before settlement. `struckAtMs` set
// ⇒ the live touch cue fired in-window (the server's WON condition).
function lockedPosition(over: Partial<CellPosition> & { cellKey: string }): CellPosition {
  return {
    clientRequestId: over.cellKey,
    stake: USDC,
    state: 'LOCKED' as LifecycleState,
    tOpenMs: 1_000,
    tCloseMs: 2_000,
    strikeLo: 100,
    strikeHi: 101,
    asset: 'SUI',
    demo: true,
    multiplierAtTap: 3,
    positionId: -1,
    ...over,
  };
}

beforeEach(() => {
  for (const k of [...positionsStore.getSnapshot().keys()]) positionsStore.delete(k);
});

test('starts with 100 USDC of play money', () => {
  expect(demoEngine.getBalance()).toBe(100 * USDC);
});

test('lock debits the stake up front and issues a negative (demo) id', () => {
  const before = demoEngine.getBalance();
  const { positionId } = demoEngine.lock(2 * USDC);
  expect(demoEngine.getBalance()).toBe(before - 2 * USDC);
  // Negative so demo ids never collide with server-issued position ids.
  expect(positionId).toBeLessThan(0);
});

test('lock rejects a stake the play-money balance cannot cover', () => {
  expect(() => demoEngine.lock(demoEngine.getBalance() + 1)).toThrow(DemoInsufficientBalance);
});

test('settles WON only when the touch cue fired in-window, paying stake × multiplier', () => {
  // A WON nets stake × (mult − 1): the stake was debited at lock, the gross
  // stake × mult is credited back. This is the same payout the server settles.
  const stake = 4 * USDC;
  const mult = 2.5;
  demoEngine.lock(stake);
  positionsStore.set(
    'won-cell',
    lockedPosition({
      cellKey: 'won-cell',
      stake,
      multiplierAtTap: mult,
      positionId: -7,
      struckAtMs: 1_500, // touched inside [tOpen, tClose]
    }),
  );

  const before = demoEngine.getBalance();
  demoEngine.settleDue(2_500); // now > tClose + grace
  expect(demoEngine.getBalance()).toBe(before + Math.round(stake * mult));
  expect(positionsStore.get('won-cell')?.state).toBe('WON');

  const item = demoEngine.getHistory()[0];
  expect(item.status).toBe('WON');
  // points_delta is the GROSS payout so the strip's `delta − stake` shows net gain.
  expect(item.settlement?.points_delta).toBe(Math.round(stake * mult));
});

test('settles LOST when the window closed untouched, forfeiting only the stake', () => {
  const stake = 3 * USDC;
  demoEngine.lock(stake);
  positionsStore.set(
    'lost-cell',
    lockedPosition({
      cellKey: 'lost-cell',
      stake,
      positionId: -8,
      struckAtMs: undefined, // never touched
    }),
  );

  const before = demoEngine.getBalance();
  demoEngine.settleDue(2_500);
  expect(demoEngine.getBalance()).toBe(before); // no credit — stake already gone
  expect(positionsStore.get('lost-cell')?.state).toBe('LOST');
  expect(demoEngine.getHistory()[0].settlement?.points_delta).toBe(-stake);
});

test('does not settle a position whose window has not closed yet', () => {
  positionsStore.set(
    'open-cell',
    lockedPosition({
      cellKey: 'open-cell',
      positionId: -9,
      tCloseMs: 9_999_999,
      struckAtMs: 1_500,
    }),
  );
  const before = demoEngine.getBalance();
  demoEngine.settleDue(2_500);
  expect(positionsStore.get('open-cell')?.state).toBe('LOCKED');
  expect(demoEngine.getBalance()).toBe(before);
});

test('ignores live (non-demo) positions — those settle server-side', () => {
  positionsStore.set(
    'live-cell',
    lockedPosition({
      cellKey: 'live-cell',
      positionId: 42,
      demo: false,
      struckAtMs: 1_500,
    }),
  );
  const before = demoEngine.getBalance();
  demoEngine.settleDue(2_500);
  expect(positionsStore.get('live-cell')?.state).toBe('LOCKED'); // untouched by demo engine
  expect(demoEngine.getBalance()).toBe(before);
});

test('shows the tap as an in-flight OPEN chip, then flips it in place on settle', () => {
  // The UX fix: demo surfaces the tap immediately (as live does) instead of
  // leaving the strip empty until the window closes.
  const stake = 2 * USDC;
  demoEngine.lock(stake);
  const p = lockedPosition({ cellKey: 'open-then-won', stake, positionId: -21, struckAtMs: 1_500 });
  positionsStore.set('open-then-won', p);

  demoEngine.recordOpen(p);
  const open = demoEngine.getHistory()[0];
  expect(open.position_id).toBe(-21);
  expect(open.status).toBe('OPEN');
  expect(open.settlement).toBeNull();

  // Settling must flip THIS row in place, not prepend a duplicate — mirroring
  // live, where the history row holds its slot and just changes status.
  const lenBefore = demoEngine.getHistory().length;
  demoEngine.settleDue(2_500);
  expect(demoEngine.getHistory().length).toBe(lenBefore);
  expect(demoEngine.getHistory().find((h) => h.position_id === -21)?.status).toBe('WON');
});

test('recordOpen is idempotent per position', () => {
  const p = lockedPosition({ cellKey: 'dup', positionId: -22 });
  demoEngine.recordOpen(p);
  demoEngine.recordOpen(p);
  expect(demoEngine.getHistory().filter((h) => h.position_id === -22).length).toBe(1);
});

test('persists the balance so play money survives a reload', () => {
  demoEngine.lock(1 * USDC);
  expect(storage.get(BALANCE_KEY)).toBe(String(demoEngine.getBalance()));
});
