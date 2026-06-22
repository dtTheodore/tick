# ADR-0012 — Tick near-cell multiplier floor set above Pacifica

**Date:** 2026-06-05
**Status:** Accepted
**Workstream:** Tick (tap-trading)
**Supersedes:** —
**Superseded by:** —

## Context

The tap-trading multiplier has a τ-dependent floor, `floor(τ) = floor_a +
floor_b·τ`, that sets the minimum payout for near/in-band cells. It exists
because the raw first-touch math pays an in-band cell only `(1 − house_margin)
≈ 0.9×`; the floor overlays a product-chosen minimum so near cells feel worth
tapping (MATH_SPEC §4.1/§285).

v1 fit the floor to the Pacifica BTC in-band row (`a = 1.30, b = 0.01` →
1.35×→1.75× over 5–45 s, within ±2% of Pacifica). In play-testing this felt
too stingy: near cells paid ~1.4×, which reads as "not enough reason to tap."
The product owner explicitly wanted the **near** cells more rewarding and had
**no objection to the high OTM tail** multipliers.

Two facts shaped the decision:

- The floor is a **pure UX dial**, decoupled from the risk math. It does not
  touch `P_touch`, the BGK continuity correction, `jump_buffer`, or
  `house_margin`. Changing it cannot break the touch-probability model or the
  TS↔Rust↔QuantLib parity (QuantLib fixtures pin probabilities, not the floor).
- The house edge lives in the **OTM tail**, not the near cells. Near/in-band
  cells already pay slightly above their first-touch-fair value — they are a
  loss-leader. The tail (untouched here) funds them.

`jump_buffer` was explicitly **not** considered as a lever: it is a risk
parameter to be calibrated against observed-vs-predicted touch rates
(MATH_SPEC §6.2), not a generosity dial.

## Decision

Raise the floor coefficients to **`floor_a = 1.50, floor_b = 0.025`**
("steeper"), in both the Rust source-of-truth (`pricing-engine`) and the TS
port. Near/in-band payouts become:

| τ_to_close | before (1.30 + 0.01τ) | after (1.50 + 0.025τ) | Pacifica ref |
|---|---|---|---|
| 5 s  | 1.35× | 1.63× | 1.37× |
| 10 s | 1.40× | 1.75× | 1.41× |
| 30 s | 1.60× | 2.25× | 1.64× |
| 45 s | 1.75× | 2.63× | — |

The steeper slope (`b`) also makes the multiplier visibly grow as τ increases,
giving a reason to engage the whole betting band rather than only the soonest
cell. The high outer (OTM) multipliers, `house_margin` (0.10), `jump_buffer`
(1.30), and BGK are all unchanged.

This **deliberately contradicts** MATH_SPEC's original "fit to Pacifica ±2%"
calibration; MATH_SPEC §4.1/§4.2 and the summary line have been updated to
document the new curve and reference this ADR.

## Consequences

- **Fairness / profit:** modeled blended player-EV ≈ 78% under a uniform bet
  spread (house edge ~22% on that pessimistic GBM baseline; true edge higher
  because real crypto tails are fatter than the baseline assumes). The OTM tail
  that carries the edge is untouched, so the product still profits.
- **Per-cell:** near/in-band cells move further into deliberately
  player-favorable territory vs. first-touch-fair. This is intended (loss-leader)
  and bounded — the guard test enforces a ceiling so the giveaway can't silently
  run away.
- **Tests:** the old Pacifica-fit guard
  (`floor_curve_mean_fit_within_2pct_of_pacifica_reference`) is replaced by
  `floor_curve_runs_above_pacifica_for_near_cell_incentive`, which asserts the
  floor (a) exceeds Pacifica at every τ, (b) hits the chosen anchors
  (1.625× @ 5 s, 2.25× @ 30 s), and (c) stays under a fairness ceiling. In-band
  unit tests (Rust + TS) and the regenerated `ui/tests/fixtures/parity.json`
  reflect the new values.
- **Calibration debt unchanged:** this does not address the OTM-tail edge or the
  uncalibrated `jump_buffer`. The MATH_SPEC §6.2 weekly observed-vs-predicted
  recalibration remains the path to a data-grounded tail; the touch-rate
  instrumentation for it is **not yet wired up** and remains a prerequisite
  before real-money launch.

## Alternatives considered

- **Lower `house_margin` (0.10 → 0.05):** transparent but ineffective for this
  complaint — it barely moves near-cell payouts (the floor binds there) and is a
  uniform cut better reserved for a deliberate edge decision.
- **Lower `jump_buffer` (1.30 → ~1.15):** opens the OTM tail (the dominant
  hidden edge) but is a **risk** parameter; lowering it without §6.2 calibration
  knowingly runs at unmeasured tail risk. Out of scope — the owner did not want
  the tail changed, and points-phase data should set this value, not a guess.
- **Reshape the fan (`STRIKE_STEP_K`):** edge-neutral, would smooth the
  row-to-row ramp toward Euphoria's gradient. Deferred; orthogonal to the
  near-cell floor complaint.
