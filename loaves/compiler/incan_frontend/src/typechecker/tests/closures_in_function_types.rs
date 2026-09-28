//! Closures that capture local values in function-typed slots (#1561): a new local, a parameter its function only
//! calls, and the one `return` of a function hold one; any other function-typed slot refuses it with `INCAN-T0001`,
//! while a named function and a closure that captures nothing are accepted everywhere.

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
