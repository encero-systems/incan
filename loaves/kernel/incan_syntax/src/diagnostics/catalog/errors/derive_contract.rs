//! Declarations and uses that break the derive contract: a static method with a receiver (#1882), an operator a
//! type does not provide (#1870), a derive whose requirements the declaration does not meet or whose declaration kind
//! it does not apply to, and a dunder with the wrong signature (#1871), a dunder beside the derive that provides the
//! same operator (#1872), and `sorted()` of elements without a total order (#1881).
//!
//! Each one used to pass the checker and fail the generated program's build, or behave inconsistently at run time.
//! All of them are `INCAN-T0001`.

use crate::ast::Span;
use crate::diagnostics::CompileError;

use super::DerivedMember;

/// Render a list of names as prose: `Eq`, `Eq and Hash`, `Eq, Hash and Ord`.
fn prose_list(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [only] => (*only).to_string(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

/// Report a `@staticmethod` that declares a `self` or `mut self` receiver (#1882).
///
/// `decorator` is the decorator as written without `@` and `receiver` the receiver as written (`self`, `mut self`).
pub fn receiver_on_static_method(method: &str, decorator: &str, receiver: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("'{method}' is a @{decorator}, which is called on the type and takes no '{receiver}' receiver"),
        span,
    )
    .with_hint(format!(
        "Remove '{receiver}' from '{method}', or remove @{decorator} to make it an instance method"
    ))
}

/// Report an equality or ordering operator on a type that does not implement it (#1870).
///
/// `operand` is the operands' type and `holder` the type inside it that lacks the operator's trait (the same type, or
/// `Point` inside `list[Point]`). `derive` names the derive that provides the operator, `Eq` or `Ord`.
/// `dunder` is the method that defines the operator when `holder` is a declared type that could define it, and `None`
/// for a builtin such as `dict[str, int]`.
pub fn operator_not_provided(
    operand: &str,
    holder: &str,
    operator: &str,
    derive: &str,
    dunder: Option<&str>,
    span: Span,
) -> CompileError {
    let subject = if operand == holder {
        "it".to_string()
    } else {
        format!("its '{holder}'")
    };
    let (reason, hint) = match dunder {
        Some(dunder) => (
            format!("{subject} neither derives {derive} nor defines {dunder}"),
            format!(
                "Add @derive({derive}) to the declaration of '{holder}', or define {dunder}(self, other: Self) -> bool"
            ),
        ),
        None => (
            format!("{subject} has no '{operator}'"),
            format!("Compare values of a type that supports '{operator}'"),
        ),
    };
    CompileError::type_error(format!("'{operand}' does not support '{operator}': {reason}"), span).with_hint(hint)
}

/// Report a derive on a declaration whose member type does not implement what the derive needs of it (#1871).
///
/// `derive` is the derive as written, `member_type` the member's type as written and `holder_type` the type inside it
/// that lacks `missing` (the same type for a direct field, `float` inside `list[float]`).
#[allow(clippy::too_many_arguments)]
pub fn derive_member_lacks_derive(
    derive: &str,
    owner_kind: &str,
    owner_name: &str,
    member: DerivedMember<'_>,
    member_type: &str,
    holder_type: &str,
    missing: &[&str],
    span: Span,
) -> CompileError {
    let subject = if member_type == holder_type {
        "which".to_string()
    } else {
        format!("whose '{holder_type}'")
    };
    let missing = prose_list(missing);
    let prefix = format!("@derive({derive}) on {owner_kind} '{owner_name}' needs {missing}");
    let (message, member_text) = match member {
        DerivedMember::Underlying => (
            format!("{prefix} of its underlying type '{member_type}', {subject} does not implement {missing}"),
            "the underlying type".to_string(),
        ),
        DerivedMember::Field(field) => (
            format!(
                "{prefix} of every member, but field '{field}' has type '{member_type}', {subject} does not implement \
                 {missing}"
            ),
            format!("field '{field}'"),
        ),
        DerivedMember::VariantPayload(variant) => (
            format!(
                "{prefix} of every member, but a payload of variant '{variant}' has type '{member_type}', {subject} \
                 does not implement {missing}"
            ),
            format!("the payload of variant '{variant}'"),
        ),
    };
    CompileError::type_error(message, span).with_hint(format!(
        "Remove {derive} from @derive(...) on '{owner_name}', or give {member_text} a type that implements {missing}"
    ))
}

/// Report a derive on a declaration that does not provide another derive it builds on (#1871).
///
/// `required` names what the declaration must also provide, such as `PartialEq` for `PartialOrd`.
pub fn derive_requires_derive(
    derive: &str,
    required: &str,
    owner_kind: &str,
    owner_name: &str,
    span: Span,
) -> CompileError {
    CompileError::type_error(
        format!(
            "@derive({derive}) on {owner_kind} '{owner_name}' needs {required}, which '{owner_name}' does not provide"
        ),
        span,
    )
    .with_hint(format!("Derive both: @derive({required}, {derive})"))
}

/// Report a derive on a declaration kind it does not apply to (#1871).
///
/// `applies_to` states what the derive applies to, such as `a model` or `a model, class or newtype`.
pub fn derive_not_applicable(
    derive: &str,
    owner_kind: &str,
    owner_name: &str,
    applies_to: &str,
    span: Span,
) -> CompileError {
    CompileError::type_error(
        format!("@derive({derive}) applies to {applies_to}, and '{owner_name}' is {owner_kind}"),
        span,
    )
    .with_hint(format!("Remove {derive} from @derive(...) on '{owner_name}'"))
}

/// Report a dunder whose declaration does not have the signature its operator or builtin calls it with (#1871).
///
/// `expected` is the signature it must have, such as `(self, other: Self) -> bool`.
pub fn dunder_signature_mismatch(owner_name: &str, dunder: &str, expected: &str, span: Span) -> CompileError {
    CompileError::type_error(
        format!("'{owner_name}.{dunder}' must have the signature {expected}"),
        span,
    )
    .with_hint(format!("Declare it as 'def {dunder}{expected}'"))
}

/// Report a type that defines a dunder and derives what provides the same behavior (#1872).
///
/// `derive` is the derive as written, which may provide the dunder's behavior through what it implies (`Ord` implies
/// `Eq`).
pub fn dunder_conflicts_with_derive(
    owner_kind: &str,
    owner_name: &str,
    dunder: &str,
    derive: &str,
    span: Span,
) -> CompileError {
    CompileError::type_error(
        format!(
            "{owner_kind} '{owner_name}' defines {dunder} and derives {derive}, which both provide the same behavior"
        ),
        span,
    )
    .with_hint(format!(
        "Keep one: remove {derive} from @derive(...), or remove the {dunder} method"
    ))
}

/// Report `sorted()` of a list whose element type has no total order (#1881).
///
/// `element` is the list's element type and `holder` the type inside it that does not implement `Ord` (the same type,
/// or `Task` inside `tuple[int, Task]`); `holder_is_declared` is whether `holder` is a declared type that can derive
/// `Ord`.
pub fn sorted_element_not_ordered(element: &str, holder: &str, holder_is_declared: bool, span: Span) -> CompileError {
    let subject = if element == holder {
        format!("'{element}' does not implement Ord")
    } else {
        format!("its '{holder}' does not implement Ord")
    };
    let hint = if holder_is_declared {
        format!("Add @derive(Ord) to the declaration of '{holder}'")
    } else {
        "Sort a list whose elements have a total order, such as int, str or a type that derives Ord".to_string()
    };
    CompileError::type_error(
        format!("sorted() needs list elements with a total order, and {subject}"),
        span,
    )
    .with_hint(hint)
}
