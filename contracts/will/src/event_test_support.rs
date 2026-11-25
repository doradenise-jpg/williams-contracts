#![cfg(test)]

//! Shared helpers for asserting on contract events under the soroban-sdk v28
//! [`ContractEvents`] API.
//!
//! `env.events().all()` used to return `Vec<(Address, Vec<Val>, Val)>`, which
//! tests could iterate and destructure directly. Since SDK v25 it returns
//! [`ContractEvents`], whose events are only reachable in their XDR form:
//! `env.events().all().events()` yields `&[xdr::ContractEvent]`, where each
//! event's topics and data are [`xdr::ScVal`]s. These helpers wrap the small
//! amount of XDR plumbing the migrated tests need: reading the leading
//! symbol/u64 topics, finding an event by topic, and building the expected
//! XDR event for a full-equality assertion.

use soroban_sdk::{
    symbol_short,
    testutils::Events as _,
    xdr::{self, ContractEvent, ContractEventBody, ScSymbol, ScVal},
    Address, Env, IntoVal, Symbol, TryFromVal as _, Val, Vec,
};

/// Converts an XDR symbol value into a Soroban [`Symbol`].
pub fn symbol_value(env: &Env, sc: &ScSymbol) -> Option<Symbol> {
    Symbol::try_from_val(env, sc).ok()
}

/// Returns the `ContractEventV0` body of `event`, or `None` if its shape is
/// not the one the host emits for contract events.
fn body(event: &ContractEvent) -> Option<&xdr::ContractEventV0> {
    match &event.body {
        ContractEventBody::V0(v0) => Some(v0),
        #[allow(unreachable_patterns)]
        _ => None,
    }
}

/// Returns the topics vector of `event`, or an empty slice when absent.
fn topics_of(event: &ContractEvent) -> &[ScVal] {
    match body(event) {
        Some(v0) => v0.topics.as_slice(),
        None => &[],
    }
}

/// Returns the `index`-th topic of `event` when it exists.
fn topic(event: &ContractEvent, index: usize) -> Option<&ScVal> {
    topics_of(event).get(index)
}

/// Returns the leading symbol topic of `event`, or `None` if the first topic
/// is not a symbol.
pub fn topic_symbol(env: &Env, event: &ContractEvent) -> Option<Symbol> {
    match topic(event, 0)? {
        ScVal::Symbol(sc) => symbol_value(env, sc),
        _ => None,
    }
}

/// Returns the `index`-th topic of `event` when it is a `u64`.
pub fn topic_u64(event: &ContractEvent, index: usize) -> Option<u64> {
    match topic(event, index)? {
        ScVal::U64(v) => Some(*v),
        _ => None,
    }
}

/// Returns `event`'s data payload when it is a vector of XDR values.
pub fn data_vec(event: &ContractEvent) -> Option<&[ScVal]> {
    match &body(event)?.data {
        ScVal::Vec(Some(v)) => Some(v.as_slice()),
        _ => None,
    }
}

/// Returns `event`'s data payload when it is a `u64`.
pub fn data_u64(event: &ContractEvent) -> Option<u64> {
    match &body(event)?.data {
        ScVal::U64(v) => Some(*v),
        _ => None,
    }
}

/// Returns `event`'s data payload when it is an address.
///
/// Several lifecycle events publish a single scalar (the guardian address
/// events are the simplest example); a scalar is represented in XDR as a
/// bare `ScVal`, not a one-element vector.
pub fn data_address(env: &Env, event: &ContractEvent) -> Option<Address> {
    address_from(&body(event)?.data, env)
}

/// Decodes an [`Address`] from an XDR address value.
pub fn address_from(sc: &ScVal, env: &Env) -> Option<Address> {
    match sc {
        ScVal::Address(addr) => Address::try_from_val(env, addr).ok(),
        _ => None,
    }
}

/// Decodes a `u64` from an XDR value.
pub fn u64_from(sc: &ScVal) -> Option<u64> {
    match sc {
        ScVal::U64(v) => Some(*v),
        _ => None,
    }
}

/// Decodes a `u32` from an XDR value.
pub fn u32_from(sc: &ScVal) -> Option<u32> {
    match sc {
        ScVal::U32(v) => Some(*v),
        _ => None,
    }
}

/// Decodes an `i128` from an XDR value.
pub fn i128_from(sc: &ScVal) -> Option<i128> {
    match sc {
        ScVal::I128(parts) => Some((parts.hi as i128) << 64 | parts.lo as i128),
        _ => None,
    }
}

/// Decodes an [`Option<u64>`] — an enum with the `Some`/`None`-style shape
/// the SDK uses for `Option` fields — from an XDR value.
pub fn option_u64_from(sc: &ScVal) -> Option<Option<u64>> {
    match sc {
        ScVal::Void => Some(None),
        ScVal::U64(v) => Some(Some(*v)),
        // `None` serialises as the unit variant with discriminant 0, i.e. an
        // empty vector.
        ScVal::Vec(opt) if opt.as_ref().is_none_or(|v| v.is_empty()) => Some(None),
        // `Some(x)` serialises as the newtype variant with discriminant 1, a
        // one-element vector.
        ScVal::Vec(Some(v)) if v.len() == 1 => Some(Some(u64_from(&v[0])?)),
        _ => None,
    }
}

/// Decodes a `Vec<u64>` from a tuple payload's XDR vector element.
///
/// Returns `None` if any element is not a `u64`; malformed elements must not
/// be silently dropped, or assertions could pass against a schema regression.
pub fn u64_list_from(sc: &[ScVal]) -> Option<std::vec::Vec<u64>> {
    sc.iter().map(u64_from).collect()
}

/// Decodes a `Vec<Symbol>` from a tuple payload's XDR vector element.
pub fn symbol_list_from(env: &Env, sc: &ScVal) -> Option<Vec<Symbol>> {
    let items = match sc {
        ScVal::Vec(Some(v)) => v,
        _ => return None,
    };
    let mut out = Vec::new(env);
    for item in items.iter() {
        match item {
            ScVal::Symbol(v) => out.push_back(symbol_value(env, v)?),
            _ => return None,
        }
    }
    Some(out)
}

/// Decodes a [`Guardian`] (address, weight, consent) from its XDR map form.
pub fn guardian_from(env: &Env, sc: &ScVal) -> Option<crate::Guardian> {
    let map = match sc {
        ScVal::Map(Some(map)) => map,
        _ => return None,
    };
    let mut address = None;
    let mut weight = None;
    let mut consent = None;
    for entry in map.iter() {
        let key = match &entry.key {
            ScVal::Symbol(k) => symbol_value(env, k)?,
            _ => return None,
        };
        if key == symbol_short!("address") {
            address = address_from(&entry.val, env);
        } else if key == symbol_short!("weight") {
            weight = u32_from(&entry.val);
        } else if key == symbol_short!("consent") {
            consent = match &entry.val {
                ScVal::U32(0) => Some(crate::GuardianConsent::Pending),
                ScVal::U32(1) => Some(crate::GuardianConsent::Accepted),
                ScVal::U32(2) => Some(crate::GuardianConsent::Rejected),
                ScVal::Symbol(v) => {
                    symbol_value(env, v).and_then(|sym| match sym.to_string().as_str() {
                        "Pending" => Some(crate::GuardianConsent::Pending),
                        "Accepted" => Some(crate::GuardianConsent::Accepted),
                        "Rejected" => Some(crate::GuardianConsent::Rejected),
                        _ => None,
                    })
                }
                _ => None,
            };
        } else {
            return None;
        }
    }
    Some(crate::Guardian {
        address: address?,
        weight: weight?,
        consent: consent?,
    })
}

/// Decodes a `Vec<Guardian>` from its XDR vector form.
pub fn guardian_list_from(env: &Env, sc: &ScVal) -> Option<Vec<crate::Guardian>> {
    let items = match sc {
        ScVal::Vec(Some(v)) => v,
        _ => return None,
    };
    let mut out = Vec::new(env);
    for item in items.iter() {
        out.push_back(guardian_from(env, item)?);
    }
    Some(out)
}

/// Decodes an [`Address`] from the `index`-th topic of `event`.
pub fn topic_address(env: &Env, event: &ContractEvent, index: usize) -> Option<Address> {
    match topic(event, index)? {
        ScVal::Address(sc) => Address::try_from_val(env, sc).ok(),
        _ => None,
    }
}

/// Decodes an [`Allocation`] from its XDR map form.
pub fn allocation_from(env: &Env, sc: &ScVal) -> Option<crate::Allocation> {
    match sc {
        ScVal::Vec(Some(v)) if v.len() == 2 => {
            let variant = match &v[0] {
                ScVal::Symbol(s) => symbol_value(env, s)?,
                _ => return None,
            };
            if variant == Symbol::new(env, "Percentage") {
                Some(crate::Allocation::Percentage(u32_from(&v[1])?))
            } else if variant == Symbol::new(env, "FixedAmount") {
                Some(crate::Allocation::FixedAmount(i128_from(&v[1])?))
            } else {
                None
            }
        }
        // `Percentage(u32)` is an enum whose single variant carries a
        // one-element tuple; `FixedAmount(i128)` the same with an `i128`.
        ScVal::Map(Some(map)) => {
            // The enum is a single-entry map; extra entries mean the payload
            // does not match the expected schema.
            if map.len() != 1 {
                return None;
            }
            let entry = map.iter().next()?;
            let variant = match &entry.key {
                ScVal::Symbol(v) => symbol_value(env, v)?,
                _ => return None,
            };
            let args = match &entry.val {
                ScVal::Vec(Some(v)) => v,
                _ => return None,
            };
            if variant == Symbol::new(env, "Percentage") {
                let [bps] = args.as_slice() else { return None };
                Some(crate::Allocation::Percentage(u32_from(bps)?))
            } else if variant == Symbol::new(env, "FixedAmount") {
                let [amount] = args.as_slice() else {
                    return None;
                };
                Some(crate::Allocation::FixedAmount(i128_from(amount)?))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Decodes a [`Beneficiary`] from its XDR map form
/// (`{address: ..., allocation: ...}`).
pub fn beneficiary_from(env: &Env, sc: &ScVal) -> Option<crate::Beneficiary> {
    let map = match sc {
        ScVal::Map(Some(map)) => map,
        _ => return None,
    };
    let mut address = None;
    let mut allocation = None;
    for entry in map.iter() {
        let key = match &entry.key {
            ScVal::Symbol(k) => symbol_value(env, k)?,
            _ => return None,
        };
        if key == symbol_short!("address") {
            address = address_from(&entry.val, env);
        } else if key == Symbol::new(env, "allocation") {
            allocation = allocation_from(env, &entry.val);
        } else {
            return None;
        }
    }
    Some(crate::Beneficiary {
        address: address?,
        allocation: allocation?,
    })
}

/// Decodes a `Vec<Beneficiary>` from its XDR vector form.
pub fn beneficiary_list_from(env: &Env, sc: &ScVal) -> Option<Vec<crate::Beneficiary>> {
    let items = match sc {
        ScVal::Vec(Some(v)) => v,
        _ => return None,
    };
    let mut out = Vec::new(env);
    for item in items.iter() {
        out.push_back(beneficiary_from(env, item)?);
    }
    Some(out)
}

/// Returns the XDR contract id of `event`'s emitter, or `None` when the event
/// carries no contract id (e.g. diagnostic events).
fn emitter_of(event: &ContractEvent) -> Option<&xdr::Hash> {
    event.contract_id.as_ref()
}

/// Returns whether `event` was emitted by `emitter`.
///
/// Emitter identity is checked before any topic/payload matching so that
/// identical topics published by another contract (for example a token
/// fixture invoked from the same test) can never satisfy a will assertion.
pub fn emitted_by(event: &ContractEvent, emitter: &Address) -> bool {
    let expected = match soroban_sdk::xdr::ScAddress::from(emitter) {
        soroban_sdk::xdr::ScAddress::Contract(hash) => hash,
        _ => return false,
    };
    emitter_of(event) == Some(&expected)
}

/// Finds the event emitted by `emitter` whose topics lead with `symbol` and —
/// when `will_id` is given — whose second topic is `will_id`, panicking with a
/// message naming `what` when no such event from `emitter` exists.
///
/// Events from other contracts are ignored even when their topics collide, so
/// an unrelated emitter cannot satisfy the assertion. Only events from the
/// last top-level invocation are visible: the host retains nothing older, so
/// this must be called before any further contract call.
pub fn find_event(
    env: &Env,
    emitter: &Address,
    symbol: Symbol,
    will_id: Option<u64>,
    what: &str,
) -> ContractEvent {
    let events = env.events().all();
    events
        .events()
        .iter()
        .find(|event| {
            if !emitted_by(event, emitter) {
                return false;
            }
            if topic_symbol(env, event).as_ref() != Some(&symbol) {
                return false;
            }
            match will_id {
                Some(id) => topic_u64(event, 1) == Some(id),
                None => true,
            }
        })
        .cloned()
        .unwrap_or_else(|| panic!("{what} event not found from emitter {emitter:?}"))
}

/// Counts the events emitted by `emitter` whose leading topic is `symbol`.
///
/// Events from other contracts are excluded even when their topics collide.
/// Like [`find_event`], only events from the last top-level invocation are
/// visible.
pub fn count_events(env: &Env, emitter: &Address, symbol: Symbol) -> u32 {
    let events = env.events().all();
    events
        .events()
        .iter()
        .filter(|event| emitted_by(event, emitter))
        .filter(|event| topic_symbol(env, event).as_ref() == Some(&symbol))
        .count() as u32
}

/// Builds the XDR form an event published by `contract_id` with the given
/// topics and data takes, for use in `assert_eq!` against a found event.
///
/// The `contract_id` recorded here is the emitter identity that
/// [`find_event`] and [`count_events`] enforce, so a full-equality assertion
/// against a found event also proves the event came from the expected
/// contract.
pub fn expected_event(
    env: &Env,
    contract_id: &Address,
    topics: Vec<Val>,
    data: Val,
) -> ContractEvent {
    let contract_id = match soroban_sdk::xdr::ScAddress::from(contract_id) {
        soroban_sdk::xdr::ScAddress::Contract(hash) => hash,
        other => panic!("expected a contract address, got {other:?}"),
    };
    ContractEvent {
        ext: xdr::ExtensionPoint::V0,
        type_: xdr::ContractEventType::Contract,
        contract_id: Some(contract_id),
        body: ContractEventBody::V0(xdr::ContractEventV0 {
            topics: topics.into(),
            data: xdr::ScVal::try_from_val(env, &data).expect("data converts to XDR"),
        }),
    }
}

/// Builds `topics` as `Vec<Val>` from a leading symbol plus `u64` suffixes.
///
/// Most Williams lifecycle events use exactly this shape
/// (`("created", will_id)`, `("checkin", will_id)`, ...), so spelling it out
/// each time adds noise rather than intent.
pub fn topics(env: &Env, symbol: Symbol, ids: &[u64]) -> Vec<Val> {
    let mut out = Vec::new(env);
    out.push_back(symbol.into_val(env));
    for id in ids {
        out.push_back(id.into_val(env));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::Address as _;

    /// A minimal contract that publishes an event with caller-chosen topics,
    /// used to simulate a token fixture emitting colliding symbols/ids.
    mod token_fixture {
        use soroban_sdk::{contract, contractimpl, symbol_short, Address, Env};

        #[contract]
        pub struct TokenFixture;

        #[contractimpl]
        impl TokenFixture {
            pub fn emit(env: Env, id: u64) {
                env.events()
                    .publish((symbol_short!("created"), id), id);
            }

            pub fn emit_other(env: Env, id: u64) {
                env.events()
                    .publish((symbol_short!("checkin"), id), id);
            }
        }
    }

    /// A minimal contract that publishes the same symbols/ids as the fixture.
    mod will_fixture {
        use soroban_sdk::{contract, contractimpl, symbol_short, Env};

        #[contract]
        pub struct WillFixture;

        #[contractimpl]
        impl WillFixture {
            pub fn emit(env: Env, id: u64) {
                env.events()
                    .publish((symbol_short!("created"), id), id);
            }

            pub fn emit_other(env: Env, id: u64) {
                env.events()
                    .publish((symbol_short!("checkin"), id), id);
            }
        }
    }

    #[test]
    fn lookups_and_counts_are_scoped_to_emitter() {
        let env = Env::default();
        let token_id = env.register(token_fixture::TokenFixture, ());
        let will_id = env.register(will_fixture::WillFixture, ());
        let token = token_fixture::TokenFixtureClient::new(&env, &token_id);
        let will = will_fixture::WillFixtureClient::new(&env, &will_id);

        // Publish identical symbol/id events from both contracts.
        token.emit(&7);
        will.emit(&7);

        // Counts are scoped: each emitter sees exactly its own event.
        assert_eq!(count_events(&env, &will_id, symbol_short!("created")), 1);
        assert_eq!(count_events(&env, &token_id, symbol_short!("created")), 1);

        // Lookups are scoped: the found event is the will's, not the token's.
        let found = find_event(&env, &will_id, symbol_short!("created"), Some(7), "created");
        assert!(emitted_by(&found, &will_id));
        assert!(!emitted_by(&found, &token_id));

        let token_found =
            find_event(&env, &token_id, symbol_short!("created"), Some(7), "created");
        assert!(emitted_by(&token_found, &token_id));
        assert!(!emitted_by(&token_found, &will_id));

        // A symbol only the token emitted cannot satisfy a will lookup.
        token.emit_other(&7);
        assert_eq!(count_events(&env, &will_id, symbol_short!("checkin")), 0);
        assert_eq!(count_events(&env, &token_id, symbol_short!("checkin")), 1);
    }

    #[test]
    #[should_panic(expected = "checkin event not found from emitter")]
    fn missing_expected_emitter_event_fails_clearly() {
        let env = Env::default();
        let token_id = env.register(token_fixture::TokenFixture, ());
        let will_id = env.register(will_fixture::WillFixture, ());
        let token = token_fixture::TokenFixtureClient::new(&env, &token_id);

        // Only the token emits; the will has no matching event.
        token.emit_other(&7);
        find_event(&env, &will_id, symbol_short!("checkin"), Some(7), "checkin");
    }

    #[test]
    fn expected_event_records_emitter_identity() {
        let env = Env::default();
        let will_id = Address::generate(&env);
        let event = expected_event(
            &env,
            &will_id,
            topics(&env, symbol_short!("created"), &[7]),
            7u64.into_val(&env),
        );
        assert!(emitted_by(&event, &will_id));
    }
}
