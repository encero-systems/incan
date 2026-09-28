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

/// #1773: a caller-visible `mut` parameter is used only directly. Binding it to another name (`mut`, annotated,
/// reassigned or `let`), holding it in a tuple, list or dict literal, a comprehension, a field, a construction (a
/// model, `Some`, an enum variant) or a `partial` preset, producing it as a `match`, `if`, `break` or `yield` value,
/// binding it in a `match` arm, iterating a literal that holds it, or a closure that changes or returns it is refused,
/// and the hint names the copy for its type. Returning it, reading it, iterating it, calling methods on it, passing it
/// to a call and a comprehension variable of the same name are accepted, and a scalar `mut` parameter is the function's
/// own copy, which any binding may hold.
#[test]
fn holding_a_mut_parameter_in_another_name_or_value_is_refused_issue1773() -> Result<(), String> {
    let spellings = [
        "    mut other = items\n    return len(other)\n",
        "    mut other: list[int] = items\n    return len(other)\n",
        "    mut other: list[int] = []\n    other = items\n    return len(other)\n",
        "    let other = items\n    return len(other)\n",
        "    other = match len(items):\n        0 => items\n        _ => []\n    return len(other)\n",
        "    other = if len(items) > 0:\n        items\n    else:\n        []\n    return len(other)\n",
        "    other = loop:\n        break items\n    return len(other)\n",
        "    match items:\n        xs => xs.append(3)\n    return len(items)\n",
        "    for xs in [items]:\n        xs.append(3)\n    return len(items)\n",
        "    pair = (items, 1)\n    return pair[1]\n",
        "    table = {\"k\": items}\n    return len(table)\n",
        "    mut holder = Holder(items=[])\n    holder.items = items\n    return len(holder.items)\n",
        "    add = () => items.append(1)\n    add()\n    return len(items)\n",
        "    get = () => items\n    return len(get())\n",
        "    return apply(() => items)\n",
        "    p = partial extend(items=items)\n    p()\n    return len(items)\n",
        "    mut holder = Holder(items=items)\n    return len(holder.items)\n",
        "    held = Some(items)\n    return len(held.unwrap())\n",
        "    wrapped = Wrap.Held(items)\n    return len(items)\n",
        "    rows: list[list[int]] = [[1]]\n    firsts = [items for _ in rows]\n    return len(firsts)\n",
    ];
    let prelude = "model Holder:\n    items: list[int]\n\nenum Wrap:\n    Held(list[int])\n\ndef apply(f: () -> list[int]) -> int:\n    return len(f())\n\ndef extend(mut items: list[int]) -> None:\n    items.append(3)\n\n";
    for body in spellings {
        let source = format!("{prelude}def f(mut items: list[int]) -> int:\n{body}");
        let errors = check_errors(&source);
        let held = errors
            .iter()
            .filter(|error| {
                error.message == "The 'mut' parameter 'items' cannot be bound to another name or held in another value"
            })
            .collect::<Vec<_>>();
        assert_eq!(held.len(), 1, "one refusal for\n{body}got {errors:?}");
        assert!(
            held[0].hints.iter().any(|hint| hint.contains("write list(items)")),
            "the hint names the list copy, got {:?}",
            held[0].hints
        );
    }
    let yielded = check_errors("def gen(mut items: list[int]) -> Generator[list[int]]:\n    yield items\n");
    assert!(
        yielded
            .iter()
            .any(|error| error.message.starts_with("The 'mut' parameter 'items' cannot be bound")),
        "a yielded parameter is held by the generator's consumer, got {yielded:?}"
    );
    for (ty, route) in [
        ("dict[str, int]", "build a new value from 'value'"),
        ("set[int]", "build a new value from 'value'"),
        ("str", "write str(value)"),
    ] {
        let errors = check_errors(&format!(
            "def f(mut value: {ty}) -> int:\n    other = value\n    return 1\n"
        ));
        assert!(
            errors
                .iter()
                .any(|error| error.hints.iter().any(|hint| hint.contains(route))),
            "the hint for `{ty}` offers `{route}`, got {errors:?}"
        );
    }
    let model =
        check_errors("model Box:\n    n: int\n\ndef f(mut box: Box) -> int:\n    other = box\n    return other.n\n");
    assert!(
        model.iter().any(|error| error
            .hints
            .iter()
            .any(|hint| hint.contains("build a new value from 'box'"))),
        "a model has no single copy expression, got {model:?}"
    );
    checked(
        r#"
def reads(mut items: list[int], mut n: int) -> list[int]:
    for x in items:
        n += x
    total = len(items)
    m = n
    items.append(total + m)
    extend(items)
    rows: list[list[int]] = [[1]]
    firsts = [items for items in rows]
    mut stored: list[list[int]] = []
    stored.append(items)
    println(len(firsts) + len(stored))
    return items

def extend(mut items: list[int]) -> None:
    items.append(1)
"#,
    )?;
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

/// #1773: a call through a local bound to a function is checked as that function only while the local is never
/// reassigned; once it is, anywhere in the module, the call may run any function assigned to it and counts as changing
/// each marked parameter, and the refusal says the callee may change it.
#[test]
fn call_through_a_reassigned_function_local_counts_as_changing_issue1773() -> Result<(), String> {
    let prelude = r#"
def reads(mut items: list[int]) -> int:
    return len(items)

def extend(mut items: list[int]) -> int:
    items.append(9)
    return len(items)
"#;
    let refused = mut_argument_refusals(&format!(
        r#"{prelude}
def main() -> None:
    fixed: list[int] = [1]
    mut f = reads
    println(f(fixed))
    f = extend
    println(f(fixed))
"#
    ));
    assert_eq!(
        refusals_by_callee(&refused),
        ["reads", "reads"],
        "both calls through the reassigned local are refused, the one before the reassignment too, got {refused:?}"
    );
    assert!(
        refused
            .iter()
            .all(|refusal| refusal.notes.iter().any(|note| note.starts_with("'reads' may change"))),
        "the callee is not known at the call, got {refused:?}"
    );
    let (_, info) = checked(&format!(
        "{prelude}\ndef main() -> None:\n    fixed: list[int] = [1]\n    g = reads\n    println(g(fixed))\n"
    ))?;
    assert_eq!(
        info.calls.mut_argument_copies.len(),
        1,
        "a local that is never reassigned runs the function it was bound to, which only reads"
    );
    Ok(())
}

/// #1773: the variable of a `for` loop over a caller-visible list parameter, a field of it, or the variable of an
/// enclosing such loop is a view into the parameter's elements, even when it reuses the iterated name, so a change
/// through it is a change to the parameter and an immutable binding passed for it is refused. A loop over a copy (a
/// call such as `list(items)` or `enumerate(items)`, a method such as `items.clone()`, an element such as `items[0]`)
/// changes only the copy. A loop variable passed to a parameter the callee changes is refused, with a hint that can be
/// followed (no index recipe for a destructured one), and one passed to a parameter the callee never changes is copied.
#[test]
fn change_through_a_loop_variable_counts_as_changing_issue1773() -> Result<(), String> {
    let changes = r#"
def grow(mut items: list[list[int]]) -> int:
    for row in items:
        row.append(3)
    return len(items[0])

def reuse(mut items: list[list[int]]) -> int:
    for items in items:
        items.append(3)
    return 1

def nested(mut items: list[list[list[int]]]) -> int:
    for row in items:
        for row in row:
            row.append(3)
    return 1
"#;
    let refused = mut_argument_refusals(&format!(
        "{changes}\ndef main() -> None:\n    live: list[list[int]] = [[1]]\n    deep: list[list[list[int]]] = [[[1]]]\n    println(grow(live))\n    println(reuse(live))\n    println(nested(deep))\n"
    ));
    assert_eq!(
        refusals_by_callee(&refused),
        ["grow", "reuse", "nested"],
        "got {refused:?}"
    );
    checked(&format!(
        "{changes}\ndef main() -> None:\n    mut live: list[list[int]] = [[1]]\n    mut deep: list[list[list[int]]] = [[[1]]]\n    println(grow(live))\n    println(reuse(live))\n    println(nested(deep))\n"
    ))?;

    let copies = r#"
def fresh(n: int) -> list[list[int]]:
    return [[n]]

def copied(mut items: list[list[int]]) -> int:
    for row in list(items):
        row.append(1)
    for row in items.clone():
        row.append(2)
    for row in fresh(len(items)):
        row.append(3)
    for i, row in enumerate(items):
        row.append(i)
    for cell in items[0]:
        println(cell)
    return len(items)

def main() -> None:
    live: list[list[int]] = [[1]]
    println(copied(live))
"#;
    let (_, info) = checked(copies)?;
    assert_eq!(
        info.calls.mut_argument_copies.len(),
        1,
        "loops over copies leave the parameter unchanged, so the immutable binding is passed as a copy"
    );

    let passed = mut_argument_refusals(
        "def bump(mut row: list[int]) -> None:\n    row.append(1)\n\ndef each(mut items: list[list[int]]) -> None:\n    for row in items:\n        bump(row)\n",
    );
    assert_eq!(refusals_by_callee(&passed), ["bump"], "got {passed:?}");
    assert!(
        passed[0]
            .hints
            .iter()
            .any(|hint| hint.contains("loop over the indexes")),
        "a loop variable cannot be declared `mut`, so the hint says what can be done, got {:?}",
        passed[0].hints
    );
    let destructured = mut_argument_refusals(
        "def bump(mut row: list[int]) -> None:\n    row.append(1)\n\ndef each(mut pairs: list[tuple[list[int], int]]) -> None:\n    for xs, n in pairs:\n        bump(xs)\n",
    );
    assert_eq!(refusals_by_callee(&destructured), ["bump"], "got {destructured:?}");
    assert!(
        destructured[0]
            .hints
            .iter()
            .all(|hint| !hint.contains("...[i]") && hint.contains("cannot be passed")),
        "a destructured loop variable gets no index recipe, got {:?}",
        destructured[0].hints
    );
    let (_, unchanged) = checked(
        "def show(mut row: list[int]) -> int:\n    return len(row)\n\ndef main() -> None:\n    rows: list[list[int]] = [[1], [2, 3]]\n    mut t = 0\n    for row in rows:\n        t += show(row)\n    println(t)\n",
    )?;
    assert_eq!(
        unchanged.calls.mut_argument_copies.len(),
        1,
        "a loop variable passed to a parameter the callee never changes is handed over as a copy"
    );
    Ok(())
}

/// #1561: any use of a `Generator` parameter advances it (iterating it, collecting it, iterating it in a comprehension,
/// passing it on to a parameter that iterates it), so an immutable binding passed for it is refused. One whose callee
/// never touches it is refused too: an unchanged parameter receives a copy of an immutable argument, and a generator
/// cannot be copied. A `mut` binding is accepted for each.
#[test]
fn generator_parameter_use_counts_as_changing_issue1561() -> Result<(), String> {
    let callees = r#"
def numbers(limit: int) -> Generator[int]:
    for value in range(limit):
        yield value

def first(mut g: Generator[int]) -> int:
    for v in g:
        return v
    return -1

def drained(mut g: Generator[int]) -> list[int]:
    return list(g)

def doubled(mut g: Generator[int]) -> list[int]:
    return [v * 2 for v in g]

def outer(mut g: Generator[int]) -> int:
    return first(g)

def ignore(mut g: Generator[int]) -> int:
    return 0
"#;
    let calls = "    println(first(g))\n    println(drained(g))\n    println(doubled(g))\n    println(outer(g))\n    println(ignore(g))\n";
    let refused = mut_argument_refusals(&format!("{callees}\ndef main() -> None:\n    g = numbers(3)\n{calls}"));
    assert_eq!(
        refusals_by_callee(&refused),
        ["first", "drained", "doubled", "outer", "ignore"],
        "got {refused:?}"
    );
    for refusal in &refused[..4] {
        assert!(
            refusal
                .notes
                .iter()
                .any(|note| note.contains("changes the parameter 'g'")),
            "a callee that uses the generator changes it, got {:?}",
            refusal.notes
        );
    }
    assert!(
        refused[4]
            .notes
            .iter()
            .any(|note| note.contains("does not change the parameter 'g'")
                && note.contains("'Generator[int]' cannot be copied")),
        "an unchanged generator parameter would need a copy, got {:?}",
        refused[4].notes
    );
    checked(&format!(
        "{callees}\ndef main() -> None:\n    mut g = numbers(3)\n{calls}"
    ))?;
    Ok(())
}

/// #1561: the names a `match`, `if let` or `while let` pattern binds from a `mut` parameter, from a field of it or from
/// a view into it are views into the parameter, even when one reuses the parameter's name, so a change through one is a
/// change to the parameter and an immutable binding passed for it is refused. A pattern that only reads leaves the
/// parameter unchanged, and the immutable binding is handed over as a copy.
#[test]
fn change_through_a_pattern_binding_counts_as_changing_issue1561() -> Result<(), String> {
    let changes = r#"
model Holder:
    inner: Option[list[int]]

def by_match(mut box: Option[list[int]]) -> None:
    match box:
        Some(xs) => xs.append(1)
        None => pass

def by_if_let(mut box: Option[list[int]]) -> None:
    if let Some(xs) = box:
        xs.append(1)

def by_while_let(mut box: Option[list[int]]) -> None:
    while let Some(xs) = box:
        xs.append(1)
        break

def by_field(mut h: Holder) -> None:
    match h.inner:
        Some(xs) => xs.append(1)
        None => pass

def by_loop_view(mut rows: list[Option[list[int]]]) -> None:
    for row in rows:
        if let Some(xs) = row:
            xs.append(1)

def by_shadow(mut box: Option[list[int]]) -> None:
    match box:
        Some(box) => box.append(1)
        None => pass
"#;
    let refused = mut_argument_refusals(&format!(
        "{changes}\ndef main() -> None:\n    box = Some([0])\n    h = Holder(inner=Some([0]))\n    rows = [Some([0])]\n    by_match(box)\n    by_if_let(box)\n    by_while_let(box)\n    by_field(h)\n    by_loop_view(rows)\n    by_shadow(box)\n"
    ));
    assert_eq!(
        refusals_by_callee(&refused),
        [
            "by_match",
            "by_if_let",
            "by_while_let",
            "by_field",
            "by_loop_view",
            "by_shadow"
        ],
        "got {refused:?}"
    );

    let reads = "def total(mut box: Option[list[int]]) -> int:\n    match box:\n        Some(xs) => return len(xs)\n        None => return 0\n\ndef main() -> None:\n    box = Some([0])\n    println(total(box))\n";
    let (_, info) = checked(reads)?;
    let call = span_of(reads, "total(box)", 0)?;
    assert!(
        info.mut_argument_is_copied(Span::new(call.start + "total(".len(), call.end - 1)),
        "a pattern that only reads leaves the parameter unchanged, so the immutable binding is copied"
    );
    Ok(())
}

/// #1561: a `match`, `if let` or `while let` whose arm changes a `mut` parameter through a name its pattern binds is
/// recorded for lowering as matched in place, and so is each enclosing scrutinee the change goes through, and a field
/// of `self` in a `mut self` method. A form whose arms only read, and one over a value the parameter does not own,
/// are not.
#[test]
fn scrutinee_changed_through_a_pattern_binding_is_matched_in_place_issue1561() -> Result<(), String> {
    let source = r#"
model Holder:
    pub inner: Option[list[int]]

class Grid:
    pub rows: list[Option[list[int]]]

    def fill(mut self) -> None:
        for row in self.rows:
            if let Some(xs) = row:
                xs.append(1)

    def count(self) -> int:
        match self.rows[0]:
            Some(xs) => return len(xs)
            None => return 0

def fresh() -> Option[list[int]]:
    return Some([0])

def changes(mut box: Option[Option[list[int]]], mut h: Holder, mut other: Option[list[int]]) -> int:
    match box:
        Some(inner) =>
            match inner:
                Some(xs) => xs.append(1)
                None => pass
        None => pass
    if let Some(ys) = h.inner:
        ys.append(2)
    while let Some(zs) = other:
        zs.append(3)
        break
    match other:
        Some(ws) => return len(ws)
        None => pass
    match fresh():
        Some(vs) => vs.append(4)
        None => pass
    return 0
"#;
    let (_, info) = checked(source)?;
    for (header, scrutinee, occurrence, in_place) in [
        ("match box:", "box", 0, true),
        ("match inner:", "inner", 0, true),
        ("= h.inner:", "h.inner", 0, true),
        ("= other:", "other", 0, true),
        ("match other:", "other", 0, false),
        ("match fresh():", "fresh()", 0, false),
        ("= row:", "row", 0, true),
        ("match self.rows[0]:", "self.rows[0]", 0, false),
    ] {
        let header_span = span_of(source, header, occurrence)?;
        let start = header_span.end - 1 - scrutinee.len();
        assert_eq!(
            info.match_scrutinee_is_changed_in_place(Span::new(start, start + scrutinee.len())),
            in_place,
            "`{header}` matches `{scrutinee}` in place: {in_place}"
        );
    }
    Ok(())
}

/// #1561: a change through a name a pattern binds from a place that does not permit it is refused (`INCAN-T0001`),
/// naming the place: a binding or parameter declared without `mut`, a static, `self` in a plain `self` method, a dict
/// value, and the variable of a loop over an immutable list. A pattern that only reads, a direct change to an immutable
/// local, a subject that is no place, and a method the checker cannot classify are not refused.
#[test]
fn change_through_a_pattern_binding_of_a_read_only_place_is_refused_issue1561() -> Result<(), String> {
    let refused = r#"
static BOX: Option[list[int]] = Some([1])

class Holder:
    pub inner: Option[list[int]]

    def fill(self) -> None:
        match self.inner:
            Some(xs) => xs.append(1)
            None => pass

def by_param(box: Option[list[int]]) -> None:
    if let Some(xs) = box:
        xs.append(1)

def main() -> None:
    fixed = Some([1])
    match fixed:
        Some(xs) => xs.append(1)
        None => pass
    match BOX:
        Some(xs) => xs.append(1)
        None => pass
    mut table = {"a": Some([1])}
    match table["a"]:
        Some(xs) => xs.append(1)
        None => pass
    rows = [Some([1])]
    for row in rows:
        while let Some(xs) = row:
            xs.append(1)
            break
"#;
    let messages = check_errors(refused)
        .into_iter()
        .map(|error| {
            assert_eq!(
                error.stable_code(),
                None,
                "the refusal takes the typecheck code INCAN-T0001: {error:?}"
            );
            error.message
        })
        .collect::<Vec<_>>();
    assert_eq!(
        messages,
        [
            "Cannot change 'xs' - it is bound from 'self', which this method takes as plain 'self'",
            "Cannot change 'xs' - it is bound from the parameter 'box', which is not declared 'mut'",
            "Cannot change 'xs' - it is bound from 'fixed', which is immutable",
            "Cannot change 'xs' - it is bound from the static 'BOX', which a pattern cannot change in place",
            "Cannot change 'xs' - it is bound from a dict value, which a pattern binds as a copy",
            "Cannot change 'xs' - it is bound from 'rows', which is immutable",
        ]
    );

    checked(
        r#"
from std.async.task import TaskJoinError

def fresh() -> Option[list[int]]:
    return Some([1])

def describe(result: Result[int, TaskJoinError]) -> str:
    match result:
        Ok(_) => return "ok"
        Err(error) => return error.message()

def main() -> None:
    fixed = Some([1, 2])
    match fixed:
        Some(xs) => println(len(xs))
        None => pass
    items = [1]
    items.append(2)
    match fresh():
        Some(xs) => xs.append(3)
        None => pass
"#,
    )?;
    Ok(())
}

/// #1561: a method call changes a `mut` parameter exactly when the one receiver classifier says so: a builtin
/// collection method by its registry entry, and a declared method by its declarations, a method alias by its target and
/// a method the type does not declare by the trait that provides it. A call that only reads leaves an immutable binding
/// handed over as a copy; a call that changes the receiver refuses it.
#[test]
fn method_receiver_change_follows_its_declaration_issue1561() -> Result<(), String> {
    let callees = r#"
trait Peek:
    def peek(self) -> int:
        return 7

trait Bump:
    def bump(mut self) -> None:
        pass

class Counter with Peek, Bump:
    n: int
    look = read
    grow = add

    def read(self) -> int:
        return self.n

    def add(mut self) -> None:
        self.n += 1

def counted(mut items: list[int]) -> int:
    return items.count(1)

def appended(mut items: list[int]) -> None:
    items.append(1)

def looked(mut c: Counter) -> int:
    return c.look()

def grown(mut c: Counter) -> None:
    c.grow()

def peeked(mut c: Counter) -> int:
    return c.peek()

def bumped(mut c: Counter) -> None:
    c.bump()

def main() -> None:
    items = [1, 2]
    c = Counter(n=1)
    println(counted(items))
    appended(items)
    println(looked(c))
    grown(c)
    println(peeked(c))
    bumped(c)
"#;
    let refused = mut_argument_refusals(callees);
    assert_eq!(
        refusals_by_callee(&refused),
        ["appended", "grown", "bumped"],
        "got {refused:?}"
    );
    let (_, info) = checked(
        &callees
            .replace("    appended(items)\n", "")
            .replace("    grown(c)\n", "")
            .replace("    bumped(c)\n", ""),
    )?;
    assert_eq!(
        info.calls.mut_argument_copies.len(),
        3,
        "`count`, a method alias of a plain-`self` method and a plain-`self` trait method only read, so each immutable binding is copied"
    );
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

/// RFC 009: `int` is an alias of `i64`, so a `mut` parameter spelled `i64`, `long` or `bigint` is the function's own
/// copy as a `mut int` one is: its body may rebind it, and a function type cannot mark it.
#[test]
fn every_int_spelling_is_a_copied_scalar_parameter() -> Result<(), String> {
    checked(
        r#"
def bump(mut n: i64) -> i64:
    n += 1
    return n

def grow(mut n: long) -> int:
    n = n * 2
    return n

def shrink(mut n: bigint) -> bigint:
    n -= 1
    return n

def main() -> None:
    count: int = 1
    println(bump(count) + grow(count) + shrink(count))
"#,
    )?;
    let errors = check_errors("def apply(f: (mut i64) -> None) -> None:\n    pass\n");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("`mut` cannot mark the `i64` parameter of a function type")),
        "a function type cannot mark an i64 parameter, as it cannot mark an int one, got {errors:?}"
    );
    Ok(())
}
