//! Values and declarations whose type lacks a capability the program needs of it.
//!
//! Each diagnostic here refuses at check time a program that the checker used to accept and the generated program's
//! build then refused: a field whose type cannot satisfy its declaration's automatic derives (#1754), a set element or
//! dict key whose type does not derive `Eq` and `Hash` (#1758), and a function value passed where a running task is
//! required (#1772).

use crate::ast::Span;
use crate::diagnostics::CompileError;

/// The member of a nominal declaration whose type must support the declaration's automatic derives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DerivedMember<'a> {
    /// A named field of a `model` or `class`.
    Field(&'a str),
    /// A payload of an `enum` variant, named by the variant.
    VariantPayload(&'a str),
}

/// Render a list of derive names as prose: `Clone`, `Clone and Debug`, `Clone, Debug and Eq`.
fn derive_list(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [only] => (*only).to_string(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

/// Report a `model`, `class` or `enum` member whose type cannot satisfy the automatic `Clone` and `Debug` derives
/// (#1754).
///
/// A `model`, `class` or `enum` always derives `Clone` and `Debug`, and a derive holds only when every field type
/// supports it. `member_type` is the member's type as written and `holder_type` the type inside it that lacks the
/// derives (the same type for a direct field, `JoinHandle[int]` inside `list[JoinHandle[int]]`); `missing` names the
/// derives `holder_type` lacks.
pub fn member_type_lacks_automatic_derives(
    owner_kind: &str,
    owner_name: &str,
    member: DerivedMember<'_>,
    member_type: &str,
    holder_type: &str,
    missing: &[&str],
    span: Span,
) -> CompileError {
    let missing = derive_list(missing);
    let member_text = match member {
        DerivedMember::Field(field) => format!("Field '{field}' of {owner_kind} '{owner_name}'"),
        DerivedMember::VariantPayload(variant) => {
            format!("A payload of variant '{variant}' of {owner_kind} '{owner_name}'")
        }
    };
    let subject = if member_type == holder_type {
        "which".to_string()
    } else {
        format!("whose '{holder_type}'")
    };
    CompileError::type_error(
        format!(
            "{member_text} has type '{member_type}', {subject} does not support {missing}, the derives every \
             {owner_kind} carries automatically"
        ),
        span,
    )
    .with_hint(format!(
        "Keep the '{holder_type}' in a local variable or pass it as a parameter instead of storing it in '{owner_name}'"
    ))
    .with_note(
        "A model, class or enum always derives Clone and Debug, and a derive holds only when every field type supports it",
    )
}

/// The position in a hashed collection whose type must implement `Eq` and `Hash`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashedCollectionRole {
    /// An element of a `set`.
    SetElement,
    /// A key of a `dict`.
    DictKey,
}

/// Report a set element or dict key type that does not implement `Eq` and `Hash` (#1758).
///
/// A set's elements and a dict's keys are compared and hashed, and a source-declared type implements both only through
/// its derives. `member_type` is the element or key type as written and `holder_type` the type inside it that lacks
/// the derives (the same type, or `Tag` inside `tuple[Tag, int]`); `missing` names the derives to add.
pub fn collection_member_lacks_hash_derives(
    role: HashedCollectionRole,
    member_type: &str,
    holder_type: &str,
    missing: &[&str],
    span: Span,
) -> CompileError {
    let role_text = match role {
        HashedCollectionRole::SetElement => "a set element",
        HashedCollectionRole::DictKey => "a dict key",
    };
    let subject = if member_type == holder_type {
        "it".to_string()
    } else {
        format!("its '{holder_type}'")
    };
    let missing_list = derive_list(missing);
    CompileError::type_error(
        format!("'{member_type}' cannot be {role_text}: {subject} does not derive {missing_list}"),
        span,
    )
    .with_hint(format!(
        "Add the derives to the declaration of '{holder_type}': @derive({})",
        missing.join(", ")
    ))
    .with_note("A set's elements and a dict's keys are compared and hashed, so their type must derive Eq and Hash")
}

/// Report a function value passed where a running task is required, such as `spawn(work)` (#1772).
///
/// `callee` is the called function and `value` the argument as written when it is a name or a field path (`work`,
/// `self.work`). An `async def` call's result is the task the runtime polls; the function itself is not one, so the
/// remedy calls it.
pub fn function_value_is_not_a_task(callee: &str, value: Option<&str>, span: Span) -> CompileError {
    let (message, hint) = match value {
        Some(value) => (
            format!("'{callee}' needs a task to run, but '{value}' is a function, not a task"),
            format!("Call the function to create the task: '{callee}({value}())'"),
        ),
        None => (
            format!("'{callee}' needs a task to run, but this argument is a function, not a task"),
            format!("Pass the result of calling an async function, such as '{callee}(work())'"),
        ),
    };
    CompileError::type_error(message, span)
        .with_hint(hint)
        .with_note("Calling an async function creates the task; the function's name alone is the function itself")
}
