import type { HistoryItem } from '@/hooks/useMe';
import { type CellPosition, positionsStore } from '@/lib/positions-store';

// Demo (play-money) engine — the entire economy of demo mode, client-side.
//
// Faithful, not faked: it settles on the SAME `struckAtMs` touch the on-screen
// cue uses, and useLiveHitDetection stamps that with the exact predicate the
// server's settlement runs (price-path segment ∩ band, gated on the tick's own
// timestamp). So a demo WON/LOST matches what the real server would decide for
// the same live price path — the only thing missing is server authority and the
// on-chain Walrus proof, neither of which play money needs.
//
// Read/write shape: a single writer (this tab); reads are O(1) snapshot lookups
// with no aggregation; history is prepend-on-tap then replace-in-place on settle,
// display-bounded. Balance is maintained at write time (debited on lock, credited
// on win) — the cheapest possible "cache", a counter updated at the mutation.

const BALANCE_KEY = 'tick.demoBalanceMicro';
const INITIAL_BALANCE_MICRO = 100_000_000; // 100 USDC play money
const HISTORY_CAP = 40; // headroom over the strip's 14-item display depth

// Settle just after the window closes, leaving room for the closing tick's cue
// to land (useLiveHitDetection stamps struckAtMs on that tick).
const SETTLE_GRACE_MS = 400;

function loadBalance(): number {
  if (typeof localStorage === 'undefined') return INITIAL_BALANCE_MICRO;
  // Distinguish "never funded" (absent key) from a real stored 0 — Number(null)
  // is 0, so a missing key must grant the initial play money, not bankrupt a
  // first-time player.
  const raw = localStorage.getItem(BALANCE_KEY);
  if (raw === null) return INITIAL_BALANCE_MICRO;
  const n = Number(raw);
  return Number.isFinite(n) && n >= 0 ? n : INITIAL_BALANCE_MICRO;
}

let balance = loadBalance();
let history: HistoryItem[] = []; // newest-first, capped
let nextId = -1; // demo ids are negative so they never collide with server ids
const listeners = new Set<() => void>();

function persistBalance() {
  try {
    localStorage.setItem(BALANCE_KEY, String(balance));
  } catch {
    // private-mode / quota — balance still tracks for the session.
  }
}

function emit() {
  for (const cb of listeners) cb();
}

/** Thrown by `lock` when the play-money balance can't cover the stake. useTap
 *  maps it to the same "insufficient balance" toast the live server returns. */
export class DemoInsufficientBalance extends Error {}

export const demoEngine = {
  subscribe(cb: () => void) {
    listeners.add(cb);
    return () => {
      listeners.delete(cb);
    };
  },
  /** Balance in micro-USDC. Stable reference (a primitive) until it changes. */
  getBalance(): number {
    return balance;
  },
  /** Recent taps — in-flight OPEN chips and settled WON/LOST — newest-first.
   *  Stable reference until a tap or settle replaces it. */
  getHistory(): HistoryItem[] {
    return history;
  },

  /** Reserve a tap: debit the stake now — mirroring the server debiting on lock,
   *  so the displayed balance behaves identically — and return a local id. */
  lock(stakeMicro: number): { positionId: number } {
    if (stakeMicro > balance) throw new DemoInsufficientBalance();
    balance -= stakeMicro;
    persistBalance();
    emit();
    return { positionId: nextId-- };
  },

  /** Surface the tap as an in-flight "live" chip the instant it locks, mirroring
   *  live mode — there the server returns the OPEN position and the strip's
   *  refetch shows it ~one round-trip after tap. `settleDue` later flips this
   *  same entry to WON/LOST in place. Idempotent per position. */
  recordOpen(p: CellPosition) {
    if (p.positionId === undefined) return;
    if (history.some((h) => h.position_id === p.positionId)) return;
    history = [toHistoryItem(p, 'OPEN', Date.now(), 0), ...history].slice(0, HISTORY_CAP);
    emit();
  },

  /** Settle every demo position whose window has closed: WON if the price touched
   *  the band during the window (struckAtMs stamped in-window by the cue), else
   *  LOST. Idempotent — terminal positions are skipped. Driven by a 250ms interval
   *  while the game screen is mounted (see PositionTracker). */
  settleDue(now: number) {
    let changed = false;
    for (const p of positionsStore.getSnapshot().values()) {
      if (!p.demo || p.state !== 'LOCKED' || p.positionId === undefined) continue;
      if (now < p.tCloseMs + SETTLE_GRACE_MS) continue;
      const won = p.struckAtMs !== undefined;
      const payoutMicro = won ? Math.round(p.stake * (p.multiplierAtTap ?? 0)) : 0;
      if (won) balance += payoutMicro;
      positionsStore.update(p.cellKey, { state: won ? 'WON' : 'LOST', settledAtMs: now });
      const settled = toHistoryItem(p, won ? 'WON' : 'LOST', now, payoutMicro);
      // Replace the in-flight OPEN chip in place (keep its created_at_ms so its
      // slot doesn't reshuffle) — mirrors live, where the row holds its position
      // and just flips status. Fall back to prepend if no OPEN entry exists.
      const idx = history.findIndex((h) => h.position_id === p.positionId);
      if (idx >= 0) {
        settled.created_at_ms = history[idx].created_at_ms;
        const next = history.slice();
        next[idx] = settled;
        history = next;
      } else {
        history = [settled, ...history].slice(0, HISTORY_CAP);
      }
      changed = true;
    }
    if (changed) {
      persistBalance();
      emit();
    }
  },
};

/** Build the strip's HistoryItem for a demo position. OPEN carries no settlement
 *  (an in-flight "live" chip); WON/LOST carry `points_delta` as the GROSS payout
 *  on a win (stake × multiplier) — the strip subtracts the stake to show net
 *  gain — or the forfeited stake on a loss, matching the live history contract.
 *  `atMs` is the tap time for OPEN, the settle time for terminal states. */
function toHistoryItem(
  p: CellPosition,
  status: 'OPEN' | 'WON' | 'LOST',
  atMs: number,
  payoutMicro: number,
): HistoryItem {
  const won = status === 'WON';
  return {
    position_id: p.positionId as number,
    asset: p.asset ?? '',
    strike_lo: p.strikeLo,
    strike_hi: p.strikeHi,
    t_open_ms: p.tOpenMs,
    t_close_ms: p.tCloseMs,
    stake_points: p.stake,
    multiplier_at_tap: p.multiplierAtTap ?? 0,
    status,
    created_at_ms: atMs,
    settlement:
      status === 'OPEN'
        ? null
        : {
            outcome: won ? 'W' : 'L',
            points_delta: won ? payoutMicro : -p.stake,
            settled_at_ms: atMs,
          },
    // No on-chain proof in demo — null so the strip shows the LIVE-only fairness
    // teaser instead of a verify button.
    walrus_blob_id: null,
    proof_status: null,
  };
}
