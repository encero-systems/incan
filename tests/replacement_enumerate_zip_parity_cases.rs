//! Native comparison cases for canonical list enumeration and shortest-side Zip polling.

use incan::backend::selection::{BackendKind, FallbackOutcome};
use incan::backend::shadow::{FunctionResultKind, ShadowComparisonProfile, SourceObservable, TypedFunctionResult};
use incan::cli::commands::compare_source_observable;

#[path = "support/shadow_capability.rs"]
mod shadow_capability;

/// Require independent receipt-backed native and direct observations of the same source and exact streams.
fn assert_matched_case(source: &str, result: &str, stdout: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(reason) = shadow_capability::unstaged_legacy_route_reason()? {
        eprintln!("skipping: {reason}");
        return Ok(());
    }
    let workspace = tempfile::tempdir()?;
    let capability = shadow_capability::legacy_capability()?;
    let profile = ShadowComparisonProfile::new(source, "observe", vec![]);
    let comparison = compare_source_observable(&profile, &capability, workspace.path());
    assert!(comparison.matched(), "{:?}", comparison.state);
    let legacy = comparison.legacy.as_ref().ok_or("matched case needs native evidence")?;
    let direct = comparison
        .replacement
        .as_ref()
        .ok_or("matched case needs direct evidence")?;
    let expected = SourceObservable::Completed {
        result: TypedFunctionResult {
            kind: FunctionResultKind::Int,
            value: result.to_string(),
        },
    };
    for (route, backend) in [(legacy, BackendKind::Legacy), (direct, BackendKind::Replacement)] {
        assert_eq!(route.observation.observable, expected);
        assert_eq!(route.observation.stdout, stdout);
        assert!(route.observation.stderr.is_empty());
        let receipt = route.receipt()?;
        receipt.verify_identity()?;
        assert_eq!(receipt.executed_backend, backend);
        assert_eq!(receipt.fallback_outcome, FallbackOutcome::NotNeeded);
        assert_eq!(receipt.selection.source_identity, comparison.source_identity);
        assert_eq!(receipt.shadow_comparison, comparison.state);
    }
    assert_ne!(legacy.receipt()?.identity, direct.receipt()?.identity);
    let authority = comparison
        .legacy_authority
        .as_ref()
        .ok_or("native observation needs its Oven authority")?;
    assert!(!authority.cargo_process_started);
    assert!(authority.direct_rustc_plan_identity.starts_with("sha256:"));
    Ok(())
}

/// Both shorter-side directions and typed-empty lists must agree, not only one nonempty Zip pair.
#[test]
fn zip_equal_unequal_and_empty_lists_match_native() -> Result<(), Box<dyn std::error::Error>> {
    assert_matched_case(
        r#"
def observe() -> int:
  mut total = 0
  for equal_left, equal_right in zip([1, 2], [10, 20]):
    println(equal_left + equal_right)
    total += equal_left + equal_right
  for short_left, long_right in zip([3], [30, 40]):
    println(short_left + long_right)
    total += short_left + long_right
  for long_left, short_right in zip([4, 5], [40]):
    println(long_left + short_right)
    total += long_left + short_right
  empty: list[int] = []
  for empty_left, unused_right in zip(empty, [100]):
    total += empty_left + unused_right
  for unused_left, empty_right in zip([100], empty):
    total += unused_left + empty_right
  for both_empty_left, both_empty_right in zip(empty, empty):
    total += both_empty_left + both_empty_right
  return total
"#,
        "110",
        b"11\n22\n33\n44\n",
    )
}

/// Written sibling-call effects, list reuse, and bare canonical iterator aliases have the same native behavior.
#[test]
fn list_argument_order_and_iterator_aliases_match_native() -> Result<(), Box<dyn std::error::Error>> {
    assert_matched_case(
        r#"
def left_values() -> list[int]:
  println("left")
  return [4, 5]

def right_values() -> list[int]:
  println("right")
  return [10, 20]

def observe() -> int:
  mut total = 0
  values = left_values()
  for prior_value in values:
    total += prior_value
  enumerated = enumerate(values)
  enumerated_alias = enumerated
  for enum_index, enum_value in enumerated_alias:
    total += enum_index + enum_value
  paired = zip(left_values(), right_values())
  paired_alias = paired
  for pair_left, pair_right in paired_alias:
    println("pair")
    total += pair_left + pair_right
  return total
"#,
        "58",
        b"left\nleft\nright\npair\npair\n",
    )
}

/// Nested structural payloads and single list/tuple projections retain the same ordered pairs.
#[test]
fn projected_nested_structural_lists_match_native() -> Result<(), Box<dyn std::error::Error>> {
    assert_matched_case(
        r#"
def observe() -> int:
  groups = [[(1, "one"), (2, "two")]]
  labels = ([true, false], 0)
  mut total = 0
  for numbered_pair, flag in zip(groups[0], labels.0):
    println(numbered_pair.1)
    total += numbered_pair.0
    if flag:
      total += 10
  return total
"#,
        "13",
        b"one\ntwo\n",
    )
}

/// Source-accepted alias assignment preserves both independently consumed values, in either use order.
#[test]
fn zip_alias_and_original_each_match_native() -> Result<(), Box<dyn std::error::Error>> {
    for (first, second) in [("pairs", "alias"), ("alias", "pairs")] {
        let source = format!(
            r#"def observe() -> int:
    pairs = zip([1], [2])
    alias = pairs
    mut total = 0
    for left, right in {first}:
        println(left + right)
        total += left + right
    for other_left, other_right in {second}:
        println(other_left + other_right)
        total += other_left + other_right
    return total
"#
        );
        assert_matched_case(&source, "6", b"3\n3\n")?;
    }
    Ok(())
}
