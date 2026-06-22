# Tick — Product Requirements

**Status:** v0.1
**Scope:** A points-only tap-trading game on Sui. Web + mobile PWA only — no Telegram. Multipliers driven by real-market touch-probability math against a Pyth-anchored oracle, with τ-dependent floor curve calibrated to Pacifica SWIM. No real-money trading in v1. Sets up a future on-chain token via Sui-anchored snapshots of points history.

> Sibling docs: `MATH_SPEC.md`, `ORACLE_SPEC.md`, `SYSTEM_DESIGN.md`, `TESTING_STRATEGY.md`.

---

## 0. Locked Decisions

Anchors from frame-by-frame review of Pacifica SWIM, Euphoria, and BC.Game tap trading. Do not re-litigate without new evidence.

- **Product name**: Tick. Repo lives at `games/tap-trading/`; package scope `@tap-trading/*`.
- **Cell duration**: 5 s.
- **Visible columns**: 4 future columns on mobile (20 s look-ahead), 6 on desktop (30 s).
- **Strike & time grid**: globally anchored strikes (everyone sees the same `…3812.0 / 3812.5 / 3813.0…` ladder), clock-aligned time columns (every 5 s on the wall clock).
- **In-band cells tappable** at the τ-dependent floor `floor(τ) = 1.30 + 0.01·τ_seconds`. See `MATH_SPEC §4`.
- **Lock-at-tap**: 10 Hz client refresh, PENDING → LOCKED → SETTLED lifecycle, 3% drift tolerance.
- **No energy system** in v1. Points are a finite per-user currency; full economy in §15.
- **No Telegram** in any phase.
- **API stack**: Rust + axum.
- **Stake**: variable from day one (`{50, 100, 500, 1000}` at Tier 1).
- **Independent positions per tap**: no stake aggregation.

---

## 1. Problem

A new consumer category — **tap trading** — emerged in 2026 and is taking real volume. The defining product is **Euphoria** (`euphoria.finance`, on MegaETH, mainnet 14 May 2026): a price-grid UI where each cell is a `(price band × time window)` touch-bet, settled when the spot price enters the band during the window. Multipliers per cell are computed live from distance, time-to-expiry, and volatility. **Pacifica's SWIM** (`app.pacifica.fi/swim`, on Solana, inside their perp DEX) ships the same mechanic.

Why this category is winning:
1. **Higher dopamine than perps.** 5–60s feedback loop. Tap → see line move → win or lose. Faster than Polymarket, faster than perps.
2. **Higher integrity than tap-to-earn.** Multipliers come from real market math, not random rewards. *"Your chart-reading skill actually matters."*
3. **Mobile-first PWA.** Euphoria ships PWA + Telegram; Pacifica still desktop-first. We ship mobile-first via PWA only — no Telegram surface in v1 or later phases.
4. **Real distribution.** Euphoria: $1.13M cumulative volume + 533K taps + 1,593 accounts in week one. Pacifica: $153B cumulative perp volume, with SWIM riding the same balance.

The category is open in three ways:
- **No Sui-native tap-trading product exists.** Pandora Finance is the closest (5-min binary, won Sui Overflow 2024) but is European-binary, not touch-grid.
- **No one ships the points-first variant.** Euphoria and SWIM require real-money deposits. Hamster Kombat / Notcoin proved points-first acquisition (600K+ users pre-TGE on Notcoin alone) but have no real-market mechanic — their tap is decoration. The **points-first × real-oracle-math** combination is unoccupied.
- **Mobile + zkLogin** onboarding isn't well served. Euphoria uses Privy embedded wallets; Pacifica is desktop-led. Sui's **zkLogin + Enoki sponsored gas** is materially smoother than anything in the EVM/Solana competitive set.

## 2. Solution

**Tick** is a tap-trading game on Sui where:

- **Mechanic** — A live price-grid. Y-axis = price bands (asset-specific tick size: Δ$0.5 ETH, Δ$10 BTC, Δ$0.1 SOL). X-axis = clock-aligned 5s future time windows. Tap a cell = lock in a prediction that price will enter that band during that window.
- **Settlement** — Path-dependent (touch). The cell wins the instant the price line enters the band, even mid-window. *European binary is a worse game; we ship touch from day one.*
- **Multipliers** — Computed live from a Brownian-bridge first-touch probability formula, against an EWMA-smoothed aggregate oracle (Pyth + select CEX WS feeds). Multipliers refresh ~1–4 Hz. **Locked at tap time** (Pacifica's UX guarantee).
- **Currency** — **Points only in v1.** No USDC. No real-money exposure. Daily quests, streaks, squads, leaderboards. Hamster-Kombat retention loop, Euphoria math. Points are finite per user; pacing handled by API rate limit, not an energy meter. Full economy spec in §15.
- **Identity** — zkLogin via Google/Apple/Twitter. No seed phrase. No wallet popup.
- **On-chain footprint** — Minimal but load-bearing. Weekly Merkle snapshot of `(user, points, week)` published to a Sui Move contract. Users can verifiably prove their points history. Sets up a future Sui-native token (TGE Phase 5+).
- **One codebase, one surface family** — Web + mobile PWA (installable, push-capable). No Telegram Mini App.

Tick is **not** a casino, **not** a broker, **not** a derivatives venue. It is a **free-to-play mobile game with an on-chain integrity story** and a believable path to a token.

## 3. Vision & Positioning

| | |
|---|---|
| **Vision** | The default tap-trading game outside MegaETH and Solana. |
| **Mission** | Make watching the chart feel like a game — and make the game feel honest. |
| **Tagline** | **Tap. Watch. Earn points.** (v1) → **Tap. Watch. Win.** (Phase 5 with real-money mode) |
| **Brand name** | **Tick** |
| **Category framing** | "Tap trading" — Euphoria's category name; we adopt it. |
| **Positioning vs Euphoria / Pacifica SWIM** | Same mechanic, no money at risk, smoother onboarding, real Sui integrity story. |
| **Positioning vs Hamster Kombat / Notcoin** | Same retention loop, real chart math instead of random tap rewards. |
| **Anti-positioning** | Not a casino, not a perp DEX, not a binary options broker, not a tap-to-earn farming game. |

### Design language

**Primary aesthetic — anime betting parlor** (the energy)
The product is a game, and the brand has to *feel* like one. Kaiji / Akagi / Death Parade lineage — dramatic stakes, tension lines (ザワ…ザワ…), a charismatic narrator. The grid + price line is inherently dramatic; the brand frames it.

- **Type**: Space Grotesk (numbers, multipliers, sector readouts) + Inter (UI/body)
- **Palette**:
  - Asphalt black `#0A0A0A` (canvas)
  - Hot pink `#FF2D7E` (primary accent — Euphoria-adjacent but cleaner)
  - Win green `#00FF88`
  - Loss red `#FF3030`
  - Info blue `#00B8FF`
  - Data white `#FFFFFF`
- **Mascot**: "Tick-kun" — narrator energy. Appears on tutorials, big wins/losses, daily quest unveils, share cards.
- **Grid aesthetic**: a glowing lattice. Cells subtly pulse with their multiplier value. The price line draws live with a comet trail. When a cell lights up (touch), tension lines + sound.
- **Share cards**: anime portrait + multiplier badge + verifiable share link.

**Secondary aesthetic — F1 telemetry** (the chrome, in pro mode)
F1 telemetry stays in the codebase for "pro mode" — a toggle for the sophisticated trader segment. Race-line motion blur, sector splits on the expiry countdown, telemetry overlays. Hidden by default; unlocked by reaching Tier 3+.

**What we are NOT**:
- ❌ Bloomberg-terminal-on-acid (saturated; Photon, GMGN, BullX, Padre)
- ❌ Hamster Kombat-style cute (devalues the integrity pitch)
- ❌ Generic AI-slop crypto (purple gradients on white)

---

## 4. Users

### Primary personas

| Persona | Profile | Primary goal | Surface |
|---|---|---|---|
| **The Casual Tapper** | 18–30, mobile-native, holds <$500 crypto or none, knows Hamster Kombat | Fun, brag, accumulate points expecting a TGE | Mobile PWA, web |
| **The Chart Watcher** | 22–35, watches crypto charts daily, never gambled but enjoys "skill" feel | Test reading skill on real prices, no money risk | Web, mobile PWA |
| **The Streamer / Creator** | Twitch/Kick/YouTube crypto creators, 1K–100K followers | Run live tap-trade sessions with embedded widget | Embedded widget + web |
| **The Geo-Local Tapper** | 18–35 in IN/ID/VN/BR/NG/TR, low/no banking | Earn potential token without depositing money | Mobile PWA |

### Phase 5+ personas (post-token / real-money mode)
- The Sui Degen — wants leverage on Sui memecoin price moves
- The Yield Seeker — wants to be the house (re-introduces vault)
- The Returning Euphoria/SWIM player — wants the same game with better onboarding

### Out-of-scope personas (Phase 1–4)
- US retail (geo-blocked at frontend for real-money mode; v1 points-only is permissive)
- Institutional / structured-product builders (Mysten's first-party Predict app)

---

## 5. Competitive Landscape

### Direct competitors

| Competitor | Chain | Mechanic | Money | Tick advantage |
|---|---|---|---|---|
| **Euphoria** | MegaETH | Touch grid, 5s windows | Real USDC, in-house MM | Points-only (broader funnel), zkLogin (smoother onboard), Sui ecosystem support, mobile-first |
| **Pacifica SWIM** | Solana | Touch grid, sec windows | Real USDC, house-banked | Same, plus: SWIM is desktop-bundled inside their perp DEX — we are mobile-first standalone |
| **Pandora Finance** (Sui) | Sui | European binary, 5-min | Real USDC | Different mechanic (touch grid vs binary UP/DOWN), broader brand, mobile/TG-native |
| **Mysten 1st-party Predict app** | Sui | Binary UP/DOWN (likely) | Real USDC | Different audience (consumer/casual vs pro/LP), different mechanic, complementary |
| **Hamster Kombat / Notcoin** | TON | Random taps for points | Points → TGE | Same retention loop, but our taps are math-driven not RNG — chart-reading skill matters |
| **OKX Racer** | Telegram | BTC up/down marketing toy | Free, prizes | Real product depth, not a marketing funnel |
| **Quotex / Pocket Option** | Web2 brokers | Real-money UP/DOWN | Real money, broker = counterparty | Verifiable oracle, points-first onboarding, no withdrawal gates |

### Differentiation matrix

| Feature | Tick v1 | Euphoria | Pacifica SWIM | Pandora | Hamster Kombat |
|---|---|---|---|---|---|
| Touch-settlement grid | ✅ | ✅ | ✅ | ❌ (binary) | ❌ |
| Real oracle-driven multipliers | ✅ | ✅ | ✅ | ✅ | ❌ |
| Points-only (no money at risk) | ✅ | ❌ | ❌ | ❌ | ✅ |
| zkLogin / silent onboarding | ✅ | ❌ (Privy) | ❌ | ❌ | ⚠️ TG only |
| Sponsored gas | ✅ (Enoki) | ⚠️ (Privy paymaster) | ❌ | ❌ | n/a |
| Mobile-first PWA | ✅ | ✅ | ❌ (desktop) | ❌ | ✅ |
| Verifiable points snapshots on-chain | ✅ | ❌ | ❌ | n/a | ❌ |
| Path to token | Sui TGE Phase 5+ | Roadmapped | Roadmapped | Unknown | TGE happened |
| Real-money mode | Phase 5+ | Live | Live | Live | n/a |

---

## 6. Product Principles

1. **Real math, not random rewards.** Multipliers come from a published touch-probability formula. Users can verify the math. This is the integrity moat vs Hamster Kombat-class games.
2. **Locked at tap.** The multiplier the user sees at the moment of tap is what they get paid. Never re-priced. This is the trust contract — copied from Pacifica's explicit UX guarantee.
3. **No money, big stakes.** Points must feel valuable even without dollars. Scarcity (finite per-user balance), streaks, leaderboards, share cards, and the credible token path all do this work.
4. **Mobile first.** Every feature works on 360px before web. Desktop is second priority.
5. **Premium design.** Anime betting parlor primary. Atmospheric, dramatic, not cute. Anti-AI-slop.
6. **Silent onboarding.** zkLogin → playing in <10 seconds. No seed phrase. No popup per tap. No gas confirmation.
7. **Surface the integrity.** Show the oracle source. Show the formula. Show the Sui snapshot. *"Built on Pyth and Sui"* is a brand asset, not a footnote.
8. **Two products, one stack.** v1 ships points game. Same engine powers future real-money mode without rewriting.

---

## 7. Feature Pillars

```
8. Token & Real-Money Mode  ─── $TICK TGE, vault, real-money grid (Phase 5+)
7. Education & Pro Mode     ─── Paper-mode tutorials, F1 chrome unlocks
6. Creator & Distribution   ─── Streamer widget, Discord bot
5. Points Economy           ─── Streaks, daily quests, squad battles
4. Social Layer             ─── Profiles, leaderboards, share cards, Mirror Mode
3. Game Loop UX             ─── Grid render, tap flow, settle reveal, multi-asset
2. Pricing Engine           ─── Brownian-bridge touch math + EWMA oracle + τ-floor curve
1. Core Engine              ─── Sui Move (Phase 3+) + Pyth + off-chain settler
```

MVP ships pillars 1–4 fully and pillars 5–6 partially.

---

## 8. MVP Scope (Phase 1)

**MVP goal**: Ship a web + mobile PWA client that proves the points-game loop on Sui testnet. Submit Sui Overflow 2026. Build 5K-user funnel in 60 days.

### MVP feature requirements

| ID | Feature | Description | Priority |
|---|---|---|---|
| MVP-01 | zkLogin sign-in | Google/Apple/Twitter via Sui Enoki | P0 |
| MVP-02 | Asset selector | 3 assets at launch: ETH, BTC, SOL (Pyth feeds) | P0 |
| MVP-03 | Live price grid | Y = globally-anchored strike ladder (Δ$0.5 ETH, Δ$10 BTC, Δ$0.1 SOL); X = 4 columns × 5s on mobile / 6 on desktop, clock-aligned to wall clock | P0 |
| MVP-04 | Live multiplier per cell | Hui+BGK touch math + τ-dependent floor `1.30+0.01·τ`; refreshed at 10 Hz client-side; lock-at-tap with PENDING→LOCKED→SETTLED lifecycle | P0 |
| MVP-05 | Live price line | Drawn left→right across grid, comet trail; subscribes to oracle aggregator WS at 20 Hz | P0 |
| MVP-06 | Tap cell | Single tap mints a position (DB row); server validates within 3% drift; multiplier locked at server value | P0 |
| MVP-07 | Touch settlement | Off-chain worker watches oracle; cell lights up when price enters band during `[t_open, t_close]`; points credited within 500 ms p95 | P0 |
| MVP-08 | Multi-tap per column | User can tap multiple cells before column expires; each is independent (no stake aggregation); tap blocked in final 1 s before close | P0 |
| ~~MVP-09~~ | _Removed: energy meter. Pacing handled by balance + per-user rate limit instead — see §15.3._ | — |
| MVP-10 | Points wallet + history | Balance display, last 50 taps with cell, multiplier, outcome | P0 |
| MVP-11 | Daily leaderboard | Top 20 by 24h points won. Refreshed every 60s. | P0 |
| MVP-12 | Streak tracker | Consecutive winning taps; visible counter; bonus multiplier on streaks ≥5 | P1 |
| MVP-13 | Share card | Auto-PNG with winning streak / biggest tap / multiplier; share to X | P1 |
| MVP-14 | Daily quest | 3 quests/day (e.g. "tap 20 cells", "win 5x in a row", "tap a >10x cell") | P1 |
| MVP-15 | Mobile PWA install flow | Add-to-home-screen prompt, manifest, service worker. (Push: best-effort on iOS 16.4+ PWA; not required for acceptance.) | P0 |
| MVP-16 | Brand identity live | Anime primary aesthetic per §3, including Tick-kun mascot | P0 |
| MVP-17 | Pro mode toggle (locked) | Visible in settings, gated behind Tier 3 (Phase 2 unlock). MVP ships only the toggle UI + locked state; F1 chrome rendering is Phase 2. | P2 |
| MVP-18 | Math explainer page | "How are multipliers computed?" — open math, with link to formula | P0 |

### Out of scope for MVP
- Native mobile apps (iOS / Android) — PWA only in v1
- Mirror Mode (copy trading)
- Tournaments
- Squad battles (Phase 2)
- Streamer widget
- Real-money mode
- Token, governance
- Multi-language (English only)
- F1 pro-mode chrome (visible toggle only; chrome unlocked Phase 2)

### MVP acceptance criteria

A stranger should be able to:
1. Land on tick.xyz, see the grid alive within 5 seconds
2. Sign in with Google in <10 seconds via zkLogin
3. Tap their first cell within 30 seconds of arrival, with the price line already moving
4. See cells light up (or not) within 10 seconds
5. View the math explainer in 2 taps
6. Share a winning streak with a verifiable link in 2 taps

---

## 9. Roadmap

### Phase 1 — MVP (now → mid-July 2026)
Web + mobile PWA. 3 assets. Grid mechanic. Streaks + daily quests. Daily leaderboard. Submit Sui Overflow 2026.

> **Overflow build addendum (ADR-0010/0011):** a testnet on-chain mode is
> pulled forward from Phase 5 into the hackathon build — a Circle-USDC
> `tick_vault` with touch-settled payouts and per-tap Walrus proofs,
> running **parallel** to the points loop. This does not change the
> points-first funnel; it adds an on-chain, real-(testnet)-money mode and
> the verifiability story. Primary Overflow track: **DeFi & Payments**
> (programmable money primitive); Walrus integration is a value-add.

### Phase 2 — Retention layer (Aug–Sep 2026)
- Squads (weekly squad leaderboard)
- Tournaments (weekly, with token-redeemable prize pool tracked on-chain)
- F1 pro-mode chrome unlocked at Tier 3
- 10 assets total (add SUI, DEEP, WAL, DOGE, PEPE, SHIB, WIF)
- Multi-language: EN, ES, PT, RU, ID, VI, TR
- Mirror Mode (verifiable copy-tap)
- 5 streamer partnerships in pipeline

### Phase 3 — Sui-anchored snapshots, native mobile (Oct–Dec 2026)
- Weekly on-chain merkle snapshot of `(user, points, week)` published to Sui contract
- Anyone can verify their points history with a merkle proof
- Native iOS + Android apps (React Native, shared codebase)
- Streamer Widget SDK + first 10 streamer partners live

### Phase 4 — Optional: DeepBook Predict slow-cadence companion (Q1 2027)
- Add a "Markets" tab inside Tick using DeepBook Predict (15-min to 28-day prediction markets)
- Polymarket-on-Sui product, same brand, different mechanic
- Sui mainnet Predict launch happens in this window — we ride it
- Bridges to real-money mode

### Phase 5 — $TICK token + real-money mode (Q2–Q3 2027)
- Retroactive airdrop weighted by points history (merkle proofs from Phase 3 snapshots)
- Real-money tap-trading mode launches in parallel:
  - Same grid UI
  - User-funded USDC balance
  - Off-chain settlement → on-chain receipt per session
  - LP vault (sophisticated users supply USDC, earn from tappers' losses)
  - Audited Move contracts
- $100K USDC mainnet tournament prize pool
- Two surfaces, one product: free-points mode (the funnel) + real-money mode (the monetization)

### Phase 6+ — Geo expansion (Q4 2027+)
- India (UPI on-ramp), Brazil (PIX), Indonesia (Dana), Vietnam (VNĐ), Nigeria (M-Pesa)
- Local-language community channels (Discord / X / regional platforms; no Telegram)
- Local KOL programs

---

## 10. Detailed Requirements — MVP Features

### MVP-01 · zkLogin sign-in
**User story**: As a new user, I want to sign in with my Google/Apple/Twitter so I can start playing immediately without crypto knowledge.

**Acceptance**:
- Sui Enoki SDK; supports Google, Apple, Twitter, Facebook
- Sign-in flow ≤ 10 seconds from tap to grid loaded
- No seed phrase, no wallet popup
- Failed sign-in shows clear error + retry

### MVP-02 · Asset selector
**User story**: As a player, I want to pick which asset I'm betting on.

**Acceptance**:
- 3 assets: ETH/USD, BTC/USD, SOL/USD (Pyth feed IDs in `ORACLE_SPEC.md`)
- Each row shows: name, current price, 24h change %, "active tappers" count
- Tap switches grid, multipliers, price line atomically
- Default on first load: ETH/USD

### MVP-03 · Live price grid
**User story**: As a player, I want to see a clear grid of future cells I can tap.

**Acceptance**:
- Y-axis: globally-anchored strike ladder, asset-specific tick size (ETH Δ$0.5, BTC Δ$10, SOL Δ$0.1)
- X-axis: 4 columns × 5s on mobile (20s look-ahead), 6 columns × 5s on desktop (30s), clock-aligned to wall clock
- Current price marker (horizontal line) clearly visible
- Cells render multiplier centered, larger font for higher multipliers
- Cells closer to spot: low multiplier (1.5–3×); far OTM: high multiplier (50–100×)
- Cells highlight on hover (web) / press (mobile)
- Scroll vertically reveals more strikes
- Visible: about 12 strikes × 4 columns (mobile) or × 6 columns (desktop) = 48–72 cells at any moment

### MVP-04 · Live multiplier per cell
**User story**: As a player, I need to know what payout I'm betting for before tapping.

**Acceptance**:
- Per cell, compute multiplier = `max(floor(τ), (1 − house_margin) / P_touch(cell))`; `floor(τ) = 1.30 + 0.01·τ`
- `P_touch` from Brownian-bridge first-touch formula (see §13 — Math & Pricing Engine)
- Refresh 10 Hz (every 100ms), client-side in JS, against the latest oracle tick
- When user taps, capture the displayed multiplier server-side as `multiplier_at_tap`; lock for the rest of the cell's lifetime
- Smooth interpolation between refreshes (no flicker)
- House margin: `0.10` (10%) for v1; tunable

### MVP-05 · Live price line
**User story**: As a player, I want to watch the chart move in real time.

**Acceptance**:
- Price line drawn left to right across the grid
- Updates as oracle aggregate price updates (~4–10Hz target via aggregated WS — see §14)
- Comet trail effect: last 30s of ticks fade out
- Color: white with hot-pink glow on direction change
- When line enters a cell with an active position → cell flashes green + tension lines + sound (Hamster Kombat-style impact frame)

### MVP-06 · Tap cell
**User story**: As a player, I want to commit a prediction with one tap.

**Acceptance**:
- Tap = position created as a DB row (NOT an on-chain tx)
- Body: `(user_id, cell {asset, strike_lo, strike_hi, t_open_ms, t_close_ms}, multiplier_at_tap, oracle_seq_at_tap, stake_points, ts_tapped)`
- Stake debited from balance immediately; tap rejected if balance < stake
- Per-user API rate limit: 10 taps/sec hard ceiling (token bucket); tap rejected with 429 if exceeded
- Stake: variable from day 1 — selectable from `{50, 100, 500, 1000}` at Tier 1; larger stakes unlock at higher tiers
- Multi-tap allowed: user can tap the same cell multiple times (each is INDEPENDENT — no stake aggregation), or tap multiple cells in the same column
- Confirmation: optional "double-tap to confirm" setting; default off (one-tap commit for speed)
- Tap lifecycle: render PENDING badge with stake + "—" multiplier during ~100–300 ms server roundtrip; replace with LOCKED badge on success; show error toast and clear PENDING on stale_quote / drift / rate-limit errors

### MVP-07 · Touch settlement
**User story**: As a player, I want my winning taps credited the instant the price line enters my cell.

**Acceptance**:
- Off-chain worker subscribes to oracle WS aggregate (see §14)
- Per oracle tick: check all open positions; if `tick.price ∈ [strike_lo, strike_hi]` and `tick.ts ∈ [t_open, t_close]` → mark `touched=true`, credit `stake × multiplier_at_tap` points
- If column expires (`now > t_close`) without touch → position resolves as loss; no points credited
- Settlement is at-most-once: idempotent on `position_id`
- Latency from oracle tick to UI flash: target <500ms p95

### MVP-08 · Multi-tap per column
**User story**: As a power player, I want to tap multiple cells before the column expires.

**Acceptance**:
- No limit on cells per column other than available balance and per-user 10 taps/sec rate limit
- Each tap is a separate position with its own `multiplier_at_tap` (no stake aggregation per cell)
- Cells the user has tapped show a small "you've tapped X times" badge
- Tappable until `t_close − 1s` (lock window 1 second before expiry to prevent oracle front-run)

### MVP-10 · Points wallet + history
**User story**: As a player, I want to see my points and verify any past tap.

**Acceptance**:
- Top bar: points balance, with delta animation on every credit
- "History" tab: last 50 taps, paginated
- Per row: time, asset, cell (band + window), multiplier_at_tap, outcome (win/loss), points delta
- Filter by asset and outcome
- Phase 3+: "Verify on Sui" button (uses weekly merkle snapshot)

### MVP-11 · Daily leaderboard
**User story**: As a competitive player, I want to see how I rank.

**Acceptance**:
- Top 20 by 24h net points won
- Per row: rank, display name (zkLogin name or anon ID), points, win rate, biggest multiplier
- Refreshed every 60s
- "Your rank" pinned at bottom if user is in top 200
- Tap row → trader profile view (read-only in MVP)

### MVP-12 · Streak tracker (P1, may slip)
**User story**: As a player, I want a visible streak counter to push me to keep going.

**Acceptance**:
- Counter visible above grid
- Increments on each winning tap; resets on losing tap
- Visual milestones at 5, 10, 25 streaks
- Streak ≥5 unlocks a bonus multiplier (`1.0 + 0.02 × streak`, capped at 1.5×) — applied at credit time, audited in history

### MVP-13 · Share card (P1, may slip)
**User story**: As a winner, I want to brag without sounding fake.

**Acceptance**:
- After a big tap (multiplier ≥10× OR streak milestone), "Share" button appears
- Card shows: asset, cell, multiplier, points won, streak (if applicable), Tick logo, Tick-kun art
- One-tap share to X, TG, copy link
- Link points to `tick.xyz/t/{tap_id}` (Phase 3+: verifiable via merkle proof)

### MVP-14 · Daily quest (P1, may slip)
**User story**: As a returning player, I want a reason to log in today.

**Acceptance**:
- 3 quests per day, rotating from a pool
- Examples: "tap 20 cells", "win 5 in a row", "tap a cell with multiplier ≥10×", "win 1000 points in one session"
- Reward: 500 points per quest, plus quest-complete share card
- Persistent across devices (web + PWA, same account via zkLogin)

### MVP-15 · Mobile PWA install flow
**User story**: As a mobile user, I want to install Tick to my home screen and use it like a native app.

**Acceptance**:
- Valid `manifest.json` with name, short_name, icons (192/512), theme_color, display=standalone
- Service worker registered for offline shell + asset caching
- "Add to Home Screen" prompt appears after 2 taps + 30 seconds (iOS Safari + Android Chrome)
- Push notification permission requested on first daily-quest claim
- After install: launches in standalone mode, splash matches brand
- Lighthouse PWA score ≥ 90

### MVP-16 · Brand identity
**User story**: The product looks like a coherent, premium game.

**Acceptance**:
- Anime betting parlor primary aesthetic per §3
- Tick-kun mascot on splash, big-win modal, daily quest reveal, share cards
- Palette: black + hot pink + win green + loss red
- Grid: glowing lattice with subtle pulse on multipliers
- Type: Space Grotesk for numbers, Inter for UI
- Motion: snap-to-grid; bouncy on big wins only

### MVP-18 · Math explainer page
**User story**: As a skeptical user, I want to verify the math behind the multipliers.

**Acceptance**:
- Static page `/how-it-works`
- Sections: How we compute multipliers (formula visible), Where our prices come from (Pyth + EWMA aggregation), How we settle (oracle tick → DB credit), How points map to future tokens (Phase 5 plan)
- Plain language explanation of Brownian-bridge first-touch
- Link from main UI footer + onboarding
- Link to GitHub repo (open-sourced pricing engine, Phase 2+)

---

## 11. Critical User Flows

### Flow A — First-time visitor → first tap
```
Land on tick.xyz  (≤3s load)
  └─→ Splash with Tick-kun + "Tap your first prediction"
      └─→ zkLogin (Google, 5s)
          └─→ Grid loads with live price line moving;
              multipliers refresh at 10 Hz
              ├─→ Onboarding overlay: highlights a cell, "tap this"
              │   └─→ Tap → PENDING badge (stake + "—")
              │       └─→ ~200ms later → LOCKED badge (stake + mult)
              │           └─→ 5–10s later: cell flashes green OR fades
              │               └─→ Points credited (or not) with delta animation
              │                   └─→ "Tap another?" CTA + daily quest unlock
```
**Target**: 70% of first-time visitors place a tap within 60s of landing.

### Flow B — Returning player → daily check-in
```
Open app  (cached session)
  └─→ Daily quest banner: "3 quests today"
      └─→ Tap into first quest
          └─→ Grid loads with full balance
              └─→ Tap cells, build streak
                  └─→ Streak ≥5 → bonus multiplier + share card prompt
                      └─→ Share to X → friend taps link → returns A
```
**Target**: 40% D1 retention, 25% D7, 10% D30.

### Flow C — Power player → leaderboard climb
```
Sign in (cached)
  └─→ Leaderboard widget: "you're rank 47, top 20 = 12,000 pts"
      └─→ Tap "Climb" → grid view with target overlay
          └─→ Strategic tapping (higher-multiplier cells)
              └─→ Mid-session: check rank again
                  └─→ Reach top 20 → notification + share card
```
**Target**: 20% of returning users check leaderboard each session.

---

## 12. Technical Architecture

### Stack overview

```
┌──────────────────────────────────────────────────────────────────┐
│ FRONTEND — Web + Mobile PWA, one Next.js codebase                │
│  ├─ Web (Next.js 15 + RSC)                                        │
│  └─ Mobile PWA (same Next.js, installable, push-capable)         │
│                                                                   │
│  Auth:        zkLogin via Enoki                                  │
│  State:       Zustand                                            │
│  Charts:      Custom canvas/SVG grid (not TradingView)           │
│  Oracle WS:   Multi-source aggregator at 20 Hz                   │
│  Multipliers: Recomputed client-side at 10 Hz                    │
└────────────────────────────┬─────────────────────────────────────┘
                             │  REST/WS
                             ▼
┌──────────────────────────────────────────────────────────────────┐
│ TICK API (Rust + axum)                                            │
│  ├─ Position registry (Postgres)                                  │
│  ├─ Points wallet + ledger (Postgres)                             │
│  ├─ Session + rate-limit buckets (Redis)                          │
│  ├─ Daily quests + streaks (Postgres)                             │
│  ├─ Leaderboard cache (Redis, sorted-set top-N)                   │
│  ├─ Auth verifier (Sui zkLogin proof)                            │
│  └─ Share card renderer (Cloudflare Workers or local Satori)     │
└────────────────────────────┬─────────────────────────────────────┘
                             │
       ┌─────────────────────┼─────────────────────┐
       ▼                     ▼                     ▼
┌─────────────┐     ┌─────────────────┐     ┌─────────────────┐
│ SETTLEMENT  │     │ ORACLE          │     │ SUI ANCHOR      │
│ WORKER      │     │ AGGREGATOR      │     │ (weekly)        │
│ (Rust)      │     │ (Rust)          │     │ (Move pkg)      │
│             │     │                 │     │                 │
│ Subscribes  │     │ Pyth Hermes WS  │     │ Weekly merkle   │
│ to oracle   │     │   + Binance WS  │     │ snapshot of all │
│ feed; per   │     │   + Bybit WS    │     │ users + points  │
│ tick, scans │     │   + OKX WS      │     │ published as    │
│ open posns; │     │ → EWMA blend    │     │ root in Sui     │
│ credits     │     │ → broadcast to  │     │ Move contract.  │
│ points; idem│     │   API + clients │     │ Users prove via │
│ per tap.    │     │                 │     │ merkle paths.   │
└─────────────┘     └─────────────────┘     └─────────────────┘
                                                       │
                                                       ▼
                                              ┌─────────────────┐
                                              │ SUI MOVE PKG    │
                                              │ tick_anchor    │
                                              │                 │
                                              │ - publish_root  │
                                              │ - verify_proof  │
                                              │ - epoch counter │
                                              │ (Phase 3+)      │
                                              └─────────────────┘
```

### What's on-chain vs off-chain

| Concern | Location | Why |
|---|---|---|
| User identity (zkLogin) | Sui | Native Sui primitive; no seed phrase; preserves token-launch path |
| Points balance & ledger | Postgres | Per-tap on-chain writes are economically and operationally absurd at 100K+ taps/day |
| Settlement (touch detection) | Off-chain Rust worker | Oracle sub-second feed isn't on-chain on Sui at sufficient cadence; this is also free |
| Multiplier computation | Client-side JS | Anyone can verify; published formula in §13 |
| Weekly snapshot | Sui Move contract | Merkle root of points history; verifiability without per-tap cost |
| Future token (TGE) | Sui | Standard Sui token; airdrop weighted from merkle snapshots |
| Real-money mode (Phase 5+) | Hybrid | Vault as Move object, off-chain settlement with periodic on-chain reconciliation |

### Move contracts (minimal in v1)

```
move/tick_anchor/
├── Move.toml
└── sources/
    ├── anchor.move           // publish_root(epoch, root); read root by epoch
    ├── proof.move            // verify_merkle_proof for (user, points, epoch)
    └── events.move           // RootPublished, ProofVerified
```

Total LOC target: ~300. No audit blocking issue (it only stores merkle roots).

### Repo structure (target)

All paths below are relative to `games/tap-trading/`. The whole game is self-contained inside that directory — backend Rust crates live as a single Cargo workspace under `backend/`; they do **not** join the platform-wide workspace at `platform/`.

```
backend/                          // Rust workspace (Cargo.toml at this level)
├── api/                          // axum service (REST + WS)
├── oracle-aggregator/            // multi-WS → EWMA → broadcast
├── settlement-worker/            // oracle ticks → DB credits
├── anchor-publisher/             // Phase 3+ — weekly merkle root publisher
└── pricing-engine/               // canonical Rust crate (shared lib)
apps/
├── web/                          // Next.js 15 — Web + Mobile PWA
└── mobile/                       // Phase 3 — React Native + Expo
packages/
├── ui/                           // Shared TS components (anime + F1 modes)
├── pricing-engine-ts/            // thin TS port of backend/pricing-engine; 10 Hz client recompute
├── oracle-client/                // Browser subscription to aggregator
└── sui-client/                   // zkLogin, anchor reads
move/
└── tick_anchor/                  // Sui Move package (Phase 3+ deploy)
```

---

## 13. Math & Pricing Engine

**This is the IP. Get this right and the game feels honest. Get it wrong and it feels like a slot machine with extra steps.**

### 13.1 What we're pricing

Each cell `c` is a vertical-range one-touch barrier option:

> Cell `c` wins iff `spot(t) ∈ [L_c, H_c]` for some `t ∈ [t_open^c, t_close^c]`.

We compute the **fair probability of touch** `P_touch(c)` and convert to a multiplier:

```
multiplier(c) = (1 − house_margin) / P_touch(c)
```

with `house_margin = 0.10` in v1. (Calibratable; see §13.5.)

### 13.2 The closed-form formula

Under geometric Brownian motion `dS/S = μ dt + σ dW` with `μ ≈ 0` over short windows:

**Probability of *no* touch** (band `(L, H)` over `[0, τ]`, starting at `S_0`):

Using Hui (1996) — "One-Touch Double Barrier Binary Option Values", `Applied Financial Economics` 6:343–346 — and Haug's *Complete Guide to Option Pricing Formulas* (2nd ed., p. 180):

```
P_no_touch(S_0, L, H, σ, τ) ≈
  Σ_{n=1}^{N}  [2πn / Z²] · [(S_0/L)^α − (−1)^n (S_0/H)^α] / (α² + (nπ/Z)²)
              · sin(nπ · ln(S_0/L) / Z)
              · exp(−½ ((nπ/Z)² − β) σ² τ)

  Z = ln(H/L)
  α = −½ (2(r−q)/σ² − 1)
  β = −¼ (2(r−q)/σ² − 1)² − 2r/σ²
  N = 10–20 (truncation)
```

For sub-minute cells with `r ≈ q ≈ 0`: `α = ½`, `β = −¼`. The series converges in ~5 terms.

**Probability of touch:** `P_touch = 1 − P_no_touch`.

### 13.3 The Broadie–Glasserman–Kou continuity correction (mandatory)

Our oracle is discretely sampled (200–500ms gap between ticks). Continuous-monitoring formulas **overprice** the touch probability vs the discretely-monitored reality. Without correction we systematically over-pay users.

Per Broadie, Glasserman, Kou (1997) *Mathematical Finance* 7(4):325–349:

```
H_corrected = H · exp(+β · σ · √(τ/m))      // widen upper barrier
L_corrected = L · exp(−β · σ · √(τ/m))      // widen lower barrier
β = −ζ(½) / √(2π) ≈ 0.5826
m = number of monitoring ticks in window  (e.g. τ=5s, 50ms ticks → m=100)
```

Apply this shift, then run the Hui formula on `(L_corrected, H_corrected)`. This is the most-skipped correction in barrier-option implementations; it is the #1 reason naïve implementations bleed money on a real product.

### 13.4 Volatility estimation

We need a `σ` per asset, refreshed continuously.

**v1 estimator** (simple, robust):
```
σ̂_t = sqrt( EWMA(r_i²) ) · √(seconds_per_year)
  r_i = ln(p_i / p_{i-1})        // 1-second log returns
  EWMA with λ = 0.94             // RiskMetrics standard
```

Recompute every second from the oracle stream. Use as `σ` in Hui formula.

**v2 upgrade**: Garman-Klass on Pyth EMA + 5-min realized vol from window high/low. Add jump-buffer multiplier `1.3` to widen for fat tails (crypto is leptokurtic at sub-minute scale; pure GBM underprices touches near the band edge).

**Where the math breaks** — and our defense:
- Vol regime shift (CPI announcement, exchange hack) → our `σ̂` lags reality → cells are mispriced for ~minutes. **Defense in points game: no vault to drain — worst case is users tap a profitable mispriced window and accumulate points.** Acceptable.
- Jumps (liquidation cascades, single-block MEV) → return distribution has fat tails GBM misses. **Defense: jump-buffer multiplier in v2. v1 absorbs the cost — points only.**
- Oracle gap (Pyth feed stalls 5s) → `σ̂` becomes meaningless. **Defense: pause new taps if `now − last_oracle_tick > threshold` (default 2s).**

### 13.5 House margin & calibration

`house_margin = 0.10` in v1 means cells pay `0.90 / P_touch` instead of `1.0 / P_touch`. This is the "rake" — it doesn't fund a real vault (points game), but it shapes the user experience:
- Multipliers feel slightly less generous than purely fair odds (expected)
- Long-run points distribution: total points earned < total points wagered by 10% per tap
- Without rake, the game has no edge — points accumulate unboundedly and the eventual token would be meaningless

For v1, we tune `house_margin` empirically (full runbook in `MATH_SPEC.md §6.3`):
1. Run shadow mode: simulate cell payouts with `margin ∈ {0.05, 0.08, 0.10, 0.12, 0.15}`
2. Pick the margin where median user has slightly negative points over 100 taps but variance is high enough to feel rewarding (target: 30% of users net positive over a 1-hour session)

### 13.6 Reference implementation

We will ship the canonical Rust crate at `backend/pricing-engine/` (used server-side) and a thin TypeScript port at `packages/pricing-engine-ts/` (used by the frontend for 10 Hz local recompute). Both expose the same surface:
- `computeNoTouch(S0, L, H, sigma, tau, opts)` — Hui series, configurable N
- `applyBGKCorrection(L, H, sigma, tau, m)` — barrier shift
- `computeMultiplier(cell, oracleState, opts)` — main entry point
- `estimateRealizedVol(ticks, lambda)` — EWMA
- Unit tests against QuantLib's `AnalyticDoubleBarrierBinaryEngine` outputs

Open-sourced on GitHub at Phase 2 — part of the integrity story.

---

## 14. Oracle Architecture

### 14.1 Why not just Pyth?

Pyth on Sui is excellent for on-chain settlement (1 MIST fee, 400ms cadence, signed VAAs) — but for **a smooth live UX**, raw Pyth ticks are too jittery:
- Pyth publishes per-slot aggregates: every ~400ms there's a discrete jump
- Single-publisher outlier can momentarily skew the median
- Confidence interval spikes at news events
- A jittery price line + flickering multipliers = "feels rigged"

Pacifica SWIM solved this with EWMA across CEX + DEX. We copy.

### 14.2 The aggregator design

Off-chain Rust service (`backend/oracle-aggregator/`):

```
INPUTS (parallel WS subscriptions per asset):
  ├─ Pyth Hermes SSE stream  /v2/updates/price/stream      ← signed integrity anchor
  ├─ Binance Spot WS         /ws/{symbol}@aggTrade          ← high-volume reference
  ├─ Bybit Spot WS           /v5/public/spot                ← redundancy
  └─ OKX Spot WS             /ws/v5/public                  ← redundancy

PROCESSING (per asset, every ~50ms):
  1. Collect latest tick from each source
  2. Reject sources with stale data (>1s since last update) — soft fail
  3. Compute median of remaining sources (robust to single outlier)
  4. EWMA smooth: p_smooth = 0.7 · median + 0.3 · prev_smooth
  5. Broadcast to clients + settlement worker

OUTPUTS:
  ├─ WebSocket fanout to all connected clients
  └─ Internal channel → settlement worker
```

### 14.3 Cadence target

| Path | Target |
|---|---|
| External feeds → aggregator | as fast as each source pushes (50–200ms typical) |
| Aggregator → clients | 50ms (20 Hz) |
| Aggregator → settlement worker | every tick |
| UI render | 60fps (price line interpolates between ticks) |

### 14.4 Settlement integrity & verifiability

**Question**: if everything is off-chain, how is this verifiable?

**Answer (layered, v1 → Phase 3+)**:
1. **Pyth is in the aggregator.** Pyth's signed feed is one of four sources contributing to every emitted tick. Removing or spoofing Pyth would change the on-chain-attested median visibly; it's the integrity anchor inside the aggregation.
2. **Math is open.** The pricing engine (Hui + BGK + τ-floor curve) and aggregator (median + EMA) source code are public from Phase 2; anyone can verify multiplier correctness against the documented formula in `MATH_SPEC.md`.
3. **Weekly snapshot on Sui** (Phase 3+) commits a merkle root of `(user, total_points, weekly_won, weekly_lost)`. Users can prove their points history with a merkle path against the on-chain root.

This isn't tx-level on-chain proof. It's "audit-grade" verifiability via the weekly snapshot — cheap enough to ship a points game, strong enough to defend against "you rigged it" claims. Per-settlement on-chain proof is deferred to Phase 5 (real-money mode), when the integrity stakes warrant the cost.

### 14.5 Anti-manipulation

| Attack | Defense |
|---|---|
| Single CEX feed manipulated | Median across 3+ sources |
| Pyth confidence spike | If `pyth.conf / pyth.price > 100 bps`, drop Pyth from median temporarily |
| Aggregator front-run (user sees tick before settle) | Settlement triggers on aggregator-emitted ticks, not on user-visible ones (server-authoritative) |
| Tap window vs settle window front-run | Lock cell tappability 1s before column expiry (MVP-08) |
| EMA divergence (Pyth's EMA wildly different from spot) | Reject if `|spot - ema| / ema > 200 bps` |

---

## 15. Points Economy

### 15.1 Currency design

Points are an in-game currency, not a token (yet). One unit:
- Stake = points wagered per tap
- Payout = `stake × multiplier_at_tap` (if won) or `0` (if lost)
- Variable stake from day one: selectable from `{50, 100, 500, 1000}` at Tier 1; wider range (5,000+) unlocks at Tier 2 (see §15.6)
- Default UI selection: **100 points** (≈ $1 mental model if we ever map 100pt ↔ $1)

### 15.2 Onboarding bonus

- New zkLogin sign-up: **10,000 points** starting balance
- Daily login bonus: **500 points** (resets at UTC midnight)

### 15.3 Session pacing (no-energy model)

Pacing in v1 is purely balance + rate-limit, no separate energy meter:
- **Finite point balance** — losing taps eat balance; can't tap with insufficient balance.
- **Per-user API rate limit** — 10 taps/sec hard ceiling (token bucket, burst 20).
- **Daily quest gating** — quests reset at UTC midnight, so quest income is time-bounded.

This is the deliberate v1 choice: an energy meter adds a second pacing surface users have to learn before they can tap, and the balance + rate-limit combination already provides natural session shaping without that overhead.

### 15.4 Streaks

- Increments on every winning tap, resets on loss
- Visible counter above grid
- Bonus multiplier on credit: `1 + 0.02 × streak`, capped at `1.5×` (50 winning taps in a row)
- Major milestones (5, 10, 25): visual celebration + share card prompt

### 15.5 Daily quests

3 per day, rotating pool. Examples:
- "Tap 20 cells" — reward 500 pts
- "Win 5 in a row" — reward 750 pts
- "Win a tap with multiplier ≥10×" — reward 1000 pts
- "Net positive 2000 points in one session" — reward 1500 pts
- "Play 3 different assets" — reward 500 pts

### 15.6 Tier progression

| Tier | Threshold (lifetime points won) | Unlocks |
|---|---|---|
| 1 | 0 | Base game |
| 2 | 100,000 | Wider stake range (5,000+), custom share-card frames |
| 3 | 1,000,000 | F1 pro-mode chrome, 6→8 columns visible on desktop, asset slot 4 (SUI) |
| 4 | 10,000,000 | Phase 2: Squad-leader role, Tournament priority |
| 5 | 100,000,000 | Phase 5+: Real-money mode early access |

### 15.7 The TGE path (Phase 5+)

Points → $TICK token via retroactive airdrop:
- Eligibility: any user with verifiable points history (merkle proof from Phase 3 snapshots)
- Weight: weighted lifetime points (more recent weighted higher to penalize farming)
- Sybil defense: zkLogin = one unique identity per provider; behavioral clustering for likely duplicates
- Float: ~30% of supply to points-holders, vesting over 12 months
- Mechanic: claim via Sui contract, verifying merkle proof on-chain

This is what makes points feel valuable from day one. *"Earn points now, claim later"* — Hamster Kombat's exact funnel mechanic.

---

## 16. Success Metrics

### North Star
**Weekly Active Tappers (WAT)** = unique users who placed ≥10 taps in the last 7 days. Captures real engagement, scales across surfaces.

### Phase 1 (MVP) — measured 30 days post-Overflow submission

| Metric | Target | Stretch |
|---|---|---|
| Unique sign-ups | 10,000 | 30,000 |
| Cumulative taps | 1,000,000 | 5,000,000 |
| Weekly Active Tappers (peak) | 2,000 | 8,000 |
| D7 retention | 25% | 40% |
| Twitter followers | 2,000 | 8,000 |
| Discord members | 5,000 | 20,000 |
| Overflow placement | Top 25% of DeepBook track | Win prize |

### Phase 3 (post-snapshot) — 90 days post-launch

| Metric | Target |
|---|---|
| Cumulative sign-ups | 250,000 |
| Cumulative taps | 100,000,000 |
| WAT | 25,000 |
| D30 retention | 12% |
| Native mobile installs | 50,000 |
| Streamer partners live | 10 |

### Phase 5 (TGE + real-money mode) — 90 days post

| Metric | Target |
|---|---|
| Airdrop claimants | 100,000 |
| Real-money mode DAU | 5,000 |
| Real-money daily volume | $1M |
| Vault TVL | $5M |

### Health metrics (always monitored)
- Multiplier display latency p95 (target <500ms)
- Settlement latency p95 (target <500ms from oracle tick → UI flash)
- Oracle aggregator uptime (target 99.9%)
- Anti-cheat flag rate (% of taps with suspicious patterns)
- Bust-state frequency (% of users who hit zero balance in 24h)
- Points distribution Gini (avoid winner-takes-all)

---

## 17. Risks & Mitigations

| # | Risk | Severity | Mitigation |
|---|---|---|---|
| R1 | Mysten ships first-party Predict app that competes for casual users | Medium | Predict app is binary-UP/DOWN, pro-trader oriented; our touch-grid + points + smoother onboarding is a different game. Move fast on community distribution (X, Discord, streamers). |
| R2 | Euphoria / Pacifica SWIM ports to Sui | Medium | First-mover on Sui + zkLogin + points-first is meaningful. Their MegaETH/Solana brand doesn't transfer easily. |
| R3 | Pyth feed degrades / goes offline | High (operational) | Multi-source aggregator (Binance/Bybit/OKX); auto-pause if all sources stale; surface oracle status in UI |
| R4 | Multipliers computed wrong → users hate the game (feels rigged) | High | Open-source pricing engine; math explainer page; QuantLib parity tests; shadow-mode calibration on the τ-floor curve and house_margin |
| R5 | Oracle front-running by sophisticated users | Medium | Server-authoritative settlement, locked tap window 1s before close, `oracle_seq_at_tap` replay |
| R6 | Sybil farming for future token | High | zkLogin = one identity per provider; behavioral anomaly detection (rate, win-rate, fingerprint); retroactive sub-linear weighting of cluster duplicates |
| R7 | Settlement worker bug → wrong credits | High | Idempotent settlement (UNIQUE position_id); single-tx settle; full ledger audit; shadow-mode pre-launch |
| R8 | Sui chain outage | Low | Game runs without Sui (Postgres is system of record); zkLogin auth degrades to cached session |
| R9 | Pacing model too permissive or too punishing | Medium | Tune signup bonus + daily quest income in shadow to keep median user "alive but pressured" over a 1-hour session; add bust-state cooldown if churn data demands |
| R10 | Points feel meaningless without token | High | Day 1 communicate TGE path; leaderboard prestige; share cards; daily quests; eventual concrete airdrop |
| R11 | Brand confusion (anime vs F1, casual vs pro) | Medium | Lock anime primary at MVP; F1 chrome behind Tier 3 unlock; clear copy in onboarding |
| R12 | Open math gets exploited (someone finds an arbitrage) | Low | Math is sound (Hui+BGK); house margin covers normal exploits; v2 jump-buffer for fat tails |
| R13 | Regulatory action on "trading" framing in some jurisdictions | Medium | Points-only, no money in/out, no withdrawal; positioned as game; consult counsel before real-money mode |
| R14 | Vault insolvency under correlated player wins (BTC pump puts most bullish cells in-the-money at once; streamer pile-on on one cell) | High (mainnet); nil (testnet, faucet USDC) | On-chain exposure caps in `tick_vault` (per-cell, directional imbalance, treasury buffer — ADR-0010 §5); external perp-hedging via Sui DeepBook in Phase 5; over-fund treasury on testnet. Hyperliquid-pattern controls. |
| R15 | `SettlerCap` keypair compromise → attacker authorizes fraudulent payouts | Medium | Testnet value is nil; mainnet hardens with a multisig settler + on-chain payout-rate limits (ADR-0010 Forecast). Walrus proofs make any fraudulent settlement publicly detectable after the fact. |

---

## 18. Open Questions (decisions needed before / during build)

| # | Question | Decision needed by | Stakeholders |
|---|---|---|---|
| Q2 | House margin exact value (10%, 8%, 12%)? | Shadow mode result | Eng |
| Q3 | Domain: tick.xyz vs alternates? | **Day-1 standup** (blocks brand assets, share-card link templating, OAuth consent screens) | Product |
| Q4 | Open source contracts + pricing engine at launch, or Phase 2? | MVP | Product |
| Q5 | Audit firm for `tick_anchor` Move package (Phase 3) | Phase 2 | Eng |
| Q6 | Token model: utility (governance + fee discount) or revenue-share burn? | Phase 4 | Product + Legal |
| Q7 | Real-money mode regulatory posture (jurisdictions, structure) | Phase 4 | Legal |
| Q8 | Multi-language priority (which 3 first beyond EN)? | Phase 2 | Product + BD |
| Q9 | Streamer commission economics | Phase 3 | Product + BD |
| Q10 | Squad battle rules (per-day matchup? weekly?) | Phase 2 | Product |

---

## 19. Out of Scope

Explicitly NOT part of Tick — at least not in any planned phase:

- **RNG / casino games** (Crash, Mines, Plinko, Dice, slots). We compete on real-market integrity, not RNG.
- **Sports betting**. Wrong oracle category.
- **Event prediction markets** (Polymarket-style "Will X happen?"). Phase 4+ via DeepBook Predict integration, not v1.
- **Perpetual futures**. Bluefin/Cetus/Pacifica serve this.
- **Spot trading**. DeepBook Spot serves this.
- **Lending / borrowing**. Other Sui protocols serve this.
- **US retail in real-money mode** (Phase 5+ geo-blocked).
- **Fiat custody**. Crypto deposits only when real-money mode ships (Phase 5+).
- **Fully decentralized governance** before $TICK token launch.

---

## 20. Appendix

### A. Glossary

| Term | Meaning |
|---|---|
| **Tap trading** | Mobile-first prediction game where users tap cells on a price-grid; settled when price enters cell (touch). Category defined by Euphoria, Pacifica SWIM, et al. |
| **Cell** | A `(price band × time window)` square on the grid |
| **Touch settlement** | Path-dependent: cell wins the moment spot enters the band, mid-window |
| **European settlement** | Snapshot at expiry; what DeepBook Predict does. Not used here. |
| **Multiplier** | Payout ratio per cell, locked at tap time |
| **Brownian-bridge first-touch** | Closed-form probability that GBM path touches a barrier in a window; basis of our pricing |
| **BGK correction** | Broadie-Glasserman-Kou continuity correction; shifts barriers to account for discrete monitoring |
| **EWMA** | Exponentially-weighted moving average; used for vol estimation and oracle smoothing |
| **zkLogin** | Sui-native auth; sign in with Google/Apple/Twitter, no seed phrase |
| **Enoki** | Mysten's SDK for zkLogin + sponsored transactions |
| **Pyth** | Pull-based price oracle on Sui; 400ms cadence, signed VAAs |
| **Pyth Hermes** | The off-chain WebSocket/REST service that broadcasts Pyth updates |
| **VAA** | Verified Action Approval — Pyth's signed update payload |
| **Merkle snapshot** | Weekly root of points history published to Sui; users prove balance via merkle path |

### B. References (research foundation for this PRD)

**Tap trading category — competitor analysis:**
- Euphoria: https://euphoria.finance, https://docs.euphoria.finance, https://blog.redstone.finance/2026/01/13/euphoria-redstone-bolt/
- Pacifica SWIM: https://pacifica.gitbook.io/docs/swim/swim
- Independent dashboard: https://euphorialens.vercel.app
- Coverage: https://www.theblock.co/post/366016/megaeth-based-crypto-derivatives-trading-app-euphoria-seed-funding, https://www.blocmates.com/articles/euphoria-speculation-simplified-to-a-single-tap

**Math:**
- Hui 1996, "One-Touch Double Barrier Binary Option Values" *Applied Financial Economics* 6:343–346
- Haug 2007, *Complete Guide to Option Pricing Formulas* 2nd ed., McGraw-Hill, pp. 152–180
- Broadie-Glasserman-Kou 1997, *Mathematical Finance* 7(4):325–349 — https://www.columbia.edu/~sk75/mfBGK.pdf
- Reflection principle, Brownian-bridge identity: https://almostsuremath.com/2023/04/18/the-maximum-of-brownian-motion-and-the-reflection-principle/
- QuantLib reference impl: `AnalyticDoubleBarrierBinaryEngine`

**Pyth on Sui:**
- Docs: https://docs.pyth.network/price-feeds/contract-addresses/sui
- Hermes API: https://hermes.pyth.network/docs/
- Best practices: https://docs.pyth.network/price-feeds/best-practices
- DeepBook margin production reference: `MystenLabs/deepbookv3/packages/deepbook_margin/sources/helper/oracle.move`

**Retention mechanics:**
- Notcoin / Hamster Kombat playbook overview: https://coindcx.com/blog/crypto-highlights/telegram-crypto-games/

**Sui infrastructure:**
- zkLogin + Enoki: https://docs.enoki.mysten.app
- DeepBook Predict (Phase 4+ companion product, not v1): https://blog.sui.io/introducing-deepbook-predict/

