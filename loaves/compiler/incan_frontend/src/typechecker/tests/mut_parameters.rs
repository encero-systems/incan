//! `mut` parameters (#1773): a `mut` parameter is a mutable binding inside its function, method or trait default, and
//! a call passes a mutable place for each `mut` parameter whose changes reach the caller (`INCAN-T0117`).

use super::*;

const MUT_ARGUMENT_CODE: &str = "INCAN-T0117";

/// Return the `INCAN-T0117` refusals of one program, which must fail to check.
fn mut_argument_refusals(source: &str) -> Result<Vec<CompileError>, String> {
    match check_str(source) {
        Ok(()) => Err(format!("expected INCAN-T0117 refusals, the program checked:\n{source}")),
        Err(errors) => Ok(errors
            .into_iter()
            .filter(|error| error.stable_code() == Some(MUT_ARGUMENT_CODE))
            .collect()),
    }
}

/// Check one program that must be accepted, reporting its errors otherwise.
fn assert_accepted(source: &str) -> Result<(), String> {
    check_str(source).map_err(|errors| format!("expected the program to check, got {errors:?}\n{source}"))
}

/// #1773: `n += 1` and a reassignment of a `mut` parameter are accepted in a function, an inherent method and a trait
/// default method, whatever the parameter's type, while a parameter without `mut` stays immutable.
#[test]
fn mut_parameter_is_a_mutable_binding_in_its_body_issue1773() -> Result<(), String> {
    assert_accepted(
        r#"
def bumped(mut n: int) -> int:
    n += 1
    n = n * 2
    return n

def relabeled(mut label: str) -> str:
    label = label + "!"
    return label

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
        items = [0]
        items.append(1)
        return len(items)

model Walker with Stepper:
    id: int

def main() -> None:
    println(bumped(1))
    mut label = "a"
    println(relabeled(label))
    println(Counter(step=2).advanced(1))
    println(Walker(id=1).stepped(1))
    mut items: list[int] = []
    println(Walker(id=1).extended(items))
"#,
    )?;

    let errors = match check_str("def frozen(n: int) -> int:\n    n += 1\n    return n\n") {
        Ok(()) => return Err("a parameter without `mut` must stay immutable".to_string()),
        Err(errors) => errors,
    };
    assert!(
        errors.iter().any(|error| error.message.contains("Cannot mutate 'n'")),
        "expected the immutable-parameter refusal, got {errors:?}"
    );
    Ok(())
}

/// #1773: an immutable binding, a literal, a call result and a field of an immutable binding passed to a `mut`
/// parameter whose changes reach the caller are refused, for a function, an inherent method and a trait default
/// method, positionally and by name; each refusal names the parameter and the callee.
#[test]
fn immutable_argument_to_mut_parameter_is_refused_issue1773() -> Result<(), String> {
    let refusals = mut_argument_refusals(
        r#"
model Basket:
    items: list[int]

def extend(mut items: list[int]) -> None:
    items.append(9)

class Store:
    def fill(self, mut items: list[int]) -> None:
        items.append(1)

trait Replacer:
    def replace(self, mut items: list[int]) -> int:
        items.append(9)
        return len(items)

model Widget with Replacer:
    id: int

def fresh() -> list[int]:
    return [3]

def main() -> None:
    items: list[int] = [1, 2]
    basket = Basket(items=[4])
    extend(items)
    extend([1, 2])
    extend(fresh())
    extend(basket.items)
    extend(items=items)
    Store().fill(items)
    println(Widget(id=1).replace(items))
"#,
    )?;
    assert_eq!(
        refusals.len(),
        7,
        "one refusal per immutable argument, got {refusals:?}"
    );
    for (index, callee) in ["extend", "extend", "extend", "extend", "extend", "fill", "replace"]
        .iter()
        .enumerate()
    {
        let refusal = &refusals[index];
        assert!(
            refusal
                .message
                .contains(&format!("'mut' parameter 'items' of '{callee}'")),
            "refusal {index} must name the parameter and `{callee}`, got {:?}",
            refusal.message
        );
    }
    assert!(
        refusals[0]
            .hints
            .iter()
            .any(|hint| hint.contains("Declare 'items' with 'mut'")),
        "an immutable binding's refusal says to declare it `mut`, got {:?}",
        refusals[0].hints
    );
    assert!(
        refusals[1]
            .hints
            .iter()
            .any(|hint| hint.contains("Bind the value to a 'mut' variable")),
        "a literal's refusal says to bind it to a `mut` variable, got {:?}",
        refusals[1].hints
    );
    Ok(())
}

/// #1773: a `mut` binding, a `mut` parameter passed on, a field of `mut self`, a static and an element of a `mut`
/// binding are mutable places; a `mut` parameter of type `int`, `float` or `bool` only makes the parameter reassignable
/// in its body, so any argument, a literal included, is accepted for it.
#[test]
fn mutable_places_and_copied_mut_parameters_are_accepted_issue1773() -> Result<(), String> {
    assert_accepted(
        r#"
static LOG: list[int] = []

def extend(mut items: list[int]) -> None:
    items.append(9)

def forward(mut items: list[int]) -> None:
    extend(items)

def bumped(mut n: int, mut ratio: float, mut flag: bool) -> int:
    n += 1
    return n

class Store:
    pub items: list[int]
    pub rows: list[list[int]]

    def refill(mut self) -> None:
        extend(self.items)
        extend(self.rows[0])

def main() -> None:
    mut items: list[int] = [1]
    extend(items)
    forward(items)
    extend(items=items)
    extend(LOG)
    println(bumped(1, 2.0, true))
    mut store = Store(items=[], rows=[[1]])
    store.refill()
    extend(store.items)
"#,
    )
}

/// #1773: the refusal follows a call into another source module, where the `mut` parameter is declared.
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
