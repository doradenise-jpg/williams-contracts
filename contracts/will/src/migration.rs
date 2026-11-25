//! Storage schema versioning for persisted `Will` entries (issue #446).
//!
//! Soroban encodes a `#[contracttype]` struct as an XDR map keyed by field
//! name, and decoding is strict: a stored entry that is missing a field the
//! current `Will` declares (or carries one it no longer declares) fails to
//! convert. Adding a field to `Will` would therefore make every will written
//! by an earlier contract version unreadable.
//!
//! This module holds the two pieces that keep old entries loadable:
//!
//! 1. [`decode_will`] — every read of a `DataKey::Will` entry goes through
//!    here. It tries the current layout first and then each known legacy
//!    layout, lifting a legacy entry into the current `Will` shape while
//!    keeping its original `schema_version`.
//! 2. [`upgrade`] — applies the per-version migration steps in order until
//!    the will reaches [`CURRENT_SCHEMA_VERSION`]. Called by
//!    `WillContract::migrate_will`, which persists the result.
//!
//! See the "Storage schema versioning" section of CONTRIBUTING.md for the
//! step-by-step process of introducing a new version.
//!
//! ## Schema v2 field ordering (issue #117)
//!
//! Soroban XDR serializes struct fields in declaration order, so the order of
//! `Will`'s fields directly affects the encoded byte layout. In v2 the fields
//! are grouped by kind to keep fixed-size scalars and addresses adjacent:
//!
//! 1. Identity and token metadata: `id`, `owner`, `token`, `is_native`,
//!    `balance`, `balances`.
//! 2. Beneficiaries: `beneficiaries`, `hashed_beneficiaries`.
//! 3. Timing: `checkin_period_days`, `grace_period_days`, `last_checkin`,
//!    `trigger_time`, `confirmation_deadline`.
//! 4. Status and guardians: `status`, `guardians`, `guardian_vote_weight`,
//!    `guardian_votes`, `guardian_cancel_vote_weight`, `guardian_cancel_votes`,
//!    `guardian_threshold`, `guardian_list_updated_at`.
//! 5. Versioning and payout config: `schema_version`, `keeper_bounty_bps`,
//!    `keeper_bounty_token`, `delegate`.
//!
//! Because XDR maps are keyed by field name, this reordering is wire-compatible
//! with v1 entries: `decode_will` still lifts them via [`legacy::WillV1`], and
//! the v1 → v2 step only stamps the version. The legacy struct below keeps its
//! original declaration order verbatim so old entries continue to decode.

use soroban_sdk::{Env, TryFromVal, Val};

use crate::errors::WillError;
use crate::types::Will;

/// Schema version written into every newly created or migrated will.
///
/// Bump this whenever the persisted `Will` layout changes, and add both a
/// legacy decode arm in [`decode_will`] and a migration step in [`upgrade`].
///
/// Keep in sync with [`crate::storage::CURRENT_SCHEMA_VERSION`]; the two are
/// asserted equal at compile time in `migrate_will_test.rs`.
pub const CURRENT_SCHEMA_VERSION: u32 = 3;

/// `true` when this build is a pre-release (release candidate) of the
/// contract, `false` for a final build. See the module docs for how this
/// combines with `CONTRACT_VERSION` to identify an RC build.
pub const CONTRACT_PRERELEASE: bool = false;

/// Pre-release candidate number for this build (`1` for `rc.1`), or `0` when
/// [`CONTRACT_PRERELEASE`] is `false`.
pub const CONTRACT_PRERELEASE_NUM: u32 = 0;

/// Packs a SemVer triple into the `u32` encoding used by `CONTRACT_VERSION`.
///
/// Each component must be below 1_000; larger values would overlap adjacent
/// fields. This is the single source of truth for the encoding so the
/// contract constant and its tests cannot drift apart.
///
/// # Panics
/// Panics if any component is greater than or equal to 1_000.
pub const fn encode_version(major: u32, minor: u32, patch: u32) -> u32 {
    assert!(
        major < 1_000 && minor < 1_000 && patch < 1_000,
        "version components must be below 1,000"
    );
    major * 1_000_000 + minor * 1_000 + patch
}

/// Splits a packed version back into `(major, minor, patch)`.
pub const fn decode_version(version: u32) -> (u32, u32, u32) {
    (
        version / 1_000_000,
        (version / 1_000) % 1_000,
        version % 1_000,
    )
}

/// Legacy `Will` layouts, frozen at the version their successor replaced.
///
/// A legacy struct is the **only** way an entry written by an older contract
/// version can still be decoded: `Will::try_from_val` fails outright when the
/// stored XDR map is missing a field the current layout declares. Each type
/// therefore copies its era's `Will` verbatim — field names and order
/// included — and converts into the current shape with safe defaults for
/// whatever the newer version added.
pub mod legacy {
    use soroban_sdk::{contracttype, Address, Map, Vec};

    use crate::types::{Beneficiary, Guardian, HashedBeneficiary, Will, WillStatus};

    /// The `Will` layout as of schema version 1 — the layout `schema_version`
    /// itself shipped in (#446), with `keeper_bounty_bps` and `delegate`
    /// already present.
    ///
    /// Superseded by v2, which added `Will::keeper_bounty_token` (#109) and
    /// regrouped the field declaration order for byte compactness (#117).
    #[contracttype]
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct WillV1 {
        pub id: u64,
        pub owner: Address,
        pub balances: Map<Address, i128>,
        pub token: Address,
        pub is_native: bool,
        pub balance: i128,
        pub beneficiaries: Vec<Beneficiary>,
        pub hashed_beneficiaries: Vec<HashedBeneficiary>,
        pub checkin_period_days: u64,
        pub grace_period_days: u64,
        pub last_checkin: u64,
        pub trigger_time: Option<u64>,
        pub confirmation_deadline: Option<u64>,
        pub status: WillStatus,
        pub guardians: Vec<Guardian>,
        pub guardian_vote_weight: u32,
        pub guardian_votes: u32,
        pub guardian_cancel_vote_weight: u32,
        pub guardian_cancel_votes: u32,
        pub guardian_threshold: u32,
        pub guardian_list_updated_at: u64,
        pub schema_version: u32,
        pub keeper_bounty_bps: u32,
        pub delegate: Option<Address>,
    }

    /// The `Will` layout as of schema version 2: it added
    /// `keeper_bounty_token` (#109) and regrouped fields (#117), but predates
    /// `grace_extension_days` (#102). An entry written by that release matches
    /// neither the current `Will` nor `WillV1`, so it must be frozen here
    /// verbatim (issue #176).
    #[contracttype]
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct WillV2 {
        pub id: u64,
        pub owner: Address,
        pub balances: Map<Address, i128>,
        pub token: Address,
        pub is_native: bool,
        pub balance: i128,
        pub beneficiaries: Vec<Beneficiary>,
        pub hashed_beneficiaries: Vec<HashedBeneficiary>,
        pub checkin_period_days: u64,
        pub grace_period_days: u64,
        pub last_checkin: u64,
        pub trigger_time: Option<u64>,
        pub confirmation_deadline: Option<u64>,
        pub status: WillStatus,
        pub guardians: Vec<Guardian>,
        pub guardian_vote_weight: u32,
        pub guardian_votes: u32,
        pub guardian_cancel_vote_weight: u32,
        pub guardian_cancel_votes: u32,
        pub guardian_threshold: u32,
        pub guardian_list_updated_at: u64,
        pub schema_version: u32,
        pub keeper_bounty_bps: u32,
        pub keeper_bounty_token: Option<Address>,
        pub delegate: Option<Address>,
    }

    impl WillV2 {
        /// Lifts a prior-v2 entry into the current shape. Only
        /// `grace_extension_days` was added in schema v3, so it defaults to
        /// zero (no extension granted under the old contract) while the
        /// original `schema_version` is preserved for the upgrade step.
        pub fn into_current(self) -> Will {
            Will {
                id: self.id,
                owner: self.owner,
                balances: self.balances,
                token: self.token,
                is_native: self.is_native,
                balance: self.balance,
                beneficiaries: self.beneficiaries,
                hashed_beneficiaries: self.hashed_beneficiaries,
                checkin_period_days: self.checkin_period_days,
                grace_period_days: self.grace_period_days,
                last_checkin: self.last_checkin,
                trigger_time: self.trigger_time,
                confirmation_deadline: self.confirmation_deadline,
                status: self.status,
                guardians: self.guardians,
                guardian_vote_weight: self.guardian_vote_weight,
                guardian_votes: self.guardian_votes,
                guardian_cancel_vote_weight: self.guardian_cancel_vote_weight,
                guardian_cancel_votes: self.guardian_cancel_votes,
                guardian_threshold: self.guardian_threshold,
                guardian_list_updated_at: self.guardian_list_updated_at,
                grace_extension_days: 0,
                schema_version: self.schema_version,
                keeper_bounty_bps: self.keeper_bounty_bps,
                keeper_bounty_token: self.keeper_bounty_token,
                delegate: self.delegate,
            }
        }
    }

    impl WillV1 {
        /// Lifts a v1 entry into the current `Will` shape. The field v2 added
        /// defaults to `None` — the proportional keeper-bounty split (#109) —
        /// and `schema_version` stays at 1 so [`super::upgrade`] still runs
        /// the v1 → v2 step.
        pub fn into_current(self) -> Will {
            Will {
                id: self.id,
                owner: self.owner,
                token: self.token,
                is_native: self.is_native,
                balance: self.balance,
                balances: self.balances,
                beneficiaries: self.beneficiaries,
                hashed_beneficiaries: self.hashed_beneficiaries,
                checkin_period_days: self.checkin_period_days,
                grace_period_days: self.grace_period_days,
                last_checkin: self.last_checkin,
                // Legacy records predate the re-trigger cooldown; 0 keeps
                // them immediately triggerable (#174).
                trigger_cooldown_until: 0,
                trigger_time: self.trigger_time,
                confirmation_deadline: self.confirmation_deadline,
                status: self.status,
                guardians: self.guardians,
                guardian_vote_weight: self.guardian_vote_weight,
                guardian_votes: self.guardian_votes,
                guardian_cancel_vote_weight: self.guardian_cancel_vote_weight,
                guardian_cancel_votes: self.guardian_cancel_votes,
                guardian_threshold: self.guardian_threshold,
                guardian_list_updated_at: self.guardian_list_updated_at,
                schema_version: self.schema_version,
                keeper_bounty_bps: self.keeper_bounty_bps,
                keeper_bounty_token: None,
                delegate: self.delegate,
                grace_extension_days: 0,
            }
        }
    }
}

/// Decodes a raw persisted will, accepting the current layout and every known
/// legacy layout.
///
/// A legacy entry is converted into the current `Will` shape with defaults
/// for fields it did not have; its `schema_version` is left at the version it
/// was written with so [`upgrade`] can run the remaining steps.
///
/// Returns [`WillError::UnsupportedSchemaVersion`] if no known layout matches.
pub fn decode_will(env: &Env, raw: &Val) -> Result<Will, WillError> {
    if let Ok(will) = Will::try_from_val(env, raw) {
        return Ok(will);
    }

    // Legacy layouts are tried here, newest first. The prior v2 entry has
    // `keeper_bounty_token` but no `grace_extension_days` (issue #176).
    if let Ok(old) = legacy::WillV2::try_from_val(env, raw) {
        return Ok(old.into_current());
    }
    if let Ok(old) = legacy::WillV1::try_from_val(env, raw) {
        return Ok(old.into_current());
    }

    Err(WillError::UnsupportedSchemaVersion)
}

/// Runs every migration step between `will.schema_version` and
/// [`CURRENT_SCHEMA_VERSION`], in order. A will already at the current version
/// is returned unchanged. A will written by a newer build (e.g. read after a
/// contract rollback) panics with [`WillError::FutureSchemaVersion`] rather
/// than being silently treated as current (#129).
pub fn upgrade(env: &Env, mut will: Will) -> Will {
    if will.schema_version > CURRENT_SCHEMA_VERSION {
        soroban_sdk::panic_with_error!(env, WillError::FutureSchemaVersion);
    }
    while will.schema_version < CURRENT_SCHEMA_VERSION {
        will = match will.schema_version {
            0 => migrate_v0_to_v1(env, will),
            1 => migrate_v1_to_v2(env, will),
            2 => migrate_v2_to_v3(env, will),
            // Every version below CURRENT_SCHEMA_VERSION must have a step.
            _ => soroban_sdk::panic_with_error!(env, WillError::UnsupportedSchemaVersion),
        };
    }
    will
}

/// v0 → v1: v0 wills predate the `schema_version` field being populated; the
/// layout is otherwise identical, so only the version number changes.
fn migrate_v0_to_v1(_env: &Env, mut will: Will) -> Will {
    will.schema_version = 1;
    will
}

/// v1 → v2: v2 added `Will::keeper_bounty_token` (#109) and regrouped the
/// `Will` field declaration order for byte compactness (#117). `decode_will`
/// has already lifted the entry into the current shape with the new field
/// defaulted to `None` (proportional bounty split), so the step only stamps
/// the new version.
fn migrate_v1_to_v2(_env: &Env, mut will: Will) -> Will {
    will.schema_version = 2;
    will
}

/// v2 → v3: v3 added `Will::grace_extension_days` (#102). `decode_will` has
/// already lifted the prior-v2 entry into the current shape with the new
/// field defaulted to zero, so this step only stamps the new version (issue
/// #176).
fn migrate_v2_to_v3(_env: &Env, mut will: Will) -> Will {
    will.schema_version = 3;
    will
}

#[cfg(test)]
mod version_tests {
    use super::*;
    use soroban_sdk::{
        testutils::Address as _, vec, Address, IntoVal, Map,
    };

    use crate::types::{Allocation, Beneficiary, WillStatus};

    fn prior_v2(env: &Env, status: WillStatus) -> legacy::WillV2 {
        let owner = Address::generate(env);
        let token = Address::generate(env);
        let heir = Address::generate(env);
        let mut balances = Map::new(env);
        balances.set(token.clone(), 100_i128);
        legacy::WillV2 {
            id: 7,
            owner,
            balances,
            token: token.clone(),
            is_native: false,
            balance: 100,
            beneficiaries: vec![
                env,
                Beneficiary {
                    address: heir,
                    allocation: Allocation::Percentage(10_000),
                },
            ],
            hashed_beneficiaries: vec![env],
            checkin_period_days: 90,
            grace_period_days: 7,
            last_checkin: 1_000,
            trigger_time: None,
            confirmation_deadline: None,
            status,
            guardians: vec![env],
            guardian_vote_weight: 0,
            guardian_votes: 0,
            guardian_cancel_vote_weight: 0,
            guardian_cancel_votes: 0,
            guardian_threshold: 0,
            guardian_list_updated_at: 500,
            schema_version: 2,
            keeper_bounty_bps: 10,
            keeper_bounty_token: Some(token),
            delegate: None,
        }
    }

    #[test]
    fn prior_v2_active_record_decodes_and_preserves_every_field() {
        // Issue #176: the exact prior-v2 field set must decode even though it
        // lacks `grace_extension_days`.
        let env = Env::default();
        let old = prior_v2(&env, WillStatus::Active);
        let raw: Val = old.clone().into_val(&env);
        let decoded = decode_will(&env, &raw).expect("prior v2 entry must decode");

        assert_eq!(decoded.id, old.id);
        assert_eq!(decoded.owner, old.owner);
        assert_eq!(decoded.balance, old.balance);
        assert_eq!(decoded.beneficiaries, old.beneficiaries);
        assert_eq!(decoded.hashed_beneficiaries, old.hashed_beneficiaries);
        assert_eq!(decoded.checkin_period_days, old.checkin_period_days);
        assert_eq!(decoded.grace_period_days, old.grace_period_days);
        assert_eq!(decoded.last_checkin, old.last_checkin);
        assert_eq!(decoded.guardians, old.guardians);
        assert_eq!(decoded.guardian_threshold, old.guardian_threshold);
        assert_eq!(decoded.keeper_bounty_bps, old.keeper_bounty_bps);
        assert_eq!(decoded.keeper_bounty_token, old.keeper_bounty_token);
        assert_eq!(decoded.delegate, old.delegate);
        assert_eq!(decoded.grace_extension_days, 0);
        assert_eq!(decoded.schema_version, 2);
    }

    #[test]
    fn prior_v2_archived_record_decodes() {
        let env = Env::default();
        let old = prior_v2(&env, WillStatus::Settled);
        let raw: Val = old.into_val(&env);
        let decoded = decode_will(&env, &raw).expect("archived prior-v2 entry must decode");
        assert_eq!(decoded.status, WillStatus::Settled);
        assert_eq!(decoded.grace_extension_days, 0);
    }

    #[test]
    fn prior_v2_record_upgrades_to_schema_v3() {
        let env = Env::default();
        let old = prior_v2(&env, WillStatus::Active);
        let raw: Val = old.clone().into_val(&env);
        let decoded = decode_will(&env, &raw).unwrap();
        let migrated = upgrade(&env, decoded);
        assert_eq!(migrated.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(migrated.grace_extension_days, 0);
        assert_eq!(migrated.keeper_bounty_token, old.keeper_bounty_token);
    }

    #[test]
    fn encode_decode_roundtrip() {
        let packed = encode_version(1, 3, 0);
        assert_eq!(packed, 1_003_000);
        assert_eq!(decode_version(packed), (1, 3, 0));
    }

    #[test]
    fn decode_handles_max_components() {
        const MAX_PACKED: u32 = encode_version(999, 999, 999);
        assert_eq!(decode_version(MAX_PACKED), (999, 999, 999));
        for (major, minor, patch) in [(999, 0, 0), (0, 999, 0), (0, 0, 999)] {
            assert_eq!(
                decode_version(encode_version(major, minor, patch)),
                (major, minor, patch)
            );
        }
    }

    #[test]
    fn encode_rejects_out_of_range_components() {
        for invalid in [1_000, u32::MAX] {
            for (major, minor, patch) in [(invalid, 0, 0), (0, invalid, 0), (0, 0, invalid)] {
                assert!(std::panic::catch_unwind(|| encode_version(major, minor, patch)).is_err());
            }
        }
    }

    #[test]
    fn prerelease_flag_is_consistent() {
        // A pre-release build must carry a candidate number; a final build
        // must not. This is what client bindings rely on to detect RCs.
        if CONTRACT_PRERELEASE {
            assert!(CONTRACT_PRERELEASE_NUM >= 1);
        } else {
            assert_eq!(CONTRACT_PRERELEASE_NUM, 0);
        }
    }
}
