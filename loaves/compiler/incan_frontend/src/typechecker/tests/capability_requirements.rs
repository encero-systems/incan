//! Types that lack a capability the program needs of them: a field that cannot satisfy its declaration's automatic
//! derives (`INCAN-T0113`, #1754), a set element or dict key type without `Eq` and `Hash` (`INCAN-T0114`, #1758), and
//! an argument that is not a task where a task is required (`INCAN-T0115`, #1772), all decided by one derive relation.

use super::*;

/// Return the diagnostics of a program the checker must refuse, or a message naming what was expected.
fn refused(source: &str, context: &str) -> Result<Vec<CompileError>, String> {
    match check_str(source) {
        Err(errors) => Ok(errors),
        Ok(()) => Err(format!("{context}: the checker accepted the program")),
    }
}

/// Return the diagnostics carrying `code`.
fn with_code<'a>(errors: &'a [CompileError], code: &str) -> Vec<&'a CompileError> {
    errors
        .iter()
        .filter(|error| error.stable_code() == Some(code))
        .collect()
}

/// Return whether any diagnostic with `code` has a message containing every fragment.
fn has_refusal(errors: &[CompileError], code: &str, fragments: &[&str]) -> bool {
    with_code(errors, code)
        .iter()
        .any(|error| fragments.iter().all(|fragment| error.message.contains(fragment)))
}

/// Require that the checker accepts `source`.
fn accepted(source: &str, context: &str) -> Result<(), String> {
    check_str(source).map_err(|errors| format!("{context}, got: {errors:?}"))
}

// ---- INCAN-T0113 (#1754): automatic Clone and Debug derives ----

/// The issue's program: a model holding a `JoinHandle[int]` is refused at the field, naming the field, its type and
/// the derives the handle lacks.
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
    if !has_refusal(
        &errors,
        "INCAN-T0113",
        &[
            "Field 'handle' of model 'Pending' has type 'JoinHandle[int]'",
            "does not support Clone and Debug",
        ],
    ) {
        return Err(format!("expected the T0113 refusal for `handle`, got: {errors:?}"));
    }
    if has_refusal(&errors, "INCAN-T0113", &["Field 'label'"]) {
        return Err(format!("the `str` field must not be refused, got: {errors:?}"));
    }
    Ok(())
}

/// A class field, an enum variant payload, a handle nested in a collection and a newtype over a handle are refused the
/// same way; the nested case names the handle inside the collection.
#[test]
fn class_enum_nested_and_newtype_handles_are_refused_issue1754() -> Result<(), String> {
    let source = r#"
import std.async
from std.async.task import JoinHandle
from std.async.channel import Receiver

type Handle = newtype JoinHandle[int]

class Worker:
    handle: JoinHandle[int]

enum Job:
    Idle
    Running(JoinHandle[str])

model Batch:
    handles: list[JoinHandle[int]]
    by_name: dict[str, Option[JoinHandle[int]]]
    wrapped: Handle
    inbox: Receiver[int]
"#;
    let errors = refused(
        source,
        "handles in a class, an enum, a collection and a newtype must be refused",
    )?;
    let expected: [&[&str]; 6] = [
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
        &[
            "Field 'wrapped' of model 'Batch' has type 'Handle'",
            "does not support Clone and Debug",
        ],
        &[
            "Field 'inbox' of model 'Batch' has type 'Receiver[int]'",
            "does not support Clone,",
        ],
    ];
    for fragments in expected {
        if !has_refusal(&errors, "INCAN-T0113", fragments) {
            return Err(format!(
                "expected a T0113 refusal containing {fragments:?}, got: {errors:?}"
            ));
        }
    }
    Ok(())
}

/// A newtype over a type without `Clone` is itself valid: it carries only the derives its underlying type supports.
/// A newtype over `str` carries `Clone`, so a model may hold it; runtime handles over shared state clone whatever they
/// hold; a handle passed as a parameter is never refused.
#[test]
fn valid_holders_of_newtypes_handles_and_parameters_are_accepted_issue1754() -> Result<(), String> {
    accepted(
        r#"
import std.async
from std.async.sync import Mutex
from std.async.task import JoinHandle, TaskJoinError

type Handle = newtype JoinHandle[int]
type Email = newtype str

model User:
    email: Email

model Shared:
    counter: Mutex[int]
    last_error: Option[TaskJoinError]

async def wait_for(handle: JoinHandle[int]) -> Result[int, TaskJoinError]:
    return await handle

def keep(handle: Handle) -> None:
    pass
"#,
        "newtypes, shared-state handles and handle parameters must be accepted",
    )
}

/// A newtype a compiled library exports may carry `Clone` only through `@rust.derive(Clone)`, which its manifest does
/// not list, so a consumer's field of that newtype is not refused; the same newtype declared without it in the
/// consumer's own module is.
#[test]
fn library_newtype_with_rust_derived_clone_is_not_refused_issue1754() -> Result<(), Box<dyn std::error::Error>> {
    let provider = parse_program(
        r#"
import std.async
from std.async.task import JoinHandle

@rust.derive(Clone)
pub type Handle = newtype JoinHandle[int]
"#,
        "handle library",
    );
    let mut checker = TypeChecker::new();
    checker
        .check_program(&provider)
        .map_err(|errors| std::io::Error::other(format!("provider typecheck failed: {errors:?}")))?;
    let exports = collect_checked_public_exports(&provider, &checker);
    let manifest = LibraryManifest::from_checked_exports("handles", "0.1.0", &exports);
    let index = LibraryManifestIndex::from_entries(HashMap::from([(
        "handles".to_string(),
        LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(manifest),
            metadata: LibraryArtifactMetadata::from_crate_root(
                "handles",
                "handles",
                synthetic_artifact_root("handles"),
            ),
        },
    )]));
    let consumer = r#"
import std.async
from std.async.task import JoinHandle
from pub::handles import Handle

type LocalHandle = newtype JoinHandle[int]

model Holder:
    handle: Handle

model LocalHolder:
    handle: LocalHandle
"#;
    let errors = match check_str_with_library_index(consumer, index) {
        Err(errors) => errors,
        Ok(()) => return Err("the local newtype without Clone must be refused".into()),
    };
    let automatic = with_code(&errors, "INCAN-T0113");
    if automatic.iter().any(|error| error.message.contains("'Holder'")) {
        return Err(format!("the library newtype's derives are unknown, not missing, got: {errors:?}").into());
    }
    if !automatic.iter().any(|error| error.message.contains("'LocalHolder'")) {
        return Err(format!("the local newtype without Clone must be refused, got: {errors:?}").into());
    }
    Ok(())
}

/// One relation answers the generic bounds too: a model satisfies `Clone` through its automatic derive, a list of a
/// shared-state handle accepts an append, and a task handle does not satisfy `Clone`.
#[test]
fn generic_bounds_and_clone_requirements_follow_the_derive_relation_issue1754() -> Result<(), String> {
    accepted(
        r#"
import std.async
from std.async.sync import Mutex

model Point:
    x: int

def duplicate[T with Clone](value: T) -> T:
    return value

def main(lock: Mutex[int]) -> None:
    point = duplicate(Point(x=1))
    mut locks: list[Mutex[int]] = []
    locks.append(lock)
    println(point.x)
"#,
        "a model must satisfy Clone and a Mutex must be appendable",
    )?;
    let errors = refused(
        r#"
import std.async
from std.async.task import JoinHandle

def duplicate[T with Clone](value: T) -> T:
    return value

def main(handle: JoinHandle[int]) -> None:
    _ = duplicate(handle)
"#,
        "a JoinHandle must not satisfy Clone",
    )?;
    if !errors
        .iter()
        .any(|error| error.message.contains("violates generic bound"))
    {
        return Err(format!("expected the Clone bound violation, got: {errors:?}"));
    }
    Ok(())
}

// ---- INCAN-T0114 (#1758): set elements and dict keys implement Eq and Hash ----

/// The issue's program: a `set[Tag]` field over an enum with no derives is refused at the element type, naming the
/// derives to add.
#[test]
fn set_of_an_underived_enum_is_refused_at_the_element_type_issue1758() -> Result<(), String> {
    let errors = refused(
        r#"
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
"#,
        "set[Tag] over an underived enum must be refused",
    )?;
    let refusals = with_code(&errors, "INCAN-T0114");
    let Some(refusal) = refusals.first() else {
        return Err(format!("expected a T0114 refusal, got: {errors:?}"));
    };
    if !refusal
        .message
        .contains("'Tag' cannot be a set element: it does not implement Eq and Hash")
        || !refusal.hints.iter().any(|hint| hint.contains("@derive(Eq, Hash)"))
    {
        return Err(format!("the refusal must name the derives to add, got: {refusal:?}"));
    }
    Ok(())
}

/// An annotated binding over a set literal is refused once, at the annotation.
#[test]
fn annotated_set_literal_is_refused_once_issue1758() -> Result<(), String> {
    let errors = refused(
        r#"
enum Tag:
    A

def main() -> None:
    tags: set[Tag] = {Tag.A}
    println(len(tags))
"#,
        "an annotated set of an underived enum must be refused",
    )?;
    let refusals = with_code(&errors, "INCAN-T0114");
    if refusals.len() != 1 {
        return Err(format!("expected exactly one T0114 refusal, got: {errors:?}"));
    }
    Ok(())
}

/// Every place a value is hashed is covered: dict keys in annotations and literals, nested tuple elements, `set(...)`,
/// dict comprehensions, the interop `HashMap`, builtins that do not hash, a type with only `__eq__`, and `Ord`
/// supplying `Eq`.
#[test]
fn every_hashed_position_refuses_a_type_without_eq_and_hash_issue1758() -> Result<(), String> {
    let errors = refused(
        r#"
enum Tag:
    A
    B

@derive(Ord)
enum Level:
    Low
    High

model Point:
    x: int

    def __eq__(self, other: Point) -> bool:
        return self.x == other.x

def by_tag(counts: dict[Tag, int]) -> int:
    return len(counts)

def pairs(items: set[tuple[Tag, int]]) -> int:
    return len(items)

def levels(items: set[Level]) -> int:
    return len(items)

def floats(items: set[float]) -> int:
    return len(items)

def nested(items: set[set[int]], table: dict[dict[str, int], int]) -> int:
    return len(items) + len(table)

def interop(table: HashMap[Tag, int]) -> int:
    return len(table)

def points(items: set[Point]) -> int:
    return len(items)

def main(tags: list[Tag]) -> None:
    labels = {Tag.A: "a"}
    unique = set(tags)
    counted = {tag: 1 for tag in tags}
    println(len(labels) + len(unique) + len(counted))
"#,
        "unhashable element and key types must be refused",
    )?;
    let expected: [&[&str]; 11] = [
        &["'Tag' cannot be a dict key: it does not implement Eq and Hash"],
        &["cannot be a set element: its 'Tag' does not implement Eq and Hash"],
        &["'Level' cannot be a set element: it does not implement Hash"],
        &["'float' cannot be a set element: it does not implement Eq and Hash"],
        &["'Set[int]' cannot be a set element: it does not implement Hash"],
        &["cannot be a dict key: it does not implement Hash"],
        &["'Point' cannot be a set element"],
        &["'Tag' cannot be a set element"],
        &["'Tag' cannot be a dict key"],
        &["'Tag' cannot be a dict key: it does not implement Eq and Hash"],
        &["'Tag' cannot be a set element: it does not implement Eq and Hash"],
    ];
    for fragments in expected {
        if !has_refusal(&errors, "INCAN-T0114", fragments) {
            return Err(format!(
                "expected a T0114 refusal containing {fragments:?}, got: {errors:?}"
            ));
        }
    }
    let point = with_code(&errors, "INCAN-T0114")
        .into_iter()
        .find(|error| error.message.contains("'Point'"))
        .ok_or("missing the Point refusal")?;
    if !point.hints.iter().any(|hint| hint.contains("defines __eq__")) {
        return Err(format!(
            "a type with __eq__ must be told to key by a field, got: {point:?}"
        ));
    }
    let tag_key_refusals = with_code(&errors, "INCAN-T0114")
        .into_iter()
        .filter(|error| error.message.contains("'Tag' cannot be a dict key"))
        .count();
    if tag_key_refusals < 3 {
        return Err(format!(
            "the annotation, the literal and the comprehension must each refuse the Tag key, got: {errors:?}"
        ));
    }
    Ok(())
}

/// A generic function whose body hashes its type parameter refuses, at the call, a type argument without `Eq` and
/// `Hash`, also through a second generic function that passes its own parameter on.
#[test]
fn generic_instantiation_with_an_unhashable_type_argument_is_refused_issue1758() -> Result<(), String> {
    let errors = refused(
        r#"
enum Tag:
    A

def unique[T](items: list[T]) -> set[T]:
    return set(items)

def relay[U](items: list[U]) -> int:
    return len(unique(items))

def main() -> None:
    println(len(unique([Tag.A])))
    println(relay([Tag.A]))
    println(len(unique([1, 2])))
"#,
        "a generic set over an underived enum must be refused at the call",
    )?;
    for callee in [
        "'unique' uses its type parameter 'T'",
        "'relay' uses its type parameter 'U'",
    ] {
        if !has_refusal(&errors, "INCAN-T0114", &[callee, "'Tag' cannot be its type argument"]) {
            return Err(format!(
                "expected a T0114 refusal containing {callee:?}, got: {errors:?}"
            ));
        }
    }
    if has_refusal(&errors, "INCAN-T0114", &["'int' cannot be its type argument"]) {
        return Err(format!("an int type argument must be accepted, got: {errors:?}"));
    }
    Ok(())
}

/// A generic method whose body hashes its type parameter refuses the same type argument at the call, declared before
/// or after the call, while a parameter already bounded by `Eq` and `Hash` is left to the bound check.
#[test]
fn generic_method_with_an_unhashable_type_argument_is_refused_issue1758() -> Result<(), String> {
    let errors = refused(
        r#"
enum Tag:
    A

def main() -> None:
    println(len(Tags().unique([Tag.A])))
    println(len(Tags().unique([1, 2])))
    println(len(Tags().bounded([Tag.A])))

class Tags:
    def unique[T](self, items: list[T]) -> set[T]:
        return set(items)

    def bounded[T with (Eq, Hash)](self, items: list[T]) -> set[T]:
        return set(items)
"#,
        "a generic method hashing an underived enum must be refused at the call",
    )?;
    let hashed = with_code(&errors, "INCAN-T0114");
    let unique_refusals = hashed
        .iter()
        .filter(|error| {
            error.message.contains("'unique' uses its type parameter 'T'")
                && error.message.contains("'Tag' cannot be its type argument")
        })
        .count();
    if unique_refusals != 1 {
        return Err(format!("expected one T0114 refusal of 'unique', got: {errors:?}"));
    }
    if hashed.iter().any(|error| error.message.contains("'bounded'")) {
        return Err(format!(
            "a declared Eq and Hash bound belongs to the bound check, got: {errors:?}"
        ));
    }
    if has_refusal(&errors, "INCAN-T0114", &["'int' cannot be its type argument"]) {
        return Err(format!("an int type argument must be accepted, got: {errors:?}"));
    }
    Ok(())
}

/// A generic function imported from a source module carries its hashed parameter to the importing module's calls.
#[test]
fn imported_generic_function_with_an_unhashable_type_argument_is_refused_issue1758() -> Result<(), String> {
    let helpers = parse_program(
        r#"
pub def unique[T](items: list[T]) -> set[T]:
    return set(items)

pub def relay[U](items: list[U]) -> int:
    return len(unique(items))
"#,
        "helpers module",
    );
    let main = parse_program(
        r#"
from helpers import unique, relay

enum Tag:
    A

def main() -> None:
    println(len(unique([Tag.A])))
    println(relay([Tag.A]))
    println(len(unique([1, 2])))
"#,
        "importing module",
    );
    let mut checker = TypeChecker::new();
    let errors = match checker.check_with_imports(&main, &[("helpers", &helpers)]) {
        Err(errors) => errors,
        Ok(()) => return Err("an imported generic set over an underived enum must be refused".to_string()),
    };
    for callee in [
        "'unique' uses its type parameter 'T'",
        "'relay' uses its type parameter 'U'",
    ] {
        if !has_refusal(&errors, "INCAN-T0114", &[callee, "'Tag' cannot be its type argument"]) {
            return Err(format!(
                "expected a T0114 refusal containing {callee:?}, got: {errors:?}"
            ));
        }
    }
    if has_refusal(&errors, "INCAN-T0114", &["'int' cannot be its type argument"]) {
        return Err(format!("an int type argument must be accepted, got: {errors:?}"));
    }
    Ok(())
}

/// Return the named type parameter's exported bound names.
fn exported_bound_names(type_params: &[TypeParamExport], name: &str) -> Vec<String> {
    type_params
        .iter()
        .filter(|param| param.name == name)
        .flat_map(|param| param.bounds.iter().map(|bound| bound.name.clone()))
        .collect()
}

/// A compiled library publishes a hashed type parameter as `Eq` and `Hash` bounds, for a function and a method, and a
/// consumer of the written `.incnlib` manifest is refused by the ordinary bound check, for a literal argument too.
#[test]
fn library_manifest_carries_inferred_hash_bounds_issue1758() -> Result<(), Box<dyn std::error::Error>> {
    let provider = parse_program(
        r#"
pub def unique[T](items: list[T]) -> set[T]:
    return set(items)

pub class Tags:
    def unique[T](self, items: list[T]) -> set[T]:
        return set(items)
"#,
        "hashing library",
    );
    let mut checker = TypeChecker::new();
    checker
        .check_program(&provider)
        .map_err(|errors| std::io::Error::other(format!("provider typecheck failed: {errors:?}")))?;
    let exports = collect_checked_public_exports(&provider, &checker);
    let manifest = LibraryManifest::from_checked_exports("hashing", "0.1.0", &exports);
    let temp = tempfile::tempdir()?;
    let manifest_path = temp.path().join("hashing.incnlib");
    manifest.write_to_path(&manifest_path)?;
    let manifest = LibraryManifest::read_from_path(&manifest_path)?;

    let function = manifest
        .exports
        .functions
        .iter()
        .find(|function| function.name == "unique")
        .ok_or("missing unique export")?;
    let method = manifest
        .exports
        .classes
        .iter()
        .find(|class| class.name == "Tags")
        .and_then(|class| class.methods.iter().find(|method| method.name == "unique"))
        .ok_or("missing Tags.unique export")?;
    for (owner, bounds) in [
        ("unique", exported_bound_names(&function.type_params, "T")),
        ("Tags.unique", exported_bound_names(&method.type_params, "T")),
    ] {
        if !(bounds.iter().any(|bound| bound == "Eq") && bounds.iter().any(|bound| bound == "Hash")) {
            return Err(format!("{owner} must export Eq and Hash bounds on T, got: {bounds:?}").into());
        }
    }

    let index = LibraryManifestIndex::from_entries(HashMap::from([(
        "hashing".to_string(),
        LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(manifest),
            metadata: LibraryArtifactMetadata::from_crate_root(
                "hashing",
                "hashing",
                synthetic_artifact_root("hashing"),
            ),
        },
    )]));
    let errors = check_str_with_library_index_err(
        r#"
from pub::hashing import unique, Tags

enum Tag:
    A

def main() -> None:
    tags: list[Tag] = [Tag.A]
    println(len(unique([Tag.A])))
    println(len(Tags().unique(tags)))
    println(len(unique([1, 2])))
"#,
        index,
        "a library consumer passing an underived enum must be refused",
    )?;
    let bound_refusals = errors
        .iter()
        .filter(|error| error.message.contains("violates generic bound") && error.message.contains("Tag"))
        .count();
    if bound_refusals < 2 {
        return Err(format!("expected the function and the method call refused, got: {errors:?}").into());
    }
    if errors.iter().any(|error| error.message.contains("'int'")) {
        return Err(format!("an int type argument must be accepted, got: {errors:?}").into());
    }
    Ok(())
}

/// Derived enums, `@rust.derive` builtins, scalars, tuples of scalars and type parameters are accepted as set elements
/// and dict keys; a generic body over an unbounded parameter is accepted as written.
#[test]
fn hashable_set_elements_and_dict_keys_are_accepted_issue1758() -> Result<(), String> {
    accepted(
        r#"
@derive(Eq, Hash)
enum Tag:
    A
    B

@rust.derive(PartialEq, Eq, Hash)
model Key:
    id: int

def unique[T](items: set[T]) -> int:
    return len(items)

def count(by_tag: dict[Tag, int], by_pair: dict[tuple[int, str], bool], names: set[str]) -> int:
    return len(by_tag) + len(by_pair) + len(names)

def main() -> None:
    tags = {Tag.A, Tag.B}
    seen: set[Key] = {Key(id=1)}
    println(len(tags) + len(seen))
    println(unique({1, 2, 3}))
"#,
        "hashable element and key types must be accepted",
    )
}

// ---- INCAN-T0115 (#1772): an argument that is not a task ----

/// The issue's program: `spawn(work)` passes the async function instead of the task `work()` creates, and is refused
/// with a remedy that rewrites only that argument.
#[test]
fn spawn_of_a_function_value_is_refused_naming_the_call_issue1772() -> Result<(), String> {
    let errors = refused(
        r#"
import std.async
from std.async.task import spawn

async def work() -> int:
    return 41

async def main() -> None:
    handle = spawn(work)
    match await handle:
        Ok(value) => println(value + 1)
        Err(error) => println(error.message())
"#,
        "spawn(work) must be refused",
    )?;
    let refusal = with_code(&errors, "INCAN-T0115")
        .into_iter()
        .find(|error| {
            error
                .message
                .contains("'spawn' needs a task to run, but 'work' is a function")
        })
        .ok_or_else(|| format!("expected the T0115 refusal, got: {errors:?}"))?;
    if !refusal
        .hints
        .iter()
        .any(|hint| hint.contains("write 'work()' in place of 'work'"))
    {
        return Err(format!(
            "the remedy must rewrite the argument, got: {:?}",
            refusal.hints
        ));
    }
    Ok(())
}

/// A deadline helper names only its task argument; a function that is not `async def` is told to become one; a value
/// of another type is refused as not a task.
#[test]
fn non_task_arguments_name_the_right_remedy_issue1772() -> Result<(), String> {
    let errors = refused(
        r#"
import std.async
from std.async.task import spawn
from std.async.time import timeout

async def slow() -> int:
    return 1

def compute() -> int:
    return 2

async def main() -> None:
    _ = await timeout(5.0, slow)
    _ = spawn(compute)
    _ = spawn(42)
"#,
        "non-task arguments must be refused",
    )?;
    let refusals = with_code(&errors, "INCAN-T0115");
    let checks: [(&str, &str); 3] = [
        (
            "'timeout' needs a task to run, but 'slow' is a function",
            "write 'slow()' in place of 'slow'",
        ),
        (
            "'compute' is a function that is not `async def`",
            "Declare 'compute' with `async def`",
        ),
        (
            "this argument has type 'int', which is not a task",
            "directly as the argument, such as 'spawn(work())'",
        ),
    ];
    for (message, hint) in checks {
        let found = refusals.iter().any(|error| {
            error.message.contains(message) && error.hints.iter().any(|candidate| candidate.contains(hint))
        });
        if !found {
            return Err(format!(
                "expected a T0115 refusal {message:?} with {hint:?}, got: {errors:?}"
            ));
        }
    }
    Ok(())
}

/// The task an `async def` call creates, and a task handle, are what `spawn` and `timeout` take.
#[test]
fn tasks_and_handles_are_accepted_issue1772() -> Result<(), String> {
    accepted(
        r#"
import std.async
from std.async.task import spawn, JoinHandle
from std.async.time import timeout

async def work() -> int:
    return 41

def launch() -> JoinHandle[int]:
    return spawn(work())

async def bounded() -> None:
    _ = await timeout(1.0, work())
"#,
        "spawn(work()) and timeout(1.0, work()) must be accepted",
    )
}

/// A function of the program's own with a `RuntimeFuture[T]` bound refuses a function value the same way, through the
/// generic call path, and reports it once.
#[test]
fn runtime_future_bound_refuses_a_function_value_once_issue1772() -> Result<(), String> {
    let errors = refused(
        r#"
import std.async
from std.rust import RuntimeFuture

def run_task[T, F with RuntimeFuture[T]](task: F) -> None:
    pass

async def work() -> int:
    return 1

def main() -> None:
    run_task(work)
"#,
        "a function value must not satisfy RuntimeFuture[T]",
    )?;
    let refusals = with_code(&errors, "INCAN-T0115")
        .into_iter()
        .filter(|error| error.message.contains("'run_task' needs a task to run, but 'work'"))
        .count();
    if refusals != 1
        || errors
            .iter()
            .any(|error| error.message.contains("violates generic bound"))
    {
        return Err(format!(
            "expected exactly one T0115 refusal and no bound violation, got: {errors:?}"
        ));
    }
    Ok(())
}

/// The bound relation itself: a function value never satisfies `RuntimeFuture[T]`, under a plain or a Rust-path
/// spelling, while any other type is still admitted and left to the argument check.
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
