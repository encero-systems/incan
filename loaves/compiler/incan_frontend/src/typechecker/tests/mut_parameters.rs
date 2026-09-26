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
        .map(|param| {
            info.declarations
                .mut_param_shows_changes_to_caller(param.span, &param.node.name)
        })
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
