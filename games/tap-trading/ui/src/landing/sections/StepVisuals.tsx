// Small inline SVG diagrams for the "How it works" stepper. Decorative
// (aria-hidden); they mirror the game's real visuals — a price grid with a
// chosen cell, a locked multiplier chip, and a line winning on first touch.

const PINK = '#FF2D7E';
const GREEN = '#00FF88';

function Frame({ children }: { children: React.ReactNode }) {
  return (
    <svg viewBox="0 0 200 120" role="img" aria-hidden="true" className="h-full w-full">
      <title>step diagram</title>
      {children}
    </svg>
  );
}

// Step 1 — a price grid with one cell picked out, the line sweeping past it.
export function PickCellVisual() {
  const cols = [20, 65, 110, 155];
  const rows = [16, 44, 72, 100];
  return (
    <Frame>
      {rows.map((y) =>
        cols.map((x) => (
          <rect
            key={`${x}-${y}`}
            x={x}
            y={y}
            width={36}
            height={20}
            rx={3}
            fill="rgba(255,255,255,0.03)"
            stroke="rgba(255,255,255,0.07)"
          />
        )),
      )}
      {/* the chosen cell */}
      <rect x={110} y={44} width={36} height={20} rx={3} fill="rgba(255,45,126,0.18)" stroke={PINK} />
      <text x={128} y={58} textAnchor="middle" fontSize="11" fontFamily="monospace" fill={PINK}>
        8.4×
      </text>
      {/* price line */}
      <path
        d="M4 92 C 40 84, 70 70, 100 64 S 160 40, 196 34"
        fill="none"
        stroke={PINK}
        strokeWidth="2"
        strokeLinecap="round"
        opacity="0.9"
      />
      <circle cx={196} cy={34} r="3" fill={PINK} />
    </Frame>
  );
}

// Step 2 — a multiplier chip snapping shut under a lock.
export function LockVisual() {
  return (
    <Frame>
      <circle cx={100} cy={60} r={46} fill="none" stroke="rgba(255,45,126,0.18)" strokeWidth="1.5" />
      <circle cx={100} cy={60} r={32} fill="rgba(255,45,126,0.10)" stroke={PINK} strokeWidth="1.5" />
      {/* padlock */}
      <path
        d="M92 54 v-5 a8 8 0 0 1 16 0 v5"
        fill="none"
        stroke={PINK}
        strokeWidth="2.2"
        strokeLinecap="round"
      />
      <rect x={88} y={54} width={24} height={18} rx={3} fill={PINK} opacity="0.9" />
      <text x={100} y={92} textAnchor="middle" fontSize="13" fontFamily="monospace" fontWeight="700" fill="#fff">
        8.4×
      </text>
    </Frame>
  );
}

// Step 3 — the line entering a band and winning on touch.
export function TouchWinVisual() {
  return (
    <Frame>
      {/* target band */}
      <rect x={0} y={36} width={200} height={26} fill="rgba(0,255,136,0.12)" />
      <line x1={0} y1={36} x2={200} y2={36} stroke={GREEN} strokeWidth="1" strokeDasharray="4 4" opacity="0.7" />
      <line x1={0} y1={62} x2={200} y2={62} stroke={GREEN} strokeWidth="1" strokeDasharray="4 4" opacity="0.7" />
      {/* line rising into the band */}
      <path
        d="M4 104 C 50 100, 80 92, 110 78 S 160 52, 184 49"
        fill="none"
        stroke={PINK}
        strokeWidth="2"
        strokeLinecap="round"
      />
      {/* touch point */}
      <circle cx={184} cy={49} r="5.5" fill="none" stroke={GREEN} strokeWidth="1.5" opacity="0.7" />
      <circle cx={184} cy={49} r="3.2" fill={GREEN} />
      <path d="M176 30 l4 4 l7 -8" fill="none" stroke={GREEN} strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round" />
    </Frame>
  );
}

export const STEP_VISUALS = [PickCellVisual, LockVisual, TouchWinVisual];
