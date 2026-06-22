import { describe, expect, test } from 'bun:test';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import {
  type ProofBlob,
  extractProof,
  tamperDoctorEvidence,
  tamperFlipOutcome,
  tamperInflateMultiplier,
  verifyProof,
  verifyProofDetailed,
} from '../verify-proof';

// Real settlement proofs, fetched from Walrus and confirmed `Valid` by the Rust
// `proof-verify` binary. The TS verifier MUST agree with the Rust one on the
// exact same blobs — that parity is the whole point: the in-browser replay is
// only trustworthy if it reproduces the server-side verdict bit-for-bit.
function loadProof(name: string): ProofBlob {
  return JSON.parse(readFileSync(join(import.meta.dir, 'fixtures', name), 'utf8')) as ProofBlob;
}

describe('verifyProof — parity with the Rust verifier on real Walrus blobs', () => {
  test('a real WON proof replays to Valid', () => {
    expect(verifyProof(loadProof('proof_won.json')).kind).toBe('Valid');
  });

  test('a real LOST proof replays to Valid', () => {
    expect(verifyProof(loadProof('proof_lost.json')).kind).toBe('Valid');
  });

  test('tampering with the locked multiplier is caught', () => {
    const blob = loadProof('proof_won.json');
    blob.multiplier_bps += 500; // claim a fatter payout than the math supports
    expect(verifyProof(blob).kind).toBe('MultiplierMismatch');
  });

  test('a WON claim with no band touch in the evidence is an outcome mismatch', () => {
    const blob = loadProof('proof_won.json');
    // Drag every tick far below the band so the path never enters it.
    const below = blob.band.lo / 1e9 - 1000;
    blob.settlement.evidence_ticks = blob.settlement.evidence_ticks.map((t) => ({
      ...t,
      mid: below,
    }));
    expect(verifyProof(blob).kind).toBe('OutcomeMismatch');
  });

  test('empty evidence is insufficient, not Valid', () => {
    const blob = loadProof('proof_won.json');
    blob.settlement.evidence_ticks = [];
    expect(verifyProof(blob).kind).toBe('InsufficientEvidence');
  });
});

// The drawer needs more than a pass/fail verdict — it shows each claimed value
// next to what the browser independently recomputed. `verifyProofDetailed` exposes
// that breakdown; the verdict (`result`) must still agree with `verifyProof`.
describe('verifyProofDetailed — claimed vs recomputed breakdown', () => {
  test('a real LOST proof: every row matches and the verdict is Valid', () => {
    const d = verifyProofDetailed(loadProof('proof_lost.json'));
    expect(d.result.kind).toBe('Valid');
    expect(d.multiplier.ok).toBe(true);
    expect(d.multiplier.recomputedBps).toBe(d.multiplier.claimedBps);
    expect(d.outcome).toMatchObject({ claimed: 'LOST', recomputed: 'LOST', ok: true });
    expect(d.touch).toMatchObject({ claimedSeq: null, recomputedSeq: null, ok: true });
  });

  test('a real WON proof: recomputes WON at the same first-touch seq', () => {
    const d = verifyProofDetailed(loadProof('proof_won.json'));
    expect(d.result.kind).toBe('Valid');
    expect(d.outcome).toMatchObject({ claimed: 'WON', recomputed: 'WON', ok: true });
    expect(d.touch.recomputedSeq).toBe(d.touch.claimedSeq);
    expect(d.touch.ok).toBe(true);
  });

  test('an inflated multiplier fails only the multiplier row, exposing both bps', () => {
    const d = verifyProofDetailed(tamperInflateMultiplier(loadProof('proof_lost.json')));
    expect(d.result.kind).toBe('MultiplierMismatch');
    expect(d.multiplier.ok).toBe(false);
    expect(d.multiplier.recomputedBps).not.toBe(d.multiplier.claimedBps);
    expect(d.outcome.ok).toBe(true);
  });
});

// The tamper demo's whole point: a viewer mutates the proof and watches the SAME
// verifier reject it. Each helper must break a valid proof in a distinct way and
// must never mutate the caller's blob (the "original" half of the comparison).
describe('tamper helpers — break a valid proof without touching the original', () => {
  test('inflating the multiplier is caught as a MultiplierMismatch', () => {
    expect(verifyProof(tamperInflateMultiplier(loadProof('proof_lost.json'))).kind).toBe(
      'MultiplierMismatch',
    );
    expect(verifyProof(tamperInflateMultiplier(loadProof('proof_won.json'))).kind).toBe(
      'MultiplierMismatch',
    );
  });

  test('claiming the opposite outcome on a LOST proof is an OutcomeMismatch', () => {
    expect(verifyProof(tamperFlipOutcome(loadProof('proof_lost.json'))).kind).toBe(
      'OutcomeMismatch',
    );
  });

  test('fabricating a band touch on a LOST proof is an OutcomeMismatch', () => {
    expect(verifyProof(tamperDoctorEvidence(loadProof('proof_lost.json'))).kind).toBe(
      'OutcomeMismatch',
    );
  });

  test('erasing the touch on a WON proof is an OutcomeMismatch', () => {
    expect(verifyProof(tamperDoctorEvidence(loadProof('proof_won.json'))).kind).toBe(
      'OutcomeMismatch',
    );
  });

  test('tampering returns a copy and leaves the original blob byte-identical', () => {
    const blob = loadProof('proof_lost.json');
    const snapshot = JSON.stringify(blob);
    tamperInflateMultiplier(blob);
    tamperFlipOutcome(blob);
    tamperDoctorEvidence(blob);
    expect(JSON.stringify(blob)).toBe(snapshot);
  });
});

// Proofs are now batched: a flush bundles many settlements into one Walrus blob
// and each history item indexes its own entry. Extracting an entry must yield a
// proof byte-identical to a legacy single-proof blob, so the parity above holds.
describe('extractProof — batched Walrus blobs', () => {
  test('the indexed entry of a batch verifies like a standalone proof', () => {
    const won = loadProof('proof_won.json');
    const lost = loadProof('proof_lost.json');
    const batch = { v: 1, proofs: [lost, won] };
    expect(verifyProof(extractProof(batch, 1)).kind).toBe('Valid');
    expect(verifyProof(extractProof(batch, 0)).kind).toBe('Valid');
  });

  test('an out-of-range index throws instead of returning a bad proof', () => {
    const batch = { v: 1, proofs: [loadProof('proof_won.json')] };
    expect(() => extractProof(batch, 5)).toThrow();
  });
});
