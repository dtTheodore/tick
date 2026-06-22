# Tick chart: honest near-miss rendering + 10s window

Date: 2026-06-11
Status: proposed (awaiting review)

## 1. Problem

User report: *"it doesn't even settle when the UI clearly reached that cell"*, plus
the price line still slopes/clamps and a floating price pill collides with a held
bet tile (Image #1). Reference look: Images #2/#3 — auto-ranged line that fills
the frame, a glowing leading dot (no text pill), faint grid.

## 2. Confirmed diagnosis (driven from the real app, not static reading)

Investigated against the running dev stack: real Binance/Bybit/OKX/Pyth feed,
real settlement worker, real Postgres.

- **Clock skew ruled out.** Measured δ = `client Date.now() − oracle ts_ms` =
  **1–4 ms** (max 47) over the live feed. The win cue judges its window on
  `Date.now()`; the server on `tick.ts_ms`. They share an identical segment
  predicate on identical mids, so a negligible δ means they agree — the cue
  cannot stamp a touch the server then rejects.
- **Settlement is correct, proven with real bets.** Placed 4 real positions
  through the full pipeline (replay-quote → server pricing → worker settlement)
  while recording the raw tick path. Every outcome matched
  *"did the raw path cross the band during `[t_open, t_close]`"*:

  | band | raw crossed? | server | match |
  |---|---|---|---|
  | [1656,1657) | yes (→1656.41) | WON | ✓ |
  | [1657,1658) | yes | WON | ✓ |
  | [1658,1659) | **no** (max 1657.30) | LOST | ✓ |
  | [1659,1660) | no | LOST | ✓ |

  The `[1658,1659)` loss is the complaint in miniature: price reached 1657.30 —
  within **0.70** of the edge — but never crossed.
- **DB confirms the shape.** W=1557, L=387, **V=1**. Losses are clean expiries
  (settle ~17–30 ms after close) at prices just **outside narrow** (0.25–1.0)
  bands.
- **Image #1 is the rendering evidence.** The pill reads `1637.99` (below the
  band) sitting over the `[1638.00,…)` tile. The number is below the band; only
  the pill *rectangle* overlaps the tile *rectangle*.

**Root cause:** there is **no settlement-logic bug**. A win requires the raw
price path to actually cross the band. The complaint is a **rendering /
perception** problem — the chart oversells near-misses (a thick glowing dot +
halo + the price pill, on a coarse vertical scale where ~$0.01 ≈ 1 px, bleed
across a narrow band edge) — compounded by a **5 s window** that yields too few
genuine touches.

### Residuals NOT covered by the reproduction (flagged, not claimed fixed)
- **In-play column betting** — server allows `t_open ≤ now` (`validation.rs:58`).
  Not exercised by the repro.
- **Window inside a feed silence** — ETH ticks intermittently go silent for
  seconds (measured a 7.5 s gap). The line freezes while columns keep scrolling,
  so a held cell can *look* parked while no in-window tick exists. A 10 s window
  mitigates (ticks before+after the gap → the path-segment leap is tested), but
  the underlying feed instability is infra and out of scope here.

## 3. Organizing principle

**What you see == what settles.** The rendered line and head marker must be the
same curve the server settles (the raw path), and the marker must never visually
claim a cell the price has not entered.

## 4. Changes

### 4.1 Window → 10 s; minimum bettable horizon → 10 s
- `CELL_DURATION_MS` 5000 → 10000 in **both** `ui/src/lib/time.ts` and
  `backend/api/src/validation.rs` (move together — the server rejects any cell
  whose `t_close − t_open ≠ CELL_DURATION_MS`).
- `NEAR_COLUMN_LOCK_MS` (`Grid.tsx`) → 10000 so the soonest bettable cell opens
  ~10 s out (tunable; resulting cadence: bet → opens ~10 s → runs 10 s).
- **Blast radius:** the window-aware multiplier (`multiplier.rs`) already derives
  `tau_win`/`tau_open` from the cell timestamps, so it reprices correctly for
  10 s with no engine change — but the multipliers **shift**, so the parity
  fixtures (`ui/tests/fixtures/parity.json` and the backend's) must be
  regenerated and any test asserting `5000`/specific multipliers updated.
  `calibratedStrikeStep` takes `cellDurationMs` and rescales the ladder step
  automatically.
- **Why:** ~2× the time for the price to genuinely cross a band → fewer
  near-miss losses; and a 10 s window survives a ~7.5 s feed gap.

### 4.2 Zoom-to-fit (auto-range) shared axis
- Replace the strike-pinned vertical map (`pxPerPrice = slot/step`, clamped to
  `[-16, h+16]`) with an **eased auto-range**: fit the visible window's padded
  `[min,max]` price range to the viewport height, head near vertical center.
- The line and the strike cells share **one dynamic `price→y` mapping**. Cells
  become positioned by price on that mapping (not a fixed-height flexbox), so
  they zoom with the line and stay glued to real price — a row's dollar band is
  unchanged; only its on-screen height/position adapts.
- **Honesty bonus:** a tight range zooms in, so a $0.01 near-miss renders as
  visible pixels instead of hiding under the dot.

### 4.3 Drop the pill; honest head marker
- Remove the DOM price pill (`PriceLine.tsx:457-465`) — the Image #1 collision
  and the prime over-claim. Live price remains in the header.
- Keep the canvas head dot, but (a) anchor it at the **raw** head and (b)
  size/clip it so its halo **cannot straddle a band edge** — it must not paint
  into a cell the price has not entered. Under 4.2's higher px/$, this matters
  more, not less.

### 4.4 Render-source: draw raw, not EMA
- Draw the line + dot from the **raw mid path** (the same mids the server
  settles). Get smoothness from the existing monotone-cubic geometry + the
  zoom-to-fit axis — not from temporal EMA lag. This removes the last
  render-vs-settle displacement and continues the existing "calm comes from the
  axis, not lag" direction (`PriceLine.tsx:36-37`). The stale `display-line.ts`
  comment (hit detection reads the smoothed value — it no longer does) gets
  corrected/removed.

## 5. Success criteria

- **Regression guard:** re-run the live repro harness after changes — outcome
  must still equal raw-crossed across a spread of bands ("what you see ==
  what settles").
- Parity fixtures regenerated for 10 s; all pricing/validation tests green.
- **Visual:** at a near-miss the head dot sits visibly outside the band edge
  (does not paint into the cell); the line fills the frame with the head near
  center; no clamp cliff; no pill.
- **Unit:** the `price→y` auto-range mapping is a pure function tested for fit,
  easing, and line/cell alignment.

## 6. Out of scope
- Oracle-aggregator resilience (feed silences) — infra, separate track.
- In-play-column hardening (`t_open > now` gate) — flag for future.
