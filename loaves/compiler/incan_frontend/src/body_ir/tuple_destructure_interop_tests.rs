use super::IncanType;
use super::refusals::unsupported_tuple_destructure;

/// Lowering must apply the same accepted-shape rule as the typechecker to interop values (#1132).
///
/// A blanket `RustInteropPath` exemption here would leave the original defect reachable through interop: an
/// opaque Rust value would lower to a `.0`/`.1` projection and fail as raw `rustc` output.
#[test]
fn opaque_rust_interop_values_refuse_to_lower_a_tuple_destructure() {
    assert!(
        unsupported_tuple_destructure(&IncanType::RustInteropPath("String".to_string()), 2).is_some(),
        "an opaque Rust value must not lower to a tuple field projection"
    );
    assert!(
        unsupported_tuple_destructure(&IncanType::RustInteropPath("std::vec::Vec<u8>".to_string()), 2).is_some(),
        "a Rust generic that is not a tuple must not lower to a tuple field projection"
    );
    // `(String)` is a parenthesized `String`, not a one-element tuple, so a single-name destructure must not
    // lower to `.0` against it.
    assert!(
        unsupported_tuple_destructure(&IncanType::RustInteropPath("(String)".to_string()), 1).is_some(),
        "a parenthesized Rust type has no `.0` field and must refuse to lower"
    );
    // The genuine one-element spelling still lowers.
    assert!(
        unsupported_tuple_destructure(&IncanType::RustInteropPath("(String,)".to_string()), 1).is_none(),
        "`(String,)` is a real one-element tuple and must keep lowering"
    );
}

/// The readable tuple spelling the stdlib relies on must still lower, so the refusal stays narrow.
#[test]
fn readable_rust_tuple_values_still_lower_a_tuple_destructure() {
    assert!(
        unsupported_tuple_destructure(
            &IncanType::RustInteropPath("(String,incan_std_data::json::JsonValue)".to_string()),
            2
        )
        .is_none(),
        "`std.json` destructures a `rust::HashMap` item and must keep lowering"
    );
    assert!(
        unsupported_tuple_destructure(&IncanType::RustInteropPath("(String,JsonValue)".to_string()), 3).is_some(),
        "a Rust tuple of the wrong arity must still be refused"
    );
}
