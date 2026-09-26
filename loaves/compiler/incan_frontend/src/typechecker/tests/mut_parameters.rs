//! `mut` parameters (#1773): a `mut` parameter is a mutable binding inside its function, method or trait default; a
//! caller-visible one (any type but `int`, `float`, `bool` or a Rust type) is changed in place, never rebound, and a
//! call passes a mutable place for it when the callee changes it (`INCAN-T0117`).

use super::*;

const MUT_ARGUMENT_CODE: &str = "INCAN-T0117";

/// Check a program and return its errors, or an empty list when it checks.
fn check_errors(source: &str) -> Vec<CompileError> {
    check_str(source).err().unwrap_or_default()
}

/// Return the `INCAN-T0117` refusals among a program's errors.
fn mut_argument_refusals(source: &str) -> Vec<CompileError> {
    check_errors(source)
        .into_iter()
        .filter(|error| error.stable_code() == Some(MUT_ARGUMENT_CODE))
        .collect()
}

/// Check a program that must be accepted, returning the program and its type facts.
fn checked(source: &str) -> Result<(crate::ast::Program, TypeCheckInfo), String> {
    let program = parse_program(source, "mut parameter program");
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("expected the program to check, got {errors:?}\n{source}"))?;
    Ok((program, checker.type_info().clone()))
}

/// Return the span of the `occurrence`-th (0-based) appearance of `needle` in `source`.
fn span_of(source: &str, needle: &str, occurrence: usize) -> Result<Span, String> {
    let start = source
        .match_indices(needle)
        .nth(occurrence)
        .map(|(index, _)| index)
        .ok_or_else(|| format!("`{needle}` #{occurrence} is not in the program"))?;
    Ok(Span::new(start, start + needle.len()))
}

/// #1773: a `mut` scalar parameter is the callee's own copy, reassignable in a function, an inherent method and a
/// trait default; a caller-visible `mut` parameter is changed in place, and rebinding it is refused; a parameter
/// without `mut` stays immutable.
#[test]
fn mut_parameter_bindings_in_their_bodies_issue1773() -> Result<(), String> {
    checked(
        r#"
type Count = int

def bumped(mut n: int, mut c: Count) -> int:
    n += 1
    c = c + n
    return c

class Counter:
    step: int

    def advanced(self, mut n: int) -> int:
        n += self.step
        return n

trait Stepper:
    def stepped(self, mut n: int) -> int:
        n += 1
        return n

    def extended(self, mut items: list[int]) -> int:
        items.append(1)
        items[0] = 2
        return len(items)

model Walker with Stepper:
    id: int

def main() -> None:
    println(bumped(1, 2))
    println(Counter(step=2).advanced(1))
    println(Walker(id=1).stepped(1))
    mut items: list[int] = []
    println(Walker(id=1).extended(items))
"#,
    )?;

    let rebinding = check_errors(
        r#"
def reset(mut items: list[int]) -> None:
    items = [0]

def relabeled(mut label: str) -> str:
    label += "!"
    return label

trait Resetter:
    def cleared(self, mut items: list[int]) -> int:
        items = []
        return 0
"#,
    );
    let refused = rebinding
        .iter()
        .filter(|error| error.message.contains("Cannot rebind the 'mut' parameter"))
        .count();
    assert_eq!(
        refused, 3,
        "each rebinding of a caller-visible parameter is refused, got {rebinding:?}"
    );

    let frozen = check_errors("def frozen(n: int) -> int:\n    n += 1\n    return n\n");
    assert!(
        frozen.iter().any(|error| error.message.contains("Cannot mutate 'n'")),
        "a parameter without `mut` stays immutable, got {frozen:?}"
    );
    Ok(())
}

/// #1773: when the callee changes a caller-visible parameter, an immutable binding, a field of one, a list element and
/// a static are refused, for a function, a method and a trait default, positionally, by name, through a function value,
/// and through a callee that passes the parameter on to one that changes it.
#[test]
fn argument_for_a_changed_mut_parameter_must_be_a_mutable_binding_issue1773() -> Result<(), String> {
    let refusals = mut_argument_refusals(
        r#"
static LOG: list[int] = []

model Basket:
    items: list[int]

def extend(mut items: list[int]) -> None:
    items.append(9)

def forward(mut items: list[int]) -> None:
    extend(items)

class Store:
    def fill(self, mut items: list[int]) -> None:
        items.append(1)

trait Replacer:
    def replace(self, mut items: list[int]) -> int:
        items.append(9)
        return len(items)

model Widget with Replacer:
    id: int

def main() -> None:
    items: list[int] = [1, 2]
    basket = Basket(items=[4])
    mut rows: list[list[int]] = [[1]]
    extend(items)
    extend(basket.items)
    extend(rows[0])
    extend(LOG)
    extend(items=items)
    forward(items)
    f = extend
    f(items)
    Store().fill(items)
    println(Widget(id=1).replace(items))
"#,
    );
    let callees = refusals
        .iter()
        .map(|refusal| {
            refusal
                .message
                .split("' of '")
                .nth(1)
                .map(|rest| rest.trim_end_matches("' must be a mutable binding").to_string())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        callees,
        [
            "extend", "extend", "extend", "extend", "extend", "forward", "extend", "fill", "replace"
        ],
        "one refusal per argument that cannot receive the change, got {refusals:?}"
    );
    assert!(
        refusals[0]
            .hints
            .iter()
            .any(|hint| hint.contains("Declare 'items' with 'mut'")),
        "an immutable binding's refusal says to declare it `mut`, got {:?}",
        refusals[0].hints
    );
    assert!(
        refusals[2].hints.iter().any(|hint| hint.contains("store it back")),
        "an element's refusal says to pass a `mut` variable and store it back, got {:?}",
        refusals[2].hints
    );
    Ok(())
}

/// #1773: temporaries, mutable places, scalar and scalar-alias parameters, and any argument for a parameter the callee
/// never changes are accepted; an immutable binding for an unchanged parameter is handed over as a copy.
#[test]
fn arguments_the_callee_can_receive_are_accepted_issue1773() -> Result<(), String> {
    let source = r#"
type Count = int

def extend(mut items: list[int]) -> None:
    items.append(9)

def total(mut items: list[int]) -> int:
    return len(items)

def touch[T](mut value: T) -> None:
    pass

def bump(mut n: Count) -> Count:
    n += 1
    return n

def fresh() -> list[int]:
    return [3]

trait Reader:
    def measured(self, mut items: list[int]) -> int:
        return len(items)

model Probe with Reader:
    id: int

class Store:
    pub items: list[int]

    def refill(mut self) -> None:
        extend(self.items)

def main() -> None:
    mut items: list[int] = [1]
    fixed: list[int] = [1, 2]
    c: Count = 1
    extend(items)
    extend([3])
    extend(fresh())
    println(total(fixed))
    touch(3)
    println(bump(c))
    println(Probe(id=1).measured(fixed))
    mut store = Store(items=[])
    store.refill()
    extend(store.items)
"#;
    let (_, info) = checked(source)?;
    for occurrence in [1, 2] {
        let span = span_of(source, "fixed)", occurrence - 1)?;
        let argument = Span::new(span.start, span.start + "fixed".len());
        assert!(
            info.mut_argument_is_copied(argument),
            "an immutable binding passed to an unchanged `mut` parameter is copied (occurrence {occurrence})"
        );
    }
    let changed_argument = span_of(source, "extend(items)", 0)?;
    assert!(
        !info.mut_argument_is_copied(Span::new(changed_argument.start + 7, changed_argument.end - 1)),
        "a mutable binding passed to a changed parameter is passed as itself"
    );
    Ok(())
}

/// #1773: the checker publishes each `mut` parameter's caller visibility for lowering, by resolved type, so an alias
/// of `int` is the callee's own copy like `int` itself.
#[test]
fn mut_parameter_caller_visibility_follows_the_resolved_type_issue1773() -> Result<(), String> {
    let source = "type Count = int\n\ndef touch(mut items: list[int], mut n: int, mut c: Count, mut label: str) -> None:\n    pass\n";
    let (program, info) = checked(source)?;
    let Some(crate::ast::Declaration::Function(function)) = program
        .declarations
        .iter()
        .map(|declaration| &declaration.node)
        .find(|declaration| matches!(declaration, crate::ast::Declaration::Function(_)))
    else {
        return Err("missing `touch`".to_string());
    };
    let visibility = function
        .params
        .iter()
        .map(|param| info.declarations.mut_param_marker(param.span, &param.node.name))
        .collect::<Vec<_>>();
    assert_eq!(visibility, [Some(true), Some(false), Some(false), Some(true)]);
    Ok(())
}

/// #1773: the refusal follows a call into another source module, whose body this check does not read.
#[test]
fn immutable_argument_to_imported_mut_parameter_is_refused_issue1773() -> Result<(), String> {
    let helpers = parse_program(
        "pub def extend(mut items: list[int]) -> None:\n    items.append(9)\n",
        "helpers",
    );
    let main = parse_program(
        "from helpers import extend\n\ndef main() -> None:\n    items: list[int] = [1]\n    extend(items)\n    mut kept: list[int] = [1]\n    extend(kept)\n",
        "main",
    );
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["main".to_string()]));
    let errors = match checker.check_with_imports(&main, &[("helpers", &helpers)]) {
        Ok(()) => return Err("an immutable binding passed to an imported `mut` parameter must be refused".to_string()),
        Err(errors) => errors,
    };
    let refusals = errors
        .iter()
        .filter(|error| error.stable_code() == Some(MUT_ARGUMENT_CODE))
        .count();
    assert_eq!(refusals, 1, "only the immutable binding is refused, got {errors:?}");
    Ok(())
}

/// #1773: a method reached by trait dispatch (a generic bound, `self` in a default) may run an override that changes
/// its `mut` parameter although the trait's default only reads it, so an immutable binding passed to it, directly or
/// through a forwarding `mut` parameter, is refused; a call on a concrete receiver without an override runs the
/// default, which only reads, and accepts it.
#[test]
fn trait_dispatched_mut_parameter_counts_as_changed_issue1773() -> Result<(), String> {
    let prelude = r#"
trait Grower:
    def grow(self, mut items: list[int]) -> int:
        return len(items)

model Box with Grower:
    id: int

    def grow(self, mut items: list[int]) -> int:
        items.append(1)
        return len(items)

model Plant with Grower:
    id: int

def forward[T with Grower](g: T, mut items: list[int]) -> int:
    return g.grow(items)
"#;
    let refused = mut_argument_refusals(&format!(
        r#"{prelude}
trait Regrower with Grower:
    def regrow(self, items: list[int]) -> int:
        return self.grow(items)

def run[T with Grower](g: T, items: list[int]) -> int:
    return g.grow(items)

def main() -> None:
    fixed: list[int] = [1]
    println(forward(Box(id=1), fixed))
"#
    ));
    assert_eq!(
        refusals_by_callee(&refused),
        ["grow", "grow", "forward"],
        "the call on `self` in a default, the generic call and the forwarding call are refused, got {refused:?}"
    );
    checked(&format!(
        "{prelude}\ndef main() -> None:\n    fixed: list[int] = [1]\n    println(Plant(id=1).grow(fixed))\n    mut items: list[int] = []\n    println(forward(Box(id=1), items))\n"
    ))?;
    Ok(())
}

/// #1773: a local bound to a caller-visible `mut` parameter (`mut other = items`) holds the parameter's value, so a
/// change through the local counts as a change to the parameter and an immutable binding passed for it is refused.
#[test]
fn mut_parameter_changed_through_a_local_alias_counts_as_changed_issue1773() -> Result<(), String> {
    let prelude = r#"
def sneaky(mut items: list[int]) -> int:
    mut other = items
    other.append(3)
    return len(other)

def relabeled(mut items: list[int]) -> int:
    mut other: list[int] = []
    other = (items)
    other.append(4)
    return len(other)
"#;
    let refused = mut_argument_refusals(&format!(
        "{prelude}\ndef main() -> None:\n    fixed: list[int] = [1]\n    println(sneaky(fixed))\n    println(relabeled(fixed))\n"
    ));
    assert_eq!(
        refusals_by_callee(&refused),
        ["sneaky", "relabeled"],
        "a new `mut` binding and a reassignment of one both hold the parameter, got {refused:?}"
    );
    checked(&format!(
        "{prelude}\ndef main() -> None:\n    mut items: list[int] = [1]\n    println(sneaky(items))\n    println(relabeled(items))\n"
    ))?;
    Ok(())
}

/// #1773: a callee known only by its callable type, `(mut Counter) -> int`, is taken to change the parameter its type
/// marks, so an immutable binding passed through a callable-typed parameter is refused, naming the parameter by
/// position; a local bound to a declared function is checked as that function; a `mut` binding and a temporary are
/// accepted.
#[test]
fn immutable_argument_to_a_marked_callable_type_parameter_is_refused_issue1773() -> Result<(), String> {
    let prelude = r#"
class Counter:
    pub value: int

def grow(mut counter: Counter) -> int:
    counter.value += 1
    return counter.value
"#;
    let refused = mut_argument_refusals(&format!(
        r#"{prelude}
def apply(step: (mut Counter) -> int, counter: Counter) -> int:
    return step(counter)

def main() -> None:
    c = Counter(value=1)
    handler: (mut Counter) -> int = grow
    println(handler(c))
"#
    ));
    assert_eq!(refusals_by_callee(&refused), ["step", "grow"], "got {refused:?}");
    assert!(
        refused[0].message.contains("parameter at position 1"),
        "a callable type's parameter is named by position, got {refused:?}"
    );
    checked(&format!(
        r#"{prelude}
def apply(step: (mut Counter) -> int, mut counter: Counter) -> int:
    return step(counter)

def main() -> None:
    mut c = Counter(value=1)
    handler: (mut Counter) -> int = grow
    println(handler(c))
    println(apply(grow, c))
    println(handler(Counter(value=2)))
"#
    ))?;
    Ok(())
}

/// #1773: a compiled library's function whose manifest marks a parameter `mut` is taken to change it, so an immutable
/// binding passed to it is refused; a `mut` binding, a temporary and an argument for an unmarked `mut int` parameter
/// are accepted.
#[test]
fn immutable_argument_to_a_marked_library_parameter_is_refused_issue1773() -> Result<(), Box<dyn std::error::Error>> {
    let producer = parse_program(
        "pub class Counter:\n    pub value: int\n\npub def grow(mut counter: Counter) -> int:\n    counter.value += 1\n    return counter.value\n\npub def bump(mut n: int) -> int:\n    n += 1\n    return n\n",
        "counters producer",
    );
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["lib".to_string()]));
    checker.set_current_package_identity(Some("counters".to_string()));
    checker
        .check_program(&producer)
        .map_err(|errors| format!("producer check failed: {errors:?}"))?;
    let exports = crate::library_exports::collect_checked_public_exports(&producer, &checker);
    let json = LibraryManifest::from_checked_exports("counters", "0.1.0", &exports).to_json_string()?;
    let manifest = LibraryManifest::from_json_str(&json)?;
    let index = || {
        LibraryManifestIndex::from_entries(HashMap::from([(
            "counters".to_string(),
            LibraryManifestIndexEntry::Loaded {
                manifest: Box::new(manifest.clone()),
                metadata: LibraryArtifactMetadata::from_crate_root(
                    "counters",
                    "counters",
                    synthetic_artifact_root("issue1773_counters"),
                ),
            },
        )]))
    };
    let errors = check_str_with_library_index_err(
        "from pub::counters import Counter, grow\n\ndef main() -> None:\n    c = Counter(value=1)\n    println(grow(c))\n",
        index(),
        "an immutable binding passed to a marked library parameter must be refused",
    )?;
    let refused = errors
        .iter()
        .filter(|error| error.stable_code() == Some(MUT_ARGUMENT_CODE))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(refusals_by_callee(&refused), ["grow"], "got {errors:?}");
    check_str_with_library_index(
        "from pub::counters import Counter, grow, bump\n\ndef main() -> None:\n    mut c = Counter(value=1)\n    n = 3\n    println(grow(c) + grow(Counter(value=2)) + bump(n))\n",
        index(),
    )
    .map_err(|errors| format!("the accepted calls were refused: {errors:?}"))?;
    Ok(())
}

/// Return the callee each `INCAN-T0117` refusal names, in order.
fn refusals_by_callee(refusals: &[CompileError]) -> Vec<String> {
    refusals
        .iter()
        .map(|refusal| {
            refusal
                .message
                .split(" of '")
                .nth(1)
                .map(|rest| rest.trim_end_matches("' must be a mutable binding").to_string())
                .unwrap_or_default()
        })
        .collect()
}
