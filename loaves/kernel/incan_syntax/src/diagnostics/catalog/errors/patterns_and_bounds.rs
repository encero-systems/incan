//! Match-pattern literals, builtin list-method forms, and bounds a generic signature owes a bounded nominal.

use crate::ast::Span;
use crate::diagnostics::CompileError;

// -- Match-pattern literals ---------------------------------------------------

/// Report a literal pattern whose value can never have the type of the position it matches (#1741).
///
/// `expected` is the type of the matched position (the scrutinee, a tuple element, a variant payload or a field) and
/// `found` the literal's own type. An integer literal against a float position is this mismatch too: an integer
/// literal is an `int`, and no implicit conversion turns it into a float.
pub fn pattern_literal_type_mismatch(expected: &str, found: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("Pattern type mismatch: the literal is '{found}' but the matched value is '{expected}'"),
        span,
    )
    .with_expected_actual(expected, found)
    .with_hint(format!(
        "Write a '{expected}' literal in this position, or match it with a name or '_'"
    ))
}

/// Report a tuple or variant pattern whose sub-pattern count differs from what it matches (#1561).
///
/// A tuple pattern has one sub-pattern per element of the matched tuple, and a variant pattern one per payload value
/// of its variant. `pattern` names the pattern (`A tuple pattern`, `The pattern 'Circle'`), `part` what each
/// sub-pattern matches (`element`, `payload value`), `expected` their count and `found` the number of sub-patterns
/// written.
pub fn pattern_arity_mismatch(pattern: &str, part: &str, expected: usize, found: usize, span: Span) -> CompileError {
    let plural = |count: usize| if count == 1 { "" } else { "s" };
    CompileError::type_error(
        format!(
            "{pattern} has {found} sub-pattern{}, but the value it matches has {expected} {part}{}",
            plural(found),
            plural(expected)
        ),
        span,
    )
    .with_hint(format!(
        "Write one sub-pattern per {part}, {expected} in all; use '_' for one the arm does not need"
    ))
}

/// Report a literal that has no pattern form: a decimal or a bytes literal (#1741).
///
/// `kind` names the literal (`decimal`, `bytes`). Such a value can only be compared, so the hint names the guard that
/// compares it.
pub fn pattern_literal_not_matchable(kind: &str, span: Span) -> CompileError {
    CompileError::type_error(format!("A {kind} literal cannot be used as a match pattern"), span)
        .with_hint("Match the value with a name and compare it in a guard, such as `value if value == ...`")
}

// -- Builtin list-method forms --------------------------------------------------

/// Report a builtin list method called with an argument count that fits none of its forms (#1776).
///
/// On a list the argument count picks the form: `count()` is the iterator terminal (how many items) and
/// `count(value)` the list method (how many items equal `value`), while `index(value)` has only the list form.
/// `iterator_form` says whether the no-argument iterator form exists for `method`, so the message names exactly the
/// forms a call may take.
pub fn list_method_argument_count(method: &str, iterator_form: bool, found: usize, span: Span) -> CompileError {
    let (accepted, forms) = if iterator_form {
        (
            "no argument or one value",
            format!(
                "`items.{method}()` is the iterator terminal over the list's items and `items.{method}(value)` is the \
                 list method that takes one value"
            ),
        )
    } else {
        (
            "one value",
            format!("`items.{method}(value)` is the list method that takes one value"),
        )
    };
    CompileError::type_error(
        format!("List method '{method}' takes {accepted}, but this call passes {found} argument(s)"),
        span,
    )
    .with_note(forms)
    .with_hint(format!("Call '{method}' in one of those forms"))
}

// -- Bounds a generic signature owes a bounded nominal --------------------------

/// Report a generic declaration that instantiates a bounded model or class with a type parameter lacking the bound
/// (#1280).
///
/// `type_name` declares its parameter `type_param` with `bound`, and the enclosing declaration passes its own type
/// parameter `argument` there without declaring that bound. The hint names the declaration to write.
pub fn nominal_type_argument_missing_bound(
    type_name: &str,
    type_param: &str,
    argument: &str,
    bound: &str,
    span: Span,
) -> CompileError {
    CompileError::type_error(
        format!(
            "Type parameter '{argument}' is used as '{type_param}' of '{type_name}', which requires '{bound}', but \
             '{argument}' does not declare it"
        ),
        span,
    )
    .with_note(format!(
        "'{type_name}' declares '{type_param} with {bound}', so every '{type_name}[...]' needs a '{bound}' argument"
    ))
    .with_hint(format!("Declare the type parameter as '{argument} with {bound}'"))
}

/// Report a comprehension that must copy elements of an unbounded type parameter from a retained collection
/// (#1983).
///
/// `type_param` is the collection element parameter and `collection_type` is the checked collection spelling. The
/// source may omit `Clone` only when the checker proves that the comprehension consumes the collection and therefore
/// moves its elements.
pub fn comprehension_element_copy_missing_clone_bound(
    type_param: &str,
    collection_type: &str,
    span: Span,
) -> CompileError {
    CompileError::type_error(
        format!(
            "This comprehension keeps its '{collection_type}' source, so it must copy elements of type parameter \
             '{type_param}', which does not declare 'Clone'"
        ),
        span,
    )
    .with_note("A comprehension may move unbounded elements only when it consumes its collection source")
    .with_hint(format!("Declare the type parameter as '{type_param} with Clone'"))
}
