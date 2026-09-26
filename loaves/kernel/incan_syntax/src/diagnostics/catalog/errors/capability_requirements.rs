//! Values and declarations whose type lacks a capability the program needs of it.
//!
//! Each diagnostic here refuses at check time a program that the checker used to accept and the generated program's
//! build then refused: a field whose type cannot satisfy its declaration's automatic derives (`INCAN-T0113`, #1754), a
//! set element or dict key whose type does not implement `Eq` and `Hash` (`INCAN-T0114`, #1758), and an argument that
//! is not a task where a task is required (`INCAN-T0115`, #1772).

use crate::ast::Span;
use crate::diagnostics::CompileError;

/// Stable code of [`member_type_lacks_automatic_derives`].
pub const AUTOMATIC_DERIVE_FIELD_CODE: &str = "INCAN-T0113";
/// Stable code of [`collection_member_lacks_hash_derives`] and [`type_argument_lacks_hash_derives`].
pub const HASHED_COLLECTION_MEMBER_CODE: &str = "INCAN-T0114";
/// Stable code of [`argument_is_not_a_task`].
pub const TASK_ARGUMENT_CODE: &str = "INCAN-T0115";

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
    .with_stable_code(AUTOMATIC_DERIVE_FIELD_CODE)
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
    /// A key of a `dict` or an interop `HashMap`.
    DictKey,
}

/// Which remedy a set-element or dict-key refusal can offer for the type that lacks `Eq` or `Hash`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashRemedy {
    /// A declared type: add the missing derives to its declaration.
    AddDerives,
    /// A declared type that defines `__eq__`: its custom equality provides neither `Eq` nor `Hash`, and cannot be
    /// combined with `@derive(Eq)`.
    CustomEquality,
    /// A builtin such as `float` or `set[int]`, which cannot take derives.
    Builtin,
}

/// Render the remedy for the type that lacks `Eq` or `Hash`.
fn hash_remedy_hint(holder_type: &str, missing: &[&str], remedy: HashRemedy) -> String {
    match remedy {
        HashRemedy::AddDerives => format!(
            "Add the derives to the declaration of '{holder_type}': @derive({})",
            missing.join(", ")
        ),
        HashRemedy::CustomEquality => format!(
            "'{holder_type}' defines __eq__, which provides neither Eq nor Hash; key by one of its fields instead"
        ),
        HashRemedy::Builtin => {
            format!("'{holder_type}' cannot derive them; use a type that implements Eq and Hash, such as int or str")
        }
    }
}

/// Report a set element or dict key type that does not implement `Eq` and `Hash` (#1758).
///
/// A set's elements and a dict's keys are compared and hashed. `member_type` is the element or key type as written and
/// `holder_type` the type inside it that lacks the derives (the same type, or `Tag` inside `tuple[Tag, int]`);
/// `missing` names the derives it lacks, and `remedy` what can be done about it.
pub fn collection_member_lacks_hash_derives(
    role: HashedCollectionRole,
    member_type: &str,
    holder_type: &str,
    missing: &[&str],
    remedy: HashRemedy,
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
        format!("'{member_type}' cannot be {role_text}: {subject} does not implement {missing_list}"),
        span,
    )
    .with_stable_code(HASHED_COLLECTION_MEMBER_CODE)
    .with_hint(hash_remedy_hint(holder_type, missing, remedy))
    .with_note("A set's elements and a dict's keys are compared and hashed, so their type must implement Eq and Hash")
}

/// Report a generic call whose type argument lacks `Eq` or `Hash` where the callee uses that type parameter as a set
/// element or dict key (#1758).
///
/// `callee` names the called function, `type_param` the parameter its body hashes, and `argument_type` what the call
/// binds it to; `holder_type` and `missing` are as in [`collection_member_lacks_hash_derives`].
pub fn type_argument_lacks_hash_derives(
    callee: &str,
    type_param: &str,
    argument_type: &str,
    holder_type: &str,
    missing: &[&str],
    remedy: HashRemedy,
    span: Span,
) -> CompileError {
    let subject = if argument_type == holder_type {
        "it".to_string()
    } else {
        format!("its '{holder_type}'")
    };
    let missing_list = derive_list(missing);
    CompileError::type_error(
        format!(
            "'{callee}' uses its type parameter '{type_param}' as a set element or dict key, so '{argument_type}' \
             cannot be its type argument: {subject} does not implement {missing_list}"
        ),
        span,
    )
    .with_stable_code(HASHED_COLLECTION_MEMBER_CODE)
    .with_hint(hash_remedy_hint(holder_type, missing, remedy))
    .with_note("A set's elements and a dict's keys are compared and hashed, so their type must implement Eq and Hash")
}

/// What was passed where a task is required, for [`argument_is_not_a_task`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskArgument<'a> {
    /// An `async def` named without being called, as the source wrote it (`work`, `self.work`).
    AsyncFunction(&'a str),
    /// A function that is not `async def`, named as the source wrote it.
    SyncFunction(&'a str),
    /// A function value the source spelled some other way, such as a closure.
    FunctionValue,
    /// A value of a type that is neither a task nor awaitable, rendered as a type.
    Value(&'a str),
}

/// Report an argument that is not a task where a task is required, such as `spawn(work)` (#1772).
///
/// `callee` is the called function. A task is what calling an `async def` returns, a `JoinHandle[T]`, or another
/// awaitable value; the hint rewrites only the offending argument.
pub fn argument_is_not_a_task(callee: &str, argument: TaskArgument<'_>, span: Span) -> CompileError {
    let (message, hint) = match argument {
        TaskArgument::AsyncFunction(value) => (
            format!("'{callee}' needs a task to run, but '{value}' is a function, not a task"),
            format!("Call the function to create the task: write '{value}()' in place of '{value}'"),
        ),
        TaskArgument::SyncFunction(value) => (
            format!("'{callee}' needs a task to run, but '{value}' is a function that is not `async def`"),
            format!("Declare '{value}' with `async def` and write '{value}()' in place of '{value}'"),
        ),
        TaskArgument::FunctionValue => (
            format!("'{callee}' needs a task to run, but this argument is a function, not a task"),
            "Pass the result of calling an `async def` in place of the function".to_string(),
        ),
        TaskArgument::Value(ty) => (
            format!("'{callee}' needs a task to run, but this argument has type '{ty}', which is not a task"),
            "Pass the result of calling an `async def`, such as 'work()', or a JoinHandle".to_string(),
        ),
    };
    CompileError::type_error(message, span)
        .with_stable_code(TASK_ARGUMENT_CODE)
        .with_hint(hint)
        .with_note(
            "Calling an `async def` creates the task; the function itself, or a value of another type, is not one",
        )
}
