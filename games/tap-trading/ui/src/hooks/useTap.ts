import { ApiError, api } from '@/lib/api';
import { DemoInsufficientBalance, demoEngine } from '@/lib/demo-engine';
import { gameModeStore } from '@/lib/game-mode';
import { type LifecycleState, positionsStore } from '@/lib/positions-store';
import { stakeStore } from '@/lib/stake-store';
import { telemetry } from '@/lib/telemetry';
import { tickStore } from '@/lib/tick-store';
import { cellKey as makeCellKey } from '@/lib/time';
import { toast } from '@/lib/toast';
import { InvalidSigma, InvalidSpot } from '@/pricing/errors';
import { HuiConvergenceFailure } from '@/pricing/hui';
import { computeMultiplier } from '@/pricing/multiplier';
import { type Cell, DEFAULT_PRICING_CONFIG } from '@/pricing/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';

interface TapResponse {
  position_id: number;
  multiplier_at_tap: number;
  status: string;
  t_close_ms: number;
}

function mapStatus(s: string): LifecycleState {
  switch (s) {
    case 'OPEN':
      return 'LOCKED';
    case 'WON':
      return 'WON';
    case 'LOST':
      return 'LOST';
    case 'VOIDED':
      return 'VOIDED';
    default:
      return 'LOCKED';
  }
}

function errToast(err: unknown, reason: string): string {
  if (err instanceof ApiError) {
    switch (reason) {
      case 'stale_quote':
        return 'quote moved, try again';
      case 'drift_exceeded':
        return `price drifted (server ${err.serverMultiplier?.toFixed(2) ?? '—'}×)`;
      case 'insufficient_balance':
        return 'insufficient balance';
      case 'lock_window':
        return 'too late, cell closing';
      case 'invalid_cell':
      case 'invalid_stake':
      case 'unknown_asset':
        return 'invalid request (bug)';
      case 'missing_account_id':
      case 'invalid_account_id':
        return 'session lost, reload';
      case 'forbidden':
        return 'not your position';
      case 'rate_limited':
        return `slow down (retry in ${err.retryAfterSecs ?? 1}s)`;
      case 'not_found':
        return 'position not found';
      case 'internal':
        return 'server hiccup';
    }
    if (err.status === 429) return 'slow down (retry in 1s)';
    if (err.status >= 500) return 'server hiccup';
  }
  return 'network error';
}

export function useTap() {
  const qc = useQueryClient();

  return useMutation({
    mutationFn: async (cell: Cell) => {
      const key = makeCellKey(cell.strike_lo, cell.strike_hi, cell.t_open_ms);
      if (positionsStore.get(key)) {
        throw new Error('cell already tapped');
      }
      const snap = tickStore.getSnapshot();
      if (!snap.tick) throw new Error('no tick yet');
      if (Date.now() + 1000 >= cell.t_close_ms) throw new Error('lock window');

      // Read the live selection at tap time so the latest bet size always wins.
      const stake = stakeStore.get();
      const mode = gameModeStore.getSnapshot();
      const clientRequestId = crypto.randomUUID();
      positionsStore.set(key, {
        cellKey: key,
        clientRequestId,
        stake,
        state: 'PENDING',
        tOpenMs: cell.t_open_ms,
        tCloseMs: cell.t_close_ms,
        strikeLo: cell.strike_lo,
        strikeHi: cell.strike_hi,
        asset: cell.asset,
        demo: mode === 'demo',
      });

      let clientMultiplier: number;
      try {
        clientMultiplier = computeMultiplier(
          cell,
          {
            asset: cell.asset,
            spot: snap.tick.mid,
            sigma_annualized: snap.tick.vol_annualized,
            timestamp_ms: snap.tick.ts_ms,
          },
          DEFAULT_PRICING_CONFIG,
          Date.now(),
        );
      } catch (e) {
        if (
          e instanceof InvalidSpot ||
          e instanceof InvalidSigma ||
          e instanceof HuiConvergenceFailure
        ) {
          positionsStore.update(key, { state: 'REJECTED', rejectReason: 'client_pricing' });
          setTimeout(() => positionsStore.delete(key), 800);
          throw e;
        }
        throw e;
      }

      telemetry.tap(key, clientMultiplier);

      // Demo: settle locally. No network, no chain — lock against the play-money
      // balance, and let demo-engine settle at t_close off the same touch cue.
      if (mode === 'demo') {
        try {
          const { positionId } = demoEngine.lock(stake);
          positionsStore.update(key, {
            state: 'LOCKED',
            positionId,
            multiplierAtTap: clientMultiplier,
          });
          // Show the tap as an in-flight "live" chip immediately, as live mode
          // does — settleDue flips it to WON/LOST in place at t_close.
          const locked = positionsStore.get(key);
          if (locked) demoEngine.recordOpen(locked);
          telemetry.locked(positionId, clientMultiplier);
          return {
            position_id: positionId,
            multiplier_at_tap: clientMultiplier,
            status: 'OPEN',
            t_close_ms: cell.t_close_ms,
          };
        } catch (err) {
          const reason =
            err instanceof DemoInsufficientBalance ? 'insufficient_balance' : 'network';
          telemetry.reject(key, reason);
          toast.warn(reason === 'insufficient_balance' ? 'insufficient balance' : 'tap failed');
          positionsStore.update(key, { state: 'REJECTED', rejectReason: reason });
          setTimeout(() => positionsStore.delete(key), 800);
          throw err;
        }
      }

      try {
        const resp = await api<TapResponse>('/v1/positions', {
          method: 'POST',
          body: {
            client_request_id: clientRequestId,
            asset: cell.asset,
            strike_lo: cell.strike_lo,
            strike_hi: cell.strike_hi,
            t_open_ms: cell.t_open_ms,
            t_close_ms: cell.t_close_ms,
            stake_points: stake,
            client_multiplier: clientMultiplier,
            oracle_seq_at_tap: snap.tick.seq,
            oracle_run_id_at_tap: snap.tick.run_id,
          },
        });
        positionsStore.update(key, {
          state: mapStatus(resp.status),
          positionId: resp.position_id,
          multiplierAtTap: resp.multiplier_at_tap,
        });
        telemetry.locked(resp.position_id, resp.multiplier_at_tap);
        qc.invalidateQueries({ queryKey: ['me'] });
        return resp;
      } catch (err) {
        const reason = err instanceof ApiError ? err.code : 'network';
        if (err instanceof ApiError && reason === 'drift_exceeded') {
          telemetry.drift(err.serverMultiplier ?? Number.NaN, clientMultiplier);
        } else {
          telemetry.reject(key, reason);
        }
        const msg = errToast(err, reason);
        toast.warn(msg);
        positionsStore.update(key, { state: 'REJECTED', rejectReason: reason });
        setTimeout(() => positionsStore.delete(key), 800);
        throw err;
      }
    },
  });
}
