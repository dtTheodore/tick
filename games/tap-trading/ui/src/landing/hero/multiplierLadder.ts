export interface LadderRow {
  strike: number;
  cols: number[]; // one display multiplier per future time column
}

export interface LadderOptions {
  mid: number;
  tickSize: number;
  rowCount: number;
  colCount: number;
}

// Display-only multiplier curve for the hero demo: ~1.5x at the money, growing
// with distance from the mid; nearer time columns pay slightly more. This is
// NOT the real pricing engine — it just reproduces the shape players recognize
// (low near the line, exploding far from it) for a believable static ladder.
export function buildMultiplierLadder(opts: LadderOptions): LadderRow[] {
  const { mid, tickSize, rowCount, colCount } = opts;
  const mids = (rowCount - 1) / 2;
  const rows: LadderRow[] = [];
  for (let i = 0; i < rowCount; i++) {
    const offsetTicks = mids - i; // top row positive (above mid)
    const strike = mid + offsetTicks * tickSize;
    const distance = Math.abs(offsetTicks);
    const cols: number[] = [];
    for (let c = 0; c < colCount; c++) {
      const base = 1.5 + distance ** 1.9 * 0.55;
      const timeBoost = 1 + (colCount - 1 - c) * 0.06;
      const m = Math.max(1, base * timeBoost);
      cols.push(Math.round(m * 10) / 10);
    }
    rows.push({ strike: Math.round(strike * 100) / 100, cols });
  }
  return rows;
}
