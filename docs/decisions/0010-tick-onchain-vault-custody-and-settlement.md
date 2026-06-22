# ADR-0010 — Tick on-chain vault: custody & settlement authority

**Date:** 2026-05-27
**Status:** Accepted
**Workstream:** Tick (tap-trading)
**Supersedes:** —
**Superseded by:** —

## Context

`SYSTEM_DESIGN.md §1` locks v1 as points-only with an explicit
"**No vault contract. Points only; no funds to manage.**" The vault was
deferred to Phase 5 (real-money mode). Two things changed that calculus:

1. **Hackathon track fit.** Sui Overflow 2026's DeFi & Payments track
   rewards a real on-chain financial primitive. A points-only game reads
   as a consumer toy; a touch-settled DUSDC vault reads as "programmable
   money beyond traditional DeFi" — the exact track language. Pandora
   Finance won Sui Overflow 2024 with a 5-min binary product, precedent
   that this genre belongs in (and can win) a DeFi-style track.

2. **Verifiability story.** The vault is the on-chain anchor that makes
   the off-chain settler's outcomes trustworthy (see ADR-0011 for the
   Walrus proof layer). Without on-chain custody there is nothing to
   verify against.

We are **not** waiting for Phase 5. We pull a *testnet* USDC vault
forward into the hackathon build. We use **Circle's native USDC**,
which is live on Sui testnet *and* mainnet:

- Testnet: `0xa1ec7fc00a6f40db9693ad1415d0c193ad3906494428cf252621037bd7117e29::usdc::USDC`
- Mainnet: `0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC`
- Faucet: `faucet.circle.com` (20 USDC per 2 h, no account required)

Testnet USDC is free from Circle's faucet, so there is zero
real-capital risk and no audit-blocking custody question during the
build. We chose Circle USDC over DUSDC (DeepBook's test stablecoin)
deliberately: it is *real* Circle USDC, the mainnet asset is the same
coin family (only the package address differs across networks), and
"built on Circle's native USDC" is a stronger real-world-application
signal for the DeFi & Payments track than a DeepBook test token.

This does **not** remove the points mode. Points stay exactly as
planned (off-chain, Postgres, plans A–E). The vault is a **parallel
mode**: the same tap-grid UX, the same off-chain touch detection, but
stakes and payouts settle on-chain in DUSDC instead of in the points
ledger. A player chooses their mode at session start.

The hard question this ADR resolves: **who is allowed to move money out
of the vault, and on what authority?** Settlement is off-chain (touch
detection over `[t_open, t_close]` against the oracle aggregate — the
same logic the points settler already runs). The vault cannot
re-derive a touch on-chain; it must trust an external signer. We adopt
the Hyperliquid pattern: a credibly-centralized settler authorizes
payouts, and ADR-0011's Walrus proofs make every authorization
publicly replayable after the fact.

## Decision

### 1. New Move package `tick_vault`

Lives at `games/tap-trading/move/tick_vault/`. Separate from the
Phase-3 `tick_anchor` package (merkle snapshots) — different lifecycle,
different risk surface. `tick_anchor` stays unbuilt until Phase 3.

### 2. `GameVault<phantom Quote>` is generic over the quote coin

```move
public struct GameVault<phantom Quote> has key {
    id: UID,
    treasury: Balance<Quote>,
    settler: address,                 // the off-chain settler's authorized address
    total_open_liability: u64,        // sum of (stake × multiplier) across OPEN positions
    bullish_liability: u64,
    bearish_liability: u64,
    config: VaultConfig,
    paused: bool,
}
```

Both networks use Circle's native USDC (`::usdc::USDC`); the package
address differs per network, so the concrete type is
`0xa1ec…::usdc::USDC` on testnet and `0xdba3…::usdc::USDC` on mainnet.
The `<Quote>` generic lets the *same* contract code serve both — the
named address `usdc` in `Move.toml` resolves per build. We
hard-instantiate one quote per deployment rather than supporting
multiple quotes in one vault — a single-asset treasury keeps the
exposure math (§5) one-dimensional.

### 3. Player funds live in per-player `PlayerBalance` objects, not the vault

```move
public struct PlayerBalance<phantom Quote> has key {
    id: UID,
    owner: address,
    available: Balance<Quote>,        // deposited, not yet staked
}
```

Deposit moves coins from wallet → `PlayerBalance.available`. Mint debits
`available` into the vault's `treasury` and records an on-chain
`Position`. This mirrors DeepBook Predict's `PredictManager` model and
keeps custody legible: a player's idle funds are in *their* object, the
vault only holds staked premiums + its own treasury reserve.

### 4. `Position` records the locked multiplier on-chain

```move
public struct Position<phantom Quote> has key {
    id: UID,
    owner: address,
    vault_id: ID,
    asset: u8,                        // 0=BTC 1=ETH 2=SOL (matches AssetSymbol ordinal)
    strike_lo: u64,                   // band, in oracle base units (1e9 fixed-point)
    strike_hi: u64,
    t_open_ms: u64,
    t_close_ms: u64,
    stake: u64,                       // Quote base units
    multiplier_bps: u64,              // locked at mint; 10000 = 1.00x
    oracle_seq_at_tap: u64,           // ties to ADR-0008 (asset, run_id, seq)
    oracle_run_id: u64,
    status: u8,                       // 0=OPEN 1=WON 2=LOST 3=VOID
}
```

`multiplier_bps` is the lock-at-tap invariant (`MATH_SPEC §4.3`) made
on-chain: settlement reads this field, never recomputes. Basis points,
not float — Move has no floats, and bps at 1e4 resolution exceeds the
3% drift tolerance by two orders of magnitude.

### 5. Exposure caps enforced on-chain at mint (Hyperliquid-pattern controls)

```move
public struct VaultConfig has store {
    per_cell_max_liability: u64,      // refuse mint if a single band's liability exceeds this
    max_directional_imbalance_bps: u64, // |bullish - bearish| / treasury cap
    treasury_min_buffer_bps: u64,     // refuse mint if open_liability > treasury × (1 - buffer)
    max_multiplier_bps: u64,          // hard cap, mirrors DeepBook's 100x (1_000_000 bps)
}
```

Mint asserts all four before accepting a stake. These are the
correlated-win solvency defenses: per-cell stops streamer pile-ons,
directional cap stops a BTC-pump wipeout, treasury buffer stops the
vault writing checks it can't cash. Set generously on testnet; tuned
for real capital in Phase 5.

### 6. Settlement authority: `SettlerCap`, off-chain signer

```move
public struct SettlerCap has key, store { id: UID, vault_id: ID }
```

Only the holder of `SettlerCap` for a vault may call `settle_win` /
`settle_loss` / `settle_void`. The off-chain settlement worker
(plan D's `tap-trading-settlement-worker`, extended) holds the
keypair whose address equals `vault.settler` and owns the `SettlerCap`.

- `settle_win(cap, vault, position, ...)` — pays
  `stake × multiplier_bps / 10000` from `treasury` to the position
  owner's `PlayerBalance`, sets status WON, decrements liabilities.
- `settle_loss(cap, vault, position)` — stake already in treasury;
  sets status LOST, decrements liabilities. No transfer.
- `settle_void(cap, vault, position)` — refunds `stake` to owner
  (oracle gap over the window, per `SYSTEM_DESIGN §9.1`).

Players **cannot** self-settle. The vault trusts the settler; ADR-0011
makes that trust auditable. This is "credibly centralized," not
trustless — the same posture Hyperliquid's sequencer takes.

### 7. Settlement worker becomes dual-sink

The existing `tap-trading-settlement-worker` (plan D) gains a second
output. On touch detection for a position, it branches on mode:

- **Points position** → existing Postgres transaction (unchanged).
- **DUSDC position** → build + sign a `settle_win`/`settle_loss`/
  `settle_void` PTB against `tick_vault`, submit to Sui, then record
  the digest + Walrus blob id in Postgres for the indexer.

The off-chain touch logic (`touch.rs::evaluate_position`) is shared
verbatim across both sinks — one source of truth for "did it touch."

### 8. Points and DUSDC are separate balances, never auto-converted

A player's points balance (off-chain) and DUSDC `PlayerBalance`
(on-chain) are independent. No implicit conversion in v1. Mode is a
session-level choice surfaced in the UI. This avoids building a
points↔DUSDC exchange (regulatorily fraught, out of scope) and keeps
each ledger's accounting clean.

## Consequences

- The v1 "no vault" decision in `SYSTEM_DESIGN §1` is overridden for the
  hackathon build. `SYSTEM_DESIGN` must be updated to document the
  parallel DUSDC mode and reference this ADR.
- The settlement worker is no longer Postgres-only; it gains a Sui
  signing path and a dependency on `platform-lib-sui`. Its single-leader
  advisory-lock semantics (plan D) now also protect against
  double-submission of on-chain payouts.
- Custody risk on testnet is nil (faucet USDC). The contract is
  audit-ready in shape but **unaudited**; mainnet deployment is gated on
  an audit (OtterSec/OpenZeppelin credits are a hackathon prize).
- Vault solvency under correlated wins is bounded by the §5 caps, not
  eliminated. External perp-hedging is deferred to Phase 5 (documented
  in `PRD §17`).
- The `SettlerCap` keypair is a single point of compromise. If leaked,
  an attacker can drain the testnet treasury (no real value) or, on
  mainnet, authorize fraudulent payouts. Phase 5 hardens this with a
  multisig settler and on-chain payout-rate limits.

## Forecast

- **Mainnet:** point the `usdc` named address at the mainnet package
  (`0xdba3…`), fund treasury, swap the Circle faucet for a fiat
  on-ramp. Contract code unchanged.
- **LP'd treasury:** open `PlayerBalance`-style `LpShare` objects so
  third parties can supply treasury liquidity and earn the spread
  (the "LP the vault" model). Additive; does not change settlement.
- **Multisig settler:** replace the single `settler: address` with a
  threshold of signers; `settle_*` checks a quorum. The `SettlerCap`
  pattern extends to a `SettlerCommittee` object.
- **On-chain hedging:** a keeper rebalances treasury exposure via
  DeepBook spot/perp when `bullish_liability` skews. Reads the same
  liability fields this ADR defines.
