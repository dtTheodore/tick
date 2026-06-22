# Tick Landing Page — Design Spec

**Date:** 2026-06-20
**Owner:** Theodore
**Goal:** A premium, responsive, industry-standard marketing landing page for **Tick** (the tap-trading game in `games/tap-trading/`), built to showcase the project at **Sui Overflow 2026** to two audiences at once: players (the dopamine pitch) and hackathon judges (the Sui-native, provably-fair tech moat).

This spec bakes in the architectural and design decisions; it is the single approval gate before planning.

---

## 1. Audience & job-to-be-done

Two audiences, one page, resolved by progressive disclosure (player pitch on top, judge-grade depth below the fold via anchors):

- **Player** (18–35, mobile-native, crypto-curious): "What is this and is it fun / safe to try?" → 5-second feedback loop, no money at risk, real chart skill, one-tap to play.
- **Sui Overflow judge**: "Is this a credible, Sui-native build with real innovation?" → on-chain vault, Walrus-anchored proofs, provable fairness, real quant math, zkLogin.

**The bridge value prop** (serves both): *provably fair, settled on-chain.* It's the player's risk-reducer and the judge's tech-credibility hook simultaneously.

---

## 2. Architecture decisions (baked — rationale included)

### 2.1 Where it lives — routed into the existing UI app

Add client-side routing to the existing `games/tap-trading/ui` (React 19 + Vite 7 + Tailwind v4) via **`react-router-dom`** (smallest, most idiomatic choice; no framework change).

- `/` → **new Landing page** (the front door for the shared Sui Overflow URL).
- `/play` → **the existing game**, rendered **verbatim** (today's `App.tsx` content extracted into a `Game` route component).

**Non-negotiable success criterion:** the game must render at `/play` exactly as it renders at `/` today. Refactoring `App.tsx` into a router must not change a single pixel of the game.

*Rationale:* The landing page is the front door for a hackathon URL, so it belongs at `/`. Routing inside the existing app (vs. a separate project) reuses the design tokens, fonts, and build, and keeps one deployable.

### 2.2 The hero is self-contained — no backend dependency

The hero's signature visual (glowing price line + multiplier ladder + round timer) is recreated as a **standalone, synthetic** component, **not** wired to the live oracle WebSocket or API.

- A **synthetic price-walk generator** (seeded geometric-Brownian-style random walk with the same `tickStore`-like smoothing the real chart uses) drives a canvas that mirrors `PriceLine`'s *rendering technique*: monotone-cubic spline, hot-pink stroke (`1.75px`), soft glow, pink→transparent gradient fill, head dot.
- A **synthetic multiplier ladder** + a **looping round timer** + a **win/loss history strip** reproduce the game's HUD, computed locally.

**Rationale (load-bearing, from advisor):** the deployed landing page and the live demo must "wow" even when the full backend (oracle aggregator + API on :3221) is down. A self-contained hero deploys anywhere and never breaks in front of a judge. It can be upgraded to a live feed later behind the same component interface.

*We reuse the rendering approach, not the live `Grid`/`PriceLine` components themselves* — those are coupled to the WS stores. The landing hero is decoupled.

### 2.3 Tech additions

- `react-router-dom` (routing).
- No other runtime deps if avoidable. Motion via CSS + a tiny `IntersectionObserver` reveal hook (avoid pulling a heavy animation lib unless a specific interaction needs it). If a spring/magnetic CTA proves worth it, `framer-motion` is acceptable but is the only candidate.
- Everything else (Tailwind v4 theme, shadcn/radix, Space Grotesk + Inter, `cn()`) is already present and reused.

---

## 3. Visual design system

Refines the locked brand (`#0A0A0A` / `#FF2D7E` / `#00FF88`, Space Grotesk + Inter) per the design research — not a replacement.

### 3.1 Color tokens (added as landing-scoped CSS vars, not overriding game tokens)

```
/* Surfaces — tint the near-black; pure #000 / neutral #0A0A0A reads flat */
--lp-bg:        #0A0A0C   /* base canvas, a hair cooler than game's #0A0A0A */
--lp-raised:    #121116   /* cards / glass surfaces */
--lp-border:    rgba(255,255,255,0.08)   /* hairline borders */

/* Brand energy (locked) */
--lp-pink:      #FF2D7E   /* primary CTA, hero accent, "party" energy */
--lp-green:     #00FF88   /* wins, positive, confirmations */

/* Sui-native credibility anchor — used ONLY in Built-on-Sui + Fairness sections */
--lp-sui-blue:  #4DA2FF   /* canonical Sui Blue/500 — the judge handshake */
--lp-sui-pink:  #FE8BC2   /* sanctioned Sui pink; proves #FF2D7E is on-ecosystem */
```

The pink/green is the betting energy; **Sui blue is deliberately reserved** for the "Built on Sui" lockup and the provably-fair section so judges read "real Sui-native protocol." This consciously resolves the documented Sui-brand-vs-DeFi-casino tension.

### 3.2 Typography

- **Space Grotesk** → hero headline, section titles, all numbers/multipliers/prices. (Inter in the hero reads as AI-generated; avoid.)
- **Inter** → body copy only.
- Hero H1: Space Grotesk, clamp ~`40px`(mobile)→`104px`(desktop), weight 600, letter-spacing ~`-0.04em`.

### 3.3 Atmosphere & shape

- **One large ambient glow**: a pink/green colored shape behind the hero, `filter: blur(~140px)`, low opacity. The single biggest "premium dark" move.
- **Faint glowing 1px grid background** (the Cetus technique) — on-brand because the product *is* a price grid. Two stacked 1px-line gradients in accent color at very low opacity.
- **Grain/noise overlay** (subtle) to kill gradient banding.
- **Glassmorphism only on CTAs and key cards** (pill CTAs: `border-radius: 999px`, inset white highlight). Shape language = **pill** (playful/betting), not sharp.
- Primary CTA: solid hot-pink pill. Secondary: ghost/outline. **One** primary action, repeated.

### 3.4 Motion (tasteful + performant)

- Scroll reveals: `cubic-bezier(0.16, 1, 0.3, 1)`, 500–700ms, translateY 16–32px, **transform + opacity only**.
- Live hero line ticks continuously (synthetic walk) at the existing chart cadence.
- Optional: animated number counters for static stats (on `whileInView`), magnetic-ish primary CTA.
- **`prefers-reduced-motion`: disable reveals + freeze the hero animation to a static composed frame.**

**Performance rules (load-bearing — these prevent jank):**
- Animate **`transform`/`opacity` only**; never animate `width/height/top/left` or `box-shadow`/`filter` blur radius on a per-frame element.
- Pulse glow by animating the **opacity of a pre-blurred duplicate layer**, never the blur radius of the live line.
- DPR-cap the hero canvas at `Math.min(1.5, devicePixelRatio)`; `contain: layout` around the hero; `will-change` only while animating.

---

## 4. Page structure (top → bottom)

1. **Sticky slim nav** — Tick wordmark (mono, pink); anchors: *How it works · Fairness · Built on Sui · Tech*; persistent **Play** CTA (→ `/play`). Collapses to a compact bar / menu on mobile.
2. **Hero** — H1 + subhead + primary CTA (+ ghost secondary "See how it's fair"); the **self-contained live price-grid demo** (line + multiplier ladder + round timer + history strip). Mobile: the demo sits below the copy, full-bleed; the mobile hero must carry the whole pitch (83% of LP traffic is mobile).
3. **Trust / built-with bar** — "Built on Sui · Walrus · Pyth" marks + a "Sui Overflow 2026" badge + a "provably fair" badge. **No fabricated user counts** (crypto-natives verify on-chain); honest pre-launch proof only.
4. **How it works** — 3 steps max: *Pick a cell → Tap to lock your multiplier → Watch the line, win on touch.* Each with a tight mini-visual.
5. **Why Tick is different** — objection-handling feature cards: *Real math, not RNG* · *5-second feedback loop* · *No money at risk (points)* · *Locked multipliers (never re-priced).*
6. **Provably fair / on-chain** (Sui-blue accented) — the integrity story: lock-at-tap multiplier recorded on Sui, on-chain USDC vault custody, **every settlement anchored to Walrus and independently replayable**, "verify any bet" concept. This is the bridge section.
7. **How it's built** (judge layer) — architecture in plain language: oracle aggregator (Pyth + 3 CEX median) → pricing engine (Hui + Broadie-Glasserman-Kou) → off-chain settler → Sui `tick_vault` Move package + Walrus proof blob → pure WASM proof-verifier. Stack badges (Sui Move, Walrus, Rust, zkLogin/Enoki, React).
8. **Final CTA** — repeat the hero CTA verbatim (single-CTA pages convert far better).
9. **Footer** — links (docs, GitHub — placeholder until confirmed), socials placeholder, "points-only, no real-money in v1" honesty note, copyright.

---

## 5. Copy (draft — refine in implementation)

- **Kicker / tagline:** `Tap. Watch. Earn points.` (the official v1 tagline)
- **H1:** `Tap the chart. Win the next five seconds.`
- **Subhead:** `Tick turns live crypto prices into a tap game. Every multiplier comes from real options math — not RNG — and every settlement is provably fair, anchored on Sui and Walrus.`
- **Primary CTA:** `Play Tick`  ·  **Secondary:** `See how it's fair`
- **CTA trust microcopy:** `Points to play · No deposit · Provably fair on Sui`

**Accuracy guardrails (must hold in copy):**
- v1 is **points-only**; do not imply real-money play is live. Real-money ("Tap. Watch. Win.") is a future phase.
- The on-chain vault + Walrus proof path is **implemented and e2e-verified on Sui testnet for the USDC settlement mode**; frame fairness as the *architecture that makes settlements verifiable*, not as "every points tap is currently on-chain."
- No invented metrics, partners, audits, or user counts.

---

## 6. Out of scope (YAGNI)

- Wiring the hero to the live backend feed (synthetic only; live is a later upgrade).
- Real auth / zkLogin flow on the landing page (the CTA routes to `/play`, which owns auth).
- A separate marketing domain / CMS / blog / i18n.
- Changing any game behavior, game tokens, or backend.
- Fixing the pre-existing DB migration error surfaced by `init-worktree-dev.sh` (`settle_mode` column) — unrelated, flagged separately.

---

## 7. Success criteria (the definition of "done" — these are the e2e checks)

Verified in-browser via chrome-devtools MCP:

1. **`/` renders** the landing page with **zero console errors**.
2. **`/play` renders the game verbatim** (visual parity with today's `/`).
3. **Responsive** and correct at **390px (mobile), 768px (tablet), 1440px (desktop)** — no horizontal scroll, readable type, full-width tap targets on mobile.
4. **All nav anchors** smooth-scroll to their sections; **all CTAs** route to `/play`.
5. **Fonts load** (Space Grotesk for display/numbers, Inter for body) — no FOUT/fallback flash on the headline.
6. **`prefers-reduced-motion`** is respected (reveals off, hero frozen to a composed static frame).
7. **Hero animation is smooth** (synthetic walk), no visible jank; canvas DPR-capped.
8. **Lighthouse**: accessibility ≥ 90; performance reasonable for an animated page (no obvious red flags).
9. **OG/social meta** present in `index.html` (`<title>`, description, `og:title`/`og:description`/`og:image`, Twitter card) — the shared Sui Overflow URL previews well.
10. **`bun run build` and `bun run typecheck` pass** clean.

---

## 8. Risks & mitigations

| Risk | Mitigation |
|---|---|
| Routing refactor regresses the game | Extract game into `Game` route untouched; e2e parity check at `/play` is a gate. |
| Hero canvas jank on mobile | DPR cap, transform/opacity-only animation, pre-blurred glow layer, `contain`. |
| Reads as generic AI/casino page | Space Grotesk hero (not Inter), pink/green (never purple), Sui-blue credibility anchor, grain + tinted black. |
| Overclaiming tech to judges | Accuracy guardrails in §5; frame on-chain/Walrus as implemented-and-testnet-verified, points as the default loop. |
| Backend down during demo | Self-contained synthetic hero (§2.2). |
