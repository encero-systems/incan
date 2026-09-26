//! Types that lack a capability the program needs of them: a field that cannot satisfy its declaration's automatic
//! derives (#1754), a set element or dict key type without `Eq` and `Hash` (#1758), and a function value passed where
//! a task belongs (#1772).

use super::*;

/// Return the diagnostics of a program the checker must refuse, or a message naming what was expected.
fn refused(source: &str, context: &str) -> Result<Vec<CompileError>, String> {
    match check_str(source) {
        Err(errors) => Ok(errors),
        Ok(()) => Err(format!("{context}: the checker accepted the program")),
    }
}

/// Return whether any diagnostic's message contains every fragment.
fn has_message(errors: &[CompileError], fragments: &[&str]) -> bool {
    errors
        .iter()
        .any(|error| fragments.iter().all(|fragment| error.message.contains(fragment)))
}

// ---- #1754: automatic Clone and Debug derives ----

/// The issue's program: a model holding a `JoinHandle[int]` is refused at the field, naming the field, its type and
/// the derives the handle lacks, instead of failing the generated program's build.
#[test]
fn model_field_holding_a_join_handle_is_refused_issue1754() -> Result<(), String> {
    let source = r#"
import std.async
from std.async.task import spawn, JoinHandle

model Pending:
    label: str
    handle: JoinHandle[int]

async def nine() -> int:
    return 9

async def main() -> None:
    pending = Pending(label="nine", handle=spawn(nine()))
    println(pending.label)
"#;
    let errors = refused(source, "a model holding a JoinHandle must be refused")?;
    if !has_message(
        &errors,
        &[
            "Field 'handle' of model 'Pending' has type 'JoinHandle[int]'",
            "does not support Clone and Debug",
        ],
    ) {
        return Err(format!(
            "expected the automatic-derive refusal for `handle`, got: {errors:?}"
        ));
    }
    if has_message(&errors, &["Field 'label'"]) {
        return Err(format!("the `str` field must not be refused, got: {errors:?}"));
    }
    Ok(())
}

/// A class field, an enum variant payload and a handle nested in a collection are refused the same way; the nested
/// case names the handle inside the collection.
#[test]
fn class_enum_and_nested_join_handles_are_refused_issue1754() -> Result<(), String> {
    let source = r#"
import std.async
from std.async.task import JoinHandle

class Worker:
    handle: JoinHandle[int]

enum Job:
    Idle
    Running(JoinHandle[str])

model Batch:
    handles: list[JoinHandle[int]]
    by_name: dict[str, Option[JoinHandle[int]]]
"#;
    let errors = refused(source, "handles in a class, an enum and a collection must be refused")?;
    let expected: [&[&str]; 4] = [
        &["Field 'handle' of class 'Worker' has type 'JoinHandle[int]'"],
        &["A payload of variant 'Running' of enum 'Job' has type 'JoinHandle[str]'"],
        &[
            "Field 'handles' of model 'Batch'",
            "whose 'JoinHandle[int]' does not support",
        ],
        &[
            "Field 'by_name' of model 'Batch'",
            "whose 'JoinHandle[int]' does not support",
        ],
    ];
    for fragments in expected {
        if !has_message(&errors, fragments) {
            return Err(format!("expected a refusal containing {fragments:?}, got: {errors:?}"));
        }
    }
    Ok(())
}

/// Runtime handles over shared state implement both derives whatever they hold, so a model may keep them; a handle
/// passed as a parameter is not a field and is never refused.
#[test]
fn shared_state_handles_and_handle_parameters_are_not_refused_issue1754() -> Result<(), String> {
    let source = r#"
import std.async
from std.async.sync import Mutex
from std.async.task import JoinHandle, TaskJoinError

model Shared:
    counter: Mutex[int]
    last_error: Option[TaskJoinError]

async def wait_for(handle: JoinHandle[int]) -> Result[int, TaskJoinError]:
    return await handle
"#;
    check_str(source)
        .map_err(|errors| format!("a shared-state handle or a handle parameter must not be refused, got: {errors:?}"))
}

// ---- #1758: set elements and dict keys derive Eq and Hash ----

/// The issue's program: a `set[Tag]` field over an enum with no derives is refused at the element type, naming the
/// derives to add, and so is the set literal built from its variant.
#[test]
fn set_of_an_underived_enum_is_refused_at_the_element_type_issue1758() -> Result<(), String> {
    let source = r#"
from std.serde.json import Serialize


@derive(Serialize)
model Payload:
    tags: set[Tag]
    id: UserId


enum Tag:
    A
    B


type UserId = newtype int


def main() -> None:
    println(Payload(tags={Tag.A}, id=UserId(7)).to_json())
"#;
    let errors = refused(source, "set[Tag] over an underived enum must be refused")?;
    let refusals = errors
        .iter()
        .filter(|error| {
            error
                .message
                .contains("'Tag' cannot be a set element: it does not derive Eq and Hash")
        })
        .collect::<Vec<_>>();
    if refusals.len() != 2 {
        return Err(format!(
            "expected the annotation and the literal to be refused once each, got: {errors:?}"
        ));
    }
    if !refusals
        .iter()
        .all(|error| error.hints.iter().any(|hint| hint.contains("@derive(Eq, Hash)")))
    {
        return Err(format!("the remedy must name `@derive(Eq, Hash)`, got: {refusals:?}"));
    }
    Ok(())
}

/// A dict key is refused the same way, inside a tuple too, and only the missing derive is named when `Ord` already
/// supplies `Eq`; a dict value needs neither derive.
#[test]
fn dict_keys_and_nested_elements_name_the_missing_derives_issue1758() -> Result<(), String> {
    let source = r#"
enum Tag:
    A
    B

@derive(Ord)
enum Level:
    Low
    High

def count(by_tag: dict[Tag, int], by_name: dict[str, Tag]) -> int:
    return len(by_tag) + len(by_name)

def pairs(items: set[tuple[Tag, int]]) -> int:
    return len(items)

def levels(items: set[Level]) -> int:
    return len(items)

def main() -> None:
    labels = {Tag.A: "a"}
    println(len(labels))
"#;
    let errors = refused(source, "unhashable dict keys and set elements must be refused")?;
    let expected: [&[&str]; 4] = [
        &["'Tag' cannot be a dict key: it does not derive Eq and Hash"],
        &["cannot be a set element: its 'Tag' does not derive Eq and Hash"],
        &["'Level' cannot be a set element: it does not derive Hash"],
        &["'Tag' cannot be a dict key"],
    ];
    for fragments in expected {
        if !has_message(&errors, fragments) {
            return Err(format!("expected a refusal containing {fragments:?}, got: {errors:?}"));
        }
    }
    let dict_key_refusals = errors
        .iter()
        .filter(|error| error.message.contains("'Tag' cannot be a dict key"))
        .count();
    if dict_key_refusals != 2 {
        return Err(format!(
            "expected the `dict[Tag, int]` annotation and the literal key refused once each, got: {errors:?}"
        ));
    }
    Ok(())
}

/// Derived enums, scalars, tuples of scalars and type parameters are accepted as set elements and dict keys.
#[test]
fn hashable_set_elements_and_dict_keys_are_accepted_issue1758() -> Result<(), String> {
    let source = r#"
@derive(Eq, Hash)
enum Tag:
    A
    B

def unique[T](items: set[T]) -> int:
    return len(items)

def count(by_tag: dict[Tag, int], by_pair: dict[tuple[int, str], bool], names: set[str]) -> int:
    return len(by_tag) + len(by_pair) + len(names)

def main() -> None:
    tags = {Tag.A, Tag.B}
    println(len(tags))
    println(unique({1, 2, 3}))
"#;
    check_str(source).map_err(|errors| format!("hashable element and key types must be accepted, got: {errors:?}"))
}

// ---- #1772: a function value is not a task ----

/// The issue's program: `spawn(work)` passes the async function instead of the task `work()` creates, and is refused
/// with a message naming the call to make.
#[test]
fn spawn_of_a_function_value_is_refused_naming_the_call_issue1772() -> Result<(), String> {
    let source = r#"
import std.async
from std.async.task import spawn

async def work() -> int:
    return 41

async def main() -> None:
    handle = spawn(work)
    match await handle:
        Ok(value) => println(value + 1)
        Err(error) => println(error.message())
"#;
    let errors = refused(source, "spawn(work) must be refused")?;
    let refusal = errors
        .iter()
        .find(|error| {
            error
                .message
                .contains("'spawn' needs a task to run, but 'work' is a function")
        })
        .ok_or_else(|| format!("expected the function-value refusal, got: {errors:?}"))?;
    if !refusal.hints.iter().any(|hint| hint.contains("'spawn(work())'")) {
        return Err(format!(
            "the remedy must name `spawn(work())`, got: {:?}",
            refusal.hints
        ));
    }
    Ok(())
}

/// The task an `async def` call creates is what `spawn` takes.
#[test]
fn spawn_of_an_async_call_is_accepted_issue1772() -> Result<(), String> {
    let source = r#"
import std.async
from std.async.task import spawn, JoinHandle

async def work() -> int:
    return 41

def launch() -> JoinHandle[int]:
    return spawn(work())
"#;
    check_str(source).map_err(|errors| format!("spawn(work()) must be accepted, got: {errors:?}"))
}

/// A function of the program's own with a `RuntimeFuture[T]` bound refuses a function value the same way, through the
/// generic call path, and reports it once.
#[test]
fn runtime_future_bound_refuses_a_function_value_once_issue1772() -> Result<(), String> {
    let source = r#"
import std.async
from std.rust import RuntimeFuture

def run_task[T, F with RuntimeFuture[T]](task: F) -> None:
    pass

async def work() -> int:
    return 1

def main() -> None:
    run_task(work)
"#;
    let errors = refused(source, "a function value must not satisfy RuntimeFuture[T]")?;
    let refusals = errors
        .iter()
        .filter(|error| {
            error
                .message
                .contains("'run_task' needs a task to run, but 'work' is a function")
        })
        .count();
    if refusals != 1 {
        return Err(format!("expected exactly one function-value refusal, got: {errors:?}"));
    }
    if has_message(&errors, &["violates generic bound"]) {
        return Err(format!(
            "the argument must not also be reported as a bound violation, got: {errors:?}"
        ));
    }
    Ok(())
}

/// The bound relation itself: a function value never satisfies `RuntimeFuture[T]`, under a plain or a Rust-path
/// spelling, while any other type is still admitted and left to the build.
#[test]
fn runtime_future_bound_relation_refuses_only_function_values_issue1772() -> Result<(), String> {
    let checker = TypeChecker::new();
    let function_value = ResolvedType::Function(Vec::new(), Box::new(ResolvedType::Int));
    let bindings = HashMap::new();
    for name in ["RuntimeFuture", "::incan_std_async::task::RuntimeFuture"] {
        let bound = crate::symbols::TypeBoundInfo {
            name: name.to_string(),
            source_name: None,
            type_args: vec![ResolvedType::TypeVar("T".to_string())],
            module_path: None,
            implementation_type_params: Vec::new(),
        };
        if checker.type_satisfies_explicit_bound_info(&function_value, &bound, &bindings) {
            return Err(format!("a function value must not satisfy `{name}[T]`"));
        }
        if !checker.type_satisfies_explicit_bound_info(&ResolvedType::Int, &bound, &bindings) {
            return Err(format!("an async call's output type must still satisfy `{name}[T]`"));
        }
    }
    Ok(())
}
