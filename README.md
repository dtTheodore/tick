# Tick — tap the chart, win the next five seconds

An on-chain **USDC prediction game on Sui**. A live, multi-exchange price chart
scrolls in real time; you tap a cell to bet the price will touch it inside the
next ~5-second window. The payout multiplier is priced live from volatility, and
positions settle the instant the window closes.

**▶ Live demo: https://playtick.pages.dev** — *Sui Overflow 2026 submission.*

- **Demo mode** — play instantly, no wallet required.
- **Real USDC (testnet)** — connect a Sui wallet, deposit testnet USDC, play, withdraw.

## How it works

- **Off-chain play, on-chain custody.** Bets settle off-chain for sub-second
  responsiveness against a USDC ledger; funds enter and leave only through
  on-chain **deposit** and **withdraw** against a Move custody vault.
- **Provably fair pricing.** Multipliers are derived from a live volatility
  estimate over a consensus mid-price aggregated from **Binance, Bybit, OKX, and
  Pyth** — no house-set odds.
- **Auditability.** Settlement proofs are batched and published to **Walrus** so
  outcomes can be independently verified.

## Stack

| Layer | Tech |
|---|---|
| Contracts | **Sui Move** — `tick_vault` (USDC custody, settler-authorized settlement) |
| Backend | **Rust** — axum API, multi-source oracle aggregator, settlement worker (all `tokio`) |
| Frontend | **React + Vite + @mysten/dapp-kit**, real-time WebSocket chart |
| Infra | Docker Compose (Postgres + Redis), Caddy TLS; frontend on Cloudflare Pages |

## Layout

```
games/tap-trading/
├── move/tick_vault/   # Sui Move custody + settlement contract
├── backend/           # Rust: api · oracle-aggregator · settlement-worker · migrate
├── ui/                # React frontend (the live chart + game)
└── deploy/            # Docker Compose + deploy runbook
```

## Run it

Backend: `cd games/tap-trading/deploy && cp .env.example .env && docker compose up -d --build`
(see `games/tap-trading/deploy/README.md` for the full runbook).
Frontend: `cd games/tap-trading/ui && bun install && bun run build`.
