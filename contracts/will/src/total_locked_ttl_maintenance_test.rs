#![cfg(test)]

//! Regression and unit tests for TotalLocked TTL maintenance (ADR 0003).
//!
//! Discrete `TotalLocked(token)` entries must stay alive (not archived) while the wills
//! that contribute to them remain active. This module verifies that:
//! 1. `check_in` refreshes TTL on all token aggregates.
//! 2. `emergency_checkin` refreshes TTL on all token aggregates.
//! 3. `batch_check_in` refreshes TTL on all token aggregates for each will.
//! 4. Zero/non-existent aggregates are not unnecessarily refreshed.

use soroban_sdk::{
    testutils::{
        storage::{Instance as _, Persistent as _},
        Address as _, Ledger,
    },
    token::StellarAssetClient,
    vec, Address, Env,
};

use crate::storage::{self, DataKey, BUMP_AMOUNT, LIFETIME_THRESHOLD};
use crate::types::Allocation;
use crate::{WillContract, WillContractClient};

const DAY: u64 = 86_400;

fn setup<'a>() -> (Env, WillContractClient<'a>, Address, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_700_000_000);

    let owner = Address::generate(&env);
    let beneficiary = Address::generate(&env);

    let sac_1 = env.register_stellar_asset_contract_v2(owner.clone());
    let token_1 = sac_1.address();
    StellarAssetClient::new(&env, &token_1).mint(&owner, &1_000_000_000);

    let contract_id = env.register(WillContract, ());
    let client = WillContractClient::new(&env, &contract_id);

    (env, client, owner, token_1, beneficiary)
}

fn advance_days(env: &Env, days: u64) {
    env.ledger().with_mut(|l| l.timestamp += days * DAY);
}

fn get_will_ttl(env: &Env, will_id: u64) -> Option<(u32, u32)> {
    env.storage()
        .persistent()
        .get_ttl(&DataKey::Will(will_id))
}

fn get_total_locked_ttl(env: &Env, token: &Address) -> Option<(u32, u32)> {
    env.storage()
        .persistent()
        .get_ttl(&DataKey::TotalLocked(token.clone()))
}

// ─────────────────────────────────────────────────────────────────────────
// Test: check_in refreshes TotalLocked TTL
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn check_in_refreshes_total_locked_ttl() {
    let (env, client, owner, token, beneficiary) = setup();

    // Create a will with a stable balance
    let will_id = client.create_will(
        &vec![&env, token.clone()],
        &vec![&env, 500_000_000i128],
        &vec![&env, Allocation::Percentage(10_000)],
        &vec![&env, beneficiary.clone()],
        &90,  // 90-day check-in period
        &None,
        &None,
        &None,
        &None,
        &None,
    );

    // Check-in period is 90 days, so check that the will is Active
    client.confirm_will(&will_id, &owner);

    // Record initial TTLs
    let initial_will_ttl = get_will_ttl(&env, will_id).expect("Will TTL should exist");
    let initial_total_locked_ttl = get_total_locked_ttl(&env, &token).expect("TotalLocked TTL should exist");

    // Advance time slightly (but not past check-in deadline)
    advance_days(&env, 10);

    // Perform check-in
    client.check_in(&will_id, &owner);

    // After check-in, both TTLs should be extended
    let new_will_ttl = get_will_ttl(&env, will_id).expect("Will TTL should still exist");
    let new_total_locked_ttl = get_total_locked_ttl(&env, &token).expect("TotalLocked TTL should still exist");

    // The deadline should have moved forward (bumped to BUMP_AMOUNT)
    assert!(new_will_ttl.0 > initial_will_ttl.0, "Will TTL should be extended");
    assert!(
        new_total_locked_ttl.0 > initial_total_locked_ttl.0,
        "TotalLocked TTL should be extended during check_in"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Test: emergency_checkin refreshes TotalLocked TTL
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn emergency_checkin_refreshes_total_locked_ttl() {
    let (env, client, owner, token, beneficiary) = setup();

    // Create and confirm a will
    let will_id = client.create_will(
        &vec![&env, token.clone()],
        &vec![&env, 500_000_000i128],
        &vec![&env, Allocation::Percentage(10_000)],
        &vec![&env, beneficiary.clone()],
        &90,  // 90-day check-in period
        &None,
        &None,
        &None,
        &None,
        &None,
    );
    client.confirm_will(&will_id, &owner);

    // Advance past the check-in deadline to trigger the will
    advance_days(&env, 100);
    client.trigger_will(&will_id);

    // Record TTL before emergency check-in
    let triggered_total_locked_ttl = get_total_locked_ttl(&env, &token).expect("TotalLocked TTL should exist after trigger");

    // Perform emergency check-in (brings will back to Active)
    client.emergency_checkin(&will_id, &owner);

    // After emergency check-in, TotalLocked TTL should be extended
    let new_total_locked_ttl = get_total_locked_ttl(&env, &token).expect("TotalLocked TTL should still exist");

    assert!(
        new_total_locked_ttl.0 > triggered_total_locked_ttl.0,
        "TotalLocked TTL should be extended during emergency_checkin"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Test: batch_check_in refreshes TotalLocked TTL for all wills
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn batch_check_in_refreshes_total_locked_ttl() {
    let (env, client, owner, token, beneficiary) = setup();

    // Create two wills
    let will_id_1 = client.create_will(
        &vec![&env, token.clone()],
        &vec![&env, 300_000_000i128],
        &vec![&env, Allocation::Percentage(10_000)],
        &vec![&env, beneficiary.clone()],
        &90,
        &None,
        &None,
        &None,
        &None,
        &None,
    );
    client.confirm_will(&will_id_1, &owner);

    let will_id_2 = client.create_will(
        &vec![&env, token.clone()],
        &vec![&env, 200_000_000i128],
        &vec![&env, Allocation::Percentage(10_000)],
        &vec![&env, beneficiary.clone()],
        &90,
        &None,
        &None,
        &None,
        &None,
        &None,
    );
    client.confirm_will(&will_id_2, &owner);

    // Verify that TotalLocked is the sum of both wills
    let total = storage::get_total_locked(&env, &token);
    assert_eq!(total, 500_000_000i128, "TotalLocked should sum both wills");

    // Record initial TTL
    let initial_ttl = get_total_locked_ttl(&env, &token).expect("TotalLocked TTL should exist");

    // Advance time
    advance_days(&env, 10);

    // Batch check-in both wills
    client.batch_check_in(&vec![&env, will_id_1, will_id_2], &owner);

    // TotalLocked TTL should be extended
    let new_ttl = get_total_locked_ttl(&env, &token).expect("TotalLocked TTL should still exist");

    assert!(
        new_ttl.0 > initial_ttl.0,
        "TotalLocked TTL should be extended during batch_check_in"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Test: check_in does not refresh non-existent aggregates
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn check_in_skips_nonexistent_total_locked_entries() {
    let (env, client, owner, _token, beneficiary) = setup();

    // Create a second token that we'll NOT use in the will
    let sac_unused = env.register_stellar_asset_contract_v2(owner.clone());
    let token_unused = sac_unused.address();

    // Create the first token and a will
    let sac = env.register_stellar_asset_contract_v2(owner.clone());
    let token = sac.address();
    StellarAssetClient::new(&env, &token).mint(&owner, &1_000_000_000);

    let will_id = client.create_will(
        &vec![&env, token.clone()],
        &vec![&env, 500_000_000i128],
        &vec![&env, Allocation::Percentage(10_000)],
        &vec![&env, beneficiary.clone()],
        &90,
        &None,
        &None,
        &None,
        &None,
        &None,
    );
    client.confirm_will(&will_id, &owner);

    // Verify that TotalLocked exists for `token` but not for `token_unused`
    assert!(
        get_total_locked_ttl(&env, &token).is_some(),
        "TotalLocked should exist for used token"
    );
    assert!(
        get_total_locked_ttl(&env, &token_unused).is_none(),
        "TotalLocked should not exist for unused token"
    );

    // Advance time and check-in
    advance_days(&env, 10);
    client.check_in(&will_id, &owner);

    // The unused token should still not have a TotalLocked entry
    // (check_in should not create entries, only refresh existing ones)
    assert!(
        get_total_locked_ttl(&env, &token_unused).is_none(),
        "check_in should not create TotalLocked entries for tokens not in the will"
    );

    // But the used token should still have one (and TTL should be extended)
    assert!(
        get_total_locked_ttl(&env, &token).is_some(),
        "TotalLocked should still exist for used token after check_in"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Test: multi-token will check-in refreshes all token aggregates
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn check_in_refreshes_all_tokens_in_multi_token_will() {
    let (env, client, owner, token_1, beneficiary) = setup();

    // Create second token
    let sac_2 = env.register_stellar_asset_contract_v2(owner.clone());
    let token_2 = sac_2.address();
    StellarAssetClient::new(&env, &token_2).mint(&owner, &1_000_000_000);

    // Create a will with two tokens
    let will_id = client.create_will(
        &vec![&env, token_1.clone(), token_2.clone()],
        &vec![&env, 300_000_000i128, 200_000_000i128],
        &vec![&env, Allocation::Percentage(10_000)],
        &vec![&env, beneficiary.clone()],
        &90,
        &None,
        &None,
        &None,
        &None,
        &None,
    );
    client.confirm_will(&will_id, &owner);

    // Record initial TTLs for both tokens
    let initial_ttl_1 = get_total_locked_ttl(&env, &token_1).expect("TotalLocked for token_1 should exist");
    let initial_ttl_2 = get_total_locked_ttl(&env, &token_2).expect("TotalLocked for token_2 should exist");

    // Advance time and check-in
    advance_days(&env, 10);
    client.check_in(&will_id, &owner);

    // Both TTLs should be extended
    let new_ttl_1 = get_total_locked_ttl(&env, &token_1).expect("TotalLocked for token_1 should still exist");
    let new_ttl_2 = get_total_locked_ttl(&env, &token_2).expect("TotalLocked for token_2 should still exist");

    assert!(
        new_ttl_1.0 > initial_ttl_1.0,
        "TotalLocked TTL for token_1 should be extended"
    );
    assert!(
        new_ttl_2.0 > initial_ttl_2.0,
        "TotalLocked TTL for token_2 should be extended"
    );
}
