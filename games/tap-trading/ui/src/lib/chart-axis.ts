// Single source of truth for the chart's vertical price→y mapping.
//
// PriceLine's rAF writes this every frame; Grid reads it to pan the strike cells
// onto the SAME axis the line is drawn on. This is what makes "what you see ==
// what settles" a structural property rather than a coincidence: the line's y and
// each cell's y both derive from `center`/`pxPerPrice`, so the line can never be
// drawn inside a band the price has not actually entered (the perception bug).
//
// `center` is eased (glides toward the ladder center) so the grid pans smoothly
// instead of snapping a whole row on each recenter. `pxPerPrice` is NOT eased —
// it equals the cells' own `slot / step`, so line and cells share one scale and
// a re-tier rescales both together. A plain mutable holder (not a store): written
// ~60 Hz, read by another rAF; it must never trigger a React re-render.
export const chartAxis: { center: number | null; pxPerPrice: number } = {
  center: null,
  pxPerPrice: 0,
};
