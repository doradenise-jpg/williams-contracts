#![cfg(test)]

use crate::storage::{
    recompute_protocol_stats_chunk, DataKey, ProtocolStats, MAX_CHUNK_SIZE,
};
use crate::{Allocation, Beneficiary, WillContract, WillContractClient, MIN_CHECKIN_INTERVAL};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, Env, Vec,
};

fn setup_env() -> Env {
    let env = Env::default();
    env.mock_all_auths();
    env
}

fn register_will(
    env: &Env,
    client: &WillContractClient,
    owner: &Address,
    token: &Address,
    amount: i128,
) -> u64 {
    client.create_will(owner, token, &amount, &1000u32, &Vec::new(env))
}

fn seed_wills(env: &Env, client: &WillContractClient, count: u64) -> (Address, Address) {
    let owner = Address::generate(env);
    let token = Address::generate(env);
    for _ in 0..count {
        register_will(env, client, &owner, &token, 100);
    }
    (owner, token)
}

#[test]
fn test_zero_chunk_size_does_not_report_completion_on_populated_registry() {
    let env = setup_env();
    let contract_id = env.register_contract(None, WillContract);
    let client = WillContractClient::new(&env, &contract_id);

    seed_wills(&env, &client, 5);

    let (stats, next_start, done) = recompute_protocol_stats_chunk(&env, 1, 0);

    assert!(
        !done,
        "zero-sized chunk must not report a populated registry as fully scanned"
    );
    assert_eq!(stats.total_wills, 0, "zero-sized chunk must not count records");
    assert_eq!(next_start, 1, "zero-sized chunk must not advance the cursor");
}

#[test]
fn test_zero_chunk_size_on_empty_registry() {
    let env = setup_env();
    let contract_id = env.register_contract(None, WillContract);
    let _client = WillContractClient::new(&env, &contract_id);

    let (stats, next_start, done) = recompute_protocol_stats_chunk(&env, 1, 0);

    assert!(done, "empty registry should be reported complete");
    assert_eq!(stats.total_wills, 0);
    assert_eq!(next_start, 1);
}

#[test]
fn test_chunk_size_one_advances_by_one() {
    let env = setup_env();
    let contract_id = env.register_contract(None, WillContract);
    let client = WillContractClient::new(&env, &contract_id);

    seed_wills(&env, &client, 3);

    let (stats, next_start, done) = recompute_protocol_stats_chunk(&env, 1, 1);

    assert_eq!(stats.total_wills, 1, "chunk of size one counts one record");
    assert!(!done, "more records remain after first chunk");
    assert_eq!(next_start, 2, "cursor advances by exactly one");
}

#[test]
fn test_chunk_size_at_cap_is_accepted() {
    let env = setup_env();
    let contract_id = env.register_contract(None, WillContract);
    let client = WillContractClient::new(&env, &contract_id);

    seed_wills(&env, &client, 3);

    let (stats, next_start, done) =
        recompute_protocol_stats_chunk(&env, 1, MAX_CHUNK_SIZE);

    assert_eq!(stats.total_wills, 3, "cap-sized chunk counts all records");
    assert!(done, "cap-sized chunk covers the whole registry");
    assert_eq!(next_start, 4, "cursor advances past the last record");
}

#[test]
fn test_chunk_size_above_cap_is_rejected() {
    let env = setup_env();
    let contract_id = env.register_contract(None, WillContract);
    let client = WillContractClient::new(&env, &contract_id);

    seed_wills(&env, &client, 3);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        recompute_protocol_stats_chunk(&env, 1, MAX_CHUNK_SIZE + 1)
    }));

    assert!(
        result.is_err(),
        "chunk size above the supported cap must be rejected"
    );
}

#[test]
fn test_start_beyond_registry_reports_completion() {
    let env = setup_env();
    let contract_id = env.register_contract(None, WillContract);
    let client = WillContractClient::new(&env, &contract_id);

    seed_wills(&env, &client, 3);

    let (stats, next_start, done) = recompute_protocol_stats_chunk(&env, 100, 10);

    assert_eq!(stats.total_wills, 0, "no records beyond the registry");
    assert!(done, "start beyond registry is complete");
    assert_eq!(next_start, 100, "cursor does not advance past registry");
}

#[test]
fn test_start_near_u64_max_does_not_trap() {
    let env = setup_env();
    let contract_id = env.register_contract(None, WillContract);
    let _client = WillContractClient::new(&env, &contract_id);

    let start = u64::MAX - 1;
    let (stats, next_start, done) = recompute_protocol_stats_chunk(&env, start, 10);

    assert_eq!(stats.total_wills, 0);
    assert!(done, "start near u64::MAX is complete");
    assert_eq!(next_start, start, "cursor must not wrap past u64::MAX");
}

#[test]
fn test_start_at_u64_max_does_not_trap() {
    let env = setup_env();
    let contract_id = env.register_contract(None, WillContract);
    let _client = WillContractClient::new(&env, &contract_id);

    let (stats, next_start, done) = recompute_protocol_stats_chunk(&env, u64::MAX, 10);

    assert_eq!(stats.total_wills, 0);
    assert!(done, "start at u64::MAX is complete");
    assert_eq!(next_start, u64::MAX, "cursor must not wrap past u64::MAX");
}

#[test]
fn test_accumulated_chunks_match_expected_totals() {
    let env = setup_env();
    let contract_id = env.register_contract(None, WillContract);
    let client = WillContractClient::new(&env, &contract_id);

    let total = 25u64;
    seed_wills(&env, &client, total);

    let mut accumulated = ProtocolStats::default();
    let mut start = 1u64;
    let mut iterations = 0u32;
    let chunk_size = 4u32;

    loop {
        let (stats, next_start, done) =
            recompute_protocol_stats_chunk(&env, start, chunk_size);
        accumulated.total_wills += stats.total_wills;
        accumulated.total_balance += stats.total_balance;
        iterations += 1;
        assert!(iterations <= 100, "chunk loop must terminate");
        if done {
            break;
        }
        assert!(next_start > start, "accepted chunk must advance the cursor");
        start = next_start;
    }

    assert_eq!(
        accumulated.total_wills, total,
        "accumulated chunks must count each live record exactly once"
    );
}

#[test]
fn test_accumulated_chunks_on_empty_registry() {
    let env = setup_env();
    let contract_id = env.register_contract(None, WillContract);
    let _client = WillContractClient::new(&env, &contract_id);

    let (stats, _next_start, done) = recompute_protocol_stats_chunk(&env, 1, 10);

    assert_eq!(stats.total_wills, 0);
    assert!(done, "empty registry completes on first chunk");
}

#[test]
fn test_rejected_chunk_leaves_state_unchanged() {
    let env = setup_env();
    let contract_id = env.register_contract(None, WillContract);
    let client = WillContractClient::new(&env, &contract_id);

    seed_wills(&env, &client, 3);

    let before: ProtocolStats = env
        .storage()
        .instance()
        .get(&DataKey::ProtocolStats)
        .unwrap_or_default();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        recompute_protocol_stats_chunk(&env, 1, MAX_CHUNK_SIZE + 1)
    }));
    assert!(result.is_err(), "oversized chunk must be rejected");

    let after: ProtocolStats = env
        .storage()
        .instance()
        .get(&DataKey::ProtocolStats)
        .unwrap_or_default();

    assert_eq!(
        before.total_wills, after.total_wills,
        "rejected chunk must not mutate stored stats"
    );
    assert_eq!(
        before.total_balance, after.total_balance,
        "rejected chunk must not mutate stored balance"
    );
}