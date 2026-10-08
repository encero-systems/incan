//! Focused evidence for the hashed set/dict representation used by replacement membership (#1247).
//!
//! Two kinds of evidence live here. Behavioral tests pin the representation's own contract: hashed probes,
//! key-not-value dict membership, typed-empty answers, refusals for everything outside the key domain, and the
//! later-entry-wins dict precedence. Agreement tests then compare those answers against the
//! `incan_stdlib::collections` membership helpers over the same data — the exact functions the Rust-emission
//! backend calls for these operators — so the representation demonstrably matches the reference backend's
//! semantics at the level this slice owns. The executor-level parity runs belong to the #1247 integration work,
//! not here.

use std::collections::{HashMap, HashSet};

use incan_stdlib::collections as reference;

use super::{HashedKey, NonScalarKey, ReplacementDict, ReplacementSet, ReplacementValue};

/// A boxed error, so every fallible test can propagate with `?` instead of unwrapping.
type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Shorthand for the owned `str` needles and elements these tests build repeatedly.
fn str_value(text: &str) -> ReplacementValue {
    ReplacementValue::Str(text.to_string())
}

#[test]
fn set_membership_is_a_hashed_probe_that_agrees_with_the_runtime_helper() -> TestResult {
    // The representation probes a `HashSet<HashedKey>`; the reference helper probes the `HashSet<i64>` the
    // Rust-emission backend would build for the same source literal. Same data, same probes, same answers — and
    // both signatures state the hashed cost model, which is what #1247 rejected the pair-list representation over.
    let set = ReplacementSet::from_elements([
        ReplacementValue::Int(1),
        ReplacementValue::Int(2),
        ReplacementValue::Int(3),
    ])?;
    let emitted: HashSet<i64> = [1, 2, 3].into_iter().collect();

    for probe in 0..=4 {
        let held = set.contains(ReplacementValue::Int(probe))?;
        assert_eq!(held, reference::set_contains(&emitted, &probe), "probe {probe}");
        assert_eq!(!held, reference::set_not_contains(&emitted, &probe), "probe {probe}");
    }
    Ok(())
}

#[test]
fn dict_membership_tests_keys_not_values() -> TestResult {
    // `k in d` asks about keys, matching `HelperOp::DictContainsKey` and the typechecker's own rule. A stored
    // value must not be found by the key probe, and the answers must agree with `dict_contains_key` over the
    // `HashMap` the emission backend would build.
    let dict = ReplacementDict::from_entries([
        (str_value("a"), ReplacementValue::Int(1)),
        (str_value("b"), ReplacementValue::Int(2)),
    ])?;
    let emitted: HashMap<String, i64> = [("a".to_string(), 1), ("b".to_string(), 2)].into_iter().collect();

    for (probe, expected) in [("a", true), ("b", true), ("c", false)] {
        assert_eq!(dict.contains_key(str_value(probe))?, expected, "probe {probe}");
        assert_eq!(
            reference::dict_contains_key(&emitted, &probe.to_string()),
            expected,
            "reference probe {probe}"
        );
        assert_eq!(
            reference::dict_not_contains_key(&emitted, &probe.to_string()),
            !expected,
            "reference negated probe {probe}"
        );
    }
    // The value `1` is stored under `"a"`, but it is not a key: key membership must answer `false`, not find it.
    assert!(!dict.contains_key(ReplacementValue::Int(1))?);
    Ok(())
}

#[test]
fn distinct_scalar_kinds_never_compare_equal() -> TestResult {
    // The list-membership arm already answers through `ReplacementValue`'s own `PartialEq`, where `Int(1)` is not
    // `Bool(true)` and not `Str("1")`. The hashed key keeps that identity rule; Incan's static typing prevents the
    // mixed-kind probe in real source, so this pins the representation, not a reachable program.
    let set = ReplacementSet::from_elements([ReplacementValue::Int(1)])?;
    assert!(set.contains(ReplacementValue::Int(1))?);
    assert!(!set.contains(ReplacementValue::Bool(true))?);
    assert!(!set.contains(str_value("1"))?);
    Ok(())
}

#[test]
fn str_keys_hash_by_content_not_construction_path() -> TestResult {
    // Eq/Hash coherence evidence: two differently-built strings with the same text are one key.
    let set = ReplacementSet::from_elements([str_value("ab")])?;
    let rebuilt = format!("{}{}", 'a', 'b');
    assert!(set.contains(ReplacementValue::Str(rebuilt))?);
    Ok(())
}

#[test]
fn unit_is_a_hashable_key() -> TestResult {
    // The unit value sits inside `is_collection_scalar`, so it is a legal set element and dict key here, the same
    // way `None` keys a dict in the language this surface mirrors.
    let set = ReplacementSet::from_elements([ReplacementValue::Unit])?;
    assert!(set.contains(ReplacementValue::Unit)?);

    let dict = ReplacementDict::from_entries([(ReplacementValue::Unit, ReplacementValue::Int(1))])?;
    assert!(dict.contains_key(ReplacementValue::Unit)?);
    Ok(())
}

#[test]
fn typed_empty_containers_answer_false_not_refusal() -> TestResult {
    // Emptiness is an answer. A typed-empty set or dict holds nothing, so a scalar probe gets `false` — refusing
    // here would make the executor refuse programs the reference backend runs fine.
    let set = ReplacementSet::empty();
    let dict = ReplacementDict::empty();
    for needle in [ReplacementValue::Int(0), str_value("a"), ReplacementValue::Unit] {
        assert!(!set.contains(needle.clone())?);
        assert!(!dict.contains_key(needle)?);
    }
    // `from_elements`/`from_entries` over nothing construct the same empty containers the dedicated constructors do.
    assert_eq!(ReplacementSet::from_elements([])?, set);
    assert_eq!(ReplacementDict::from_entries([])?, dict);
    Ok(())
}

#[test]
fn a_later_dict_entry_overwrites_an_earlier_one() -> TestResult {
    // `Rvalue::Dict` documents this precedence as a property of dict construction; the representation must deliver
    // it, and the surviving value must be the later one, observably.
    let repeated = ReplacementDict::from_entries([
        (str_value("a"), ReplacementValue::Int(1)),
        (str_value("a"), ReplacementValue::Int(2)),
    ])?;
    let direct = ReplacementDict::from_entries([(str_value("a"), ReplacementValue::Int(2))])?;
    assert_eq!(repeated, direct);
    assert_eq!(repeated.observable_text(), "{a: 2}");
    Ok(())
}

#[test]
fn construction_refuses_a_non_scalar_element_or_key() {
    // A hashed container cannot even hold what it cannot hash, so the refusal lands at construction — the same
    // site where the executor refuses the whole aggregate today — and names the offending kind.
    let list_element = ReplacementValue::List {
        elements: vec![ReplacementValue::Int(1)],
        next: 0,
    };
    assert_eq!(
        ReplacementSet::from_elements([list_element]),
        Err(NonScalarKey { kind: "list" })
    );

    let tuple_key = ReplacementValue::Tuple(vec![ReplacementValue::Int(1)]);
    assert_eq!(
        ReplacementDict::from_entries([(tuple_key, ReplacementValue::Int(1))]),
        Err(NonScalarKey { kind: "tuple" })
    );
}

#[test]
fn a_non_scalar_dict_value_is_admitted() -> TestResult {
    // Only keys are hashed. A dict may store any admitted value — the typechecker allows `Dict[str, List[int]]` —
    // and key membership works over it without consulting the values.
    let dict = ReplacementDict::from_entries([(
        str_value("xs"),
        ReplacementValue::List {
            elements: vec![ReplacementValue::Int(1)],
            next: 0,
        },
    )])?;
    assert!(dict.contains_key(str_value("xs"))?);
    Ok(())
}

#[test]
fn a_non_scalar_needle_refuses_even_over_an_empty_container() {
    // Every held element is known comparable, so the needle is the one place "could not tell" could still leak in
    // disguised as "absent". Emptiness must not shortcut that: the refusal is about the needle, not the contents.
    let needle = ReplacementValue::Tuple(vec![]);
    assert_eq!(
        ReplacementSet::empty().contains(needle.clone()),
        Err(NonScalarKey { kind: "tuple" })
    );
    assert_eq!(
        ReplacementDict::empty().contains_key(needle),
        Err(NonScalarKey { kind: "tuple" })
    );
}

#[test]
fn float_stays_outside_the_hashed_key_domain() {
    // Normalizing the ordinary Float carrier does not admit floating-point equality or hashing. It still refuses
    // as an element, a key, and a needle alike.
    let float = ReplacementValue::Float(1.5);
    assert_eq!(
        ReplacementSet::from_elements([float.clone()]),
        Err(NonScalarKey { kind: "float" })
    );
    assert_eq!(
        ReplacementDict::from_entries([(float.clone(), ReplacementValue::Int(1))]),
        Err(NonScalarKey { kind: "float" })
    );
    assert_eq!(
        ReplacementSet::empty().contains(float),
        Err(NonScalarKey { kind: "float" })
    );
}

#[test]
fn key_domain_stays_in_lockstep_with_is_collection_scalar() {
    // The executor's membership guard and this module's key domain are two spellings of one rule. If either side
    // drifts — a variant admitted here but not there, or vice versa — this matrix breaks before an executor arm
    // can disagree with its own representation.
    let samples = [
        ReplacementValue::Int(3),
        ReplacementValue::Bool(true),
        str_value("a"),
        ReplacementValue::Unit,
        ReplacementValue::Float(1.5),
        ReplacementValue::Range {
            next: 0,
            end: 3,
            step: 1,
        },
        ReplacementValue::List {
            elements: vec![],
            next: 0,
        },
        ReplacementValue::Tuple(vec![]),
        ReplacementValue::CollectedGenerator {
            elements: vec![],
            next: 0,
        },
    ];
    for sample in samples {
        assert_eq!(
            HashedKey::try_from_value(sample.clone()).is_ok(),
            sample.is_collection_scalar(),
            "domain disagreement on {sample:?}"
        );
    }
}

#[test]
fn set_equality_ignores_order_and_duplicates_collapse() -> TestResult {
    // Two sets are the same value exactly when they hold the same keys: construction order is not part of the
    // value, and a repeated literal element is one entry, as in the language.
    let ordered = ReplacementSet::from_elements([ReplacementValue::Int(1), ReplacementValue::Int(2)])?;
    let reversed = ReplacementSet::from_elements([ReplacementValue::Int(2), ReplacementValue::Int(1)])?;
    let repeated = ReplacementSet::from_elements([
        ReplacementValue::Int(1),
        ReplacementValue::Int(1),
        ReplacementValue::Int(2),
    ])?;
    assert_eq!(ordered, reversed);
    assert_eq!(ordered, repeated);
    Ok(())
}

#[test]
fn rendering_is_canonical_and_deterministic() -> TestResult {
    // Receipts digest observable output, so rendering must not depend on hash-iteration order or construction
    // order. Canonical `HashedKey` order is the determinism choice; it is documented as not being source order.
    let set = ReplacementSet::from_elements([
        ReplacementValue::Int(3),
        ReplacementValue::Int(1),
        ReplacementValue::Int(2),
    ])?;
    assert_eq!(set.observable_text(), "{1, 2, 3}");
    // `Set()` — the language's zero-argument collection constructor — is the empty set's one source spelling;
    // Python's lowercase `set()` is not an Incan spelling.
    assert_eq!(ReplacementSet::empty().observable_text(), "Set()");

    let dict = ReplacementDict::from_entries([
        (str_value("b"), ReplacementValue::Int(2)),
        (str_value("a"), ReplacementValue::Int(1)),
    ])?;
    assert_eq!(dict.observable_text(), "{a: 1, b: 2}");
    assert_eq!(ReplacementDict::empty().observable_text(), "{}");
    Ok(())
}

#[test]
fn key_rendering_matches_replacement_observable_text() -> TestResult {
    // `HashedKey::observable_text` duplicates the four scalar spellings instead of converting back into a
    // `ReplacementValue`; this pin is what keeps that duplication from drifting.
    let scalars = [
        ReplacementValue::Int(-7),
        ReplacementValue::Bool(false),
        str_value("text"),
        ReplacementValue::Unit,
    ];
    for scalar in scalars {
        let key = HashedKey::try_from_value(scalar.clone())?;
        assert_eq!(
            key.observable_text(),
            scalar.observable_text(),
            "spelling drift on {scalar:?}"
        );
    }
    Ok(())
}
