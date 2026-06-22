// Single source of truth for landing copy. Accuracy guardrails: play settles in
// real USDC (deposit once into the on-chain vault, tap with no per-bet signing,
// withdraw anytime); the on-chain vault + Walrus proofs are the custody + audit
// layer, testnet-verified — never claim "every tap is its own on-chain tx"; no
// invented metrics, partners, audits, or user counts.
export const COPY = {
  brand: 'Tick',
  nav: [
    { label: 'How it works', href: '#how' },
    { label: 'Fairness', href: '#fairness' },
    { label: 'Built on Sui', href: '#built' },
  ],
  hero: {
    kicker: 'Tap. Watch. Win USDC.',
    h1: ['Tap the chart.', 'Win the next', 'five seconds.'],
    sub: 'Tick turns live crypto prices into a tap game. Every multiplier comes from real options math — not RNG — and every settlement publishes a replayable proof to Walrus.',
    primary: 'Play Tick',
    secondary: "See how it's fair",
    microcopy: 'Deposit USDC once · Tap free, no per-bet signing · Provably fair on Sui',
  },
  builtWith: {
    label: 'Built with',
    items: ['Sui', 'Walrus', 'Pyth'],
    badge: 'Sui Overflow 2026',
    fair: 'Provably fair',
  },
  // Scrolling ticker strip under the hero. Every entry is a real fact (stack,
  // mechanic, or spec value) — no fabricated prices, counts, or partners. `tone`
  // reserves Sui-blue for the ecosystem handshake and green for fairness.
  ticker: [
    { label: 'Built on Sui', tone: 'sui' },
    { label: 'Provably fair', tone: 'green' },
    { label: 'Walrus-published proofs', tone: 'none' },
    { label: 'Pyth oracle feed', tone: 'none' },
    { label: 'Real options math · not RNG', tone: 'none' },
    { label: '5-second rounds', tone: 'none' },
    { label: 'On-chain USDC vault', tone: 'sui' },
    { label: 'Lock-at-tap multipliers', tone: 'none' },
    { label: 'Win on first touch', tone: 'green' },
    { label: 'Sui Overflow 2026', tone: 'sui' },
  ],
  how: {
    id: 'how',
    eyebrow: 'The loop',
    title: 'Three taps to your first win',
    steps: [
      {
        n: '01',
        title: 'Pick a cell',
        body: 'Each cell is a price band over the next few seconds. Near the line pays less but hits more often; far out pays big.',
      },
      {
        n: '02',
        title: 'Tap to lock',
        body: 'Your multiplier freezes the instant you tap — recorded and never re-priced. What you see is what you win.',
      },
      {
        n: '03',
        title: 'Watch & win',
        body: 'If the live price touches your band before the window closes, you win on first touch. No waiting for expiry.',
      },
    ],
  },
  why: {
    eyebrow: 'The difference',
    title: "Why Tick isn't another tap-to-earn",
    cards: [
      {
        title: 'Real math, not RNG',
        body: 'Multipliers come from published options-pricing formulas (Hui + Broadie-Glasserman-Kou), driven by live Pyth oracle data. Your chart-reading actually matters.',
      },
      {
        title: '5-second feedback loop',
        body: 'Faster than perps, faster than prediction markets. Tap, watch the line, win or lose — then go again.',
      },
      {
        title: 'Real USDC, no per-tap signing',
        body: 'Deposit USDC once into an on-chain vault, then tap freely — no wallet popup per bet. Wins and losses settle in real USDC; withdraw your balance anytime.',
      },
      {
        title: 'Locked multipliers',
        body: 'The number you tap is the number you are paid. It is locked at tap and re-read at settlement — never recomputed.',
      },
    ],
  },
  // "By the numbers" interlude. Every value is a real system parameter from the
  // spec/engine — NOT a growth/vanity metric. Keep it that way.
  stats: [
    { value: 20, suffix: ' Hz', label: 'Oracle broadcast rate' },
    { value: 5, suffix: 's', label: 'Round window' },
    { value: 120, suffix: 's', label: 'Proof replay buffer' },
    { value: 4, suffix: '', label: 'Price feeds · Pyth + 3 CEX' },
  ],
  fairness: {
    id: 'fairness',
    eyebrow: 'Provably fair',
    title: 'You never have to trust the screen',
    body: 'The multiplier you lock is committed in the settlement proof. Your USDC sits in an on-chain vault you deposit to and withdraw from. And every settlement publishes a self-contained proof to Walrus — the full oracle price path, the locked multiplier, and the outcome — that anyone can replay to confirm the result.',
    points: [
      {
        k: 'Lock-at-tap',
        v: 'Your multiplier is locked at tap and published in the proof — never recomputed at settlement.',
      },
      {
        k: 'On-chain USDC vault',
        v: 'Your USDC is custodied in a Move vault — deposit once, withdraw anytime.',
      },
      {
        k: 'Walrus-published proofs',
        v: "Each settlement's oracle path and outcome are stored immutably and are independently replayable.",
      },
      {
        k: 'Pure WASM verifier',
        v: 'A dependency-free verifier replays any proof client-side using the exact server pricing code.',
      },
    ],
    note: 'On-chain USDC vault + Walrus proof flow implemented and verified end-to-end on Sui testnet. Play settles in real USDC: deposit once, tap with no per-bet signing, withdraw anytime.',
    verifyChip: { label: 'proof.replay()', verdict: 'Valid' },
    // Illustrative settlement proof — the shape of what every settlement
    // publishes to Walrus and anyone can replay. Values are an example.
    proof: {
      title: 'settlement.proof',
      tag: 'walrus',
      fields: [
        { k: 'pair', v: 'SUI / USD', tone: 'plain' },
        { k: 'locked_multiplier', v: '840 bps', tone: 'pink' },
        { k: 'window', v: '5.000 s', tone: 'plain' },
        { k: 'first_touch', v: 't = 3.21 s', tone: 'plain' },
        { k: 'outcome', v: 'WIN', tone: 'green' },
        { k: 'walrus_blob', v: '0x9f3a…e21c', tone: 'sui' },
      ],
      replay: 'proof.replay()',
      verdict: 'Valid',
    },
  },
  tech: {
    id: 'tech',
    eyebrow: 'How it works under the hood',
    title: 'From oracle to USDC settlement',
    body: 'A streaming pipeline, built Sui-native end to end.',
    flow: [
      {
        step: 'Oracle aggregator',
        body: 'Pyth Hermes + 3 CEX feeds → freshness-filtered median + EWMA smoothing, broadcast at 20 Hz with a 120s replay ring buffer.',
      },
      {
        step: 'Pricing engine',
        body: 'Hui continuous-barrier option pricing with the Broadie-Glasserman-Kou discrete-monitoring correction. QuantLib-parity tested.',
      },
      {
        step: 'Settlement worker',
        body: 'In-memory first-touch detection over a continuous price path; settles an off-chain USDC balance, with the on-chain vault + Walrus proofs as the deposit/withdraw custody and audit layer.',
      },
      {
        step: 'Sui + Walrus',
        body: 'A tick_vault Move package custodies your USDC for deposit and withdraw; settlement proofs are published to Walrus for anyone to replay.',
      },
    ],
    stack: ['Sui Move', 'Walrus', 'Rust', 'zkLogin / Enoki', 'React'],
  },
  finalCta: {
    title: 'The next five seconds are yours.',
    sub: 'Deposit USDC once. Then just tap — no per-bet signing.',
    primary: 'Play Tick',
    chips: ['Provably fair', 'On-chain USDC vault', 'Real options math'],
  },
  footer: {
    tagline: 'Tap. Watch. Win USDC.',
    builtFor: 'Built for Sui Overflow 2026',
    links: [
      { label: 'Docs', href: '#' },
      { label: 'GitHub', href: '#' },
    ],
    note: 'Real USDC on Sui testnet. Multipliers and settlements are provably fair.',
  },
} as const;
