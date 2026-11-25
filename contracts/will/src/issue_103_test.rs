//! Tests for issue #244: maintain retained history and sequence TTL alongside a live will.
//!
//! These tests verify that:
//! - Maintained live wills refresh both existing history keys (history + sequence).
//! - A missing counter cannot silently restart numbering while retained history
//!   already contains larger sequences.
//! - Explicit archival still removes both keys; terminal retention is not extended.

use crate::storage::{
    self, append_history, get_history, get_history_seq, next_history_seq, save_will,
    HistoryEntry,
};
use crate::{Will, WillStatus};
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::{Address, Env, String, Vec};

use crate::{Allocation, Beneficiary, WillContract, WillContractClient, WillError, WillStatus};

/// Helper: build a minimal live will for the given owner.
fn make_live_will(env: &Env, owner: &Address) -> Will {
    Will {
        owner: owner.clone(),
        token: Address::generate(env),
        amount: 1_000,
        beneficiaries: Vec::new(env),
        guardians: Vec::new(env),
        status: WillStatus::Active,
        last_activity: env.ledger().timestamp(),
        created_at: env.ledger().timestamp(),
    }
}

/// Helper: build a history entry with the given sequence.
fn make_entry(env: &Env, seq: u32) -> HistoryEntry {
    HistoryEntry {
        seq,
        timestamp: env.ledger().timestamp(),
        kind: String::from_str(env, "test"),
        actor: Address::generate(env),
    }
}

/// Advance the ledger by `seconds` to age TTLs.
fn age_ledger(env: &Env, seconds: u64) {
    let current = env.ledger().timestamp();
    env.ledger().set_timestamp(current + seconds);
}

fn setup(env: &Env) -> (WillContractClient<'_>, Address, Address) {
    let owner = Address::generate(env);
    let sac = env.register_stellar_asset_contract_v2(owner.clone());
    let token_address = sac.address();
    StellarAssetClient::new(env, &token_address).mint(&owner, &10_000_000);

    let contract_id = env.register(WillContract, ());
    let client = WillContractClient::new(env, &contract_id);
    (client, owner, token_address)
}

fn make_create_call<'a>(
    client: &WillContractClient<'a>,
    env: &Env,
    owner: &Address,
    token_address: &Address,
    beneficiary: &Address,
    delay_seconds: u64,
) -> u64 {
    client.create_will(
        owner,
        &vec![env, (token_address.clone(), 1_000_000_i128)],
        &vec![
            env,
            Beneficiary {
                address: beneficiary.clone(),
                allocation: Allocation::Percentage(10_000),
            },
        ],
        &90,
        &7,
        &vec![env],
        &0,
        &None,
        &delay_seconds,
    )
}

#[test]
fn live_will_refreshes_history_and_seq_ttl_on_settings_save() {
    let env = Env::default();
    env.mock_all_auths();
    let owner = Address::generate(&env);

    // Seed a live will with history and a sequence counter.
    let will = make_live_will(&env, &owner);
    save_will(&env, &owner, &will);
    append_history(&env, &owner, &make_entry(&env, 1));
    append_history(&env, &owner, &make_entry(&env, 2));
    let seq_before = get_history_seq(&env, &owner);
    assert_eq!(seq_before, 2);

    // Age the ledger significantly (but keep the will live).
    age_ledger(&env, 30 * 24 * 60 * 60);

    // A non-history settings save (e.g. changing amount) should refresh the
    // will entry and, per the retention policy, both history keys.
    let mut updated = will.clone();
    updated.amount = 2_000;
    save_will(&env, &owner, &updated);

    // All three keys should still be readable (not expired).
    let will_ttl = env.storage().persistent().get_ttl(&storage::will_key(&owner));
    let history_ttl = env
        .storage()
        .persistent()
        .get_ttl(&storage::history_key(&owner));
    let seq_ttl = env
        .storage()
        .persistent()
        .get_ttl(&storage::history_seq_key(&owner));

    assert!(will_ttl > 0, "will entry TTL should be refreshed");
    assert!(history_ttl > 0, "history TTL should be refreshed");
    assert!(seq_ttl > 0, "sequence TTL should be refreshed");

    // Appending after the settings save should continue numbering.
    let next = next_history_seq(&env, &owner);
    assert_eq!(next, 3, "sequence must continue after settings save");
    append_history(&env, &owner, &make_entry(&env, next));
    assert_eq!(get_history_seq(&env, &owner), 3);
}

#[test]
fn missing_counter_does_not_restart_numbering_when_history_exists() {
    let env = Env::default();
    env.mock_all_auths();
    let owner = Address::generate(&env);

    let will = make_live_will(&env, &owner);
    save_will(&env, &owner, &will);
    append_history(&env, &owner, &make_entry(&env, 1));
    append_history(&env, &owner, &make_entry(&env, 2));
    append_history(&env, &owner, &make_entry(&env, 3));
    assert_eq!(get_history_seq(&env, &owner), 3);

    // Simulate a restored state where the counter key is missing but history
    // remains. The next sequence must not restart at 1.
    env.storage()
        .persistent()
        .remove(&storage::history_seq_key(&owner));
    assert_eq!(get_history_seq(&env, &owner), 0, "counter is missing");

    let next = next_history_seq(&env, &owner);
    assert!(
        next > 3,
        "missing counter must not restart numbering below retained history (got {})",
        next
    );

    // Appending should preserve monotonicity relative to retained history.
    append_history(&env, &owner, &make_entry(&env, next));
    let history = get_history(&env, &owner);
    let max_seq = history.iter().map(|e| e.seq).max().unwrap_or(0);
    assert_eq!(max_seq, next);
}

#[test]
fn missing_history_with_counter_preserves_counter() {
    let env = Env::default();
    env.mock_all_auths();
    let owner = Address::generate(&env);

    let will = make_live_will(&env, &owner);
    save_will(&env, &owner, &will);
    append_history(&env, &owner, &make_entry(&env, 1));
    append_history(&env, &owner, &make_entry(&env, 2));
    assert_eq!(get_history_seq(&env, &owner), 2);

    // Simulate restored state where history is missing but counter remains.
    env.storage()
        .persistent()
        .remove(&storage::history_key(&owner));

    // Counter should still be authoritative.
    assert_eq!(get_history_seq(&env, &owner), 2);
    let next = next_history_seq(&env, &owner);
    assert_eq!(next, 3, "counter must be preserved when history is missing");
}

#[test]
fn both_keys_missing_starts_fresh() {
    let env = Env::default();
    env.mock_all_auths();
    let owner = Address::generate(&env);

    let will = make_live_will(&env, &owner);
    save_will(&env, &owner, &will);

    // No history or counter yet.
    assert_eq!(get_history_seq(&env, &owner), 0);
    let next = next_history_seq(&env, &owner);
    assert_eq!(next, 1, "fresh state should start at 1");
}

#[test]
fn archival_removes_history_and_seq_keys() {
    let env = Env::default();
    env.mock_all_auths();
    let owner = Address::generate(&env);

    let will = make_live_will(&env, &owner);
    save_will(&env, &owner, &will);
    append_history(&env, &owner, &make_entry(&env, 1));
    append_history(&env, &owner, &make_entry(&env, 2));
    assert_eq!(get_history_seq(&env, &owner), 2);

    // Explicit archival should remove both history keys.
    storage::archive_will(&env, &owner);

    assert!(
        !env.storage()
            .persistent()
            .has(&storage::history_key(&owner)),
        "archival must remove history key"
    );
    assert!(
        !env.storage()
            .persistent()
            .has(&storage::history_seq_key(&owner)),
        "archival must remove sequence key"
    );
}

#[test]
fn create_will_rejects_delay_below_threshold() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_700_000_000);

    let heir = Address::generate(&env);
    let (client, owner, token_address) = setup(&env);

    let err = client
        .try_create_will(
            &owner,
            &vec![&env, (token_address.clone(), 1_000_000_i128)],
            &vec![
                &env,
                Beneficiary {
                    address: heir.clone(),
                    allocation: Allocation::Percentage(10_000),
                },
            ],
            &90,
            &7,
            &vec![&env],
            &0,
            &None,
            &5, // below MIN_CONFIRMATION_DELAY (300s)
        )
        .unwrap_err()
        .unwrap();
    assert_eq!(
        err,
        WillError::InvalidPeriod.into(),
        "confirmation delay of 5s must be rejected"
    );
}

#[test]
fn archival_does_not_extend_terminal_retention() {
    let env = Env::default();
    env.mock_all_auths();
    let owner = Address::generate(&env);

    let will = make_live_will(&env, &owner);
    save_will(&env, &owner, &will);
    append_history(&env, &owner, &make_entry(&env, 1));

    // Archive the will (terminal state).
    storage::archive_will(&env, &owner);

    // Capture TTLs of the terminal will entry.
    let will_ttl_before = env
        .storage()
        .persistent()
        .get_ttl(&storage::will_key(&owner));

    // Age the ledger.
    age_ledger(&env, 7 * 24 * 60 * 60);

    // A subsequent save on a terminal will must not extend retention.
    let mut terminal = will.clone();
    terminal.status = WillStatus::Archived;
    save_will(&env, &owner, &terminal);

    let will_ttl_after = env
        .storage()
        .persistent()
        .get_ttl(&storage::will_key(&owner));

    assert!(
        will_ttl_after <= will_ttl_before,
        "terminal retention must not be extended by save_will"
    );

    // History keys must remain absent.
    assert!(!env
        .storage()
        .persistent()
        .has(&storage::history_key(&owner)));
    assert!(!env
        .storage()
        .persistent()
        .has(&storage::history_seq_key(&owner)));
}

#[test]
fn create_will_accepts_delay_at_threshold() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_700_000_000);

    let heir = Address::generate(&env);
    let (client, owner, token_address) = setup(&env);

    // Exactly MIN_CONFIRMATION_DELAY (300s) is accepted.
    let will_id = make_create_call(&client, &env, &owner, &token_address, &heir, 300);
    assert_eq!(
        client.get_will(&will_id).status,
        WillStatus::PendingConfirmation,
        "delay at threshold starts PendingConfirmation"
    );
}
