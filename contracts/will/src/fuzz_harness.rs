//! Shared fuzzing harness for the Williams contract.
//!
//! The harness is deliberately independent of any particular fuzzing engine.
//! It exposes plain, `Env`-free input structs plus a *runner* per entry point
//! that materialises those inputs into Soroban values, invokes the contract,
//! and asserts the contract's invariants. Two front-ends drive it:
//!
//! - the `proptest` suite in `fuzz_test.rs`, which runs on stable Rust as part
//!   of `cargo test` (and therefore as part of CI);
//! - the `cargo-fuzz`/libFuzzer targets under `fuzz/`, which drive the same
//!   runners with coverage-guided input on nightly.
//!
//! # The central invariant
//!
//! Every entry point must either succeed or fail with a *declared*
//! [`WillError`]. It must never abort. The Soroban test host distinguishes the
//! two for us: `panic_with_error!` surfaces through a `try_*` client method as
//! `Err(Ok(error))`, whereas any other panic — an arithmetic overflow, an
//! `unwrap` on `None`, an out-of-bounds index — surfaces as
//! `Err(Err(InvokeError::Abort))`. An abort means the contract hit a code path
//! its author never considered, so the harness treats it as a hard failure.
//!
//! Beyond "does not abort", each runner checks the state invariants that the
//! contract's documentation claims: shares sum to 10,000, deadlines are
//! representable, custody matches the recorded balance, and the reverse
//! indexes agree with the beneficiary list.
//!
//! # Address slots
//!
//! Soroban `Address`es cannot be generated without an `Env`, so inputs refer to
//! addresses by a small `u8` *slot* index into a fixed pool. This keeps the
//! input structs derivable by `arbitrary`, and — more usefully — makes address
//! collisions common rather than astronomically unlikely, so the fuzzer
//! actually explores duplicate beneficiaries, duplicate guardians, and owners
//! who are also beneficiaries.

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::{Client as TokenClient, StellarAssetClient},
    Address, Env, Vec as SorobanVec,
};

use crate::{
    Allocation, Beneficiary, Will, WillContract, WillContractClient, WillStatus, MAX_BENEFICIARIES,
    MAX_GUARDIANS, MAX_KEEPER_BOUNTY_BPS, MAX_PERIOD_DAYS, SECONDS_PER_DAY,
};

/// Number of distinct addresses the fuzzer picks from.
const ADDRESS_POOL_SIZE: u8 = 6;

/// Ledger timestamp every scenario starts at, far enough from zero that
/// realistic periods neither underflow nor look suspicious.
const BASE_TIMESTAMP: u64 = 1_700_000_000;

/// Upper bound on how much of the test token the owner is funded with. Kept
/// well below `i64::MAX` so the Stellar Asset Contract never rejects the mint
/// itself — we want the *will* contract to be the thing under test.
const MAX_MINT: i128 = 1_000_000_000_000_000_000;

/// Beneficiary lists longer than this are truncated. Deliberately larger than
/// [`MAX_BENEFICIARIES`] so over-long lists are still exercised.
const MAX_FUZZ_BENEFICIARIES: usize = MAX_BENEFICIARIES as usize + 3;

/// Guardian lists longer than this are truncated. Deliberately larger than
/// [`MAX_GUARDIANS`] for the same reason.
const MAX_FUZZ_GUARDIANS: usize = MAX_GUARDIANS as usize + 3;

/// How many successive update attempts a single `update_beneficiaries`
/// scenario replays against one will.
pub const MAX_FUZZ_UPDATES: usize = 4;

/// One beneficiary entry, addressed by pool slot rather than by `Address`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "fuzzing", derive(arbitrary::Arbitrary))]
pub struct BeneficiarySpec {
    pub address_slot: u8,
    pub basis_points: u32,
}

/// A full `create_will` invocation, including how much the owner is funded
/// with beforehand.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "fuzzing", derive(arbitrary::Arbitrary))]
pub struct CreateWillInput {
    /// Amount minted to the owner before the call. Clamped to `0..=MAX_MINT`.
    pub mint: i128,
    /// Amount passed to `create_will`, verbatim — including zero, negative,
    /// and values far larger than the owner holds.
    pub amount: i128,
    pub beneficiaries: Vec<BeneficiarySpec>,
    pub guardian_slots: Vec<u8>,
    pub checkin_period_days: u64,
    pub grace_period_days: u64,
    /// Number of guardian votes required to trigger early release.
    /// Sanitized to `1..=guardian_slots.len()`.
    pub guardian_threshold: u32,
    /// When set, a successfully created will is driven all the way through
    /// trigger and release so the distribution path is fuzzed too.
    pub release_after_create: bool,
}

/// A `create_will` that is forced to succeed, followed by a series of
/// `update_beneficiaries` attempts against the resulting will.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "fuzzing", derive(arbitrary::Arbitrary))]
pub struct UpdateBeneficiariesInput {
    /// Sanitised into a valid will before use, so the update path is always
    /// reached.
    pub create: CreateWillInput,
    /// Replacement beneficiary lists, applied in order. Each is passed
    /// verbatim, so most will be rejected — that is the point.
    pub updates: Vec<Vec<BeneficiarySpec>>,
    /// When set, one update is additionally attempted by a non-owner, which
    /// must always be rejected.
    pub probe_non_owner: bool,
}

/// What a fuzzed invocation did, for front-ends that want to assert coverage
/// (e.g. "at least some inputs were accepted").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The call succeeded and every invariant held afterwards.
    Accepted(u64),
    /// The call was rejected with a declared contract error, as designed.
    Rejected,
}

/// Reports an invariant violation, printing the offending input so both
/// libFuzzer artifacts and `proptest` failures are directly reproducible.
fn violation(input: &impl core::fmt::Debug, message: &str) -> ! {
    panic!("Williams invariant violation: {message}\n  input: {input:#?}");
}

/// Asserts `condition`, reporting `message` and the input if it does not hold.
fn check(condition: bool, input: &impl core::fmt::Debug, message: &str) {
    if !condition {
        violation(input, message);
    }
}

// --- Invariant helpers shared with the deterministic unit test suite ---
//
// Same correctness properties `assert_created_will` and
// `assert_updated_beneficiaries` below check, extracted into a form that
// does not depend on the fuzz-specific `Scenario`/`*Input` types, so
// `allocation_test.rs` and `beneficiary_lifecycle_test.rs` can assert the
// identical properties against their own hand-written scenarios (issue
// #267) instead of re-implementing ad-hoc checks that might not catch the
// same bugs the fuzz harness is designed to catch.

/// Every beneficiary currently on `will` must be able to find it via
/// `get_wills_by_beneficiary`.
pub(crate) fn assert_beneficiaries_are_indexed(client: &WillContractClient, will: &Will) {
    for beneficiary in will.beneficiaries.iter() {
        assert!(
            client
                .get_wills_by_beneficiary(&beneficiary.address, &None, &100)
                .iter()
                .any(|indexed| indexed.id == will.id),
            "beneficiary is missing will {} from their index",
            will.id
        );
    }
}

/// None of `removed` may still find `will_id` via `get_wills_by_beneficiary`
/// -- a removed beneficiary must not keep a stale claim in the reverse
/// index.
pub(crate) fn assert_removed_beneficiaries_are_unindexed(
    client: &WillContractClient,
    will_id: u64,
    removed: &[Address],
) {
    for address in removed {
        assert!(
            !client
                .get_wills_by_beneficiary(address, &None, &100)
                .iter()
                .any(|indexed| indexed.id == will_id),
            "a removed beneficiary still has will {will_id} in their index"
        );
    }
}

/// The contract's on-chain balance of `token_address` must equal the amount
/// `will` itself records as locked for that token -- the contract must
/// actually hold what it says it holds.
pub(crate) fn assert_custody_matches_recorded_balance(
    token: &TokenClient,
    contract_address: &Address,
    token_address: &Address,
    will: &Will,
) {
    assert_eq!(
        token.balance(contract_address),
        will.balances.get(token_address.clone()).unwrap_or(0),
        "the contract's token balance does not match will {}'s recorded balance",
        will.id
    );
}

/// Every `Allocation::Percentage` beneficiary's basis points must sum to
/// exactly 10,000 -- "100% of whatever remains after fixed amounts", per the
/// `Allocation` doc comment. Skipped entirely for a will with no
/// percentage-typed beneficiaries: a pure-`FixedAmount` will has no such
/// invariant to hold.
pub(crate) fn assert_percentage_shares_sum_to_10000(will: &Will) {
    let mut total: u128 = 0;
    let mut has_percentage = false;
    for beneficiary in will.beneficiaries.iter() {
        if let Allocation::Percentage(bp) = beneficiary.allocation {
            has_percentage = true;
            total += bp as u128;
        }
    }
    if has_percentage {
        let hashed_total: u128 = will
            .hashed_beneficiaries
            .iter()
            .map(|h| h.percentage as u128)
            .sum();
        assert!(
            total + hashed_total <= 10_000,
            "will {}'s percentage-typed beneficiaries exceed 10,000 basis points",
            will.id
        );
    }
}

/// Builds the fixed pool of addresses a scenario draws from.
fn address_pool(env: &Env) -> SorobanVec<Address> {
    let mut pool = SorobanVec::new(env);
    for _ in 0..ADDRESS_POOL_SIZE {
        pool.push_back(Address::generate(env));
    }
    pool
}

/// Resolves a fuzzer-chosen slot to a pool address.
fn slot(pool: &SorobanVec<Address>, index: u8) -> Address {
    pool.get(index as u32 % pool.len()).unwrap()
}

/// Materialises beneficiary specs into the Soroban vector the contract takes.
fn to_beneficiaries(
    env: &Env,
    pool: &SorobanVec<Address>,
    specs: &[BeneficiarySpec],
) -> SorobanVec<Beneficiary> {
    let mut out = SorobanVec::new(env);
    for spec in specs.iter().take(MAX_FUZZ_BENEFICIARIES) {
        out.push_back(Beneficiary {
            address: slot(pool, spec.address_slot),
            allocation: Allocation::Percentage(spec.basis_points),
        });
    }
    out
}

/// Materialises guardian slots into the Soroban vector the contract takes.
fn to_guardians(env: &Env, pool: &SorobanVec<Address>, slots: &[u8]) -> SorobanVec<Address> {
    let mut out = SorobanVec::new(env);
    for &index in slots.iter().take(MAX_FUZZ_GUARDIANS) {
        out.push_back(slot(pool, index));
    }
    out
}

/// Rewrites arbitrary specs into a list the contract is guaranteed to accept:
/// between 1 and [`MAX_BENEFICIARIES`] entries whose basis points sum to
/// exactly 10,000.
///
/// Address slots are left untouched, so duplicate beneficiaries survive
/// sanitisation and keep getting exercised.
pub fn sanitize_specs(specs: &[BeneficiarySpec]) -> Vec<BeneficiarySpec> {
    // Entries whose `address_slot` resolves to the same pool address (mod
    // ADDRESS_POOL_SIZE) would make `create_will`/`update_beneficiaries`
    // reject the list as a duplicate beneficiary, so this only keeps the
    // first entry per distinct resolved address — capping the result at
    // `ADDRESS_POOL_SIZE` regardless of `MAX_BENEFICIARIES`, since the pool
    // cannot supply more unique addresses than it has slots.
    let mut seen_slots: Vec<u8> = Vec::new();
    let mut out: Vec<BeneficiarySpec> = Vec::new();
    for spec in specs.iter() {
        let resolved = spec.address_slot % ADDRESS_POOL_SIZE;
        if seen_slots.contains(&resolved) {
            continue;
        }
        seen_slots.push(resolved);
        out.push(spec.clone());
        if out.len() >= MAX_BENEFICIARIES as usize {
            break;
        }
    }
    if out.is_empty() {
        out.push(BeneficiarySpec {
            address_slot: 0,
            basis_points: 10_000,
        });
    }

    // One point each, and the remainder to the first entry. With at most
    // `MAX_BENEFICIARIES` (10) entries the first share is never below 9_991.
    let extra = 10_000 - (out.len() as u32 - 1);
    for spec in out.iter_mut() {
        spec.basis_points = 1;
    }
    out[0].basis_points = extra;
    out
}

/// Rewrites an arbitrary `create_will` input into one the contract is
/// guaranteed to accept, so scenarios that fuzz a *later* entry point always
/// have a will to work with.
fn sanitize_create(input: &CreateWillInput) -> CreateWillInput {
    // Pool slot 0 is always the scenario's owner (see `Scenario::new`), and
    // the owner can never be their own guardian, so it is excluded here in
    // addition to deduplicating repeated slots.
    let mut guardian_slots: Vec<u8> = Vec::new();
    for &index in input.guardian_slots.iter() {
        let resolved = index % ADDRESS_POOL_SIZE;
        if resolved != 0
            && guardian_slots.len() < MAX_GUARDIANS as usize
            && !guardian_slots.contains(&resolved)
        {
            guardian_slots.push(resolved);
        }
    }

    let guardian_count = guardian_slots.len() as u32;
    let threshold = if guardian_count > 0 {
        input.guardian_threshold.clamp(1, guardian_count)
    } else {
        0
    };

    CreateWillInput {
        mint: MAX_MINT,
        // 1..=1_000_000_000, so the owner can always cover it.
        amount: input.amount.rem_euclid(1_000_000_000) + 1,
        beneficiaries: sanitize_specs(&input.beneficiaries),
        guardian_slots,
        checkin_period_days: input.checkin_period_days % MAX_PERIOD_DAYS + 1,
        grace_period_days: input.grace_period_days % MAX_PERIOD_DAYS + 1,
        guardian_threshold: threshold,
        release_after_create: false,
    }
}

/// Everything a scenario needs: a live env, a funded owner, a token, and a
/// registered will contract.
struct Scenario<'a> {
    env: Env,
    client: WillContractClient<'a>,
    token: TokenClient<'a>,
    token_address: Address,
    pool: SorobanVec<Address>,
    owner: Address,
}

impl<'a> Scenario<'a> {
    /// Registers the contracts and mints `mint` (clamped) to the owner.
    fn new(mint: i128) -> Scenario<'a> {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(BASE_TIMESTAMP);

        let pool = address_pool(&env);
        let owner = pool.get(0).unwrap();

        let sac = env.register_stellar_asset_contract_v2(owner.clone());
        let token_address = sac.address();
        let minted = mint.clamp(0, MAX_MINT);
        if minted > 0 {
            StellarAssetClient::new(&env, &token_address).mint(&owner, &minted);
        }

        let contract_id = env.register(WillContract, ());

        Self {
            client: WillContractClient::new(&env, &contract_id),
            token: TokenClient::new(&env, &token_address),
            token_address,
            pool,
            owner,
            env,
        }
    }

    /// Advances the ledger clock, saturating rather than panicking: the
    /// harness must never be the thing that overflows.
    fn advance_days(&self, days: u64) {
        let seconds = days.saturating_mul(SECONDS_PER_DAY);
        self.env.ledger().with_mut(|ledger| {
            ledger.timestamp = ledger.timestamp.saturating_add(seconds);
        });
    }
}

/// Drives `create_will` with arbitrary input.
///
/// Rejects malformed input via a declared error, or accepts it and holds to
/// every invariant a freshly created will is documented to have.
pub fn run_create_will(input: &CreateWillInput) -> Outcome {
    let scenario = Scenario::new(input.mint);
    let beneficiaries = to_beneficiaries(&scenario.env, &scenario.pool, &input.beneficiaries);
    let guardians = to_guardians(&scenario.env, &scenario.pool, &input.guardian_slots);
    let tokens: SorobanVec<(Address, i128)> = soroban_sdk::vec![
        &scenario.env,
        (scenario.token_address.clone(), input.amount)
    ];

    let keeper_bounty: Option<u32> = None;
    let result = scenario.client.try_create_will(
        &scenario.owner,
        &tokens,
        &beneficiaries,
        &input.checkin_period_days,
        &input.grace_period_days,
        &guardians,
        &input.guardian_threshold,
        &keeper_bounty,
        &0,
    );

    let will_id = match result {
        Ok(Ok(will_id)) => will_id,
        Ok(Err(_)) => violation(
            input,
            "create_will returned a value that is not a u64 will id",
        ),
        // A declared contract error is the correct response to bad input.
        Err(Ok(_)) => return Outcome::Rejected,
        Err(Err(invoke_error)) => violation(
            input,
            &std::format!(
                "create_will aborted instead of returning a declared WillError: {invoke_error:?}"
            ),
        ),
    };

    assert_created_will(&scenario, input, will_id, &beneficiaries, &guardians);
    if input.release_after_create {
        assert_full_release(&scenario, input, will_id);
    }

    Outcome::Accepted(will_id)
}

#[cfg(test)]
mod multi_token_distribution_setup_tests {
    //! Disposable fixtures proving the targeted-bounty setup assertions in
    //! `run_multi_token_distribution` actually fail when the setter regresses.
    //!
    //! The contract itself is not modified; instead a rejecting setter is
    //! simulated by replaying the runner's setup assertions against a value
    //! that never gets stored, which is exactly the state a regression that
    //! silently ignores the setter would leave behind.

    use super::*;

    /// Mirrors the setup assertion the runner performs after a successful
    /// setter call: the stored selection must equal the requested target.
    fn assert_target_active(stored: Option<Address>, requested: &Address) {
        assert_eq!(
            stored.as_ref(),
            Some(requested),
            "the requested bounty token is not the active selection after setup"
        );
    }

    /// A rejecting setter leaves the stored selection unset; the runner's
    /// property must fail rather than silently proceeding to default
    /// distribution.
    #[test]
    #[should_panic(expected = "not the active selection")]
    fn rejecting_setter_fails_target_active_property() {
        let env = Env::default();
        let requested = Address::generate(&env);
        // Simulated regression: the setter rejected the call, so nothing was
        // stored.
        assert_target_active(None, &requested);
    }

    /// Default campaign case: no target requested, so no setup assertion is
    /// made and the runner proceeds to default distribution.
    #[test]
    fn default_campaign_has_no_target() {
        let input = MultiTokenDistributionInput {
            token_amounts: vec![1_000_000],
            beneficiaries: Vec::new(),
            keeper_bounty_bps: 100,
            target_bounty_token_idx: None,
        };
        assert!(input.target_bounty_token_idx.is_none());
    }

    /// Targeted-token campaign case: a valid target is requested and the
    /// property holds when the setter stored it.
    #[test]
    fn targeted_campaign_confirms_active_target() {
        let env = Env::default();
        let requested = Address::generate(&env);
        assert_target_active(Some(requested.clone()), &requested);
    }
}/// Checks every invariant that must hold for a will `create_will` accepted.
fn assert_created_will(
    scenario: &Scenario<'_>,
    input: &CreateWillInput,
    will_id: u64,
    beneficiaries: &SorobanVec<Beneficiary>,
    guardians: &SorobanVec<Address>,
) {
    let will = scenario.client.get_will(&will_id);

    check(
        will.id == will_id,
        input,
        "stored will id does not match the returned id",
    );
    check(
        will.owner == scenario.owner,
        input,
        "will owner is not the creator",
    );
    check(
        will.status == WillStatus::Active,
        input,
        "a new will is not Active",
    );
    check(
        will.balances
            .get(scenario.token_address.clone())
            .unwrap_or(0)
            == input.amount,
        input,
        "recorded balance does not match the locked amount",
    );
    check(
        will.balances
            .get(scenario.token_address.clone())
            .unwrap_or(0)
            > 0,
        input,
        "a will was created with a non-positive balance",
    );
    check(
        will.trigger_time.is_none(),
        input,
        "a new will already has a trigger time",
    );
    check(
        will.guardian_votes == 0,
        input,
        "a new will already has guardian votes",
    );
    check(
        will.beneficiaries == *beneficiaries,
        input,
        "stored beneficiaries differ from the supplied list",
    );
    // Compare guardian addresses (stored as Vec<Guardian>).
    let guardian_addresses: SorobanVec<Address> = {
        let mut v: SorobanVec<Address> = SorobanVec::new(&scenario.env);
        for g in will.guardians.iter() {
            v.push_back(g.address.clone());
        }
        v
    };
    check(
        guardian_addresses == *guardians,
        input,
        "stored guardians differ from the supplied list",
    );

    // Shares: bounds, and a sum computed in u128 so the check itself cannot
    // overflow while testing for overflow.
    check(
        !will.beneficiaries.is_empty() && will.beneficiaries.len() <= MAX_BENEFICIARIES,
        input,
        "beneficiary count is outside 1..=MAX_BENEFICIARIES",
    );
    let mut total: u128 = 0;
    for beneficiary in will.beneficiaries.iter() {
        total += match beneficiary.allocation {
            Allocation::Percentage(bp) => bp as u128,
            Allocation::FixedAmount(_) => 0,
        };
    }
    let hashed_total: u128 = will
        .hashed_beneficiaries
        .iter()
        .map(|h| h.percentage as u128)
        .sum();
    check(
        total + hashed_total <= 10_000,
        input,
        "beneficiary basis points exceed 10,000",
    );

    // Guardians: a stored quorum must be reachable, which it is not if the
    // same address occupies more than one slot.
    check(
        will.guardians.len() <= MAX_GUARDIANS,
        input,
        "guardian count exceeds MAX_GUARDIANS",
    );
    for i in 0..will.guardians.len() {
        for j in (i + 1)..will.guardians.len() {
            check(
                will.guardians.get_unchecked(i) != will.guardians.get_unchecked(j),
                input,
                "the same guardian was stored twice, making the quorum unreachable",
            );
        }
    }

    // Deadlines must be representable. If they are not, `trigger_will` panics
    // on every call and the balance is locked in the contract forever.
    let checkin_deadline = will
        .checkin_period_days
        .checked_mul(SECONDS_PER_DAY)
        .and_then(|seconds| will.last_checkin.checked_add(seconds));
    check(
        checkin_deadline.is_some(),
        input,
        "the check-in deadline overflows u64, so the will can never be triggered",
    );
    check(
        will.grace_period_days
            .checked_mul(SECONDS_PER_DAY)
            .is_some(),
        input,
        "the grace period overflows u64, so the will can never be released",
    );
    check(
        will.checkin_period_days > 0 && will.grace_period_days > 0,
        input,
        "a zero-length period leaves the will triggerable in the ledger it was created in",
    );

    // Custody: the contract must actually hold what it says it holds.
    check(
        scenario.token.balance(&scenario.client.address)
            == will
                .balances
                .get(scenario.token_address.clone())
                .unwrap_or(0),
        input,
        "the contract's token balance does not match the will's recorded balance",
    );

    // Reverse indexes must find the will.
    check(
        scenario
            .client
            .get_wills_by_owner(&scenario.owner, &None, &100)
            .iter()
            .any(|indexed| indexed.id == will_id),
        input,
        "the new will is missing from its owner's index",
    );
    for beneficiary in will.beneficiaries.iter() {
        check(
            scenario
                .client
                .get_wills_by_beneficiary(&beneficiary.address, &None, &100)
                .iter()
                .any(|indexed| indexed.id == will_id),
            input,
            "a beneficiary is missing the will from their index",
        );
    }
}

/// Drives a created will through trigger and release, checking that the whole
/// balance leaves the contract with no dust stranded behind.
fn assert_full_release(scenario: &Scenario<'_>, input: &CreateWillInput, will_id: u64) {
    let will = scenario.client.get_will(&will_id);

    scenario.advance_days(will.checkin_period_days.saturating_add(1));
    match scenario.client.try_trigger_will(&will_id) {
        Ok(Ok(())) => {}
        Ok(Err(_)) => violation(input, "trigger_will returned an undecodable value"),
        Err(Ok(_)) => violation(
            input,
            "trigger_will was rejected after the check-in deadline passed",
        ),
        Err(Err(e)) => violation(input, &std::format!("trigger_will aborted: {e:?}")),
    }

    scenario.advance_days(will.grace_period_days.saturating_add(1));
    match scenario.client.try_release_inheritance(&will_id, &None) {
        Ok(Ok(())) => {}
        Ok(Err(_)) => violation(input, "release_inheritance returned an undecodable value"),
        Err(Ok(_)) => violation(
            input,
            "release_inheritance was rejected after the grace period expired",
        ),
        Err(Err(e)) => violation(input, &std::format!("release_inheritance aborted: {e:?}")),
    }

    let released = scenario.client.get_will(&will_id);
    check(
        released.status == WillStatus::Released,
        input,
        "the will is not Released after release_inheritance",
    );
    check(
        released.balances.is_empty(),
        input,
        "the released will still records balances",
    );
    check(
        scenario.token.balance(&scenario.client.address) == 0,
        input,
        "the contract kept dust after distributing the inheritance",
    );
}

/// Drives `update_beneficiaries` with arbitrary replacement lists.
///
/// A second will sharing the same address pool is created alongside the one
/// under test, so the harness can also detect a beneficiary index that is
/// clobbered across wills.
pub fn run_update_beneficiaries(input: &UpdateBeneficiariesInput) -> Vec<Outcome> {
    let create = sanitize_create(&input.create);
    let scenario = Scenario::new(create.mint);

    let will_id = create_valid_will(&scenario, input, &create);
    // A neighbouring will, whose index entries must survive every update
    // applied to `will_id`.
    let neighbour_id = create_valid_will(&scenario, input, &create);

    let mut outcomes = Vec::new();
    for specs in input.updates.iter().take(MAX_FUZZ_UPDATES) {
        let before = scenario.client.get_will(&will_id);
        let replacement = to_beneficiaries(&scenario.env, &scenario.pool, specs);

        let outcome =
            match scenario
                .client
                .try_update_beneficiaries(&will_id, &scenario.owner, &replacement)
            {
                Ok(Ok(())) => {
                    assert_updated_beneficiaries(&scenario, input, will_id, &before, &replacement);
                    Outcome::Accepted(will_id)
                }
                Ok(Err(_)) => {
                    violation(input, "update_beneficiaries returned an undecodable value")
                }
                Err(Ok(_)) => {
                    // A rejected update must leave the will exactly as it was.
                    let after = scenario.client.get_will(&will_id);
                    check(
                        after.beneficiaries == before.beneficiaries,
                        input,
                        "a rejected update still changed the beneficiary list",
                    );
                    check(
                        after.balances == before.balances && after.status == before.status,
                        input,
                        "a rejected update changed the will's balance or status",
                    );
                    Outcome::Rejected
                }
                Err(Err(e)) => violation(
                    input,
                    &std::format!(
                    "update_beneficiaries aborted instead of returning a declared WillError: {e:?}"
                ),
                ),
            };
        outcomes.push(outcome);

        // Whatever happened above, the untouched will must still be findable
        // through every one of its beneficiaries.
        let neighbour = scenario.client.get_will(&neighbour_id);
        for beneficiary in neighbour.beneficiaries.iter() {
            check(
                scenario
                    .client
                    .get_wills_by_beneficiary(&beneficiary.address, &None, &100)
                    .iter()
                    .any(|indexed| indexed.id == neighbour_id),
                input,
                "updating one will dropped an unrelated will from a beneficiary's index",
            );
        }
    }

    if input.probe_non_owner {
        let non_owner = slot(&scenario.pool, 1);
        let before = scenario.client.get_will(&will_id);
        let replacement = to_beneficiaries(
            &scenario.env,
            &scenario.pool,
            &sanitize_specs(&input.create.beneficiaries),
        );
        match scenario
            .client
            .try_update_beneficiaries(&will_id, &non_owner, &replacement)
        {
            Ok(Ok(())) => violation(
                input,
                "update_beneficiaries accepted a caller that does not own the will",
            ),
            Err(Err(e)) => violation(
                input,
                &std::format!("update_beneficiaries aborted for a non-owner caller: {e:?}"),
            ),
            _ => {
                let after = scenario.client.get_will(&will_id);
                check(
                    after.owner == before.owner,
                    input,
                    "a non-owner update changed the will's owner",
                );
            }
        }
    }

    outcomes
}

/// Creates a will from a sanitised input, treating any rejection as a harness
/// failure — sanitisation is supposed to guarantee acceptance, so a rejection
/// means validation and the documented rules have drifted apart.
fn create_valid_will(
    scenario: &Scenario<'_>,
    input: &impl core::fmt::Debug,
    create: &CreateWillInput,
) -> u64 {
    let beneficiaries = to_beneficiaries(&scenario.env, &scenario.pool, &create.beneficiaries);
    let guardians = to_guardians(&scenario.env, &scenario.pool, &create.guardian_slots);
    let tokens: SorobanVec<(Address, i128)> = soroban_sdk::vec![
        &scenario.env,
        (scenario.token_address.clone(), create.amount)
    ];

    let keeper_bounty: Option<u32> = None;
    match scenario.client.try_create_will(
        &scenario.owner,
        &tokens,
        &beneficiaries,
        &create.checkin_period_days,
        &create.grace_period_days,
        &guardians,
        &create.guardian_threshold,
        &keeper_bounty,
        &0,
    ) {
        Ok(Ok(will_id)) => will_id,
        Ok(Err(_)) => violation(
            input,
            "create_will returned a value that is not a u64 will id",
        ),
        Err(Ok(e)) => violation(
            input,
            &std::format!("create_will rejected an input the harness built to be valid: {e:?}"),
        ),
        Err(Err(e)) => violation(
            input,
            &std::format!("create_will aborted on valid input: {e:?}"),
        ),
    }
}

/// Checks every invariant that must hold after an accepted beneficiary update.
fn assert_updated_beneficiaries(
    scenario: &Scenario<'_>,
    input: &UpdateBeneficiariesInput,
    will_id: u64,
    before: &Will,
    replacement: &SorobanVec<Beneficiary>,
) {
    let will = scenario.client.get_will(&will_id);

    check(
        will.beneficiaries == *replacement,
        input,
        "the stored beneficiaries are not the ones supplied",
    );
    check(
        will.balances == before.balances,
        input,
        "an update changed the will's balances",
    );
    check(
        will.status == before.status,
        input,
        "an update changed the will's status",
    );
    check(
        will.owner == before.owner,
        input,
        "an update changed the will's owner",
    );
    check(
        will.guardians == before.guardians,
        input,
        "an update changed the will's guardians",
    );
    check(
        will.last_checkin == before.last_checkin,
        input,
        "an update changed the will's last check-in",
    );

    check(
        !will.beneficiaries.is_empty() && will.beneficiaries.len() <= MAX_BENEFICIARIES,
        input,
        "beneficiary count is outside 1..=MAX_BENEFICIARIES after an update",
    );
    let mut total: u128 = 0;
    for beneficiary in will.beneficiaries.iter() {
        total += match beneficiary.allocation {
            Allocation::Percentage(bp) => bp as u128,
            Allocation::FixedAmount(_) => 0,
        };
    }
    let hashed_total: u128 = will
        .hashed_beneficiaries
        .iter()
        .map(|h| h.percentage as u128)
        .sum();
    check(
        total + hashed_total <= 10_000,
        input,
        "beneficiary basis points exceed 10,000 after an update",
    );

    // Every current beneficiary must be able to find the will...
    for beneficiary in will.beneficiaries.iter() {
        check(
            scenario
                .client
                .get_wills_by_beneficiary(&beneficiary.address, &None, &100)
                .iter()
                .any(|indexed| indexed.id == will_id),
            input,
            "a current beneficiary is missing the will from their index",
        );
    }

    // ...and every dropped one must not, or a removed heir keeps a stale claim
    // in every client that reads the index.
    for old in before.beneficiaries.iter() {
        let retained = will
            .beneficiaries
            .iter()
            .any(|current| current.address == old.address);
        if retained {
            continue;
        }
        check(
            !scenario
                .client
                .get_wills_by_beneficiary(&old.address, &None, &100)
                .iter()
                .any(|indexed| indexed.id == will_id),
            input,
            "a removed beneficiary still has the will in their index",
        );
    }
}

// ---------------------------------------------------------------------------
// merge_wills runner
// ---------------------------------------------------------------------------

/// Input for a fuzz run of `merge_wills`.
///
/// Two valid wills are created from the sanitised `create` spec (same owner,
/// same primary token), then merged. The runner checks every conservation
/// invariant: combined balance, beneficiary counts, index consistency.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "fuzzing", derive(arbitrary::Arbitrary))]
pub struct MergeWillsInput {
    /// Template used to build both wills; sanitised before use so both
    /// creations always succeed.
    pub create: CreateWillInput,
    /// Override the beneficiary list for the second will. A `None` means
    /// both wills use the same list; a `Some` means distinct lists, which
    /// exercises the recalculation path through `merge_beneficiaries`.
    pub will_b_beneficiaries: Option<Vec<BeneficiarySpec>>,
}

/// Drives `merge_wills` with arbitrary input.
///
/// The two source wills are created from a sanitised spec, so the merge is
/// always attempted. The runner then asserts the invariants the docs promise:
///
/// - The surviving will's balance equals the sum of both sources' balances.
/// - The consumed will is `Cancelled` and has zero balance.
/// - All beneficiaries of the surviving will can still find it in their index.
pub fn run_merge_wills(input: &MergeWillsInput) -> Outcome {
    let create = sanitize_create(&input.create);
    let scenario = Scenario::new(create.mint * 2); // enough for two wills

    // Create will A.
    let will_id_a = create_valid_will(&scenario, input, &create);
    let balance_a = scenario
        .client
        .get_will(&will_id_a)
        .balances
        .get(scenario.token_address.clone())
        .unwrap_or(0);

    // Create will B — optionally with a different beneficiary list.
    let will_id_b = if let Some(ref specs) = input.will_b_beneficiaries {
        let sanitised = sanitize_specs(specs);
        // Temporarily override the beneficiary list for will B.
        let create_b = CreateWillInput {
            beneficiaries: sanitised,
            ..create.clone()
        };
        create_valid_will(&scenario, input, &create_b)
    } else {
        create_valid_will(&scenario, input, &create)
    };
    let balance_b = scenario
        .client
        .get_will(&will_id_b)
        .balances
        .get(scenario.token_address.clone())
        .unwrap_or(0);

    let expected_combined = balance_a.saturating_add(balance_b);
    let source_a = scenario.client.get_will(&will_id_a).beneficiaries;
    let source_b = scenario.client.get_will(&will_id_b).beneficiaries;

    // Attempt the merge.
    let result = scenario
        .client
        .try_merge_wills(&scenario.owner, &will_id_a, &will_id_b);

    match result {
        Ok(Ok(())) => {}
        Ok(Err(_)) => violation(input, "merge_wills returned an undecodable value"),
        // A declared error (e.g. MergeWouldExceedLimits) is acceptable — the
        // harness is intentionally constructed so most merges succeed, but
        // the fuzzer can still explore the rejection paths.
        Err(Ok(_)) => return Outcome::Rejected,
        Err(Err(e)) => violation(
            input,
            &std::format!("merge_wills aborted instead of returning a declared WillError: {e:?}"),
        ),
    }

    // --- post-merge invariants ---
    let surviving = scenario.client.get_will(&will_id_a);

    // Balance conservation: the surviving will must hold exactly the sum of
    // what both wills held before the merge.
    check(
        surviving
            .balances
            .get(scenario.token_address.clone())
            .unwrap_or(0)
            == expected_combined,
        input,
        "surviving will's balance does not equal the sum of both source balances",
    );

    // Consumed will must be Cancelled with empty balances.
    let consumed = scenario.client.get_will(&will_id_b);
    check(
        consumed.status == WillStatus::Cancelled,
        input,
        "consumed will is not Cancelled after merge",
    );
    check(
        consumed.balances.is_empty()
            || consumed
                .balances
                .get(scenario.token_address.clone())
                .unwrap_or(0)
                == 0,
        input,
        "consumed will still records a non-zero balance after merge",
    );

    // Beneficiary count must be within the allowed range.
    check(
        !surviving.beneficiaries.is_empty() && surviving.beneficiaries.len() <= MAX_BENEFICIARIES,
        input,
        "merged will's beneficiary count is outside 1..=MAX_BENEFICIARIES",
    );

    // Every beneficiary can still find the surviving will in their index.
    assert_beneficiaries_are_indexed(&scenario.client, &surviving);

    // Independent allocation oracle (#234).
    check_merged_allocations(input, &source_a, &source_b, &surviving, expected_combined);

    Outcome::Accepted(will_id_a)
}

/// Independent oracle for merged allocations (#234): every merged entry is a
/// unique address drawn from the union of both sources, every source address
/// survives, percentage totals stay within 10_000 bps, and fixed amounts never
/// exceed the combined estate.
fn check_merged_allocations(
    input: &MergeWillsInput,
    source_a: &soroban_sdk::Vec<Beneficiary>,
    source_b: &soroban_sdk::Vec<Beneficiary>,
    merged: &Will,
    combined: i128,
) {
    let mut total_bps: u64 = 0;
    let mut total_fixed: i128 = 0;
    for (i, b) in merged.beneficiaries.iter().enumerate() {
        match b.allocation {
            Allocation::Percentage(bp) => {
                check(bp > 0, input, "merged will has a zero-bps percentage share");
                total_bps += bp as u64;
            }
            Allocation::FixedAmount(amt) => {
                check(
                    amt > 0,
                    input,
                    "merged will has a non-positive fixed amount",
                );
                total_fixed = total_fixed.saturating_add(amt);
            }
        }
        check(
            source_a
                .iter()
                .chain(source_b.iter())
                .any(|s| s.address == b.address),
            input,
            "merged will contains a beneficiary absent from both sources",
        );
        check(
            merged
                .beneficiaries
                .iter()
                .skip(i + 1)
                .all(|o| o.address != b.address),
            input,
            "merged will repeats a beneficiary address",
        );
    }
    for s in source_a.iter().chain(source_b.iter()) {
        check(
            merged.beneficiaries.iter().any(|b| b.address == s.address),
            input,
            "a source beneficiary was dropped by the merge",
        );
    }
    check(
        total_bps <= 10_000,
        input,
        "merged percentage allocations exceed 10_000 bps",
    );
    check(
        total_fixed <= combined,
        input,
        "merged fixed amounts exceed the combined estate",
    );
}

// ---------------------------------------------------------------------------
// split_will runner
// ---------------------------------------------------------------------------

/// Input for a fuzz run of `split_will`.
///
/// One valid will is created, then a subset of its beneficiaries is split off
/// into a new child will. The runner checks balance conservation, beneficiary
/// normalization, and index consistency for both source and child.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "fuzzing", derive(arbitrary::Arbitrary))]
pub struct SplitWillInput {
    /// Template used to build the source will; sanitised before use.
    pub create: CreateWillInput,
    /// Fraction of the primary-token balance to split into the child will,
    /// expressed as an integer percentage (0..=100). Clamped to 1..=99 so
    /// both sides retain a positive balance.
    pub split_pct: u8,
    /// Number of beneficiaries (from the front of the list) to move to the
    /// child will. Zero or "more than available" is clamped so at least one
    /// beneficiary moves and at least one stays.
    pub beneficiaries_to_split: u8,
}

/// Drives `split_will` with arbitrary input.
///
/// The source will is created from a sanitised spec, then split. The runner
/// asserts the invariants the docs promise:
///
/// - Source balance + child balance = original total.
/// - Source retains at least one beneficiary.
/// - Child beneficiaries' percentages renormalise to exactly 10,000 bps.
/// - Both wills' beneficiaries are correctly reflected in the reverse index.
pub fn run_split_will(input: &SplitWillInput) -> Outcome {
    let create = sanitize_create(&input.create);
    let scenario = Scenario::new(create.mint);

    let will_id = create_valid_will(&scenario, input, &create);
    let will = scenario.client.get_will(&will_id);

    let original_balance = will
        .balances
        .get(scenario.token_address.clone())
        .unwrap_or(0);

    if original_balance <= 0 {
        return Outcome::Rejected;
    }

    // Determine the split amount: `split_pct` percent of the balance, clamped
    // so both sides keep at least 1 unit. `1` is the minimum positive amount
    // the contract accepts.
    let pct = (input.split_pct as i128).clamp(1, 99);
    let split_amount = (original_balance * pct / 100)
        .max(1)
        .min(original_balance - 1);
    if split_amount <= 0 || split_amount >= original_balance {
        return Outcome::Rejected;
    }

    // Determine how many beneficiaries to move: at least 1, at most n-1
    // (source must keep at least one).
    let n = will.beneficiaries.len() as usize;
    if n <= 1 {
        // Can't split if there is only one beneficiary (source would be empty).
        return Outcome::Rejected;
    }
    let to_split_count = (input.beneficiaries_to_split as usize).clamp(1, n - 1);

    // Build the `beneficiaries_to_split` list from the will's actual list.
    let mut split_beneficiaries: SorobanVec<Beneficiary> = SorobanVec::new(&scenario.env);
    for beneficiary in will.beneficiaries.iter().take(to_split_count) {
        split_beneficiaries.push_back(beneficiary);
    }
    let tokens: SorobanVec<(Address, i128)> = soroban_sdk::vec![
        &scenario.env,
        (scenario.token_address.clone(), split_amount)
    ];

    let result =
        scenario
            .client
            .try_split_will(&will_id, &scenario.owner, &split_beneficiaries, &tokens);

    let child_id = match result {
        Ok(Ok(child_id)) => child_id,
        Ok(Err(_)) => violation(input, "split_will returned an undecodable value"),
        // A declared error means the split input was malformed; both wills
        // must be unchanged — checked below.
        Err(Ok(_)) => {
            // On rejection the source will must be unmodified.
            let after = scenario.client.get_will(&will_id);
            check(
                after.balances == will.balances,
                input,
                "a rejected split changed the source will's balances",
            );
            check(
                after.beneficiaries == will.beneficiaries,
                input,
                "a rejected split changed the source will's beneficiaries",
            );
            return Outcome::Rejected;
        }
        Err(Err(e)) => violation(
            input,
            &std::format!("split_will aborted instead of returning a declared WillError: {e:?}"),
        ),
    };

    // --- post-split invariants ---
    let source_after = scenario.client.get_will(&will_id);
    let child = scenario.client.get_will(&child_id);

    // Balance conservation: source + child = original.
    let source_bal = source_after
        .balances
        .get(scenario.token_address.clone())
        .unwrap_or(0);
    let child_bal = child
        .balances
        .get(scenario.token_address.clone())
        .unwrap_or(0);
    check(
        source_bal + child_bal == original_balance,
        input,
        "source balance + child balance does not equal the original balance after split",
    );
    check(
        child_bal == split_amount,
        input,
        "child will's balance does not equal the requested split amount",
    );

    // Source must retain at least one beneficiary.
    check(
        !source_after.beneficiaries.is_empty(),
        input,
        "source will has no beneficiaries after split",
    );

    // Beneficiary count bounds.
    check(
        source_after.beneficiaries.len() <= MAX_BENEFICIARIES,
        input,
        "source will's beneficiary count exceeds MAX_BENEFICIARIES after split",
    );
    check(
        !child.beneficiaries.is_empty() && child.beneficiaries.len() <= MAX_BENEFICIARIES,
        input,
        "child will's beneficiary count is outside 1..=MAX_BENEFICIARIES",
    );

    // Percentage normalisation: both wills' percentage beneficiaries must sum
    // to 10,000 bps.
    assert_percentage_shares_sum_to_10000(&source_after);
    assert_percentage_shares_sum_to_10000(&child);

    // Index consistency: both wills' beneficiaries must be able to find them.
    assert_beneficiaries_are_indexed(&scenario.client, &source_after);
    assert_beneficiaries_are_indexed(&scenario.client, &child);

    Outcome::Accepted(child_id)
}

// ---------------------------------------------------------------------------
// multi_token_distribution runner (Issue #27)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "fuzzing", derive(arbitrary::Arbitrary))]
pub enum MixedAllocationSpec {
    Percentage(u32),
    FixedAmount(i128),
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "fuzzing", derive(arbitrary::Arbitrary))]
pub struct MixedBeneficiarySpec {
    pub address_slot: u8,
    pub allocation: MixedAllocationSpec,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "fuzzing", derive(arbitrary::Arbitrary))]
pub struct MultiTokenDistributionInput {
    /// Token balances to lock (amount per token)
    pub token_amounts: Vec<i128>,
    /// Beneficiaries with mixed allocations. Positive weights are normalised
    /// into a supported bps total; non-positive weights are treated as
    /// "unspecified" and filled in by the normaliser.
    pub beneficiaries: Vec<MixedBeneficiarySpec>,
    /// Raw keeper bounty in basis points. Values within
    /// `0..=MAX_KEEPER_BOUNTY_BPS` are used verbatim; out-of-range values are
    /// routed to an explicit validation probe instead of the accepted branch.
    pub keeper_bounty_bps: u32,
    /// Targeted token index for keeper bounty (if any)
    pub target_bounty_token_idx: Option<u8>,
    /// When set, the runner additionally probes an out-of-range bounty and
    /// asserts it is rejected, rather than folding it into the accepted run.
    pub probe_invalid_bounty: bool,
}

/// Normalises the input's beneficiary specs into a list the contract accepts,
/// preserving the *shape* of the supplied weights rather than flattening them
/// into equal shares.
///
/// Positive percentage weights are scaled into a total of 10,000 bps using
/// integer arithmetic, so a 1/9999-style split stays 1/9999 and a skewed
/// weight stays skewed. Fixed amounts are clamped so their sum stays below the
/// smallest token balance. Owner-as-beneficiary (slot 0) is permitted.
fn normalize_distribution_beneficiaries(
    pool: &SorobanVec<Address>,
    specs: &[MixedBeneficiarySpec],
    min_balance: i128,
) -> Vec<Beneficiary> {
    // Deduplicate by resolved address so the contract does not reject the
    // list as containing duplicate beneficiaries. Slot 0 (the owner) is kept:
    // owner-as-beneficiary is a supported, deliberately exercised case.
    let mut seen_slots: Vec<u8> = Vec::new();
    let mut kept: Vec<(u8, MixedAllocationSpec)> = Vec::new();
    for spec in specs.iter() {
        let resolved = spec.address_slot % ADDRESS_POOL_SIZE;
        if seen_slots.contains(&resolved) {
            continue;
        }
        seen_slots.push(resolved);
        kept.push((resolved, spec.allocation.clone()));
        if kept.len() >= MAX_BENEFICIARIES as usize {
            break;
        }
    }

    if kept.is_empty() {
        kept.push((1, MixedAllocationSpec::Percentage(10_000)));
    }

    // Fixed amounts first: clamp each so the running sum stays strictly below
    // the smallest balance, leaving something for the percentage shares.
    let mut out: Vec<Beneficiary> = Vec::new();
    let mut fixed_sum: i128 = 0;
    let fixed_cap = (min_balance / 4).max(1);
    for (resolved, allocation) in kept.iter() {
        if let MixedAllocationSpec::FixedAmount(amt) = allocation {
            let fixed = (*amt).clamp(1, fixed_cap);
            if fixed_sum.saturating_add(fixed) < min_balance {
                fixed_sum += fixed;
                out.push(Beneficiary {
                    address: slot(pool, *resolved),
                    allocation: Allocation::FixedAmount(fixed),
                });
            }
        }
    }

    // Percentage weights: take the supplied positive weights verbatim and
    // scale them to 10,000 bps. Non-positive weights fall back to an equal
    // share of whatever is left, so the total is always exactly 10,000.
    let pct_specs: Vec<(u8, u32)> = kept
        .iter()
        .filter_map(|(resolved, allocation)| match allocation {
            MixedAllocationSpec::Percentage(w) if *w > 0 => Some((*resolved, *w)),
            MixedAllocationSpec::Percentage(_) => Some((*resolved, 0)),
            MixedAllocationSpec::FixedAmount(_) => None,
        })
        .collect();

    if !pct_specs.is_empty() {
        let positive_total: u64 = pct_specs
            .iter()
            .map(|(_, w)| *w as u64)
            .filter(|w| *w > 0)
            .sum();
        let unspecified = pct_specs.iter().filter(|(_, w)| *w == 0).count() as u64;

        let mut assigned: u32 = 0;
        let mut last_positive: Option<usize> = None;
        let mut shares: Vec<u32> = Vec::with_capacity(pct_specs.len());
        for (_, weight) in pct_specs.iter() {
            if *weight > 0 && positive_total > 0 {
                // Scale into 10,000 bps, keeping at least 1 bp for any
                // positive weight so tiny shares survive normalisation.
                let scaled = ((*weight as u64) * 10_000 / positive_total) as u32;
                let share = scaled.max(1);
                shares.push(share);
                assigned = assigned.saturating_add(share);
                last_positive = Some(shares.len() - 1);
            } else {
                shares.push(0);
            }
        }

        // Distribute any remaining bps (from rounding down, or from
        // unspecified weights) across the entries, preferring the last
        // positive weight so a 1/9999 split stays exactly 1/9999.
        let mut remaining = 10_000u32.saturating_sub(assigned);
        if unspecified > 0 {
            let each = remaining / unspecified as u32;
            let mut extra = remaining - each * unspecified as u32;
            for (i, (_, weight)) in pct_specs.iter().enumerate() {
                if *weight == 0 {
                    let mut share = each;
                    if extra > 0 {
                        share += 1;
                        extra -= 1;
                    }
                    shares[i] = share;
                    remaining = remaining.saturating_sub(share);
                }
            }
        }
        if remaining > 0 {
            if let Some(idx) = last_positive {
                shares[idx] = shares[idx].saturating_add(remaining);
            } else if let Some(first) = shares.first_mut() {
                *first = first.saturating_add(remaining);
            }
        }

        for ((resolved, _), share) in pct_specs.iter().zip(shares.iter()) {
            out.push(Beneficiary {
                address: slot(pool, *resolved),
                allocation: Allocation::Percentage(*share),
            });
        }
    }

    if out.is_empty() {
        out.push(Beneficiary {
            address: slot(pool, 1),
            allocation: Allocation::Percentage(10_000),
        });
    }

    out
}

/// Picks a bounty value the contract accepts, from the boundary set
/// `{0, 1, MAX_KEEPER_BOUNTY_BPS}`. Out-of-range inputs are *not* silently
/// folded in here; they are handled by [`probe_invalid_bounty`].
fn accepted_bounty_bps(raw: u32) -> u32 {
    if raw <= MAX_KEEPER_BOUNTY_BPS {
        raw
    } else {
        match raw % 3 {
            0 => 0,
            1 => 1,
            _ => MAX_KEEPER_BOUNTY_BPS,
        }
    }
}

/// Explicit validation probe: an out-of-range bounty must be rejected by
/// `create_will` with a declared error, never accepted and never an abort.
fn probe_invalid_bounty(
    client: &WillContractClient<'_>,
    env: &Env,
    owner: &Address,
    tokens: &SorobanVec<(Address, i128)>,
    beneficiaries: &SorobanVec<Beneficiary>,
    input: &MultiTokenDistributionInput,
) {
    let guardians: SorobanVec<Address> = SorobanVec::new(env);
    let invalid = MAX_KEEPER_BOUNTY_BPS + 1;
    match client.try_create_will(
        owner,
        tokens,
        beneficiaries,
        &90,
        &7,
        &guardians,
        &2,
        &Some(invalid),
        &0,
    ) {
        Ok(Ok(_)) => violation(
            input,
            "create_will accepted a keeper bounty above MAX_KEEPER_BOUNTY_BPS",
        ),
        Ok(Err(_)) => violation(input, "create_will returned an undecodable value"),
        Err(Ok(_)) => {}
        Err(Err(e)) => violation(
            input,
            &std::format!("create_will aborted on an out-of-range bounty: {e:?}"),
        ),
    }
}

pub fn run_multi_token_distribution(input: &MultiTokenDistributionInput) -> Outcome {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(BASE_TIMESTAMP);

    let pool = address_pool(&env);
    let owner = pool.get(0).unwrap();
    let keeper = Address::generate(&env);

    // Filter token amounts to 1..=3 valid amounts (positive, <= MAX_MINT)
    let mut valid_amounts: Vec<i128> = input
        .token_amounts
        .iter()
        .filter_map(|&a| {
            let clamped = a.clamp(1, MAX_MINT);
            if clamped > 0 {
                Some(clamped)
            } else {
                None
            }
        })
        .take(3)
        .collect();

    if valid_amounts.is_empty() {
        valid_amounts.push(1_000_000);
    }

    // Register tokens and mint to owner
    let mut token_addresses: Vec<Address> = Vec::new();
    let mut tokens_to_lock: SorobanVec<(Address, i128)> = SorobanVec::new(&env);
    for &amount in valid_amounts.iter() {
        let sac = env.register_stellar_asset_contract_v2(owner.clone());
        let addr = sac.address();
        StellarAssetClient::new(&env, &addr).mint(&owner, &amount);
        token_addresses.push(addr.clone());
        tokens_to_lock.push_back((addr, amount));
    }

    // Normalise the supplied weights into a supported allocation, preserving
    // the shape of the input rather than flattening to equal shares.
    let min_balance = *valid_amounts.iter().min().unwrap();
    let sanitized_bens = normalize_distribution_beneficiaries(
        &pool,
        &input.beneficiaries,
        min_balance,
    );

    let mut soroban_bens: SorobanVec<Beneficiary> = SorobanVec::new(&env);
    for b in sanitized_bens.iter() {
        soroban_bens.push_back(b.clone());
    }

    let contract_id = env.register(WillContract, ());
    let client = WillContractClient::new(&env, &contract_id);

    // Out-of-range bounty inputs are an explicit validation concern, not part
    // of the accepted distribution branch.
    if input.probe_invalid_bounty || input.keeper_bounty_bps > MAX_KEEPER_BOUNTY_BPS {
        probe_invalid_bounty(
            &client,
            &env,
            &owner,
            &tokens_to_lock,
            &soroban_bens,
            input,
        );
    }

    let bounty_bps = accepted_bounty_bps(input.keeper_bounty_bps);
    let guardian_slots: SorobanVec<Address> = SorobanVec::new(&env);

    let will_id_res = client.try_create_will(
        &owner,
        &tokens_to_lock,
        &soroban_bens,
        &90,
        &7,
        &guardian_slots,
        &2,
        &Some(bounty_bps),
        &0,
    );

    let will_id = match will_id_res {
        Ok(Ok(id)) => id,
        Ok(Err(_)) => violation(input, "create_will returned undecodable value"),
        Err(Ok(_)) => return Outcome::Rejected,
        Err(Err(e)) => violation(input, &std::format!("create_will aborted: {e:?}")),
    };

    if let Some(target_idx) = input.target_bounty_token_idx {
        let idx = target_idx as usize % token_addresses.len();
        let target_tok = token_addresses[idx].clone();
        // The selected index is always valid (it is reduced modulo the token
        // count), so a rejection here is a harness failure, not a legitimate
        // contract outcome. Ignoring the result would let a setter regression
        // silently fall back to default distribution while conservation still
        // held, masking the loss of targeted-bounty coverage.
        match client.try_set_keeper_bounty_token(&will_id, &owner, &Some(target_tok.clone())) {
            Ok(Ok(())) => {}
            Ok(Err(_)) => violation(
                input,
                "set_keeper_bounty_token returned an undecodable value",
            ),
            Err(Ok(e)) => violation(
                input,
                &std::format!(
                    "set_keeper_bounty_token rejected a valid target token: {e:?}"
                ),
            ),
            Err(Err(e)) => violation(
                input,
                &std::format!("set_keeper_bounty_token aborted on a valid target: {e:?}"),
            ),
        }

        // Confirm the requested target is actually the active selection before
        // releasing, so the release below really exercises the targeted-bounty
        // branch rather than the default distribution path.
        let stored = client.get_will(&will_id);
        check(
            stored.keeper_bounty_token == Some(target_tok.clone()),
            input,
            "the requested bounty token is not the active selection after setup",
        );
    }

    // Advance past check-in deadline
    env.ledger()
        .set_timestamp(BASE_TIMESTAMP + 91 * SECONDS_PER_DAY);
    let trigger_res = client.try_trigger_will(&will_id);
    match trigger_res {
        Ok(Ok(())) => {}
        Ok(Err(_)) => violation(input, "trigger_will returned undecodable value"),
        Err(Ok(_)) => return Outcome::Rejected,
        Err(Err(e)) => violation(input, &std::format!("trigger_will aborted: {e:?}")),
    }

    // Advance past grace deadline
    env.ledger()
        .set_timestamp(BASE_TIMESTAMP + 99 * SECONDS_PER_DAY);
    let release_res = client.try_release_inheritance(&will_id, &Some(keeper.clone()));
    match release_res {
        Ok(Ok(())) => {}
        Ok(Err(_)) => violation(input, "release_inheritance returned undecodable value"),
        Err(Ok(_)) => return Outcome::Rejected,
        Err(Err(e)) => violation(input, &std::format!("release_inheritance aborted: {e:?}")),
    }

    // --- CONSERVATION INVARIANT CHECK ---
    for (i, token_addr) in token_addresses.iter().enumerate() {
        let token_client = TokenClient::new(&env, token_addr);
        let original_amount = valid_amounts[i];

        // 1. Contract holds 0 dust
        check(
            token_client.balance(&contract_id) == 0,
            input,
            "contract balance is not zero after release (dust was stranded)",
        );

        // 2. Total received by all parties (beneficiaries + keeper + owner refund) equals original amount
        let mut sum_beneficiaries: i128 = 0;
        for b in sanitized_bens.iter() {
            let bal = token_client.balance(&b.address);
            check(bal >= 0, input, "beneficiary received negative payout");
            sum_beneficiaries += bal;
        }

        let keeper_received = token_client.balance(&keeper);
        check(
            keeper_received >= 0,
            input,
            "keeper received negative bounty",
        );

        let owner_refund = token_client.balance(&owner);
        check(owner_refund >= 0, input, "owner received negative refund");

        let total_distributed = sum_beneficiaries + keeper_received + owner_refund;
        check(
            total_distributed == original_amount,
            input,
            "token balance conservation violated: total distributed != original amount",
        );
    }

    Outcome::Accepted(will_id)
}
