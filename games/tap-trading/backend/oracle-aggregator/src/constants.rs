//! Compile-time constants. Sourced from `ORACLE_SPEC` and ADR-0008 unless noted.

// Constants are introduced here for future tasks; not all are consumed by each commit.
#![allow(dead_code)]

use tap_trading_oracle_types::AssetSymbol;

/// Assets the aggregator supports end-to-end (sources, aggregation, healthz).
/// Single source of truth — `healthz` and `parse_asset` both read this so a
/// new asset is one edit. ORACLE_SPEC §2.
pub const SUPPORTED_ASSETS: [AssetSymbol; 3] =
    [AssetSymbol::Btc, AssetSymbol::Eth, AssetSymbol::Sui];

/// Aggregator emit cadence. ORACLE_SPEC §4.1 / §5.6.
pub const EMIT_PERIOD_MS: u64 = 50;

/// How fresh an asset's newest tick must be for `GET /healthz` to count it
/// healthy. Coupled to `DEGRADED_HYSTERESIS_MS` on purpose: readiness flips to
/// 503 within one emit period of the wire stream emitting `Status::Degraded`,
/// so probes and subscribers agree on when an asset went stale. ADR-0008 §5.
pub const HEALTHZ_FRESHNESS_MS: i64 = DEGRADED_HYSTERESIS_MS as i64;

/// Ring-buffer depth per asset. 120 s at 20 Hz (≈100 KB/asset). ADR-0011 §6
/// extended this from the original 500 ms / 10-tick replay window so the
/// settlement worker can pull a full `[t_open, t_close]` evidence-tick span for
/// each Walrus proof blob via `GET /ring/:asset/range`. The `(run_id, seq)`
/// replay semantics (ADR-0008 §5) are unchanged — only retention grew.
pub const RING_SIZE: usize = 2400;

/// Cap on the 1-s log-return deque per asset (10 min). MATH_SPEC §3.1 history.
pub const RETURN_DEQUE_CAP: usize = 600;

/// Largest elapsed time that still counts as a single 1-second return sample.
/// `next_vol` only runs on emitted ticks, so a DEGRADED/silence gap freezes the
/// sample clock; on recovery the elapsed span can be many seconds. Recording
/// `ln(mid/prev_mid)` over such a gap as one "1-second" return would inject a
/// jumbo outlier into the EWMA — instead we re-baseline and resume sampling.
pub const MAX_SAMPLE_GAP_MS: i64 = 2_000;

/// Below this many returns, the aggregator emits the cold-start vol default.
/// ADR-0008 §7.
pub const COLD_START_RETURN_THRESHOLD: usize = 30;

/// Cold-start `vol_annualized` value. ADR-0008 §7.
pub const COLD_START_VOL_ANNUALIZED: f64 = 0.60;

/// Plan decision: how long `source_count < 2` must persist before emitting
/// `Status::Degraded` (and how long `>= 2` must persist before clearing).
/// 2 s = 40 consecutive 50 ms ticks. See plan deviation notes.
pub const DEGRADED_HYSTERESIS_MS: u64 = 2_000;

/// WS heartbeat cadence. ADR-0008 §5 / ORACLE_SPEC §5.5.
pub const HEARTBEAT_PERIOD_S: u64 = 5;

/// Source freshness window. Deviates from ORACLE_SPEC §4.4 step 2 (1 s) on
/// purpose: BBO feeds (bookTicker / bbo-tbt) push on *BBO change*, not on a
/// timer, so on a calm asset the top of book sits unchanged and a venue legitimately
/// goes silent for ~2 s (measured) — Pyth's SSE gaps ~1.2 s. A 1 s window
/// false-drops these live venues, churning the active set; since Pyth trades
/// ~$0.7 below the USDT-quoted CEX mids, a CEX false-drop yanks the median to the
/// {CEX, low-Pyth} mean and the chart teleports. 5 s clears the measured gaps with
/// margin; a truly dead venue is still caught by divergence-reject and the 30 s
/// read-idle reconnect. Staleness now bites only when calm, where an old price is
/// still accurate.
pub const SOURCE_FRESHNESS_MS: u64 = 5_000;

/// Initial reconnect backoff. ORACLE_SPEC §4.2 (exp 100 ms → 30 s).
pub const BACKOFF_INITIAL_MS: u64 = 100;

/// Reconnect backoff ceiling. The jittered sleep is clamped to this too, so
/// +10% jitter on a maxed backoff cannot sleep past the documented ceiling.
pub const BACKOFF_MAX_MS: u64 = 30_000;

/// A source connection that delivers no inbound frame for this long is treated
/// as dead and force-reconnected. Without it, a half-open TCP connection
/// (silent network blackhole, no FIN/RST) parks the read loop forever and the
/// feed dies invisibly. Kept above `WS_KEEPALIVE_PERIOD_S` so a live-but-quiet
/// connection — whose keepalive pongs count as inbound frames — never trips it.
pub const SOURCE_READ_IDLE_TIMEOUT_MS: u64 = 30_000;

/// Client-initiated WS keepalive cadence for OKX/Bybit, whose servers drop idle
/// sockets after ~30 s. We ping well under that so a quiet feed stays connected.
pub const WS_KEEPALIVE_PERIOD_S: u64 = 15;

/// Pyth confidence-interval rejection threshold (basis points). ORACLE_SPEC §6.
pub const PYTH_CONF_REJECT_BPS: u32 = 100;

/// Reject a source whose price diverges from the active-set median by more than
/// this (basis points = 5%). With >= 3 sources the median already ignores one
/// outlier; this guards the 2-source case where the "median" is the mean.
/// ORACLE_SPEC §7.
pub const SOURCE_DIVERGENCE_REJECT_BPS: u32 = 500;

/// EWMA λ for vol on 1-s returns. MATH_SPEC §3.1 (RiskMetrics standard).
pub const EWMA_LAMBDA_VOL: f64 = 0.94;

/// EMA α for median-price smoothing. ORACLE_SPEC §4.4 step 6.
///
/// Distinct from EWMA_LAMBDA_VOL — this α is on the *price* path
/// (responsiveness), λ above is on the *vol* path (statistical
/// smoothing). Conflating them collapses the two-state design.
pub const EMA_ALPHA_PRICE: f64 = 0.6;
