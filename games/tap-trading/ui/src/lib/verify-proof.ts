// Client-side, trustless replay of a Walrus settlement proof — the in-browser
// twin of the Rust `tap-trading-proof-verifier` (ADR-0011 §5). It re-derives the
// locked multiplier with the SAME parity-tested pricing engine the server used
// (src/pricing, 378 tests vs the Rust crate) and re-runs first-touch over the
// evidence path with the SAME segment predicate `touch.rs` settles on — so a
// proof only reads Valid if the math and the outcome both reproduce. No network,
// no server: the caller hands in the blob bytes (fetched straight from a Walrus
// aggregator) and we check them locally. That's what makes "provably fair" real
// rather than a badge.

import { InvalidSigma, InvalidSpot } from '@/pricing/errors';
import { HuiConvergenceFailure } from '@/pricing/hui';
import { computeMultiplier } from '@/pricing/multiplier';
import { type Asset, type Cell, DEFAULT_PRICING_CONFIG } from '@/pricing/types';

/** On-chain/blob prices are USD × 1e9, matching `Position.strike_lo/hi`. */
const ORACLE_PRICE_SCALE = 1e9;
/** Multiplier-equality slack (mirrors proof-types BPS_EPSILON): the float→bps
 *  floor matches exactly, the ±1 covers a cross-platform integer-bps boundary. */
const BPS_EPSILON = 1;

export type ProofOutcome = 'WON' | 'LOST' | 'VOID';

export interface EvidenceTick {
  seq: number;
  ts_ms: number;
  mid: number;
}

export interface ProofBlob {
  v: number;
  position_id: string;
  vault_id: string;
  owner: string;
  asset: string;
  band: { lo: number; hi: number };
  window: { t_open_ms: number; t_close_ms: number };
  stake: number;
  multiplier_bps: number;
  quote_at_tap: {
    oracle_run_id: number;
    oracle_seq: number;
    tap_ms: number;
    mid: number;
    vol_annualized: number;
    formula_version: string;
    floor_curve: string;
  };
  settlement: {
    outcome: ProofOutcome;
    touch_seq: number | null;
    touch_mid: number | null;
    evidence_ticks: EvidenceTick[];
    settled_at_ms: number;
    sui_tx_digest: string;
  };
}

/** A Walrus blob now bundles many settlements: one flush writes a batch and each
 *  history settlement carries its `proof_index` into `proofs`. The inner
 *  `ProofBlob` shape is unchanged, so an extracted entry verifies exactly as a
 *  legacy single-proof blob did (see `extractProof`). */
export interface BatchProofBlob {
  v: number;
  proofs: ProofBlob[];
}

export type VerifyResult =
  | { kind: 'Valid' }
  | { kind: 'MultiplierMismatch'; claimedBps: number; recomputedBps: number }
  | { kind: 'OutcomeMismatch'; claimed: ProofOutcome; recomputed: ProofOutcome }
  | { kind: 'InsufficientEvidence'; reason: string };

/** Per-field "claimed vs recomputed" view backing the drawer's comparison panel.
 *  Every field is reported regardless of the overall verdict, so a tampered proof
 *  shows exactly which row broke. `recomputedBps`/`recomputed` are null only when
 *  the inputs make recomputation impossible (invalid pricing inputs). */
export interface VerifyBreakdown {
  result: VerifyResult;
  multiplier: { claimedBps: number; recomputedBps: number | null; ok: boolean };
  outcome: { claimed: ProofOutcome; recomputed: ProofOutcome | null; ok: boolean };
  touch: { claimedSeq: number | null; recomputedSeq: number | null; ok: boolean };
}

/** Pull one settlement out of a batch blob. Throws on an out-of-range index
 *  rather than returning a bad proof — the caller surfaces it as a fetch error. */
export function extractProof(batch: BatchProofBlob, index: number): ProofBlob {
  const proof = batch.proofs[index];
  if (!proof) {
    throw new Error(`proof index ${index} out of range (batch has ${batch.proofs.length} proofs)`);
  }
  return proof;
}

/** Canonical float→bps: **floor** (proof-types `multiplier_f64_to_bps`). */
function multiplierToBps(m: number): number {
  return Math.floor(m * 10_000);
}

function assetFromStr(s: string): Asset {
  // The pricing math is asset-agnostic; unknown labels fall back to BTC (matches
  // the Rust verifier) so a typo can't change the recomputed multiplier.
  return s === 'ETH' ? 'ETH' : s === 'SUI' ? 'SUI' : 'BTC';
}

/** Does the segment [prev, cur] cross the half-open band [lo, hi)? First tick has
 *  no predecessor → point-sampled. Identical to `touch.rs::path_touches_band`. */
function segmentTouchesBand(prev: number | null, cur: number, lo: number, hi: number): boolean {
  const segLo = prev === null ? cur : Math.min(prev, cur);
  const segHi = prev === null ? cur : Math.max(prev, cur);
  return segHi >= lo && segLo < hi;
}

function detectTouch(
  ticks: EvidenceTick[],
  tOpen: number,
  tClose: number,
  lo: number,
  hi: number,
): number | null {
  let prev: number | null = null;
  for (const t of ticks) {
    if (t.ts_ms > tClose) break;
    if (t.ts_ms >= tOpen && segmentTouchesBand(prev, t.mid, lo, hi)) return t.seq;
    prev = t.mid;
  }
  return null;
}

/** Recompute the locked multiplier (in bps) from the tap-time quote with the same
 *  pricing engine the server used. Returns null when the quote's inputs are
 *  invalid — recomputation is impossible, not merely a mismatch. */
function recomputeBps(blob: ProofBlob): number | null {
  const asset = assetFromStr(blob.asset);
  const cell: Cell = {
    asset,
    strike_lo: blob.band.lo / ORACLE_PRICE_SCALE,
    strike_hi: blob.band.hi / ORACLE_PRICE_SCALE,
    t_open_ms: blob.window.t_open_ms,
    t_close_ms: blob.window.t_close_ms,
  };
  try {
    const m = computeMultiplier(
      cell,
      {
        asset,
        spot: blob.quote_at_tap.mid,
        sigma_annualized: blob.quote_at_tap.vol_annualized,
        timestamp_ms: blob.quote_at_tap.tap_ms,
      },
      DEFAULT_PRICING_CONFIG,
      blob.quote_at_tap.tap_ms,
    );
    return multiplierToBps(m);
  } catch (e) {
    if (e instanceof InvalidSpot || e instanceof InvalidSigma || e instanceof HuiConvergenceFailure)
      return null;
    throw e;
  }
}

/** The overall verdict, computed from the already-recomputed fields. Check order
 *  is load-bearing: structural evidence gates first, then multiplier, then touch —
 *  so a proof that can't be replayed at all never reports a content mismatch. */
function deriveVerdict(
  blob: ProofBlob,
  recomputedBps: number | null,
  multiplierOk: boolean,
  touched: boolean,
): VerifyResult {
  const ticks = blob.settlement.evidence_ticks;
  const first = ticks[0];
  if (!first) return { kind: 'InsufficientEvidence', reason: 'no evidence ticks' };
  if (first.ts_ms > blob.window.t_open_ms)
    return { kind: 'InsufficientEvidence', reason: 'evidence misses the window head' };
  if (recomputedBps === null)
    return { kind: 'MultiplierMismatch', claimedBps: blob.multiplier_bps, recomputedBps: 0 };
  if (!multiplierOk)
    return { kind: 'MultiplierMismatch', claimedBps: blob.multiplier_bps, recomputedBps };

  switch (blob.settlement.outcome) {
    case 'WON':
      return touched
        ? { kind: 'Valid' }
        : { kind: 'OutcomeMismatch', claimed: 'WON', recomputed: 'LOST' };
    case 'LOST': {
      // A no-touch claim must be backed by evidence through t_close.
      const last = ticks[ticks.length - 1];
      if (!last || last.ts_ms < blob.window.t_close_ms)
        return { kind: 'InsufficientEvidence', reason: 'evidence stops before window close' };
      return touched
        ? { kind: 'OutcomeMismatch', claimed: 'LOST', recomputed: 'WON' }
        : { kind: 'Valid' };
    }
    default:
      // VOID is an oracle-gap refund; the gap isn't in the tick path, so v1
      // accepts a VOID proof's structure (documented limitation, matches Rust).
      return { kind: 'Valid' };
  }
}

/** Replay a proof blob and report each claimed value next to what we recomputed,
 *  plus the overall verdict. The drawer renders this directly; `verifyProof` is
 *  the verdict-only view. */
export function verifyProofDetailed(blob: ProofBlob): VerifyBreakdown {
  const lo = blob.band.lo / ORACLE_PRICE_SCALE;
  const hi = blob.band.hi / ORACLE_PRICE_SCALE;

  const claimedBps = blob.multiplier_bps;
  const recomputedBps = recomputeBps(blob);
  const multiplierOk =
    recomputedBps !== null && Math.abs(recomputedBps - claimedBps) <= BPS_EPSILON;

  const recomputedSeq = detectTouch(
    blob.settlement.evidence_ticks,
    blob.window.t_open_ms,
    blob.window.t_close_ms,
    lo,
    hi,
  );
  const touched = recomputedSeq !== null;

  const claimedOutcome = blob.settlement.outcome;
  // The evidence only decides WON/LOST; a VOID claim is an oracle-gap refund the
  // tick path can't speak to, so we accept its own label rather than contradict it.
  const recomputedOutcome: ProofOutcome =
    claimedOutcome === 'VOID' ? 'VOID' : touched ? 'WON' : 'LOST';

  const claimedSeq = blob.settlement.touch_seq;

  return {
    result: deriveVerdict(blob, recomputedBps, multiplierOk, touched),
    multiplier: { claimedBps, recomputedBps, ok: multiplierOk },
    outcome: {
      claimed: claimedOutcome,
      recomputed: recomputedOutcome,
      ok: recomputedOutcome === claimedOutcome,
    },
    // ok mirrors the verdict's touched-vs-claim logic, not strict seq equality:
    // a valid proof's claimed touch_seq and our recomputed seq coincide, but the
    // verdict only ever hinges on *whether* the band was touched.
    touch: { claimedSeq, recomputedSeq, ok: (claimedSeq !== null) === touched },
  };
}

/** Replay a proof blob and report whether it reproduces its own claim. */
export function verifyProof(blob: ProofBlob): VerifyResult {
  return verifyProofDetailed(blob).result;
}

// ===== Tamper demo =====
// Each helper returns a deep copy with ONE lie introduced, leaving the caller's
// blob untouched (it's the "original" the UI compares against). The point is to
// let a skeptic mutate the proof and watch the same verifier reject it — proof
// that the verdict is recomputed from evidence, not rubber-stamped.

/** +0.5000× over what the math supports → MultiplierMismatch on replay. */
const TAMPER_MULTIPLIER_BUMP_BPS = 5000;

export function tamperInflateMultiplier(blob: ProofBlob): ProofBlob {
  const tampered = structuredClone(blob);
  tampered.multiplier_bps += TAMPER_MULTIPLIER_BUMP_BPS;
  return tampered;
}

/** Claim the opposite result while leaving the evidence intact → OutcomeMismatch
 *  (or InsufficientEvidence if a WON window's evidence stops at the touch). */
export function tamperFlipOutcome(blob: ProofBlob): ProofBlob {
  const tampered = structuredClone(blob);
  tampered.settlement.outcome = blob.settlement.outcome === 'WON' ? 'LOST' : 'WON';
  return tampered;
}

/** Doctor the price history so the recomputed outcome flips: a LOST proof gets a
 *  fabricated in-band tick; a WON proof has its touches dragged outside the band.
 *  Either way the replayed outcome contradicts the (unchanged) claim. */
export function tamperDoctorEvidence(blob: ProofBlob): ProofBlob {
  const tampered = structuredClone(blob);
  const lo = blob.band.lo / ORACLE_PRICE_SCALE;
  const hi = blob.band.hi / ORACLE_PRICE_SCALE;
  const ticks = tampered.settlement.evidence_ticks;

  if (blob.settlement.outcome === 'WON') {
    // Erase the touch: push every tick a full band-width below the band.
    const below = lo - (hi - lo) - 1;
    tampered.settlement.evidence_ticks = ticks.map((t) => ({ ...t, mid: below }));
  } else {
    // Fabricate a touch: plant an in-band price on the first in-window tick.
    const inBand = (lo + hi) / 2;
    const i = ticks.findIndex(
      (t) => t.ts_ms >= blob.window.t_open_ms && t.ts_ms <= blob.window.t_close_ms,
    );
    const target = i >= 0 ? i : Math.floor(ticks.length / 2);
    if (ticks[target]) ticks[target] = { ...ticks[target], mid: inBand };
  }
  return tampered;
}
