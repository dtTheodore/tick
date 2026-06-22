#[test_only]
/// A phantom coin type for unit tests so the vault can be exercised without a
/// live USDC faucet. Coins are conjured via `sui::coin::mint_for_testing`.
module tick_vault::coin_dummy;

public struct COIN_DUMMY has drop {}
