import type { HistoryItem } from '@/hooks/useMe';
import { cn } from '@/lib/utils';
import {
  type BatchProofBlob,
  type ProofBlob,
  type VerifyResult,
  extractProof,
  tamperDoctorEvidence,
  tamperFlipOutcome,
  tamperInflateMultiplier,
  verifyProofDetailed,
} from '@/lib/verify-proof';
import { useEffect, useMemo, useState } from 'react';
import { ProofEvidenceSparkline } from './ProofEvidenceSparkline';

// Public testnet Walrus aggregator (HTTP read). Overridable for other networks.
// The browser fetches the blob straight from Walrus — not via our server — so the
// retrieval is as trustless as the replay.
const WALRUS_AGGREGATOR =
  (import.meta.env.VITE_WALRUS_AGGREGATOR as string | undefined) ??
  'https://aggregator.walrus-testnet.walrus.space';
const SUI_NETWORK = (import.meta.env.VITE_SUI_NETWORK as string | undefined) ?? 'testnet';
// Walruscan — the Walrus block explorer. Shows the blob's certification, size and
// storage epochs on-chain; the raw-bytes link is the guaranteed-resolving fallback.
const WALRUS_EXPLORER_BLOB = `https://walruscan.com/${SUI_NETWORK}/blob`;
const SUI_EXPLORER_TX = `https://suiscan.xyz/${SUI_NETWORK}/tx`;

const ORACLE_PRICE_SCALE = 1e9;
type TamperKind = 'none' | 'multiplier' | 'outcome' | 'evidence';

interface LoadedProof {
  blob: ProofBlob;
  bytes: number;
  sha256: string;
  batchSize: number;
  proofIndex: number;
}
type FetchState =
  | { phase: 'loading' }
  | { phase: 'error'; message: string }
  | { phase: 'loaded'; data: LoadedProof };

function short(s: string, head = 8, tail = 4): string {
  return s.length > head + tail + 1 ? `${s.slice(0, head)}…${s.slice(-tail)}` : s;
}

function fmtMultBps(bps: number | null): string {
  return bps === null ? '—' : `${(bps / 10_000).toFixed(4)}×`;
}

function fmtSeq(seq: number | null): string {
  return seq === null ? 'none' : `seq ${seq}`;
}

function fmtBytes(n: number): string {
  return n < 1024 ? `${n} B` : `${(n / 1024).toFixed(1)} KB`;
}

async function sha256Hex(buf: ArrayBuffer): Promise<string> {
  const digest = await crypto.subtle.digest('SHA-256', buf);
  return Array.from(new Uint8Array(digest))
    .map((b) => b.toString(16).padStart(2, '0'))
    .join('');
}

/** Pull this tap's proof from the fetched JSON and report the batch size. A blob
 *  is a `BatchProofBlob` ({ v, proofs }) bundling many settlements; older blobs
 *  were a bare `ProofBlob`. Discriminate on `proofs`, then index by `proof_index`. */
function selectProof(json: unknown, proofIndex: number): { blob: ProofBlob; batchSize: number } {
  const batch = json as Partial<BatchProofBlob>;
  if (Array.isArray(batch.proofs)) {
    return {
      blob: extractProof(batch as BatchProofBlob, proofIndex),
      batchSize: batch.proofs.length,
    };
  }
  return { blob: json as ProofBlob, batchSize: 1 };
}

export function VerifyDrawer({ item, onClose }: { item: HistoryItem | null; onClose: () => void }) {
  const open = item !== null;
  const blobId = item?.walrus_blob_id ?? null;
  // Many history items share ONE blob and differ only by index, so the index is a
  // dependency: reopening on a different tap in the same blob must re-extract.
  const proofIndex = item?.proof_index ?? 0;
  const [state, setState] = useState<FetchState>({ phase: 'loading' });
  const [verified, setVerified] = useState(false);
  const [tamper, setTamper] = useState<TamperKind>('none');

  useEffect(() => {
    if (!open || !blobId) return;
    let cancelled = false;
    setState({ phase: 'loading' });
    setVerified(false);
    setTamper('none');
    (async () => {
      try {
        const res = await fetch(`${WALRUS_AGGREGATOR}/v1/blobs/${blobId}`);
        if (!res.ok) throw new Error(`Walrus returned ${res.status}`);
        // Read raw bytes so we can fingerprint the exact payload (sha-256) and
        // report its size — then parse the same bytes.
        const buf = await res.arrayBuffer();
        const sha256 = await sha256Hex(buf);
        const json = JSON.parse(new TextDecoder().decode(buf));
        const { blob, batchSize } = selectProof(json, proofIndex);
        if (!cancelled)
          setState({
            phase: 'loaded',
            data: { blob, bytes: buf.byteLength, sha256, batchSize, proofIndex },
          });
      } catch (e) {
        if (!cancelled)
          setState({ phase: 'error', message: e instanceof Error ? e.message : 'fetch failed' });
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [open, blobId, proofIndex]);

  const loaded = state.phase === 'loaded' ? state.data : null;
  const original = loaded?.blob ?? null;

  // The active proof is the original, or a tampered copy when the skeptic edits it.
  // Everything downstream (comparison, sparkline, verdict) recomputes from this, so
  // a tamper visibly breaks the whole panel.
  const activeProof = useMemo(() => {
    if (!original) return null;
    switch (tamper) {
      case 'multiplier':
        return tamperInflateMultiplier(original);
      case 'outcome':
        return tamperFlipOutcome(original);
      case 'evidence':
        return tamperDoctorEvidence(original);
      default:
        return original;
    }
  }, [original, tamper]);

  const breakdown = useMemo(
    () => (activeProof ? verifyProofDetailed(activeProof) : null),
    [activeProof],
  );

  return (
    <>
      <button
        type="button"
        aria-label="Close proof"
        tabIndex={open ? 0 : -1}
        className={cn(
          'fixed inset-0 z-40 cursor-default bg-black/60 backdrop-blur-[2px] transition-opacity duration-300',
          open ? 'opacity-100' : 'pointer-events-none opacity-0',
        )}
        onClick={onClose}
      />
      <aside
        className={cn(
          'fixed right-0 top-0 z-50 flex h-full w-full max-w-[460px] flex-col overflow-y-auto border-l border-white/10 bg-[#0b0b0e] shadow-[-24px_0_60px_rgba(0,0,0,0.6)] transition-transform duration-300 ease-out',
          open ? 'translate-x-0' : 'translate-x-full',
        )}
      >
        <div className="flex items-center justify-between border-b border-white/10 px-5 py-4">
          <div className="font-mono text-sm uppercase tracking-[0.2em] text-white/60">
            Settlement proof
          </div>
          <button
            type="button"
            onClick={onClose}
            className="text-white/40 transition-colors hover:text-white"
            aria-label="Close proof"
          >
            ✕
          </button>
        </div>

        <div className="flex flex-1 flex-col px-5 py-5">
          {state.phase === 'loading' && (
            <div className="flex flex-1 items-center justify-center font-mono text-sm text-white/40">
              Fetching proof from Walrus…
            </div>
          )}

          {state.phase === 'error' && (
            <div className="space-y-3">
              <div className="font-mono text-sm text-tick-loss">
                Couldn’t fetch the proof from Walrus ({state.message}).
              </div>
              <div className="font-mono text-[12px] leading-relaxed text-white/45">
                The proof still exists on Walrus. Verify it yourself from a terminal:
                <pre className="mt-2 overflow-x-auto rounded-md border border-white/10 bg-black/40 p-2 text-[11px] text-white/70">
                  walrus read {blobId}
                </pre>
              </div>
            </div>
          )}

          {loaded && original && breakdown && (
            <>
              {/* identity */}
              <div className="mb-1 flex items-center gap-2.5">
                <OutcomeBadge outcome={original.settlement.outcome} />
                <span className="font-mono text-2xl font-semibold tabular-nums text-white">
                  {fmtMultBps(original.multiplier_bps)}
                </span>
              </div>
              <div className="mb-5 font-mono text-[11px] text-white/40">
                {original.asset} · band {(original.band.lo / ORACLE_PRICE_SCALE).toFixed(2)}–
                {(original.band.hi / ORACLE_PRICE_SCALE).toFixed(2)} ·{' '}
                {(original.window.t_close_ms - original.window.t_open_ms) / 1000}s window
              </div>

              {/* ① the blob's journey */}
              <SectionLabel n="1" title="The blob's journey" />
              <ol className="mb-6 mt-3">
                <JourneyStep label="settled off-chain" value="payout credited instantly" />
                <JourneyStep
                  label="proof assembled"
                  value={`${original.settlement.evidence_ticks.length} oracle ticks`}
                />
                <JourneyStep
                  label="batched"
                  value={`this tap = #${loaded.proofIndex + 1} of ${loaded.batchSize}`}
                />
                <JourneyStep
                  label="stored on Walrus"
                  value={`${fmtBytes(loaded.bytes)} · content-addressed`}
                  href={blobId ? `${WALRUS_EXPLORER_BLOB}/${blobId}` : undefined}
                  hrefLabel="Walruscan"
                />
                <JourneyStep
                  label="fetched by your browser"
                  value="public aggregator — not our server"
                  href={blobId ? `${WALRUS_AGGREGATOR}/v1/blobs/${blobId}` : undefined}
                  hrefLabel="raw"
                />
                <JourneyStep label="replayed & compared" value="↓ below" active={verified} last />
              </ol>

              {/* ② replay → reveals the comparison, evidence and verdict */}
              {!verified ? (
                <button
                  type="button"
                  onClick={() => setVerified(true)}
                  className="rounded-[10px] bg-tick-pink py-3.5 font-mono text-sm font-bold uppercase tracking-wider text-white shadow-[0_0_24px_rgba(255,45,126,0.35)] transition-all hover:scale-[1.02] active:scale-95"
                >
                  ▶ Replay proof in your browser
                </button>
              ) : (
                <>
                  <SectionLabel n="2" title="What your browser recomputed" />
                  <div className="mb-2 mt-3 rounded-[10px] border border-white/8 bg-white/[0.02] px-4 py-2">
                    <div className="flex items-baseline justify-between py-1.5 font-mono text-[10px] uppercase tracking-wider text-white/30">
                      <span>field</span>
                      <span className="flex gap-6">
                        <span className="w-24 text-right">claimed</span>
                        <span className="w-24 text-right">recomputed</span>
                      </span>
                    </div>
                    <CompareRow
                      label="multiplier"
                      claimed={fmtMultBps(breakdown.multiplier.claimedBps)}
                      recomputed={fmtMultBps(breakdown.multiplier.recomputedBps)}
                      ok={breakdown.multiplier.ok}
                    />
                    <CompareRow
                      label="outcome"
                      claimed={breakdown.outcome.claimed}
                      recomputed={breakdown.outcome.recomputed ?? '—'}
                      ok={breakdown.outcome.ok}
                    />
                    <CompareRow
                      label="first touch"
                      claimed={fmtSeq(breakdown.touch.claimedSeq)}
                      recomputed={fmtSeq(breakdown.touch.recomputedSeq)}
                      ok={breakdown.touch.ok}
                    />
                  </div>
                  <OutcomeCaption blob={activeProof ?? original} breakdown={breakdown} />

                  {/* ③ evidence */}
                  <div className="mb-1 mt-5">
                    <SectionLabel n="3" title="Evidence" />
                  </div>
                  <div className="mt-3">
                    <ProofEvidenceSparkline
                      ticks={(activeProof ?? original).settlement.evidence_ticks}
                      bandLoScaled={original.band.lo}
                      bandHiScaled={original.band.hi}
                      touchSeq={breakdown.touch.recomputedSeq}
                    />
                    <div className="mt-1.5 flex items-center justify-between font-mono text-[10px] text-white/30">
                      <span>
                        price path · {(activeProof ?? original).settlement.evidence_ticks.length}{' '}
                        ticks
                      </span>
                      <span className="flex items-center gap-1.5">
                        <span
                          className="inline-block h-2 w-3 rounded-sm"
                          style={{ background: 'var(--color-tick-info)', opacity: 0.4 }}
                        />
                        strike band
                      </span>
                    </div>
                  </div>

                  {/* verdict */}
                  <Verdict result={breakdown.result} tampered={tamper !== 'none'} />

                  {/* ⑤ tamper to break it */}
                  <div className="mt-6">
                    <SectionLabel n="4" title="Don't trust it? Break it." />
                    <p className="mt-2 font-mono text-[11px] leading-relaxed text-white/40">
                      This is your copy, in your browser — the real blob on Walrus is immutable (its
                      id is its content hash). Edit the proof and the same verifier rejects it:
                    </p>
                    <div className="mt-3 flex flex-wrap gap-2">
                      <TamperButton
                        active={tamper === 'outcome'}
                        onClick={() => setTamper('outcome')}
                      >
                        {original.settlement.outcome === 'WON' ? 'claim I lost' : 'claim I won'}
                      </TamperButton>
                      <TamperButton
                        active={tamper === 'multiplier'}
                        onClick={() => setTamper('multiplier')}
                      >
                        inflate ×
                      </TamperButton>
                      <TamperButton
                        active={tamper === 'evidence'}
                        onClick={() => setTamper('evidence')}
                      >
                        {original.settlement.outcome === 'WON' ? 'erase the touch' : 'fake a touch'}
                      </TamperButton>
                      {tamper !== 'none' && (
                        <button
                          type="button"
                          onClick={() => setTamper('none')}
                          className="rounded-md border border-white/15 px-3 py-1.5 font-mono text-[11px] text-white/60 transition-colors hover:border-white/40 hover:text-white"
                        >
                          ↺ reset to original
                        </button>
                      )}
                    </div>
                  </div>
                </>
              )}

              {/* ④ integrity */}
              <div className="mt-6">
                <SectionLabel n="5" title="Integrity" />
                <div className="mt-3 rounded-[10px] border border-white/8 bg-white/[0.02] px-4 py-2">
                  <IntegrityRow label="blob id">
                    {blobId ? (
                      <span className="flex items-center justify-end gap-2">
                        <a
                          href={`${WALRUS_EXPLORER_BLOB}/${blobId}`}
                          target="_blank"
                          rel="noreferrer"
                          className="text-tick-info hover:underline"
                          title="View on Walruscan"
                        >
                          {short(blobId)} ↗
                        </a>
                        <CopyButton text={blobId} />
                      </span>
                    ) : (
                      '—'
                    )}
                  </IntegrityRow>
                  <IntegrityRow label="bytes">{loaded.bytes.toLocaleString()}</IntegrityRow>
                  <IntegrityRow label="sha-256">
                    <span className="flex items-center justify-end gap-2">
                      {short(loaded.sha256, 8, 4)}
                      <CopyButton text={loaded.sha256} />
                    </span>
                  </IntegrityRow>
                  <IntegrityRow label="verify yourself">
                    {blobId ? (
                      <span className="flex items-center justify-end gap-2">
                        <code className="text-white/60">walrus read {short(blobId, 6, 3)}</code>
                        <CopyButton text={`walrus read ${blobId}`} />
                      </span>
                    ) : (
                      '—'
                    )}
                  </IntegrityRow>
                  {original.settlement.sui_tx_digest && (
                    <IntegrityRow label="tx">
                      <a
                        href={`${SUI_EXPLORER_TX}/${original.settlement.sui_tx_digest}`}
                        target="_blank"
                        rel="noreferrer"
                        className="text-tick-info hover:underline"
                      >
                        {short(original.settlement.sui_tx_digest)} ↗
                      </a>
                    </IntegrityRow>
                  )}
                </div>
              </div>

              <div className="mt-auto pt-6 font-mono text-[10px] leading-relaxed text-white/30">
                Fetched straight from Walrus and replayed locally with the same pricing engine the
                server settled on — the result is recomputed in your browser, not taken on our word.
              </div>
            </>
          )}
        </div>
      </aside>
    </>
  );
}

function SectionLabel({ n, title }: { n: string; title: string }) {
  return (
    <div className="flex items-center gap-2 border-b border-white/8 pb-2 font-mono text-[11px] uppercase tracking-[0.18em] text-white/45">
      <span className="flex h-4 w-4 items-center justify-center rounded-full border border-white/20 text-[9px] text-white/50">
        {n}
      </span>
      {title}
    </div>
  );
}

function JourneyStep({
  label,
  value,
  href,
  hrefLabel,
  active,
  last,
}: {
  label: string;
  value: string;
  href?: string;
  hrefLabel?: string;
  active?: boolean;
  last?: boolean;
}) {
  return (
    <li className="relative flex gap-3 pl-1">
      <div className="flex flex-col items-center">
        <span
          className={cn(
            'mt-1.5 h-1.5 w-1.5 shrink-0 rounded-full',
            active ? 'bg-tick-pink shadow-[0_0_6px_rgba(255,45,126,0.8)]' : 'bg-white/30',
          )}
        />
        {!last && <span className="my-0.5 w-px flex-1 bg-white/10" />}
      </div>
      <div
        className={cn('flex flex-1 items-baseline justify-between gap-3', last ? 'pb-0' : 'pb-3')}
      >
        <span className="font-mono text-[12px] text-white/70">{label}</span>
        <span className="text-right font-mono text-[11px] tabular-nums text-white/40">
          {value}
          {href && (
            <>
              {' '}
              <a
                href={href}
                target="_blank"
                rel="noreferrer"
                className="text-tick-info hover:underline"
              >
                {hrefLabel} ↗
              </a>
            </>
          )}
        </span>
      </div>
    </li>
  );
}

function CompareRow({
  label,
  claimed,
  recomputed,
  ok,
}: {
  label: string;
  claimed: string;
  recomputed: string;
  ok: boolean;
}) {
  return (
    <div className="flex items-baseline justify-between border-t border-white/5 py-2 font-mono text-[12px]">
      <span className="uppercase tracking-wider text-white/40">{label}</span>
      <span className="flex items-center gap-6 tabular-nums">
        <span className="w-24 text-right text-white/55">{claimed}</span>
        <span className={cn('w-24 text-right', ok ? 'text-white/90' : 'text-tick-loss')}>
          {recomputed} {ok ? '✓' : '✕'}
        </span>
      </span>
    </div>
  );
}

function OutcomeCaption({
  blob,
  breakdown,
}: { blob: ProofBlob; breakdown: ReturnType<typeof verifyProofDetailed> }) {
  const n = blob.settlement.evidence_ticks.length;
  let text: string;
  let bad = false;
  if (!breakdown.outcome.ok) {
    bad = true;
    text = `replay says ${breakdown.outcome.recomputed}, but the proof claims ${breakdown.outcome.claimed} — the evidence no longer backs the claim`;
  } else if (breakdown.outcome.recomputed === 'WON') {
    text = `price entered the band at ${fmtSeq(breakdown.touch.recomputedSeq)}, across ${n} ticks`;
  } else {
    text = `price stayed outside the band across all ${n} ticks`;
  }
  return (
    <div
      className={cn(
        'mb-1 font-mono text-[11px] leading-relaxed',
        bad ? 'text-tick-loss/90' : 'text-white/40',
      )}
    >
      → {text}
    </div>
  );
}

function TamperButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        'rounded-md border px-3 py-1.5 font-mono text-[11px] transition-all',
        active
          ? 'border-tick-loss bg-tick-loss/15 text-tick-loss'
          : 'border-white/15 text-white/65 hover:border-tick-loss/60 hover:text-white',
      )}
    >
      {children}
    </button>
  );
}

function CopyButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      type="button"
      aria-label="Copy"
      onClick={() => {
        navigator.clipboard?.writeText(text).then(() => {
          setCopied(true);
          setTimeout(() => setCopied(false), 1200);
        });
      }}
      className="text-white/30 transition-colors hover:text-white/70"
    >
      {copied ? '✓' : '⧉'}
    </button>
  );
}

function IntegrityRow({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex items-baseline justify-between gap-4 border-t border-white/5 py-2 first:border-t-0">
      <span className="shrink-0 font-mono text-[11px] uppercase tracking-wider text-white/35">
        {label}
      </span>
      <span className="text-right font-mono text-[12px] tabular-nums text-white/85">
        {children}
      </span>
    </div>
  );
}

function OutcomeBadge({ outcome }: { outcome: ProofBlob['settlement']['outcome'] }) {
  const map = {
    WON: 'border-tick-win/50 bg-tick-win/15 text-tick-win',
    LOST: 'border-tick-loss/50 bg-tick-loss/15 text-tick-loss',
    VOID: 'border-white/30 bg-white/10 text-white/60',
  } as const;
  return (
    <span
      className={cn(
        'rounded-md border px-2 py-0.5 font-mono text-[11px] font-semibold uppercase tracking-wider',
        map[outcome],
      )}
    >
      {outcome}
    </span>
  );
}

function Verdict({ result, tampered }: { result: VerifyResult; tampered: boolean }) {
  const ok = result.kind === 'Valid';
  const text =
    result.kind === 'Valid'
      ? tampered
        ? 'Valid — but you reset it; this is the untampered proof'
        : 'Valid — recomputed result matches the proof'
      : result.kind === 'MultiplierMismatch'
        ? `Multiplier mismatch — proof claims ${fmtMultBps(result.claimedBps)}, math gives ${fmtMultBps(result.recomputedBps)}`
        : result.kind === 'OutcomeMismatch'
          ? `Outcome mismatch — proof claims ${result.claimed}, replay says ${result.recomputed}`
          : `Insufficient evidence — ${result.reason}`;
  return (
    <div
      className={cn(
        'mt-4 flex items-center gap-2.5 rounded-[10px] border px-4 py-3.5 font-mono text-[13px] transition-colors',
        ok
          ? 'border-tick-win/50 bg-tick-win/10 text-tick-win'
          : 'border-tick-loss/50 bg-tick-loss/10 text-tick-loss',
      )}
    >
      <span className="text-lg">{ok ? '✓' : '✕'}</span>
      <span>{text}</span>
    </div>
  );
}
