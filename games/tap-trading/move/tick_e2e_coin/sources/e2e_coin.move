/// Module: tick_e2e_coin::e2e_coin
///
/// A 6-decimal test stablecoin used ONLY to exercise `tick_vault` end-to-end on
/// testnet without depending on the Circle USDC faucet. The deployed vault is
/// `GameVault<E2E_COIN>` for e2e; the production instantiation is
/// `GameVault<usdc::USDC>` (a config swap — the `<Quote>` generic is exactly so
/// the same vault code serves both). The publisher holds the `TreasuryCap` and
/// mints freely via `mint` so the e2e harness can fund the player and treasury.
module tick_e2e_coin::e2e_coin;

use sui::coin::{Self, TreasuryCap};

public struct E2E_COIN has drop {}

fun init(witness: E2E_COIN, ctx: &mut TxContext) {
    let (treasury, metadata) = coin::create_currency(
        witness,
        6,
        b"E2EUSD",
        b"Tick E2E USD",
        b"Test stablecoin for tick_vault end-to-end verification",
        option::none(),
        ctx,
    );
    transfer::public_freeze_object(metadata);
    transfer::public_transfer(treasury, ctx.sender());
}

/// Mint `amount` base units to `recipient`. Gated by holding the `TreasuryCap`.
public fun mint(
    treasury: &mut TreasuryCap<E2E_COIN>,
    amount: u64,
    recipient: address,
    ctx: &mut TxContext,
) {
    transfer::public_transfer(coin::mint(treasury, amount, ctx), recipient);
}
