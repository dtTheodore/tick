# Tick — Math Spec

**Status:** v0.1
**Owner:** Pricing Engine
**Audience:** Engineers building `backend/pricing-engine/` (canonical Rust) and `packages/pricing-engine-ts/` (client port), plus anyone auditing multiplier correctness
**Companions:** `PRD.md` §13, `ORACLE_SPEC.md`

> **Why this spec exists:** the multiplier per cell is the single number that determines whether the game feels honest or rigged. Get the math right and Tick has an integrity moat over Hamster Kombat-class games. Get it wrong and even points users will smell it. This doc is the canonical reference for the formula, its derivation, its inputs, its calibration, and the edge cases that will break it in production.

---

## 0. Locked Decisions

Anchors from frame-by-frame review of Pacifica SWIM, Euphoria, and BC.Game tap-trading reference products. Do not re-litigate without new evidence.

> **v2 economics revision (2026-06):** the v1 model leaked. Measured on real ETH ticks, near/in-band cells paid RTP **110–112%** (player-favorable) while OTM cells paid **6–43%** (predatory) — bimodal and unfair to both sides. Root causes: (a) the τ-growing incentive floor paid in-band cells far above fair value, and (b) `P_touch` was priced over `[now, t_close]` and returned 1.0 whenever *current* spot was in-band, ignoring that settlement is over the *future* window `[t_open, t_close]`. v2 fixes both — see the revised items below and §4. The changes here are the shipped state; older prose in §2.2/§4.1 describing BGK and the incentive floor is retained for history but marked superseded.

- **Settlement model**: first-touch over `[t_open, t_close]`, where `t_open` is the *clock-aligned column start in the future*, **not** "now". Cells aren't active until their column opens. **v2: settlement is on the continuous price *path*** — the straight segment between consecutive oracle ticks — not on discrete tick samples. A fast wick that crosses a narrow band entirely between two ticks is a touch (empirically one inter-tick jump can exceed the band step). This makes settlement agree with the chart line the player watches and removes the discrete-monitoring under-count, so the BGK band-widening correction (§2.2) is **no longer applied**.
- **Cell duration**: 5 s.
- **Columns visible**: 4 future columns on mobile (20 s look-ahead), 6 on desktop (30 s).
- **Strike grid**: globally anchored (everyone sees the same `…3812.0 / 3812.5 / 3813.0…` ladder), clock-aligned columns (every 5 s on the wall clock).
- **In-band cells are tappable.** The UI only offers future columns (`t_open > now`), but the server does not *enforce* `t_open > now` (spec §1 states it as an invariant; enforcement is future hardening). It's safe: window-aware pricing prices an already-open in-band cell at ~1.0× (τ_open=0 → p≈1 → no +EV), so the in-play column is no longer the v1 floor exploit.
- **Multiplier display refresh**: 10 Hz client-side (every 100 ms), continuously re-computed against latest oracle tick.
- **Lock-at-tap**: the multiplier shown at tap time is the multiplier paid at settle, regardless of how the cell's displayed multiplier drifts before settle.
- **Independent positions per tap**: no stake aggregation; each tap is its own DB row.
- **House margin** v2 default: **0.03** (RTP 97%; uniform across every cell ⇒ EV = −0.03 regardless of which cell is tapped — the fairness guarantee). Was 0.10 in v1.
- **Floor** v2 default: **flat `1.0×`** (`floor_a = 1.0, floor_b = 0.0`) — a tap never returns less than the stake. The v1 τ-growing incentive floor (`a = 1.50, b = 0.025`) was the over-payment leak; with window-aware pricing (§4) future near cells are genuinely uncertain (`P_touch < 1`), so their fair `(1 − margin)/P_touch` is naturally `> 1` without a floor.
- **jump_buffer** v2 default: **1.0** (no σ inflation; BGK removed). Retune only as σ-conservatism in shadow mode (§6).

---

## 1. The pricing problem, formally

A cell `c` is a vertical-range one-touch barrier option over a *future* time window:

```
c = {
  asset:    "ETH" | "BTC" | "SOL" | ...
  L:        lower strike  (price units)
  H:        upper strike  (L < H)
  t_open:   start of monitoring window  (clock-aligned, ≥ now())
  t_close:  end of monitoring window     (t_open + 5s in v1)
}
```

**Window semantics.** At tap time `t = now()`, the cell's column has not yet started — `t_open > now()` for every tappable cell. The cell becomes *active* at `t_open` and *expires* at `t_close`. The spot's path from `now()` to `t_open` is unobserved by the cell; only the path during `[t_open, t_close]` determines the outcome.

Outcome at `t_close`:
- **Win** if `spot(t) ∈ [L, H]` for *some* `t ∈ [t_open, t_close]` (first-touch settlement)
- **Lose** otherwise (untouched throughout the monitoring window)

The user pays `stake_points` at tap time and receives `stake_points × multiplier(c)` if `Win`, else `0`.

We compute the multiplier such that, in expectation under our oracle's price process, the house captures a configurable margin:

```
multiplier(c) = max( floor(τ_to_close), (1 − house_margin) / P_touch(c) )
```

- `P_touch(c)` = fair probability the spot is in `[L, H]` at some `t ∈ [t_open, t_close]`, conditional on current spot `S_0` and current volatility estimate `σ̂`. Computed via Hui + BGK; see §2.
- `house_margin = 0.10` in v1 (10%).
- `floor(τ_to_close)` = τ-dependent minimum payout, defined in §4. Reproduces the in-band multiplier behaviour observed in Pacifica SWIM / Euphoria / BC.Game (cells where spot is already in band still pay a non-trivial, τ-growing multiplier).
- **Multiplier is locked at tap time** — once committed to a position, the multiplier never changes. This is the integrity contract with the user, copied from Pacifica's explicit UX guarantee. The cell's *displayed* multiplier continues to update at 10 Hz after the tap; the *locked* multiplier on the position is what settles.

In-band cells (`S_0 ∈ [L, H]`) are **tappable**. They are priced via the floor curve, not disabled. A cell where the math gives `multiplier < floor(τ)` displays `floor(τ)`.

---

## 2. The closed-form formula

### 2.1 Continuous-monitoring no-touch probability (Hui 1996)

The asset follows geometric Brownian motion `dS/S = (r − q) dt + σ dW` under the risk-neutral measure. For Tick, `r = q = 0` (no funding, no dividends over sub-minute windows), so Itô's correction gives log-spot drift `−σ²/2` — this is built into the formula via the `β` coefficient below, not assumed away.

Hui (1996), Equation 11, gives the value of an up-and-down out binary option paying `R` at maturity if `S_t ∈ (H₁, H₂)` for all `t ∈ [0, T]`, given `S_0 ∈ (H₁, H₂)`. With `R = 1` and `r = 0` the value **equals the no-touch probability** (no discount factor); this is the form we use:

```
P_no_touch(S_0, L, H, σ, τ) =
  Σ_{n=1}^{N}  A_n(S_0, L, H) · exp(− E_n(L, H, σ, τ))

where
  Z   = ln(H / L)                                            // band log-width
  k₁  = 2(r − q) / σ²                                        // = 0 when r = q = 0
  α   = −½ · (k₁ − 1)                                        // = ½ when r = q = 0
  β   = −¼ · (k₁ − 1)² − 2r/σ²                               // = −¼ when r = q = 0

  A_n = (2πn / Z²) · [(S_0/L)^α − (−1)^n · (S_0/H)^α]
              / (α² + (nπ/Z)²)
              · sin(nπ · ln(S_0/L) / Z)

  E_n = ½ · ((nπ/Z)² − β) · σ² · τ

  N   = 10  (truncation; series converges in ~5 terms for sub-minute cells)
```

**Domain restriction**: this formula is only valid for `S_0 ∈ (L, H)`. For `S_0` outside the band, see §2.3 — Hui's series does not apply.

Source: Hui, C. H. (1996). "One-Touch Double Barrier Binary Option Values." *Applied Financial Economics* 6:343–346 — variable mapping: Hui's `H₁ ↔ L`, `H₂ ↔ H`, `L ↔ Z`. Reproduced in Haug (2007) *Complete Guide to Option Pricing Formulas* 2nd ed., McGraw-Hill, p. 180.

Probability of touch (for `S_0 ∈ (L, H)` only):
```
P_touch = 1 − P_no_touch
```

### 2.2 The Broadie–Glasserman–Kou (BGK) continuity correction

Our oracle is discretely sampled — ticks arrive every `Δt_tick` ≈ 50 ms (aggregator broadcast rate). Continuous-monitoring formulas systematically **overestimate** the touch probability vs the discretely monitored reality. Without correction, we charge users too much for far-OTM cells and too little for near-spot cells.

Per Broadie, M., Glasserman, P., Kou, S. (1997). "A Continuity Correction for Discrete Barrier Options." *Mathematical Finance* 7(4):325–349. https://www.columbia.edu/~sk75/mfBGK.pdf

```
H_corrected = H · exp(+ β_BGK · σ · √(τ / m))      // widen upper barrier
L_corrected = L · exp(− β_BGK · σ · √(τ / m))      // widen lower barrier

β_BGK = −ζ(½) / √(2π) ≈ 0.5826
m     = number of monitoring ticks in the window = τ / Δt_tick
```

`ζ(½) ≈ −1.4603545` is the Riemann zeta at one-half (Chernoff 1965).

**Algorithm**:
1. Compute `H_corrected`, `L_corrected` from raw band + current vol estimate
2. Plug into Hui formula with `(L_corrected, H_corrected)` instead of `(L, H)`
3. Return `P_touch`

**Sanity check at our parameters**: BTC σ ≈ 80%/yr → `σ_per_sec ≈ 0.000142`. For τ=5s, m=100 (50ms ticks): `σ · √(τ/m) = 0.000142 · 0.224 ≈ 0.0032%`. Shift `≈ 0.0019%`. For a Δ$10 band on $70K BTC (= 0.014% width), this is **14% of the band width** — meaningfully non-zero. Skipping BGK is the #1 reason naïve implementations overpay.

### 2.3 Out-of-band single-barrier first-passage

Hui's formula (§2.1) only handles `S_0 ∈ (L, H)` — it gives the *survival* probability inside a band. For cells where the current spot is outside the band, we need a different primitive: the probability that GBM crosses the **near** edge of the band before the window closes. Once the path touches the near edge, the cell is "touched" by definition; what happens after is irrelevant for first-touch settlement.

For BSM with `r = q = 0`, log-spot has drift `μ_log = −σ²/2`. The exact first-passage probability of arithmetic Brownian motion with drift `μ` and variance `σ²·t` to barrier `±b` (where `b > 0`) is (Karatzas–Shreve 1991, §2.6 and §3.7):

```
Upper barrier (spot below band, B > S_0):
  b = ln(B / S_0) > 0
  v = σ · √τ
  P(reach B by τ) = Φ((μτ − b) / v) + exp(2μb/σ²) · Φ(−(μτ + b) / v)

Lower barrier (spot above band, B < S_0):
  by μ → −μ symmetry
```

Substituting `μ = −σ²/2` gives `μτ = −v²/2` and `exp(2μb/σ²) = exp(−b) = S_0/B`. We use this exact form for completeness; at Tick scales we also accept the simpler reflection-principle approximation:

```
P_touch_approx = 2 · Φ(−b / v)
```

**Approximation error**: the exact and approximate forms agree as `v = σ·√τ → 0`. At Tick parameters (`σ ≈ 80%/yr`, `τ ≤ 60s` → `v ≤ 1.1 × 10⁻³`), the relative error is **≈ 4 × 10⁻⁴** across the full OTM range — well below the QuantLib parity tolerance (1%) and the starter tolerance (10%). The Rust crate uses the simple form for clarity; revisit if parity tolerances tighten below 0.04%.

**Three-regime dispatch** (the orchestration in `compute_p_touch`):

```
if S_0 ∈ (L_corrected, H_corrected):
  P_touch = 1                                    // already in band at t_open
elif S_0 ≤ L_corrected:
  b = ln(L_corrected / S_0)                      // > 0
  P_touch = 2 · Φ(−b / (σ · √τ))                 // first-passage from below
else:                                            // S_0 ≥ H_corrected
  b = ln(S_0 / H_corrected)                      // > 0
  P_touch = 2 · Φ(−b / (σ · √τ))                 // first-passage from above
```

The boundary cases (`S_0 == L_corrected` and `S_0 == H_corrected`) fall into the OTM branches with `b = 0`, giving `2·Φ(0) = 1` — consistent with the in-band branch's "already touched" semantics.

---

## 3. Volatility estimation

The single parameter `σ̂` feeds everything downstream. We compute it from oracle ticks.

### 3.1 v1 estimator: EWMA on 1-second log returns

```
For each asset, every 1 second:
  r_i      = ln( p_i / p_{i-1} )                       // 1-second log return
  σ²_i     = λ · σ²_{i-1}  +  (1 − λ) · r_i²            // exponentially weighted
  σ̂_i_ann  = √(σ²_i) · √(seconds_per_year)             // annualized

with
  λ = 0.94                  // RiskMetrics standard
  seconds_per_year = 31_557_600
```

Cold-start is the caller's responsibility — `estimate_realized_vol` is a pure function that processes whatever log-return history is passed in. The aggregator bootstraps σ̂ before opening the tap path by replaying the last 5 minutes of historical ticks (Hermes `/v2/updates/price/{publish_time}` for Pyth-backed assets); the pricing engine never knows about the oracle source.

### 3.2 Jump buffer (v1)

Crypto returns at sub-minute scale are leptokurtic (fat-tailed) — pure GBM understates touch probability in tail moves. We multiply σ̂ by a constant factor:

```
σ̂_used = σ̂_raw · 1.30
```

The 1.3× is a heuristic for v1. It widens our P_touch estimate, which means **users get smaller multipliers** (closer to fair), and the game pays out slightly less than under naïve GBM. This is a conservative posture — the points-game has no vault to drain, but multipliers should still be calibrated tightly.

### 3.3 v2 upgrades (post-launch, with data)

- **Garman-Klass estimator** using Pyth EMA + window high/low: reduces noise vs simple EWMA, but requires Pyth Benchmark integration.
- **Jump-diffusion calibration** (Kou 2002 double-exponential): fit jump parameters from 30+ days of our own oracle path data; replaces the 1.30 buffer with a model-driven correction.
- **Two-scale realized vol** (Zhang-Mykland-Aït-Sahalia 2005): noise-robust at high-frequency sampling. Use if 100ms ticks are added.

### 3.4 Failure modes & defenses

| Failure | Symptom | Defense |
|---|---|---|
| Oracle gap (>2s no tick) | σ̂ becomes meaningless | Pause new taps; existing positions continue; aggregator surfaces "oracle stalling" status |
| Vol regime shift (CPI print, exchange hack) | σ̂ lags real vol for ~minutes | Detect via `|r_i| > 5·σ̂_{i-1}` spike; bump σ̂ immediately to `max(σ̂, |r_i|·√seconds_per_year)`; log incident. Implemented as the pure helper `jump_adjusted_sigma` (`vol.rs`); aggregator wires it as `EWMA → jump-adjust → broadcast` per oracle tick, using the boolean return to drive the audit log |
| Slow vol decay after big move | Multipliers stay too high too long | EWMA λ=0.94 has ~half-life of ~10 seconds — acceptable; tune if needed |
| Initialization error on cold-start | Wrong multipliers for first 5 min | Block tap-mint until σ̂ confidence ≥ threshold (server-set boolean) |

---

## 4. Multiplier calculation

The end-to-end pipeline for one cell, evaluated at the current oracle tick. Re-runs at 10 Hz on the client (via the TS port — see §5) so the displayed multiplier tracks `τ_to_close` as it shrinks. The same pipeline runs server-side via the Rust crate for the drift check at `POST /positions`.

**v2 (shipped).** `P_touch` is the probability the *continuous* path enters `[L, H)` during the **future** window `[t_open, t_close]`. Because the path from `now` to `t_open` is unobserved, the spot at `t_open` is a random variable `S(t_open) = S₀·exp(σ_o·Z − σ_o²/2)`, `σ_o = σ_sec·√τ_open` (driftless martingale). We integrate the continuous touch probability over that distribution. This is what makes a future center cell genuinely uncertain (`P_touch < 1`): by `t_open` the spot may have drifted out of the band.

```
function computeMultiplier(cell, oracleState, config) -> number:
  S_0     = oracleState.spot
  σ̂      = oracleState.sigma_annualized
  τ_close = max(0, cell.t_close - now())              // s until expiry
  if τ_close <= 0: return 0                            // closed; not tappable
  t_open_eff = max(cell.t_open, now())
  τ_open  = (t_open_eff - now()) / 1000                // s until the window opens
  τ_win   = (cell.t_close - t_open_eff) / 1000         // monitored window length
  σ_sec   = (σ̂ / √(seconds_per_year)) * jump_buffer    // jump_buffer = 1.0 in v2

  // touch_from(s): continuous one-touch of [L,H) over τ_win from start s.
  //   in-band → 1; else first-passage to the NEAR edge only (width irrelevant
  //   under first-touch): 2·Φ(−|ln(barrier/s)| / (σ_sec·√τ_win)).
  // No BGK shift — settlement is on the continuous path (§0), not discrete ticks.

  if τ_open <= 0:
    P_touch = touch_from(S_0)                           // window already open
  else:
    σ_o = σ_sec · √τ_open
    // E_Z[ touch_from(S(t_open)) ] over a ±6σ midpoint grid, 64 steps,
    // BYTE-IDENTICAL in the Rust and TS ports (the 1/√(2π) cancels in acc/wsum).
    P_touch = Σ_k w_k·touch_from(S_0·exp(σ_o·z_k − σ_o²/2)) / Σ_k w_k

  raw = P_touch < epsilon ? MAX_MULTIPLIER : (1 − house_margin) / P_touch
  return clamp(raw, floor=1.0, MAX_MULTIPLIER)          // flat 1.0× minimum
```

For `τ_open = 0` (the parity-fixture case and any already-open cell) the integral collapses to a single `touch_from(S₀)`, so the closed-form first-passage and the integral agree exactly. The integral only runs for real future cells.

**Why this is fair to both sides:** with no incentive floor, `multiplier = (1 − house_margin)/P_touch`, so `EV = P_touch · multiplier − 1 = −house_margin` on **every** cell — a uniform 3% edge, independent of which cell the player taps. (Realized RTP per cell is only as accurate as the σ̂ estimate; see §6 — OTM cells are σ-sensitive and calibrated in shadow mode.)

### 4.1 The floor (v2: flat) — *v1 incentive curve superseded*

v2 floor is a flat `1.0×` (`floor_a = 1.0, floor_b = 0`): a tap never returns less than the stake. Fair value comes entirely from `(1 − margin)/P_touch`, which window-aware pricing keeps `> 1` for tappable near cells.

> **Superseded (v1):** v1 used `floor(τ) = 1.50 + 0.025·τ`, set deliberately *above* the Pacifica reference row as a "near-cell incentive" funded by the OTM tail (ADR-0012). Measurement showed this paid in-band cells **RTP 110–112%** (player-favorable) — the over-payment leak. It compensated for v1 pricing returning `P_touch = 1` for in-band-*now* cells; v2's window-aware `P_touch < 1` removes the need for any incentive floor. The old `floor_curve_runs_above_pacifica…` test was removed.

### 4.2 Constants

| Constant | v2 value | Rationale |
|---|---|---|
| `house_margin` | **0.03** | 3% edge (RTP 97%); uniform EV = −margin per cell. Industry band 1–4%. Was 0.10 in v1. |
| `jump_buffer` | **1.0** | No σ inflation (BGK removed; continuous-path settlement prices at fair σ). >1 would bias the house silently. |
| `tick_period_seconds` | 0.05 | 50 ms aggregator cadence (now unused in pricing; BGK removed). |
| `display_refresh_ms` | 100 | Client recomputes per cell every 100 ms (10 Hz). |
| `MAX_MULTIPLIER` | 1000 | Display cap on far-OTM cells. |
| `epsilon` | 1e-9 | Numerical floor on `P_touch` for cap detection. |
| `floor_a, floor_b` | **1.0, 0.0** | Flat 1.0× minimum (no τ-growing incentive floor; §4.1). |
| `Z_LO, Z_HI, Z_STEPS` | −6, 6, 64 | Fixed midpoint grid for the `t_open` spot integral; identical in both ports. |

> `β_BGK` (0.5826) and `apply_bgk_correction` remain in the codebase (and `bgk` keeps its tests) but are **no longer called** by the multiplier — settlement is continuous-path (§0), so the discrete-monitoring correction is unnecessary.

### 4.3 The locked-vs-displayed invariant

Once a tap is committed, the cell continues to render its live computed multiplier at 10 Hz for visualization purposes, but the *locked* multiplier on the position never changes. At settle time, the worker uses `position.multiplier_at_tap`, **never the cell's currently-displayed multiplier**. This is the integrity guarantee.

A test must enforce: for any open position, `position.multiplier_at_tap` is immutable after `POST /positions` succeeds; the settlement worker reads only `position.multiplier_at_tap` × `position.stake_points` for win payouts.

### 4.4 Float→bps conversion for the on-chain vault

`compute_multiplier` returns an `f64`. The on-chain `tick_vault::Position`
stores the locked multiplier as `u64 multiplier_bps` (basis points;
`10000 = 1.00x`) because Move has no floats (ADR-0010 §4). The conversion
is **floor**, defined canonically as:

    multiplier_bps = floor(multiplier_f64 × 10_000)

Implemented once as `tap_trading_proof_verifier::multiplier_f64_to_bps`
(plan `2026-05-27-tick-walrus-proofs.md` Task 4). Both the API's USDC-mode
mint path (which writes `multiplier_bps` on-chain) and the proof verifier
(which recomputes it for replay) MUST use this exact function. Flooring,
not rounding: the player is never charged for a fractional bps they didn't
receive, and the verifier's equality check becomes exact (tolerance
`BPS_EPSILON = 1` covers the rare integer-bps boundary across f64
platforms). Points mode is unaffected — it keeps the `f64` multiplier in
`positions.multiplier_at_tap` and never converts to bps.

---

## 5. Reference implementation plan

**Canonical implementation** — Rust crate `backend/pricing-engine/`. Source of truth for the math; used by `backend/api` for the drift check at `POST /positions` and by `backend/oracle-aggregator` for its broadcast `vol_annualized` and any server-side multiplier work.

**Client implementation** — thin TS port at `packages/pricing-engine-ts/`. Used by the Next.js frontend to recompute each visible cell at 10 Hz against the latest oracle tick (matching Pacifica / Euphoria's client-compute UX, where the multiplier display is responsive and no per-cell broadcast is required). The TS port is intentionally minimal (~150 LOC) and is **not** the source of truth — drift is policed by running both implementations against the same QuantLib parity fixtures in CI (§6.1). If the TS port diverges from the Rust crate on any fixture, CI fails.

Both implementations consume the same `test/fixtures/quantlib.json` (committed). Adding a new constant or changing the algorithm requires updating both crates and regenerating the fixture in the same PR.

### 5.1 Public API

```rust
pub struct Cell {
    pub asset: AssetSymbol,
    pub strike_lo: f64,
    pub strike_hi: f64,
    pub t_open_ms: u64,
    pub t_close_ms: u64,
}

pub struct OracleState {
    pub asset: AssetSymbol,
    pub spot: f64,
    pub sigma_annualized: f64,
    pub timestamp_ms: u64,
}

pub struct PricingConfig {
    pub house_margin: f64,        // default 0.10
    pub jump_buffer: f64,         // default 1.30
    pub tick_period_seconds: f64, // default 0.05
    pub floor_a: f64,             // default 1.50 (intercept)
    pub floor_b: f64,             // default 0.025 (slope per second of τ)
    pub multiplier_cap: f64,      // default 1000.0
}

pub enum PricingError {
    InvalidSpot(f64),         // negative, zero, or NaN
    InvalidSigma(f64),        // negative or NaN
    InvalidLambda(f64),       // outside [0, 1)
    InvalidLogReturn(f64),    // NaN or non-finite
    InvalidTerms,             // hui_no_touch with terms == 0
    InvalidBand { l: f64, h: f64 },
    InsufficientHistory,      // estimate_realized_vol with empty slice
    HuiConvergenceFailure { last_term_mag: f64, terms: u32 },
}

// Returns Err for bad oracle input (spec §7.1: caller must pause taps).
// Returns Ok(0.0) for a cell already past t_close (not tappable, not an error).
pub fn compute_multiplier(
    cell: &Cell, oracle: &OracleState, cfg: &PricingConfig, now_ms: u64,
) -> Result<f64, PricingError>;

pub fn compute_p_touch(
    cell: &Cell, oracle: &OracleState, cfg: &PricingConfig, now_ms: u64,
) -> Result<f64, PricingError>;

// Err on empty slice, λ ∉ [0, 1), or NaN/non-finite log returns.
pub fn estimate_realized_vol(log_returns: &[f64], lambda: f64) -> Result<f64, PricingError>;

// Spec §3.4 spike absorption. Pure function; caller owns the EWMA state.
// Returns (adjusted_sigma, was_spike) — bool drives the audit log.
pub fn jump_adjusted_sigma(
    raw_sigma_annualized: f64,
    last_log_return: f64,
    prev_sigma_annualized: f64,
) -> (f64, bool);

pub fn apply_bgk_correction(
    l: f64,
    h: f64,
    sigma_per_sec: f64,
    tau_sec: f64,
    m: f64,           // monitoring count in window (= tau_sec / tick_period_seconds)
) -> (f64, f64); // (L_corrected, H_corrected)

// Err for terms == 0, degenerate band, or convergence failure within `terms`.
pub fn hui_no_touch(
    s0: f64,
    l: f64,
    h: f64,
    sigma_per_sec: f64,
    tau_sec: f64,
    terms: u32,
) -> Result<f64, PricingError>;
```

The TS port mirrors this surface 1:1 (snake_case → camelCase, `f64` → `number`):

```typescript
// Rust's Result<T, E> maps to a discriminated union in TS. Functions throw a
// typed PricingError subclass on the same conditions the Rust Err variants
// fire, so the port preserves fail-loud semantics from spec §7.1.
export class PricingError extends Error {
  constructor(public kind:
    | "InvalidSpot" | "InvalidSigma" | "InvalidLambda" | "InvalidLogReturn"
    | "InvalidTerms" | "InvalidBand" | "InsufficientHistory"
    | "HuiConvergenceFailure",
    public detail?: unknown) { super(`${kind}: ${JSON.stringify(detail)}`); }
}

export function computeMultiplier(cell: Cell, oracle: OracleState, cfg: PricingConfig | undefined, nowMs: number): number;
export function computePTouch(cell: Cell, oracle: OracleState, cfg: PricingConfig | undefined, nowMs: number): number;
export function estimateRealizedVol(logReturns: readonly number[], lambda?: number): number;
export function jumpAdjustedSigma(rawSigmaAnnualized: number, lastLogReturn: number, prevSigmaAnnualized: number): { adjustedSigma: number; wasSpike: boolean };
export function applyBGKCorrection(L: number, H: number, sigmaPerSec: number, tauSec: number, m: number): { LCorrected: number; HCorrected: number };
export function huiNoTouch(S0: number, L: number, H: number, sigmaPerSec: number, tauSec: number, terms?: number): number;
```

### 5.2 Internals

```
backend/pricing-engine/                  ← canonical (Rust)
├── Cargo.toml
├── src/
│   ├── lib.rs                 // public re-exports
│   ├── multiplier.rs          // compute_multiplier, public entry
│   ├── hui.rs                 // hui_no_touch series, ≤80 lines
│   ├── bgk.rs                 // apply_bgk_correction
│   ├── vol.rs                 // estimate_realized_vol EWMA
│   ├── constants.rs           // BETA_BGK, SECONDS_PER_YEAR, defaults
│   └── types.rs               // Cell, OracleState, PricingConfig
└── tests/
    ├── hui_quantlib_parity.rs // verified against test/fixtures/quantlib.json
    ├── bgk_published.rs       // verified against BGK 1997 Table 2
    ├── multiplier_e2e.rs      // end-to-end cells with known expected multipliers
    └── invariants.rs          // property-based via proptest: floor, cap, monotonicity

packages/pricing-engine-ts/             ← thin client port (TypeScript)
├── package.json
├── src/
│   ├── multiplier.ts, hui.ts, bgk.ts, vol.ts, constants.ts, types.ts, index.ts
└── test/
    ├── hui.parity.test.ts     // consumes the SAME fixtures/quantlib.json
    ├── bgk.published.test.ts
    └── invariants.property.test.ts  // fast-check
```

Stacks: Rust crate uses stable Rust + `proptest` + `serde_json`; TS port uses TypeScript 5.5+ + vitest + fast-check. Neither needs external numerics libs — `f64::exp`/`sin`/`ln` and `Math.exp`/`sin`/`log` suffice.

### 5.3 Open-source posture

Phase 2 (Aug–Sep 2026): publish `backend/pricing-engine/` on crates.io and `packages/pricing-engine-ts/` as `@tap-trading/pricing-engine` on npm, both MIT-licensed, sharing the same fixtures. Part of the integrity story.

---

## 6. Calibration runbook

The math is correct in principle — but we need to verify it produces multipliers that feel right at scale before shipping. Three calibration steps, in order:

### 6.1 Step 1 — Verify Hui implementation vs QuantLib (one-time)

```
Pre-flight check (engineering, week 1):

For 100 randomized (S_0, L, H, σ, τ) tuples covering Tick's production envelope:
  - τ ∈ [5, 60] seconds
  - σ ∈ [0.30, 2.00] annualized (steady-state ~0.80, regime-shift spike ~2.00)
  - Band widths 0.01%–0.05% of spot (matches BTC Δ$0.5 and ETH Δ$0.5 grids)
  - S_0 centred in the band (in-band parity for Hui's domain; OTM cells
    use the single-barrier first-passage branch in §2.3, parity-checked
    separately via the `otm_approximation_matches_exact_within_1e_3`
    unit test)

Compute P_no_touch via:
  (a) Our Rust implementation (canonical)
  (b) QuantLib's AnalyticDoubleBarrierBinaryEngine (Python)

Acceptance: |P_rust - P_QuantLib| / P_QuantLib < 0.01  (1% relative error),
            OR |P_rust - P_QuantLib| < 1e-3 absolute (handles the P→0 tail
            where relative error blows up).
```

Run as `backend/pricing-engine/tests/quantlib_parity.rs` and (once the port lands) `packages/pricing-engine-ts/test/hui.parity.test.ts` — both must pass against the same fixture file. If either diverges, debug before shipping.

Generator: `games/tap-trading/scripts/gen-quantlib-fixtures.py` (deterministic via `random.seed(20260523)`). Fixture set is committed at `backend/pricing-engine/tests/fixtures/quantlib.json` and is regenerated only when algorithmic constants change.

### 6.2 Step 2 — Calibrate jump buffer against historical data

```
Backtest (engineering, week 2):

1. Download 30 days of 1-second BTC ticks (Binance public archive).
2. Define a grid of test cells: every 30s, generate cells at strike offsets
   {±0.1%, ±0.25%, ±0.5%, ±1%} from spot, window=5s (cell duration).
3. For each cell, compute:
   - P_touch_predicted = computePTouch(cell, oracle_at_t_open)
   - hit_actual = did spot enter cell during window? (0 or 1)
4. Bucket by predicted P_touch in deciles (0–10%, 10–20%, ...).
5. For each bucket, compute observed_hit_rate = mean(hit_actual).
6. Compare predicted vs observed; tune jump_buffer until observed ≤ predicted
   in every bucket (slightly conservative).

Acceptance: in every decile, observed_hit_rate / predicted_midpoint ≤ 1.0.
The buffer value that achieves this is the v1 jump_buffer constant.
```

If 1.30 is too tight, raise it. If too loose, lower it. Document the chosen value in `constants.ts` with a comment linking to the backtest.

### 6.3 Step 3 — Tune house margin against UX targets

```
Pre-launch shadow test (week 3):

1. Run the pricing engine with jump_buffer calibrated from step 6.2.
2. Generate simulated user sessions:
   - Each user: 100 taps, mix of strategies (near-spot, far-OTM, random)
   - 1000 users total
3. For each candidate house_margin in {0.05, 0.08, 0.10, 0.12, 0.15}:
   compute distribution of (user net points over 100 taps)
4. Pick the margin where:
   - Median user net points: slightly negative (~ −5 to −10% of total wagered)
   - 30th percentile: positive (winners exist)
   - 99th percentile: very positive (jackpots exist)
   - Variance: high (game feels exciting)

Acceptance: house_margin chosen subjectively from UX target; document choice.
```

In v1, default to 0.10 unless step 6.3 shows a different value works better.

### 6.4 Ongoing recalibration

Post-launch, run step 6.2 weekly on the last 7 days of our own oracle data. If observed hit rate exceeds predicted in any decile by >5%, ship an updated `jump_buffer` constant. Audit log records when the constant changes.

---

## 7. Edge cases & defensive coding

### 7.1 Numerical issues

| Case | Issue | Fix |
|---|---|---|
| Z = ln(H/L) **large** (wide band, e.g. >5% of spot) | Hui series converges slowly — for narrow Z the damping `exp(−(nπ/Z)²·σ²τ/2)` kicks in fast (each `(nπ/Z)²` is large), but for wide Z the damping is weak and 10 terms is insufficient | Adaptive truncation: track last-term magnitude; bail with `HuiConvergenceFailure` if it exceeds tolerance (1e-4) within the requested `terms`. Spec §7.1 caps N at 20; callers can raise `terms` to that limit. Tick production bands (0.01-0.05% of spot) are nowhere near this regime |
| `σ · √τ` → 0 | P_no_touch → 1, P_touch → 0 | Apply MAX_MULTIPLIER cap; render cell as "100×+" badge |
| `σ · √τ` very large | P_no_touch → 0 | All cells touch with probability ~1; multiplier ≈ 1.05 floor |
| S_0 exactly at L or H | Numerical instability in sin term | Implementation sidesteps by early-returning `Ok(0.0)` for `s0 ≤ l ∨ s0 ≥ h` before entering the series; documented in `hui.rs` next to the guard. The nudge `S_0 = S_0 · (1 + 1e-10)` is the alternative for implementations that inline the boundary into the series body |
| Negative, zero, or NaN spot | Bad oracle data | `compute_p_touch` returns `Err(PricingError::InvalidSpot)`; caller must pause taps |
| NaN or non-finite σ̂ | Bad vol estimate | `compute_p_touch` returns `Err(PricingError::InvalidSigma)`; caller must pause taps |

### 7.2 Behavioral edge cases

| Case | Behavior |
|---|---|
| User taps with τ < 100ms remaining | Reject tap (locked window per PRD MVP-08) |
| Oracle gap during user's open position | Position remains open; if window expires without ANY oracle tick covered, void as "no data" and refund the stake |
| Vol estimate stale (>5s old) | Multipliers shown but tap button disabled with "stabilizing prices" message |
| Multiple cells in same column with different L/H | Each priced independently; no correlation adjustment needed (independent bets on same underlying path) |

---

## 8. Verification checklist before launch

| Check | Owner | Status |
|---|---|---|
| Hui implementation matches QuantLib to <1% on 100 test cases | Eng | ☐ |
| BGK correction matches BGK 1997 example values | Eng | ☐ |
| EWMA initialization correct on cold-start | Eng | ☐ |
| jump_buffer calibrated from 30 days of historical data | Eng | ☐ |
| house_margin chosen via shadow test | Eng + Product | ☐ |
| τ-dependent floor curve calibrated against in-band feel target | Eng + Product | ☐ |
| Multiplier floor and cap (1000×) enforced in all paths | Eng | ☐ |
| Lock-at-tap invariant: settlement reads only `position.multiplier_at_tap` | Eng | ☐ |
| Client display refresh at 10 Hz; no flicker; no jitter on tabular nums | Frontend | ☐ |
| Math explainer page live + linked from main UI footer | Product | ☐ |
| Open-source publication plan documented | Product | ☐ |

---

## 9. References

- Hui, C. H. (1996). "One-Touch Double Barrier Binary Option Values." *Applied Financial Economics* 6:343–346.
- Haug, E. (2007). *Complete Guide to Option Pricing Formulas*, 2nd ed., McGraw-Hill. Chapter 4, pp. 152–180.
- Broadie, M., Glasserman, P., Kou, S. (1997). "A Continuity Correction for Discrete Barrier Options." *Mathematical Finance* 7(4):325–349. https://www.columbia.edu/~sk75/mfBGK.pdf
- Reiner, E., Rubinstein, M. (1991). "Breaking Down the Barriers" / "Unscrambling the Binary Code." *Risk Magazine*.
- Kou, S. G. (2002). "A jump-diffusion model for option pricing." *Management Science* 48(8):1086–1101.
- RiskMetrics Technical Document (1996). J.P. Morgan / Reuters. EWMA volatility estimator, λ=0.94 default.
- QuantLib `AnalyticDoubleBarrierBinaryEngine` — reference implementation: https://github.com/lballabio/QuantLib/blob/master/ql/experimental/barrieroption/analyticdoublebarrierbinaryengine.cpp
- Reflection principle, Brownian bridge: https://almostsuremath.com/2023/04/18/the-maximum-of-brownian-motion-and-the-reflection-principle/
- BGK discrete-barrier blog: https://almostsuremath.com/2023/07/01/discrete-barrier-approximations/

