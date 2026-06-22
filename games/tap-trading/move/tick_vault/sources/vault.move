/// Module: tick_vault::vault
///
/// On-chain custody + settler-authorized settlement for Tick's USDC mode
/// (ADR-0010). A `GameVault<Quote>` custodies the treasury; players hold idle
/// funds in their own `PlayerBalance<Quote>`; a `mint` records a `Position`
/// with the multiplier locked at tap. Only the holder of the `SettlerCap` may
/// settle, and ADR-0011's Walrus proofs make every settlement replayable.
///
/// Ownership model (deliberate deviation from the 2026-05-27 plan, which used
/// owned objects): `Position` and `PlayerBalance` are **shared** objects. The
/// off-chain settler signs `settle_*` from its own address and therefore cannot
/// pass a player's *owned* objects as transaction inputs — only shared (or its
/// own) objects. Sharing them, and gating mutation on `SettlerCap` (settle) and
/// an `owner == sender` assert (mint/withdraw), is the standard capability
/// pattern (DeepBook-style) and is what lets the worker actually settle on-chain.
module tick_vault::vault;

use sui::balance::{Self, Balance};
use sui::coin::{Self, Coin};
use sui::event;

// === Errors ===
const EInvalidBand: u64 = 1;
const EMultiplierBelowFloor: u64 = 2;
const EMultiplierAboveCap: u64 = 3;
const ECellCapExceeded: u64 = 4;
const EDirectionalCapExceeded: u64 = 5;
const ETreasuryBufferExceeded: u64 = 6;
const ECapVaultMismatch: u64 = 7;
const EPositionNotOpen: u64 = 8;
const EVaultPaused: u64 = 9;
const EInsufficientPlayerBalance: u64 = 10;
const ENotBalanceOwner: u64 = 11;
const EPositionVaultMismatch: u64 = 12;

// === Constants ===
const MULTIPLIER_FLOOR_BPS: u64 = 10_000; // 1.00x — never sell a sub-1x position
const BPS_DENOM: u64 = 10_000;

// Position lifecycle
const STATUS_OPEN: u8 = 0;
const STATUS_WON: u8 = 1;
const STATUS_LOST: u8 = 2;
const STATUS_VOID: u8 = 3;

public struct VaultConfig has store {
    per_cell_max_liability: u64,
    max_directional_imbalance_bps: u64,
    treasury_min_buffer_bps: u64,
    max_multiplier_bps: u64,
}

public struct GameVault<phantom Quote> has key {
    id: UID,
    treasury: Balance<Quote>,
    settler: address,
    /// Sum of max-payout liability across OPEN positions.
    total_open_liability: u64,
    bullish_liability: u64,
    bearish_liability: u64,
    config: VaultConfig,
    paused: bool,
}

/// Idle, deposited funds. Shared so the settler can credit `available` on
/// win/void; `withdraw`/`mint` assert `owner == sender` so only the owner
/// spends.
public struct PlayerBalance<phantom Quote> has key {
    id: UID,
    owner: address,
    available: Balance<Quote>,
}

/// A touch-bet with the multiplier locked at tap (MATH_SPEC §4.3). Shared so
/// the settler can read + flip status. `is_bullish` is caller-supplied and
/// drives the directional-imbalance cap only — it never affects settlement
/// (touch is direction-agnostic).
public struct Position<phantom Quote> has key {
    id: UID,
    owner: address,
    vault_id: ID,
    asset: u8, // 0=BTC 1=ETH 2=SUI (AssetSymbol ordinal)
    strike_lo: u64, // oracle base units (price × 1e9)
    strike_hi: u64,
    t_open_ms: u64,
    t_close_ms: u64,
    stake: u64,
    multiplier_bps: u64,
    oracle_seq_at_tap: u64,
    oracle_run_id: u64,
    is_bullish: bool,
    status: u8,
}

public struct SettlerCap has key, store { id: UID, vault_id: ID }
public struct AdminCap has key, store { id: UID, vault_id: ID }

/// Emitted by `anchor_proof` after the Walrus blob is written (ADR-0011 §2).
public struct ProofAnchored has copy, drop, store {
    position_id: ID,
    walrus_blob_id: vector<u8>,
    outcome: u8,
    settled_at_ms: u64,
}

// === Vault lifecycle ===

#[allow(lint(self_transfer))]
public fun create_vault<Quote>(
    settler: address,
    per_cell_max_liability: u64,
    max_directional_imbalance_bps: u64,
    treasury_min_buffer_bps: u64,
    max_multiplier_bps: u64,
    ctx: &mut TxContext,
) {
    let vault = GameVault<Quote> {
        id: object::new(ctx),
        treasury: balance::zero<Quote>(),
        settler,
        total_open_liability: 0,
        bullish_liability: 0,
        bearish_liability: 0,
        config: VaultConfig {
            per_cell_max_liability,
            max_directional_imbalance_bps,
            treasury_min_buffer_bps,
            max_multiplier_bps,
        },
        paused: false,
    };
    let vault_id = object::id(&vault);
    transfer::transfer(SettlerCap { id: object::new(ctx), vault_id }, settler);
    transfer::transfer(AdminCap { id: object::new(ctx), vault_id }, ctx.sender());
    transfer::share_object(vault);
}

/// Fund the treasury reserve (the house's float). Anyone may top up.
public fun fund_treasury<Quote>(v: &mut GameVault<Quote>, c: Coin<Quote>) {
    v.treasury.join(c.into_balance());
}

// === Player balance ===

public fun open_balance<Quote>(ctx: &mut TxContext) {
    transfer::share_object(PlayerBalance<Quote> {
        id: object::new(ctx),
        owner: ctx.sender(),
        available: balance::zero<Quote>(),
    });
}

public fun deposit<Quote>(pb: &mut PlayerBalance<Quote>, c: Coin<Quote>) {
    pb.available.join(c.into_balance());
}

public fun withdraw<Quote>(
    pb: &mut PlayerBalance<Quote>,
    amount: u64,
    ctx: &mut TxContext,
): Coin<Quote> {
    assert!(pb.owner == ctx.sender(), ENotBalanceOwner);
    assert!(pb.available.value() >= amount, EInsufficientPlayerBalance);
    coin::from_balance(pb.available.split(amount), ctx)
}

// === Mint ===

public fun mint<Quote>(
    v: &mut GameVault<Quote>,
    pb: &mut PlayerBalance<Quote>,
    asset: u8,
    strike_lo: u64,
    strike_hi: u64,
    t_open_ms: u64,
    t_close_ms: u64,
    stake: u64,
    multiplier_bps: u64,
    oracle_seq_at_tap: u64,
    oracle_run_id: u64,
    is_bullish: bool,
    ctx: &mut TxContext,
) {
    assert!(pb.owner == ctx.sender(), ENotBalanceOwner);
    assert!(!v.paused, EVaultPaused);
    assert!(strike_lo < strike_hi, EInvalidBand);
    assert!(multiplier_bps >= MULTIPLIER_FLOOR_BPS, EMultiplierBelowFloor);
    assert!(multiplier_bps <= v.config.max_multiplier_bps, EMultiplierAboveCap);
    assert!(pb.available.value() >= stake, EInsufficientPlayerBalance);

    // Max payout this position can cost the treasury.
    let liability = mul_bps(stake, multiplier_bps);

    // Per-cell cap (per-mint; aggregate per-band tracking is a Forecast item).
    assert!(liability <= v.config.per_cell_max_liability, ECellCapExceeded);

    // Projected directional imbalance against the post-stake treasury.
    let (new_bull, new_bear) = if (is_bullish) {
        (v.bullish_liability + liability, v.bearish_liability)
    } else {
        (v.bullish_liability, v.bearish_liability + liability)
    };
    let imbalance = if (new_bull > new_bear) { new_bull - new_bear } else { new_bear - new_bull };
    let treasury_after = v.treasury.value() + stake;
    assert!(
        imbalance <= mul_bps(treasury_after, v.config.max_directional_imbalance_bps),
        EDirectionalCapExceeded,
    );

    // Treasury buffer: open liability must stay under treasury × (1 - buffer).
    let new_total = v.total_open_liability + liability;
    let allowed = mul_bps(treasury_after, BPS_DENOM - v.config.treasury_min_buffer_bps);
    assert!(new_total <= allowed, ETreasuryBufferExceeded);

    v.treasury.join(pb.available.split(stake));
    v.total_open_liability = new_total;
    v.bullish_liability = new_bull;
    v.bearish_liability = new_bear;

    transfer::share_object(Position<Quote> {
        id: object::new(ctx),
        owner: ctx.sender(),
        vault_id: object::id(v),
        asset,
        strike_lo,
        strike_hi,
        t_open_ms,
        t_close_ms,
        stake,
        multiplier_bps,
        oracle_seq_at_tap,
        oracle_run_id,
        is_bullish,
        status: STATUS_OPEN,
    });
}

// === Settlement (SettlerCap-gated) ===

public fun settle_win<Quote>(
    cap: &SettlerCap,
    v: &mut GameVault<Quote>,
    position: &mut Position<Quote>,
    pb: &mut PlayerBalance<Quote>,
) {
    assert_settlable(cap, v, position);
    let payout = mul_bps(position.stake, position.multiplier_bps);
    pb.available.join(v.treasury.split(payout));
    release_liability(v, position);
    position.status = STATUS_WON;
}

public fun settle_loss<Quote>(
    cap: &SettlerCap,
    v: &mut GameVault<Quote>,
    position: &mut Position<Quote>,
) {
    assert_settlable(cap, v, position);
    // Stake is already in the treasury; the house keeps it. No transfer.
    release_liability(v, position);
    position.status = STATUS_LOST;
}

public fun settle_void<Quote>(
    cap: &SettlerCap,
    v: &mut GameVault<Quote>,
    position: &mut Position<Quote>,
    pb: &mut PlayerBalance<Quote>,
) {
    assert_settlable(cap, v, position);
    pb.available.join(v.treasury.split(position.stake));
    release_liability(v, position);
    position.status = STATUS_VOID;
}

/// Anchor the Walrus proof blob id for a settled position (ADR-0011 §2). Kept
/// separate from `settle_*` because the blob is written *after* the settle PTB
/// confirms (we need its digest first). Takes the vault + position by reference
/// so the off-chain settler passes them as shared-object args; `position_id` and
/// `outcome` are read on-chain (no chance of the event disagreeing with state).
public fun anchor_proof<Quote>(
    cap: &SettlerCap,
    v: &GameVault<Quote>,
    position: &Position<Quote>,
    walrus_blob_id: vector<u8>,
    settled_at_ms: u64,
) {
    assert!(cap.vault_id == object::id(v), ECapVaultMismatch);
    event::emit(ProofAnchored {
        position_id: object::id(position),
        walrus_blob_id,
        outcome: position.status,
        settled_at_ms,
    });
}

// === Admin (AdminCap-gated) ===

public fun set_paused<Quote>(cap: &AdminCap, v: &mut GameVault<Quote>, paused: bool) {
    assert!(cap.vault_id == object::id(v), ECapVaultMismatch);
    v.paused = paused;
}

public fun update_config<Quote>(
    cap: &AdminCap,
    v: &mut GameVault<Quote>,
    per_cell_max_liability: u64,
    max_directional_imbalance_bps: u64,
    treasury_min_buffer_bps: u64,
    max_multiplier_bps: u64,
) {
    assert!(cap.vault_id == object::id(v), ECapVaultMismatch);
    v.config.per_cell_max_liability = per_cell_max_liability;
    v.config.max_directional_imbalance_bps = max_directional_imbalance_bps;
    v.config.treasury_min_buffer_bps = treasury_min_buffer_bps;
    v.config.max_multiplier_bps = max_multiplier_bps;
}

// === Internal ===

fun assert_settlable<Quote>(
    cap: &SettlerCap,
    v: &GameVault<Quote>,
    position: &Position<Quote>,
) {
    assert!(cap.vault_id == object::id(v), ECapVaultMismatch);
    assert!(position.vault_id == object::id(v), EPositionVaultMismatch);
    // Idempotency: a re-submitted settle aborts here, the authoritative
    // double-pay guard alongside the worker's DB `settlements` canary.
    assert!(position.status == STATUS_OPEN, EPositionNotOpen);
}

/// Decrement liability on settlement — on win, loss, AND void. Forgetting any
/// path leaks `total_open_liability` and eventually blocks every mint.
fun release_liability<Quote>(v: &mut GameVault<Quote>, position: &Position<Quote>) {
    let liability = mul_bps(position.stake, position.multiplier_bps);
    v.total_open_liability = v.total_open_liability - liability;
    if (position.is_bullish) {
        v.bullish_liability = v.bullish_liability - liability;
    } else {
        v.bearish_liability = v.bearish_liability - liability;
    };
}

/// `amount × bps / 10000` via a u128 intermediate to avoid overflow.
fun mul_bps(amount: u64, bps: u64): u64 {
    (((amount as u128) * (bps as u128)) / (BPS_DENOM as u128)) as u64
}

// === Read accessors ===

public fun settler<Quote>(v: &GameVault<Quote>): address { v.settler }
public fun treasury_value<Quote>(v: &GameVault<Quote>): u64 { v.treasury.value() }
public fun total_open_liability<Quote>(v: &GameVault<Quote>): u64 { v.total_open_liability }
public fun bullish_liability<Quote>(v: &GameVault<Quote>): u64 { v.bullish_liability }
public fun bearish_liability<Quote>(v: &GameVault<Quote>): u64 { v.bearish_liability }
public fun is_paused<Quote>(v: &GameVault<Quote>): bool { v.paused }
public fun available<Quote>(pb: &PlayerBalance<Quote>): u64 { pb.available.value() }
public fun position_status<Quote>(p: &Position<Quote>): u8 { p.status }
public fun position_owner<Quote>(p: &Position<Quote>): address { p.owner }
