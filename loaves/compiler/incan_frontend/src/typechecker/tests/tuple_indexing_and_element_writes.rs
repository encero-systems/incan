//! Constant tuple indexes and element writes: a negated literal index counts from the end, and an element of a tuple,
//! a string or a frozen collection is not assignable by a single assignment or as a target of a tuple assignment
//! (#1561).

use super::*;

/// A negated integer literal is a constant tuple index counted from the end, also in parentheses, and it is
/// bounds-checked like a non-negative one (#1561).
#[test]
fn negative_constant_tuple_index_counts_from_the_end_issue1561() -> Result<(), String> {
    for source in [
        "def last(pair: tuple[int, str]) -> str:\n    return pair[-1]\n",
        "def last(pair: tuple[int, str]) -> str:\n    return pair[(-1)]\n",
        "def first() -> int:\n    pair = (1, \"one\")\n    return pair[-2]\n",
        "def main() -> None:\n    mut pair: tuple[int, list[int]] = (0, [])\n    pair[-1].append(1)\n",
    ] {
        check_str(source).map_err(|errors| format!("{source}: {errors:?}"))?;
    }
    let errors = check_str_err(
        "def past(pair: tuple[int, str]) -> int:\n    return pair[-3]\n",
        "an index before the first element must be refused",
    );
    let Some(error) = errors.iter().find(|error| {
        error
            .message
            .contains("Tuple index -3 is out of bounds for tuple of length 2")
    }) else {
        return Err(format!("missing out-of-bounds diagnostic, got {errors:?}"));
    };
    assert_eq!(
        crate::diagnostics::code_for_error(error, crate::diagnostics::DiagnosticPhase::Typecheck),
        "INCAN-T0001"
    );
    Ok(())
}

/// An element of a tuple is not assignable, whether the tuple's type is written (`tuple[int, int]`) or inferred from a
/// literal, and whether the write is plain, compound or a target of a tuple assignment (#1561).
#[test]
fn tuple_element_assignment_is_refused_in_both_spellings_issue1561() -> Result<(), String> {
    for source in [
        "def main() -> None:\n    pair: tuple[int, int] = (1, 0)\n    pair[0] = 5\n",
        "def main() -> None:\n    pair: tuple[int, int] = (1, 0)\n    pair[0] += 1\n",
        "def main() -> None:\n    pair = (1, 0)\n    pair[-1] = 5\n",
        "def bump(mut pair: tuple[int, int]) -> None:\n    pair[0] += 1\n",
        "def main() -> None:\n    pair: tuple[int, int] = (1, 0)\n    mut other = 0\n    pair[0], other = (3, 4)\n",
        "def main() -> None:\n    pair = (1, 0)\n    mut other = 0\n    pair[1], other = (3, 4)\n",
    ] {
        let errors = check_str_err(source, "a tuple element write must be refused");
        let Some(error) = errors.iter().find(|error| {
            error
                .message
                .contains("Cannot assign to tuple field - tuples are immutable")
        }) else {
            return Err(format!("missing tuple-element refusal for:\n{source}\ngot {errors:?}"));
        };
        assert_eq!(
            crate::diagnostics::code_for_error(error, crate::diagnostics::DiagnosticPhase::Typecheck),
            "INCAN-T0001"
        );
        assert!(
            !errors.iter().any(|error| error.message.contains("is not indexable")),
            "a tuple is indexable; only its elements are not assignable: {errors:?}"
        );
    }
    Ok(())
}

/// An element target of a tuple assignment is refused where a single assignment to that element is: a string's
/// element and a frozen list's element are not assignable in either form (#1561).
#[test]
fn tuple_assignment_element_target_follows_the_single_assignment_rule_issue1561() -> Result<(), String> {
    for (source, message) in [
        (
            "def main() -> None:\n    text = \"ab\"\n    mut other = 0\n    text[0], other = (\"x\", 1)\n",
            "Strings are immutable - cannot assign to index",
        ),
        (
            "const ITEMS: FrozenList[int] = [1, 2]\n\ndef main() -> None:\n    mut other = 0\n    ITEMS[0], other = (5, 1)\n",
            "is not indexable",
        ),
    ] {
        let errors = check_str_err(source, "an unassignable element target must be refused");
        assert!(
            errors.iter().any(|error| error.message.contains(message)),
            "missing `{message}` for:\n{source}\ngot {errors:?}"
        );
    }
    Ok(())
}
