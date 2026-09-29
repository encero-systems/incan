//! Closures that capture local values in function-typed slots (#1561): a new local, a parameter its function only
//! calls, and the one `return` of a function hold one; any other function-typed slot refuses it with `INCAN-T0001`,
//! while a named function and a closure that captures nothing are accepted everywhere. The standard library's own
//! source, an SDK component compiled from its source and a module a consumer checks from source under `__incan_std`
//! included, is left as its own lowering spells it.

use super::*;

/// Check a program and return the messages of its errors.
fn error_messages(source: &str) -> Vec<String> {
    check_str(source)
        .err()
        .unwrap_or_default()
        .into_iter()
        .map(|error| error.message)
        .collect()
}

/// A capturing closure is accepted as a new local, annotated or not, as an argument for a function-typed parameter
/// that a function or method of this module only calls, as the one returned value of a function or method, and as
/// the callee of a call; a named function and a closure that captures nothing stay accepted in a list and a field.
#[test]
fn capturing_closures_are_held_where_their_own_type_is_kept_issue1561() {
    assert_check_ok(
        r#"
model Op:
    step: (int) -> int

class Acc:
    pub base: int

    def run(self, f: (int) -> int) -> int:
        return f(self.base)

    def adder(self) -> (int) -> int:
        base = self.base
        return (x) => x + base

def apply(f: (int) -> int, x: int) -> int:
    return f(x)

def make_adder(n: int) -> (int) -> int:
    return (x) => x + n

def double(x: int) -> int:
    return x * 2

def main() -> None:
    n = 5
    g: (int) -> int = (x) => x + n
    println(g(1))
    println(apply(g, 2))
    println(apply(g, 3))
    println(apply((x) => x * n, 4))
    add = make_adder(3)
    println(add(1))
    println(apply(make_adder(1), 1))
    a = Acc(base=2)
    println(a.run((x) => x + n))
    println(a.adder()(1))
    ops: list[(int) -> int] = [double, (x) => x + 1]
    op = Op(step=(x) => x - 1)
    runner = op.step
    println(len(ops) + runner(1))
"#,
    );
}

/// A capturing closure is refused in every other function-typed slot: a list element, a construction's argument, the
/// value of a `match` arm, a parameter its function stores, passes on or is read as a value, a parameter of a `pub`
/// function, a parameter of a callable value, a function with more than one `return`, a returned local, a returned
/// closure that reads `self`, and a reassignment of a local that holds one.
#[test]
fn capturing_closures_are_refused_where_a_function_type_is_a_function_pointer_issue1561() {
    let messages = error_messages(
        r#"
model Op:
    run: (int) -> int

class Acc:
    pub base: int

    def adder(self) -> (int) -> int:
        return (x) => x + self.base

def keep(f: (int) -> int) -> list[(int) -> int]:
    return [f]

pub def exported(f: (int) -> int) -> int:
    return f(1)

def valued(f: (int) -> int) -> int:
    return f(1)

def two_returns(n: int, flag: bool) -> (int) -> int:
    if flag:
        return (x) => x + n
    return (x) => x - n

def returns_local(n: int) -> (int) -> int:
    g: (int) -> int = (x) => x + n
    return g

def main() -> None:
    n = 10
    fs: list[(int) -> int] = [(x) => x + n]
    op = Op(run=(x) => x + n)
    kept = keep((x) => x + n)
    println(exported((x) => x + n))
    alias = valued
    println(valued((x) => x + n))
    println(alias((x) => x + n))
    mut g: (int) -> int = (x) => x + n
    g = (x) => x
    h: (int) -> int = match n:
        1 => (x) => x + n
        _ => (x) => x
"#,
    );
    let expected = [
        "A closure that captures local values cannot be returned as a function type",
        "A closure that captures local values cannot be returned as a function type",
        "A closure that captures local values cannot be returned as a function type",
        "'g', which holds a closure that captures local values, cannot be returned as a function type",
        "A closure that captures local values cannot be stored in a list",
        "A closure that captures local values cannot be an argument of a construction",
        "A closure that captures local values cannot be passed to the parameter 'f' of 'keep'",
        "A closure that captures local values cannot be passed to the parameter 'f' of 'exported'",
        "A closure that captures local values cannot be passed to the parameter 'f' of 'valued'",
        "A closure that captures local values cannot be passed to the parameter 'f' of 'alias'",
        "'g', which holds a closure that captures local values, cannot be assigned another value",
        "A closure that captures local values cannot be the value of a 'match' arm",
    ];
    for message in expected {
        assert!(
            messages.iter().any(|actual| actual == message),
            "expected {message:?} among {messages:?}"
        );
    }
    assert_eq!(messages.len(), expected.len(), "{messages:?}");
}

/// Parse one source module of the `stdlib-core` SDK component, named by its path below the component's `src`.
fn parse_stdlib_core_module(relative: &str) -> Result<Program, String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/core/src")
        .join(relative);
    let source = std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let tokens = lexer::lex(&source).map_err(|errors| format!("{relative} lex failed: {errors:?}"))?;
    parser::parse(&tokens).map_err(|errors| format!("{relative} parse failed: {errors:?}"))
}

/// #1561: an SDK component compiled from its own source is the standard library's own source, although its modules
/// are checked under their physical paths (`derives.collection`, not `std.derives.collection`). The rules that leave
/// the standard library's source as it is hold there too: `Iterator.flat_map`'s default builds its adapter with a
/// closure that captures `f`, and `Iterator.collect` and `FallibleIterator.collect` append the item their own
/// `__next__` returns, whose type the standard library's own lowering bounds. The same module checked outside an SDK
/// bootstrap is a project module and meets the project rules.
#[test]
fn sdk_provider_bootstrap_checks_its_modules_as_standard_library_source_issue1561() -> Result<(), String> {
    let collection = parse_stdlib_core_module("derives/collection.incn")?;
    let callable = parse_stdlib_core_module("traits/callable.incn")?;
    let check = |bootstrap_roots: &[&str]| {
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(vec!["derives".to_string(), "collection".to_string()]));
        checker.register_dependency_module_path_segments(
            "traits_callable",
            vec!["traits".to_string(), "callable".to_string()],
        );
        checker
            .set_provider_plan(Arc::new(ProviderPlan::default().with_bootstrap_sdk_namespace_roots(
                bootstrap_roots.iter().map(|root| (*root).to_string()),
            )));
        checker.check_with_imports_allow_private(&collection, &[("traits_callable", &callable)])
    };
    check(&["derives", "traits"])
        .map_err(|errors| format!("the SDK component's own source should check as the standard library: {errors:?}"))?;
    let errors = check(&[]).err().unwrap_or_default();
    let messages = errors.iter().map(|error| error.message.as_str()).collect::<Vec<_>>();
    assert!(
        messages.contains(&"A closure that captures local values cannot be an argument of a construction")
            && messages.contains(&"List.append requires element type 'T' to be Clone"),
        "outside an SDK bootstrap the module is a project module: {messages:?}"
    );
    Ok(())
}

/// #1561: a consumer with no SDK inventory, such as one on a fresh home, checks each standard-library module it imports
/// from source under the generated `__incan_std` namespace (`__incan_std.derives.collection`). That is the standard
/// library's own source as well, so `Iterator.flat_map`'s capturing adapter closure and the unbounded items
/// `Iterator.collect` and `FallibleIterator.collect` append check there as they do under `std`.
#[test]
fn a_module_under_the_generated_stdlib_namespace_checks_as_standard_library_source_issue1561() -> Result<(), String> {
    let collection = parse_stdlib_core_module("derives/collection.incn")?;
    let callable = parse_stdlib_core_module("traits/callable.incn")?;
    let namespace = incan_lang::lang::stdlib::INCAN_STD_NAMESPACE;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec![
        namespace.to_string(),
        "derives".to_string(),
        "collection".to_string(),
    ]));
    let callable_module = format!("{namespace}_traits_callable");
    checker.register_dependency_module_path_segments(
        &callable_module,
        vec![namespace.to_string(), "traits".to_string(), "callable".to_string()],
    );
    checker
        .check_with_imports(&collection, &[(callable_module.as_str(), &callable)])
        .map_err(|errors| format!("the standard library's source under `{namespace}` should check: {errors:?}"))
}
