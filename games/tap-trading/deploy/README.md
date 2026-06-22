# Tick — deploy runbook

Backend runs as a Docker Compose stack on one host (your AWS EC2 box).
Frontend deploys separately to Cloudflare Pages. Everything here is free:
local Postgres/Redis containers, Caddy auto-TLS via a free `sslip.io` hostname,
Sui + Walrus on testnet (faucet-funded).

```
[Cloudflare Pages]  ──https──▶  Caddy (TLS) ──▶ api ──▶ postgres / redis
   (static SPA)                                  │
                                       oracle-aggregator (exchange WS)
                                       settlement-worker ──▶ Sui + Walrus (testnet)
```

## Prerequisites

- Docker + Compose plugin on the box (`docker --version`, `docker compose version`).
- Security group: inbound **80** and **443** open (Caddy's TLS challenge + serving).
- A public hostname. Free path: `<EC2_PUBLIC_IP>.sslip.io` — resolves to your IP
  with no DNS setup. Use an **Elastic IP** so it survives a reboot. (A real domain
  pointed at the IP is more robust; swap it into `PUBLIC_HOST` anytime.)

## 1. Get the code onto the box

This branch is local-only with uncommitted work, so `git clone` would miss it.
Copy the working tree directly (run from the repo root on your machine):

```sh
rsync -az --delete \
  --exclude target --exclude node_modules --exclude .local --exclude tmp \
  games/tap-trading/  USER@EC2_HOST:~/tick/
```

## 2. Configure

```sh
cd ~/tick/deploy
cp .env.example .env
# edit .env: set PUBLIC_HOST and a strong POSTGRES_PASSWORD.
```

- **Points-only first (simplest):** do nothing extra — no `onchain.env` ⇒ the worker
  boots points-only; skip step 3. Good for proving the box is up.
- **USDC mode (the demo target):** `cp onchain.env.example onchain.env` and fill it
  from your deployed `tick_vault` package, then do step 3. (The vars must be *unset*,
  not blank, for points-only — that's why USDC config is a separate optional file,
  not empty entries in `.env`.)

## 3. Provision the operator keystore (USDC mode only)

Both the **worker** (settlement) and the **api** (withdrawals) shell out to the
`sui` CLI and sign as its **active address** — so the same keystore is mounted
into both services. That address must equal `TICK_SETTLER_ADDRESS` (the worker
fail-loud asserts this at boot) and be authorized to call `vault::withdraw` on
the custody balance.

The Sui CLI looks for `~/.sui/sui_config/client.yaml`, and the mount maps
`secrets/sui → /root/.sui`, so files go under the `sui_config/` subdir:

```sh
mkdir -p ~/tick/deploy/secrets/sui/sui_config ~/tick/deploy/secrets/walrus
# secrets/sui/sui_config/ : client.yaml + sui.keystore
#   (active env = testnet, active address = TICK_SETTLER_ADDRESS, faucet-funded)
# secrets/walrus/         : Walrus client config (client_config.yaml)
```

The frontend's `VITE_TICK_*` (step 5) must point at the **same** vault/custody.

## 4. Bring it up

```sh
cd ~/tick/deploy
docker compose --env-file .env up -d --build
docker compose ps
docker compose logs -f api settlement-worker
```

First build compiles the Rust workspace (~several minutes) and needs ~3 GB RAM.
On a 1 GB instance, add swap first, or build images off-box and pull:

```sh
sudo fallocate -l 4G /swapfile && sudo chmod 600 /swapfile \
  && sudo mkswap /swapfile && sudo swapon /swapfile
```

## 5. Frontend → Cloudflare Pages

- Project root: `games/tap-trading/ui`
- Build command: `bun install && bun run build`   ·   Output dir: `dist`
- Environment variables:
  - `VITE_TAP_API_URL` = `https://<PUBLIC_HOST>`
  - `VITE_TAP_API_WS_URL` = `wss://<PUBLIC_HOST>/stream`
  - USDC mode: `VITE_SUI_NETWORK=testnet`, `VITE_TICK_VAULT_PKG`, `VITE_TICK_USDC_TYPE`,
    `VITE_TICK_CUSTODY_PB` (must match the worker's vault).

`_redirects` (SPA fallback) is already in `public/`. The API's CORS is permissive,
so the split origin works without extra config.

## 6. Verify

```sh
curl https://<PUBLIC_HOST>/healthz                 # api up + TLS issued
curl https://<PUBLIC_HOST>/v1/ping
# WS: open the Pages site; the chart should stream ticks.
docker compose logs settlement-worker | grep -i "usdc sink"   # "enabled" in USDC mode
```

---

### Verified vs. needs on-box verification

- **Verified here:** frontend `bun run build` is green; backend `cargo check --workspace`
  passes; `docker compose config` parses.
- **NOT verified here (no full image build in this env):** the `sui`/`walrus` install
  in the worker image (pinned to the latest testnet release — bump `SUI_VERSION` in
  the Dockerfile if the asset name 404s) and the live on-chain settle path. Validate
  on the box with step 6. If on-chain wiring isn't ready on demo day, run points-only.
