#[test_only]
module tick_vault::vault_tests;

use sui::coin;
use sui::test_scenario as ts;
use sui::test_utils::{assert_eq, destroy};
use tick_vault::coin_dummy::COIN_DUMMY;
use tick_vault::vault::{Self, GameVault, PlayerBalance, Position, SettlerCap, AdminCap};

const ADMIN: address = @0xA;
const SETTLER: address = @0x5;
const PLAYER: address = @0x9;

// Generous defaults; the happy path needs a pre-funded treasury reserve so the
// directional/buffer caps don't reject the first bet (liability >= stake).
const PER_CELL: u64 = 50_000_000;
const DIR_BPS: u64 = 3_000; // 30%
const BUFFER_BPS: u64 = 2_000; // 20%
const MAX_MULT: u64 = 1_000_000; // 100x

/// create_vault (ADMIN) → fund treasury reserve → open + fund a PlayerBalance.
/// Leaves the scenario after the player-funding tx; the vault, the player
/// balance, the SettlerCap (SETTLER) and AdminCap (ADMIN) are all live.
fun stand_up(
    sc: &mut ts::Scenario,
    per_cell: u64,
    dir_bps: u64,
    buffer_bps: u64,
    max_mult: u64,
    treasury_reserve: u64,
    player_funds: u64,
) {
    vault::create_vault<COIN_DUMMY>(SETTLER, per_cell, dir_bps, buffer_bps, max_mult, ts::ctx(sc));
    ts::next_tx(sc, ADMIN);
    {
        let mut v = ts::take_shared<GameVault<COIN_DUMMY>>(sc);
        vault::fund_treasury(&mut v, coin::mint_for_testing<COIN_DUMMY>(treasury_reserve, ts::ctx(sc)));
        ts::return_shared(v);
    };
    ts::next_tx(sc, PLAYER);
    { vault::open_balance<COIN_DUMMY>(ts::ctx(sc)); };
    ts::next_tx(sc, PLAYER);
    {
        let mut pb = ts::take_shared<PlayerBalance<COIN_DUMMY>>(sc);
        vault::deposit(&mut pb, coin::mint_for_testing<COIN_DUMMY>(player_funds, ts::ctx(sc)));
        ts::return_shared(pb);
    };
    ts::next_tx(sc, PLAYER);
}

/// PLAYER mints a default bullish position: stake 10_000, multiplier 1.96x.
fun mint_default(sc: &mut ts::Scenario) {
    let mut v = ts::take_shared<GameVault<COIN_DUMMY>>(sc);
    let mut pb = ts::take_shared<PlayerBalance<COIN_DUMMY>>(sc);
    vault::mint<COIN_DUMMY>(
        &mut v, &mut pb,
        0, // BTC
        75_832_000_000_000, 75_842_000_000_000,
        1_779_564_600_000, 1_779_564_660_000,
        10_000, // stake
        19_600, // multiplier_bps (1.96x)
        48_213, 173_000_000,
        true, // is_bullish
        ts::ctx(sc),
    );
    ts::return_shared(v);
    ts::return_shared(pb);
}

#[test]
fun create_vault_shares_and_caps() {
    let mut sc = ts::begin(ADMIN);
    vault::create_vault<COIN_DUMMY>(SETTLER, PER_CELL, DIR_BPS, BUFFER_BPS, MAX_MULT, ts::ctx(&mut sc));
    ts::next_tx(&mut sc, ADMIN);
    {
        let v = ts::take_shared<GameVault<COIN_DUMMY>>(&sc);
        assert_eq(vault::settler(&v), SETTLER);
        assert_eq(vault::treasury_value(&v), 0);
        ts::return_shared(v);
        assert!(ts::has_most_recent_for_sender<AdminCap>(&sc), 0);
    };
    ts::next_tx(&mut sc, SETTLER);
    { assert!(ts::has_most_recent_for_sender<SettlerCap>(&sc), 0); };
    ts::end(sc);
}

#[test]
fun deposit_then_withdraw() {
    let mut sc = ts::begin(ADMIN);
    stand_up(&mut sc, PER_CELL, DIR_BPS, BUFFER_BPS, MAX_MULT, 0, 100);
    {
        let mut pb = ts::take_shared<PlayerBalance<COIN_DUMMY>>(&sc);
        assert_eq(vault::available(&pb), 100);
        let out = vault::withdraw(&mut pb, 40, ts::ctx(&mut sc));
        assert_eq(out.value(), 40);
        assert_eq(vault::available(&pb), 60);
        destroy(out);
        ts::return_shared(pb);
    };
    ts::end(sc);
}

#[test]
fun mint_happy_path_debits_player_and_credits_treasury() {
    let mut sc = ts::begin(ADMIN);
    stand_up(&mut sc, PER_CELL, DIR_BPS, BUFFER_BPS, MAX_MULT, 1_000_000, 1_000_000);
    mint_default(&mut sc);
    ts::next_tx(&mut sc, PLAYER);
    {
        let v = ts::take_shared<GameVault<COIN_DUMMY>>(&sc);
        let pb = ts::take_shared<PlayerBalance<COIN_DUMMY>>(&sc);
        assert_eq(vault::available(&pb), 990_000); // 1_000_000 − 10_000 stake
        assert_eq(vault::treasury_value(&v), 1_010_000); // reserve + stake
        assert_eq(vault::total_open_liability(&v), 19_600); // stake × 1.96
        assert_eq(vault::bullish_liability(&v), 19_600);
        ts::return_shared(v);
        ts::return_shared(pb);
        assert!(ts::has_most_recent_shared<Position<COIN_DUMMY>>(), 0);
    };
    ts::end(sc);
}

#[test]
#[expected_failure(abort_code = vault::EMultiplierAboveCap)]
fun mint_rejects_above_cap() {
    let mut sc = ts::begin(ADMIN);
    stand_up(&mut sc, PER_CELL, DIR_BPS, BUFFER_BPS, MAX_MULT, 1_000_000, 1_000_000);
    let mut v = ts::take_shared<GameVault<COIN_DUMMY>>(&sc);
    let mut pb = ts::take_shared<PlayerBalance<COIN_DUMMY>>(&sc);
    vault::mint<COIN_DUMMY>(&mut v, &mut pb, 0, 1, 2, 0, 60, 10_000, MAX_MULT + 1, 1, 1, true, ts::ctx(&mut sc));
    abort 0
}

#[test]
#[expected_failure(abort_code = vault::EMultiplierBelowFloor)]
fun mint_rejects_below_floor() {
    let mut sc = ts::begin(ADMIN);
    stand_up(&mut sc, PER_CELL, DIR_BPS, BUFFER_BPS, MAX_MULT, 1_000_000, 1_000_000);
    let mut v = ts::take_shared<GameVault<COIN_DUMMY>>(&sc);
    let mut pb = ts::take_shared<PlayerBalance<COIN_DUMMY>>(&sc);
    vault::mint<COIN_DUMMY>(&mut v, &mut pb, 0, 1, 2, 0, 60, 10_000, 9_999, 1, 1, true, ts::ctx(&mut sc));
    abort 0
}

#[test]
#[expected_failure(abort_code = vault::EInvalidBand)]
fun mint_rejects_inverted_band() {
    let mut sc = ts::begin(ADMIN);
    stand_up(&mut sc, PER_CELL, DIR_BPS, BUFFER_BPS, MAX_MULT, 1_000_000, 1_000_000);
    let mut v = ts::take_shared<GameVault<COIN_DUMMY>>(&sc);
    let mut pb = ts::take_shared<PlayerBalance<COIN_DUMMY>>(&sc);
    vault::mint<COIN_DUMMY>(&mut v, &mut pb, 0, 200, 100, 0, 60, 10_000, 19_600, 1, 1, true, ts::ctx(&mut sc));
    abort 0
}

#[test]
#[expected_failure(abort_code = vault::EInsufficientPlayerBalance)]
fun mint_rejects_insufficient_balance() {
    let mut sc = ts::begin(ADMIN);
    stand_up(&mut sc, PER_CELL, DIR_BPS, BUFFER_BPS, MAX_MULT, 1_000_000, 5_000);
    let mut v = ts::take_shared<GameVault<COIN_DUMMY>>(&sc);
    let mut pb = ts::take_shared<PlayerBalance<COIN_DUMMY>>(&sc);
    vault::mint<COIN_DUMMY>(&mut v, &mut pb, 0, 1, 2, 0, 60, 10_000, 19_600, 1, 1, true, ts::ctx(&mut sc));
    abort 0
}

#[test]
#[expected_failure(abort_code = vault::ECellCapExceeded)]
fun mint_rejects_per_cell_cap() {
    let mut sc = ts::begin(ADMIN);
    // per_cell tiny so liability (10_000 × 1x = 10_000) exceeds it.
    stand_up(&mut sc, 1_000, DIR_BPS, BUFFER_BPS, MAX_MULT, 1_000_000, 1_000_000);
    let mut v = ts::take_shared<GameVault<COIN_DUMMY>>(&sc);
    let mut pb = ts::take_shared<PlayerBalance<COIN_DUMMY>>(&sc);
    vault::mint<COIN_DUMMY>(&mut v, &mut pb, 0, 1, 2, 0, 60, 10_000, 10_000, 1, 1, true, ts::ctx(&mut sc));
    abort 0
}

#[test]
#[expected_failure(abort_code = vault::EDirectionalCapExceeded)]
fun mint_rejects_directional_cap() {
    let mut sc = ts::begin(ADMIN);
    // Small treasury, 30% directional cap: a 10_000 bullish liability against a
    // 20_000 post-stake treasury (allowed 6_000) trips the cap.
    stand_up(&mut sc, PER_CELL, 3_000, BUFFER_BPS, MAX_MULT, 10_000, 1_000_000);
    let mut v = ts::take_shared<GameVault<COIN_DUMMY>>(&sc);
    let mut pb = ts::take_shared<PlayerBalance<COIN_DUMMY>>(&sc);
    vault::mint<COIN_DUMMY>(&mut v, &mut pb, 0, 1, 2, 0, 60, 10_000, 10_000, 1, 1, true, ts::ctx(&mut sc));
    abort 0
}

#[test]
#[expected_failure(abort_code = vault::ETreasuryBufferExceeded)]
fun mint_rejects_treasury_buffer() {
    let mut sc = ts::begin(ADMIN);
    // Directional off (100%), buffer 50%: stake 10_000 × 2x = 20_000 liability
    // vs allowed 10_000 (= 20_000 post-stake treasury × 50%) → buffer trips.
    stand_up(&mut sc, PER_CELL, 10_000, 5_000, MAX_MULT, 10_000, 1_000_000);
    let mut v = ts::take_shared<GameVault<COIN_DUMMY>>(&sc);
    let mut pb = ts::take_shared<PlayerBalance<COIN_DUMMY>>(&sc);
    vault::mint<COIN_DUMMY>(&mut v, &mut pb, 0, 1, 2, 0, 60, 10_000, 20_000, 1, 1, true, ts::ctx(&mut sc));
    abort 0
}

#[test]
fun settle_win_pays_exact_and_releases_liability() {
    let mut sc = ts::begin(ADMIN);
    stand_up(&mut sc, PER_CELL, DIR_BPS, BUFFER_BPS, MAX_MULT, 1_000_000, 1_000_000);
    mint_default(&mut sc);
    ts::next_tx(&mut sc, SETTLER);
    {
        let cap = ts::take_from_sender<SettlerCap>(&sc);
        let mut v = ts::take_shared<GameVault<COIN_DUMMY>>(&sc);
        let mut pos = ts::take_shared<Position<COIN_DUMMY>>(&sc);
        let mut pb = ts::take_shared<PlayerBalance<COIN_DUMMY>>(&sc);
        vault::settle_win(&cap, &mut v, &mut pos, &mut pb);
        assert_eq(vault::available(&pb), 1_009_600); // 990_000 + 19_600 payout
        assert_eq(vault::treasury_value(&v), 990_400); // 1_010_000 − 19_600
        assert_eq(vault::total_open_liability(&v), 0);
        assert_eq(vault::bullish_liability(&v), 0);
        assert_eq(vault::position_status(&pos), 1); // WON
        ts::return_shared(v);
        ts::return_shared(pos);
        ts::return_shared(pb);
        ts::return_to_sender(&sc, cap);
    };
    ts::end(sc);
}

#[test]
#[expected_failure(abort_code = vault::EPositionNotOpen)]
fun settle_win_then_settle_again_aborts() {
    let mut sc = ts::begin(ADMIN);
    stand_up(&mut sc, PER_CELL, DIR_BPS, BUFFER_BPS, MAX_MULT, 1_000_000, 1_000_000);
    mint_default(&mut sc);
    ts::next_tx(&mut sc, SETTLER);
    {
        let cap = ts::take_from_sender<SettlerCap>(&sc);
        let mut v = ts::take_shared<GameVault<COIN_DUMMY>>(&sc);
        let mut pos = ts::take_shared<Position<COIN_DUMMY>>(&sc);
        let mut pb = ts::take_shared<PlayerBalance<COIN_DUMMY>>(&sc);
        vault::settle_win(&cap, &mut v, &mut pos, &mut pb);
        vault::settle_win(&cap, &mut v, &mut pos, &mut pb); // double-pay → abort
        ts::return_shared(v);
        ts::return_shared(pos);
        ts::return_shared(pb);
        ts::return_to_sender(&sc, cap);
    };
    ts::end(sc);
}

#[test]
fun settle_loss_keeps_stake_and_releases_liability() {
    let mut sc = ts::begin(ADMIN);
    stand_up(&mut sc, PER_CELL, DIR_BPS, BUFFER_BPS, MAX_MULT, 1_000_000, 1_000_000);
    mint_default(&mut sc);
    ts::next_tx(&mut sc, SETTLER);
    {
        let cap = ts::take_from_sender<SettlerCap>(&sc);
        let mut v = ts::take_shared<GameVault<COIN_DUMMY>>(&sc);
        let mut pos = ts::take_shared<Position<COIN_DUMMY>>(&sc);
        vault::settle_loss(&cap, &mut v, &mut pos);
        assert_eq(vault::treasury_value(&v), 1_010_000); // stake kept
        assert_eq(vault::total_open_liability(&v), 0);
        assert_eq(vault::position_status(&pos), 2); // LOST
        ts::return_shared(v);
        ts::return_shared(pos);
        ts::return_to_sender(&sc, cap);
    };
    ts::end(sc);
}

#[test]
fun settle_void_refunds_stake_and_releases_liability() {
    let mut sc = ts::begin(ADMIN);
    stand_up(&mut sc, PER_CELL, DIR_BPS, BUFFER_BPS, MAX_MULT, 1_000_000, 1_000_000);
    mint_default(&mut sc);
    ts::next_tx(&mut sc, SETTLER);
    {
        let cap = ts::take_from_sender<SettlerCap>(&sc);
        let mut v = ts::take_shared<GameVault<COIN_DUMMY>>(&sc);
        let mut pos = ts::take_shared<Position<COIN_DUMMY>>(&sc);
        let mut pb = ts::take_shared<PlayerBalance<COIN_DUMMY>>(&sc);
        vault::settle_void(&cap, &mut v, &mut pos, &mut pb);
        assert_eq(vault::available(&pb), 1_000_000); // stake refunded
        assert_eq(vault::treasury_value(&v), 1_000_000); // back to reserve
        assert_eq(vault::total_open_liability(&v), 0);
        assert_eq(vault::position_status(&pos), 3); // VOID
        ts::return_shared(v);
        ts::return_shared(pos);
        ts::return_shared(pb);
        ts::return_to_sender(&sc, cap);
    };
    ts::end(sc);
}

#[test]
fun anchor_proof_emits_event() {
    let mut sc = ts::begin(ADMIN);
    stand_up(&mut sc, PER_CELL, DIR_BPS, BUFFER_BPS, MAX_MULT, 1_000_000, 1_000_000);
    mint_default(&mut sc);
    ts::next_tx(&mut sc, SETTLER);
    {
        let cap = ts::take_from_sender<SettlerCap>(&sc);
        let mut v = ts::take_shared<GameVault<COIN_DUMMY>>(&sc);
        let mut pos = ts::take_shared<Position<COIN_DUMMY>>(&sc);
        let mut pb = ts::take_shared<PlayerBalance<COIN_DUMMY>>(&sc);
        vault::settle_win(&cap, &mut v, &mut pos, &mut pb);
        vault::anchor_proof(&cap, &v, &pos, b"walrus_blob_id_bytes", 1_700_000_000_000);
        ts::return_shared(v);
        ts::return_shared(pos);
        ts::return_shared(pb);
        ts::return_to_sender(&sc, cap);
    };
    let effects = ts::next_tx(&mut sc, SETTLER);
    assert_eq(effects.num_user_events(), 1);
    ts::end(sc);
}

#[test]
#[expected_failure(abort_code = vault::EVaultPaused)]
fun mint_rejects_when_paused() {
    let mut sc = ts::begin(ADMIN);
    stand_up(&mut sc, PER_CELL, DIR_BPS, BUFFER_BPS, MAX_MULT, 1_000_000, 1_000_000);
    ts::next_tx(&mut sc, ADMIN);
    {
        let admin = ts::take_from_sender<AdminCap>(&sc);
        let mut v = ts::take_shared<GameVault<COIN_DUMMY>>(&sc);
        vault::set_paused(&admin, &mut v, true);
        ts::return_shared(v);
        ts::return_to_sender(&sc, admin);
    };
    ts::next_tx(&mut sc, PLAYER);
    let mut v = ts::take_shared<GameVault<COIN_DUMMY>>(&sc);
    let mut pb = ts::take_shared<PlayerBalance<COIN_DUMMY>>(&sc);
    vault::mint<COIN_DUMMY>(&mut v, &mut pb, 0, 1, 2, 0, 60, 10_000, 19_600, 1, 1, true, ts::ctx(&mut sc));
    abort 0
}

#[test]
#[expected_failure(abort_code = vault::ECapVaultMismatch)]
fun settle_rejects_wrong_cap() {
    let mut sc = ts::begin(ADMIN);
    // Vault A (settler SETTLER) holds the position; capture its id.
    stand_up(&mut sc, PER_CELL, DIR_BPS, BUFFER_BPS, MAX_MULT, 1_000_000, 1_000_000);
    mint_default(&mut sc);
    ts::next_tx(&mut sc, ADMIN); // commit the mint so the shared ids are visible
    let vault_a = ts::most_recent_id_shared<GameVault<COIN_DUMMY>>().destroy_some();
    let pos_id = ts::most_recent_id_shared<Position<COIN_DUMMY>>().destroy_some();
    // Vault B with a different settler (@0x6) → @0x6 holds a cap bound to vault B.
    ts::next_tx(&mut sc, ADMIN);
    { vault::create_vault<COIN_DUMMY>(@0x6, PER_CELL, DIR_BPS, BUFFER_BPS, MAX_MULT, ts::ctx(&mut sc)); };
    ts::next_tx(&mut sc, @0x6);
    {
        let cap_b = ts::take_from_sender<SettlerCap>(&sc); // bound to vault B
        let mut v_a = ts::take_shared_by_id<GameVault<COIN_DUMMY>>(&sc, vault_a);
        let mut pos = ts::take_shared_by_id<Position<COIN_DUMMY>>(&sc, pos_id);
        let mut pb = ts::take_shared<PlayerBalance<COIN_DUMMY>>(&sc);
        vault::settle_win(&cap_b, &mut v_a, &mut pos, &mut pb); // cap.vault_id != A → abort
        ts::return_shared(v_a);
        ts::return_shared(pos);
        ts::return_shared(pb);
        ts::return_to_sender(&sc, cap_b);
    };
    ts::end(sc);
}

#[test]
/// Correlated-win solvency: pack several bullish positions, settle every one as
/// a win, and assert the treasury covers every payout without underflowing
/// (Balance::split would abort if it couldn't). ADR-0010 §5 intent.
fun solvency_under_correlated_wins() {
    let mut sc = ts::begin(ADMIN);
    // 100% directional cap, 10% buffer, large reserve so 5 × (50_000 @ 2x)
    // bullish positions all admit; payouts total 500_000 against a 1_250_000
    // post-stake treasury.
    stand_up(&mut sc, PER_CELL, 10_000, 1_000, MAX_MULT, 1_000_000, 1_000_000);
    let mut pos_ids = vector::empty<ID>();
    let mut i = 0;
    while (i < 5) {
        ts::next_tx(&mut sc, PLAYER);
        {
            let mut v = ts::take_shared<GameVault<COIN_DUMMY>>(&sc);
            let mut pb = ts::take_shared<PlayerBalance<COIN_DUMMY>>(&sc);
            vault::mint<COIN_DUMMY>(&mut v, &mut pb, 0, 100, 200, 0, 60, 50_000, 20_000, i, 1, true, ts::ctx(&mut sc));
            ts::return_shared(v);
            ts::return_shared(pb);
        };
        ts::next_tx(&mut sc, PLAYER);
        pos_ids.push_back(ts::most_recent_id_shared<Position<COIN_DUMMY>>().destroy_some());
        i = i + 1;
    };
    let mut j = 0;
    while (j < 5) {
        ts::next_tx(&mut sc, SETTLER);
        {
            let cap = ts::take_from_sender<SettlerCap>(&sc);
            let mut v = ts::take_shared<GameVault<COIN_DUMMY>>(&sc);
            let mut pos = ts::take_shared_by_id<Position<COIN_DUMMY>>(&sc, *pos_ids.borrow(j));
            let mut pb = ts::take_shared<PlayerBalance<COIN_DUMMY>>(&sc);
            vault::settle_win(&cap, &mut v, &mut pos, &mut pb);
            ts::return_shared(v);
            ts::return_shared(pos);
            ts::return_shared(pb);
            ts::return_to_sender(&sc, cap);
        };
        j = j + 1;
    };
    ts::next_tx(&mut sc, SETTLER);
    {
        let v = ts::take_shared<GameVault<COIN_DUMMY>>(&sc);
        assert_eq(vault::total_open_liability(&v), 0);
        assert_eq(vault::treasury_value(&v), 750_000); // 1_250_000 − 5 × 100_000
        ts::return_shared(v);
    };
    ts::end(sc);
}
