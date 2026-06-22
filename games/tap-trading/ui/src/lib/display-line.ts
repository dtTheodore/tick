// The price value the chart line is *currently rendering* — the EMA-smoothed mid
// PriceLine glides toward over ~LINE_TAU_MS, not the raw oracle tick.
//
// NOT used for settlement or the win cue: the cue judges the RAW price path
// directly (see useLiveHitDetection), matching the server zone. This value exists
// so the live per-cell multiplier preview (useCellMultiplier) is priced off the
// SAME value the line draws — pricing off the raw 20–50 Hz mid strobed the number
// against a line that wasn't moving.
//
// A plain mutable holder, not a store: PriceLine writes it every animation frame
// (~60 Hz); useCellMultiplier samples it at each 100 ms recompute. Reading it
// never triggers a React re-render — the multiplier hook re-renders off the tick
// store, not this holder.
export const displayLine: { smoothedMid: number | null } = { smoothedMid: null };
