#!/usr/bin/env python3
"""Generate QuantLib parity fixtures for the Tick pricing engine.

Prerequisites:
    pip install QuantLib            (or: uv pip install QuantLib in a venv)

Usage:
    python3 games/tap-trading/scripts/gen-quantlib-fixtures.py

Writes two committed fixture files directly (no stdout redirect):
  - tests/fixtures/quantlib.json            in-band double-barrier no-touch,
                                            verifies `hui_no_touch`
  - tests/fixtures/quantlib_onetouch.json   out-of-band single-barrier touch,
                                            verifies `first_passage_touch_prob`

Both are consumed by `tests/quantlib_parity.rs` (and, in a later plan, the TS
port). Deterministic: `random.seed` fixes the corpus so reruns are stable.
"""

import json
import os
import random
import sys

try:
    import QuantLib as ql
except ImportError:
    print("error: install QuantLib first (pip install QuantLib)", file=sys.stderr)
    sys.exit(1)

random.seed(20260523)

SECONDS_PER_YEAR = 31_557_600.0

FIXTURE_DIR = os.path.normpath(os.path.join(
    os.path.dirname(__file__),
    "..", "backend", "pricing-engine", "tests", "fixtures"))


def _bsm_process(spot: float, sigma_annualized: float, tau_years: float):
    """BSM process with r=q=0 and the sub-day-τ scale trick.

    QuantLib `Exercise` dates are day-granular, but Tick windows are seconds
    (`tau_years` ~1e-7 to ~2e-6). P_no_touch under r=q=0 depends only on
    `σ²·τ`, so we map onto a synthetic 1-year horizon and rescale sigma:

        σ_synth = σ_annualized · √τ_years     (keeps σ²·τ invariant)

    The drift `−σ²/2·τ` is invariant under the same rescaling. Spot and
    barrier levels are unchanged.
    """
    today = ql.Date.todaysDate()
    ql.Settings.instance().evaluationDate = today
    sigma_synthetic = sigma_annualized * (tau_years ** 0.5)
    flat0 = ql.YieldTermStructureHandle(
        ql.FlatForward(today, 0.0, ql.Actual365Fixed()))
    vol_ts = ql.BlackVolTermStructureHandle(
        ql.BlackConstantVol(today, ql.NullCalendar(),
                            sigma_synthetic, ql.Actual365Fixed()))
    spot_h = ql.QuoteHandle(ql.SimpleQuote(spot))
    process = ql.BlackScholesMertonProcess(spot_h, flat0, flat0, vol_ts)
    return today, today + 365, process


def double_barrier_no_touch(spot: float, l: float, h: float,
                             sigma_annualized: float, tau_years: float) -> float:
    """P_no_touch via QuantLib's analytic double-barrier binary engine."""
    _, expiry, process = _bsm_process(spot, sigma_annualized, tau_years)
    payoff = ql.CashOrNothingPayoff(ql.Option.Call, 0.0, 1.0)
    exercise = ql.EuropeanExercise(expiry)
    option = ql.DoubleBarrierOption(
        ql.DoubleBarrier.KnockOut, l, h, 0.0, payoff, exercise)
    option.setPricingEngine(ql.AnalyticDoubleBarrierBinaryEngine(process))
    return float(option.NPV())


def single_barrier_touch(spot: float, barrier: float,
                         sigma_annualized: float, tau_years: float,
                         up: bool) -> float:
    """P_touch of a single near barrier (out-of-band first passage).

    QuantLib's binary-barrier engine prices "touched AND in-the-money at
    expiry", which is *half* a one-touch — the wrong instrument. Instead we
    reuse the validated double-barrier no-touch engine with the FAR barrier
    pushed 1% past spot (unreachable in seconds at Tick vol, where σ·√τ ≲
    3e-3), so `P_touch = 1 − P_no_touch` collapses to single-barrier first
    passage. far=1% is the sweet spot: matches the exact Karatzas–Shreve
    formula to ~1e-13 while keeping QuantLib's own series convergent (a far
    barrier at 2%+ trips its convergence guard for narrow near bands).
    """
    if up:    # near barrier above spot; far barrier unreachable below
        l, h = spot * 0.99, barrier
    else:     # near barrier below spot; far barrier unreachable above
        l, h = barrier, spot * 1.01
    return 1.0 - double_barrier_no_touch(spot, l, h, sigma_annualized, tau_years)


def gen_no_touch_fixtures(n: int = 100) -> list:
    # In-band double-barrier no-touch, scoped to Tick's production envelope:
    #   bands 0.01%–0.05% of spot, σ 0.30–2.00, τ 5–60s. `hui_no_touch` is
    #   calibrated at 10 terms for this regime (see hui.rs module doc).
    fixtures = []
    for _ in range(n):
        spot = random.uniform(100, 100_000)
        width_pct = random.uniform(0.0001, 0.0005)
        l = spot - spot * width_pct / 2
        h = spot + spot * width_pct / 2
        sigma = random.uniform(0.30, 2.00)
        tau_sec = random.uniform(5.0, 60.0)
        try:
            p_no_touch = double_barrier_no_touch(
                spot, l, h, sigma, tau_sec / SECONDS_PER_YEAR)
        except Exception as exc:
            print(f"warn: no-touch case skipped — {exc}", file=sys.stderr)
            continue
        fixtures.append({
            "spot": spot, "l": l, "h": h,
            "sigma_annualized": sigma, "tau_sec": tau_sec,
            "expected_p_no_touch": p_no_touch,
        })
    return fixtures


def gen_one_touch_fixtures(n: int = 100) -> list:
    # Out-of-band single-barrier touch. σ/τ scoped slightly tighter than the
    # in-band set (σ ≤ 1.50, τ ≤ 45) to keep the double-barrier-far helper
    # convergent; this still spans BTC steady-state vol (~0.80). The near
    # offset is drawn so b/v ∈ [0.3, 3.3] — the range where P_touch maps to
    # non-floored, non-capped multipliers (≈0.64× down to the 1000× cap).
    import math
    fixtures = []
    for _ in range(n):
        spot = random.uniform(100, 100_000)
        sigma = random.uniform(0.30, 1.50)
        tau_sec = random.uniform(5.0, 45.0)
        v = sigma * (tau_sec / SECONDS_PER_YEAR) ** 0.5
        bv = random.uniform(0.3, 3.3)
        up = random.random() < 0.5
        offset = bv * v  # log-distance of the near barrier from spot
        barrier = spot * math.exp(offset if up else -offset)
        try:
            p_touch = single_barrier_touch(
                spot, barrier, sigma, tau_sec / SECONDS_PER_YEAR, up)
        except Exception as exc:
            print(f"warn: one-touch case skipped — {exc}", file=sys.stderr)
            continue
        fixtures.append({
            "spot": spot, "barrier": barrier,
            "sigma_annualized": sigma, "tau_sec": tau_sec,
            "expected_p_touch": p_touch,
        })
    return fixtures


def _write(name: str, data: list):
    path = os.path.join(FIXTURE_DIR, name)
    with open(path, "w") as fh:
        json.dump(data, fh, indent=2)
        fh.write("\n")
    print(f"wrote {len(data)} fixtures -> {path}", file=sys.stderr)


def main():
    _write("quantlib.json", gen_no_touch_fixtures())
    _write("quantlib_onetouch.json", gen_one_touch_fixtures())


if __name__ == "__main__":
    main()
