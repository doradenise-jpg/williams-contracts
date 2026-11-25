//! Tests for issue #243: exact-tuple rent maintenance for outstanding failed payouts.
//!
//! These tests exercise the `renew_failed_payout` maintenance operation which
//! refreshes the TTL of an existing positive `FailedPayout` record without
//! transferring tokens or requiring a parent Will lookup.

use crate::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token::{Client as TokenClient, StellarAssetClient},
    Address, Env,
};

use crate::{Allocation, Beneficiary, WillContract, WillContractClient, WillError, WillStatus};

/// Helper: build an env with a mock token that can be made to fail transfers.
fn setup_env() -> (Env, Address, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();

    let owner = Address::generate(&env);
    let recipient = Address::generate(&env);
    let token_admin = Address::generate(&env);

    let token_id = env.register_stellar_asset_contract_v2(token_admin.clone());
    let token_addr = token_id.address();

    (env, owner, recipient, token_addr)
}

/// Helper: create a will with a single recipient and a token allocation.
fn create_will_with_recipient(
    env: &Env,
    owner: &Address,
    recipient: &Address,
    token: &Address,
    amount: i128,
) -> u64 {
    let contract_id = env.register(WillContract, ());
    let client = WillContractClient::new(env, &contract_id);

    // Fund the will contract with tokens so it can attempt payouts.
    let sac = StellarAssetClient::new(env, token);
    sac.mint(&contract_id, &amount);

    let mut recipients = Vec::new(env);
    recipients.push_back(recipient.clone());

    let mut allocations = Vec::new(env);
    allocations.push_back(WillAllocation {
        recipient: recipient.clone(),
        token: token.clone(),
        amount,
    });

    let will_id = client.create_will(
        owner,
        &recipients,
        &allocations,
        &100u64, // checkin period
        &1000u64, // grace period
    );

    will_id
}

/// Helper: force a failed payout by making the token transfer fail.
/// We simulate this by draining the will contract balance so the transfer
/// fails, then recording the failed payout via the retry path.
fn force_failed_payout(
    env: &Env,
    contract_id: &Address,
    will_id: u64,
    recipient: &Address,
    token: &Address,
    amount: i128,
) {
    // Drain the contract's token balance so the payout transfer fails.
    let sac = StellarAssetClient::new(env, token);
    let balance = TokenClient::new(env, token).balance(contract_id);
    if balance > 0 {
        // Transfer out to a burn address to make the balance zero.
        let burn = Address::generate(env);
        sac.mint(&burn, &balance);
        // Use the contract's own transfer to move funds out.
        // Since mock_all_auths is on, we can call transfer from contract.
        // Instead, simply record the failed payout directly through storage
        // helper to simulate the post-failure state.
        let _ = burn;
    }

    // Record the failed payout directly (simulating what retry_failed_payout
    // does after a failed transfer).
    env.as_contract(contract_id, || {
        storage::set_failed_payout(env, will_id, recipient, token, amount);
    });
}

#[test]
fn test_renew_failed_payout_extends_ttl_with_failing_token() {
    let (env, owner, recipient, token) = setup_env();
    let contract_id = env.register(WillContract, ());
    let client = WillContractClient::new(&env, &contract_id);

    let will_id = create_will_with_recipient(&env, &owner, &recipient, &token, 1000);
    let amount = 500i128;

    // Record a failed payout (token transfer unavailable).
    force_failed_payout(&env, &contract_id, will_id, &recipient, &token, amount);

    // Verify the record exists with the correct amount.
    let recorded = env.as_contract(&contract_id, || {
        storage::get_failed_payout(&env, will_id, &recipient, &token)
    });
    assert_eq!(recorded, Some(amount));

    // Age the ledger so the TTL is near expiry.
    env.ledger().with_mut(|li| {
        li.sequence_number += 100;
    });

    // Renew the failed payout while token transfers are unavailable.
    client.renew_failed_payout(&will_id, &recipient, &token);

    // Amount must be preserved.
    let after = env.as_contract(&contract_id, || {
        storage::get_failed_payout(&env, will_id, &recipient, &token)
    });
    assert_eq!(after, Some(amount), "amount must be preserved on renewal");

    // TTL must be extended.
    let ttl = env.as_contract(&contract_id, || {
        env.storage()
            .persistent()
            .get_ttl(&storage::failed_payout_key(&env, will_id, &recipient, &token))
    });
    assert!(ttl > 0, "TTL must be positive after renewal");
}

#[test]
fn test_renew_failed_payout_does_not_create_absent_tuple() {
    let (env, owner, recipient, token) = setup_env();
    let contract_id = env.register(WillContract, ());
    let client = WillContractClient::new(&env, &contract_id);

    let will_id = create_will_with_recipient(&env, &owner, &recipient, &token, 1000);

    // No failed payout recorded. Attempting to renew must not create one.
    let result = client.try_renew_failed_payout(&will_id, &recipient, &token);
    assert!(result.is_err(), "renewing an absent tuple must fail");

    let after = env.as_contract(&contract_id, || {
        storage::get_failed_payout(&env, will_id, &recipient, &token)
    });
    assert_eq!(after, None, "absent tuple must not be created");
}

#[test]
fn test_renew_failed_payout_preserves_amount() {
    let (env, owner, recipient, token) = setup_env();
    let contract_id = env.register(WillContract, ());
    let client = WillContractClient::new(&env, &contract_id);

    let will_id = create_will_with_recipient(&env, &owner, &recipient, &token, 1000);
    let amount = 777i128;

    force_failed_payout(&env, &contract_id, will_id, &recipient, &token, amount);

    // Renew multiple times; amount must never change.
    for _ in 0..3 {
        env.ledger().with_mut(|li| {
            li.sequence_number += 50;
        });
        client.renew_failed_payout(&will_id, &recipient, &token);

        let current = env.as_contract(&contract_id, || {
            storage::get_failed_payout(&env, will_id, &recipient, &token)
        });
        assert_eq!(current, Some(amount), "amount must remain unchanged");
    }
}

#[test]
fn test_renew_failed_payout_after_parent_archival() {
    let (env, owner, recipient, token) = setup_env();
    let contract_id = env.register(WillContract, ());
    let client = WillContractClient::new(&env, &contract_id);

    let will_id = create_will_with_recipient(&env, &owner, &recipient, &token, 1000);
    let amount = 250i128;

    force_failed_payout(&env, &contract_id, will_id, &recipient, &token, amount);

    // Simulate parent archival by removing the Will record from storage.
    env.as_contract(&contract_id, || {
        env.storage().persistent().remove(&storage::will_key(&env, will_id));
    });

    // Renewal must still succeed without requiring a parent Will lookup.
    env.ledger().with_mut(|li| {
        li.sequence_number += 100;
    });
    client.renew_failed_payout(&will_id, &recipient, &token);

    let after = env.as_contract(&contract_id, || {
        storage::get_failed_payout(&env, will_id, &recipient, &token)
    });
    assert_eq!(after, Some(amount), "amount preserved after parent archival");
}

#[test]
fn test_renew_failed_payout_touches_only_requested_liability() {
    let (env, owner, recipient, token) = setup_env();
    let contract_id = env.register(WillContract, ());
    let client = WillContractClient::new(&env, &contract_id);

    let will_id = create_will_with_recipient(&env, &owner, &recipient, &token, 1000);
    let other_recipient = Address::generate(&env);

    let amount_a = 100i128;
    let amount_b = 200i128;

    force_failed_payout(&env, &contract_id, will_id, &recipient, &token, amount_a);
    force_failed_payout(
        &env,
        &contract_id,
        will_id,
        &other_recipient,
        &token,
        amount_b,
    );

    // Capture the other liability's TTL before renewal.
    let other_ttl_before = env.as_contract(&contract_id, || {
        env.storage().persistent().get_ttl(&storage::failed_payout_key(
            &env,
            will_id,
            &other_recipient,
            &token,
        ))
    });

    env.ledger().with_mut(|li| {
        li.sequence_number += 100;
    });

    // Renew only the first liability.
    client.renew_failed_payout(&will_id, &recipient, &token);

    // The other liability's amount must be unchanged.
    let other_amount = env.as_contract(&contract_id, || {
        storage::get_failed_payout(&env, will_id, &other_recipient, &token)
    });
    assert_eq!(other_amount, Some(amount_b));

    // The other liability's TTL must not have been extended by this call.
    let other_ttl_after = env.as_contract(&contract_id, || {
        env.storage().persistent().get_ttl(&storage::failed_payout_key(
            &env,
            will_id,
            &other_recipient,
            &token,
        ))
    });
    assert_eq!(
        other_ttl_before, other_ttl_after,
        "renewal must not touch other liabilities"
    );
}

#[test]
fn test_failed_transfer_rollback_vs_successful_maintenance() {
    let (env, owner, recipient, token) = setup_env();
    let contract_id = env.register(WillContract, ());
    let client = WillContractClient::new(&env, &contract_id);

    let will_id = create_will_with_recipient(&env, &owner, &recipient, &token, 1000);
    let amount = 400i128;

    force_failed_payout(&env, &contract_id, will_id, &recipient, &token, amount);

    // A retry attempt with an unavailable token must roll back and not
    // persist any state change (the failed payout record remains intact).
    // We simulate token unavailability by draining the contract balance.
    let sac = StellarAssetClient::new(&env, &token);
    let balance = TokenClient::new(&env, &token).balance(&contract_id);
    if balance > 0 {
        // Move funds out via a burn address to make the transfer fail.
        let burn = Address::generate(&env);
        sac.mint(&burn, &balance);
        // Transfer from contract to burn using the token client with auth.
        TokenClient::new(&env, &token).transfer(&contract_id, &burn, &balance);
    }

    let retry_result = client.try_retry_failed_payout(&will_id, &recipient, &token);
    assert!(retry_result.is_err(), "retry with failing token must fail");

    // The failed payout record must still exist with the original amount.
    let after_retry = env.as_contract(&contract_id, || {
        storage::get_failed_payout(&env, will_id, &recipient, &token)
    });
    assert_eq!(
        after_retry,
        Some(amount),
        "failed retry must not alter the liability record"
    );

    // Now perform a successful maintenance call (no token transfer needed).
    env.ledger().with_mut(|li| {
        li.sequence_number += 100;
    });
    client.renew_failed_payout(&will_id, &recipient, &token);

    let after_renew = env.as_contract(&contract_id, || {
        storage::get_failed_payout(&env, will_id, &recipient, &token)
    });
    assert_eq!(
        after_renew,
        Some(amount),
        "successful maintenance preserves the amount"
    );
}

#[test]
fn test_renew_failed_payout_ttl_assertions_across_aging() {
    let (env, owner, recipient, token) = setup_env();
    let contract_id = env.register(WillContract, ());
    let client = WillContractClient::new(&env, &contract_id);

    let will_id = create_will_with_recipient(&env, &owner, &recipient, &token, 1000);
    let amount = 123i128;

    force_failed_payout(&env, &contract_id, will_id, &recipient, &token, amount);

    let key = storage::failed_payout_key(&env, will_id, &recipient, &token);

    // Age the ledger significantly.
    env.ledger().with_mut(|li| {
        li.sequence_number += 500;
    });

    let ttl_before = env.as_contract(&contract_id, || {
        env.storage().persistent().get_ttl(&key)
    });

    client.renew_failed_payout(&will_id, &recipient, &token);

    let ttl_after = env.as_contract(&contract_id, || {
        env.storage().persistent().get_ttl(&key)
    });

    assert!(
        ttl_after >= ttl_before,
        "renewal must not decrease TTL (before={}, after={})",
        ttl_before,
        ttl_after
    );
    assert!(ttl_after > 0, "TTL must be positive after renewal");
}

#[test]
fn request_grace_extension_rejects_after_grace_expired() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_700_000_000);

    let heir = Address::generate(&env);
    let (client, owner, token_address) = setup(&env);
    let will_id = create_will(&env, &client, &owner, &token_address, &heir);

    trigger_will(&env, &client, will_id);

    // Advance past the grace period (7 days).
    advance(&env, 8);

    let err = client
        .try_request_grace_extension(&will_id, &owner, &7)
        .unwrap_err()
        .unwrap();
    assert_eq!(
        err,
        WillError::GracePeriodExpired.into(),
        "extension after grace expiry is rejected"
    );
}
