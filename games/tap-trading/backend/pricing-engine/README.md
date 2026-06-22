# Tap-Trading Pricing Engine

Plain-English guide to what this crate does. **No math degree required.**

If you want the full derivations, they live in
[`games/tap-trading/docs/MATH_SPEC.md`](../../docs/MATH_SPEC.md). This file is
the version you can read on a Monday morning.

---

## What it does

The game shows boxes on a moving price chart. A player taps a box. If the price
**touches that box** within the next few seconds, they win their stake back
times a **multiplier**.

This crate has exactly one job:

> **Decide what multiplier to print on each box.**

The rule is just betting odds:

- Box the price is **very likely** to hit → **small** multiplier (e.g. `1.3×`)
- Box the price **probably won't** hit → **big** multiplier (e.g. `80×`)

That's the whole product. Everything below is *how* we set those numbers fairly.

---

## The one question

To price a box fairly we only need to answer one thing:

> *"How likely is the price to touch this box in the next few seconds?"*

Call that number **P_touch** (a probability between 0 and 1). Once we have it,
the multiplier is basically `0.9 / P_touch` (the `0.9` is the house keeping a
10% margin), with a floor and a cap. Rare box → tiny P_touch → huge multiplier.

So the entire engine boils down to: **compute P_touch well, instantly, and the
same way every time.**

---

## How it answers it

We use a **formula** (a "closed-form"), not a simulation. Punch in the numbers,
get the answer in microseconds, identical every time.

| Situation | What we do |
|---|---|
| Price is **already inside** the box | It wins the moment the box opens → `P_touch = 1`, box pays the floor multiplier |
| Price is **outside** the box | A bell-curve formula: how likely is it to drift far enough to reach the box edge? This prices the high-multiplier boxes. |
| (always) | A tiny correction because the price feed updates in discrete ticks, not continuously |

**Why a formula and not a simulation?**

The game redraws every box's multiplier **10 times a second**, and the server
re-checks it whenever a player taps. A formula is instant and gives the player
and the server the *exact same number* — which is what makes the game provably
honest. A simulation ("Monte Carlo") would be too slow and give slightly
different numbers each run, breaking that promise. Simulation stays in the
toolbox only for offline testing.

---

## What's in the box (file map)

| File | Plain-English role |
|---|---|
| `src/multiplier.rs` | The main entry point. Takes a box + the current price/volatility, returns the multiplier. This is the file that matters. |
| `src/vol.rs` | Estimates how jumpy the market is right now (volatility). Jumpier market → boxes are easier to hit → smaller multipliers. |
| `src/bgk.rs` | The tiny "discrete tick" correction mentioned above. |
| `src/hui.rs` | A textbook formula kept for **verification only** — see "Trust" below. Not used when a player taps. |
| `src/constants.rs` | The tunable numbers (house margin = 10%, etc.). |
| `src/types.rs` | The data shapes: a `Cell` (box), the `OracleState` (current price), the `PricingConfig` (settings). |
| `src/error.rs` | What "bad input" looks like — e.g. a corrupt price feed makes the engine refuse to price (so the app can pause taps) rather than print a wrong number. |

---

## Trust: how we know the math is right

You don't have to check the math yourself. That's what the tests are for.

- **50 automated tests** cover the formulas, the edge cases, and the safety
  rails (`cargo test`).
- **QuantLib parity:** we cross-check our formulas against
  [QuantLib](https://www.quantlib.org/), a respected independent finance
  library, on ~200 randomized cases — **both** the "inside the box" formula
  (`src/hui.rs`) and the "outside the box" first-passage formula that prices
  the big multipliers (`first_passage_touch_prob`). If our number ever drifts
  more than 1% from QuantLib's, the test fails. Think of it as **trust
  insurance**, not game code.
- **Property tests** check invariants that must *always* hold (multipliers
  never go below the floor or above the cap, probabilities stay between 0 and 1,
  etc.) across thousands of random inputs.

---

## Running it

From inside `games/tap-trading/backend/`:

```bash
cargo test                       # run everything
cargo test --test quantlib_parity   # just the QuantLib cross-checks
cargo clippy -- -D warnings      # lint
```

**Regenerating the QuantLib fixtures** (only needed if a formula or constant
changes). QuantLib is a Python library used *only* to produce the test data —
it is **not** part of the deployed binary and never runs in production:

```bash
# one-time: install QuantLib in a throwaway environment
uv venv tmp/quantlib-venv
VIRTUAL_ENV=tmp/quantlib-venv uv pip install QuantLib

# regenerate both committed fixture files (deterministic — same output every run)
tmp/quantlib-venv/bin/python games/tap-trading/scripts/gen-quantlib-fixtures.py
```

---

## What this crate is *not*

- **Not** an interest-rate / dividend model — Tick's windows are seconds long,
  so those are zero.
- **Not** a database or API — it's pure math, no IO, no network. The API and
  settlement worker (which store positions and pay out wins) live elsewhere.
- **Not** the source of truth for *displaying* multipliers — the client has a
  thin port of this same math so the on-screen number updates smoothly; this
  Rust crate is the canonical version the server trusts.
