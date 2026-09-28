# Functions and Calls

This page defines function signatures, function types, ordinary call binding, rest parameters, call-site unpacking, and collection literal spread.

For a step-by-step introduction, see [Functions](../tutorials/book/03_functions.md). For the ideas behind rest parameters and defaults, see [Functions and calls explained](../explanation/functions_and_calls.md). For the `Callable0`, `Callable1` and `Callable2` traits, see [Callable objects](stdlib_traits/callable.md). For derived callables, see [Symbol aliases](symbol_aliases.md) and [Callable presets](callable_presets.md).

## Function Signatures

Function parameters use `name: Type`, and return types use `-> Type`:

```incan
def add(a: int, b: int) -> int:
    return a + b
```

A function declared `-> None` returns no value:

```incan
def log(message: str) -> None:
    println(message)
```

## Function Types

`(A, B) -> R` is the type of a callable that takes an `A` and a `B` and returns an `R`; `() -> R` takes no arguments. A named `def` function and a closure are values of a function type. With `def double(x: int) -> int`:

```incan
f: (int) -> int = double          # accepted
g: (int) -> int = (x) => x + 1    # accepted
h: (str) -> int = double          # refused: INCAN-T0001
```

A closure's parameters take their types from the function type the closure is checked against: the annotated type of the binding, field, element or return it is assigned to; the function type of the parameter it is passed to, with the callee's type parameters that the other arguments fix substituted; and the callback of an `Iterator[T]` method, whose parameter is `T`, a `fold` or `reduce` callback also taking the type of the first argument as its accumulator. An operation on a parameter that its type does not provide is refused (`INCAN-T0001`).

```incan
def apply_twice[T](f: (T) -> T, value: T) -> T:
    return f(f(value))

def main() -> None:
    names = ["ada", "lin"]
    shouted = names.iter().map((name) => name.upper()).collect()   # accepted: name is str
    total = names.iter().fold(0, (acc, name) => acc + len(name))   # accepted: acc is int, name is str
    four = apply_twice((x) => x + 1, 2)                              # accepted: x is int
    bad = names.iter().map((name) => name + 1).collect()           # refused: str + int (INCAN-T0001)
```

### `Callable[Params, R]`

`Callable[Params, R]` is another spelling of a function type; the two spellings are the same type.

| Sugar                 | Arrow form    |
| --------------------- | ------------- |
| `Callable[(), R]`     | `() -> R`     |
| `Callable[A, R]`      | `(A) -> R`    |
| `Callable[(A, B), R]` | `(A, B) -> R` |

`Callable[...]` with other than two type arguments, and a bracketed parameter list such as `Callable[[A], R]`, are syntax error `INCAN-P0001`.

### Closure captures

A closure reads each outer local it names as the value that local holds when the closure is constructed. A later change to the outer binding does not change the value the closure reads. How a closure captures a local: [Closures](../explanation/closures.md#how-a-closure-captures-outer-locals).

A closure does not change an outer local it reads, a local bound to a static included: a call of a method that changes it, a write to a field or element of it, a loop, comprehension or pattern that changes its items, and passing it to a `mut` parameter the call changes are refused with `INCAN-T0001`, the argument with `INCAN-T0117`. A closure changes a static through the static's own name.

```incan
def main() -> None:
    mut items: list[int] = []
    add = () => items.append(1)         # refused: the closure changes a local it reads (INCAN-T0001)
    count = () => len(items)            # accepted
```

### Closures that capture local values

A closure that reads a local of an enclosing function, a parameter or `self`, captures it; a local partial holds its presets. Such a callable, a *capturing callable*, is held by these function-typed slots:

| Slot | Contract |
| --- | --- |
| A new local binding | Accepted, annotated with a function type or not. The local is not reassigned afterwards. |
| The callee of a call | Accepted. |
| An argument for a parameter of function type | Accepted when the parameter is declared without `mut` and is not `*args`, and the function or method that declares it only calls it, outside any closure or generator expression, and qualifies (below). |
| A `return` value | Accepted when the function or method has one `return`, whose value is a closure that reads a parameter or a local of the function and not `self`, or a local partial, and qualifies (below). |

A function qualifies when it is declared in the same module, is neither `pub`, `async`, generic, decorated nor a generator, and is not used as a decorator; its name is only called, never read as a value. A method qualifies when it is declared in a model, class, newtype or enum of the same module, takes `self` or `mut self`, is neither `async`, generic, decorated, overloaded nor a generator, and implements no method of a trait the type adopts; no member read without a call spells its name in the module, and its class neither extends nor is extended by another class.

Every other function-typed slot refuses a capturing callable with `INCAN-T0001`: an element of a list, set, dict or tuple, a yielded value, an argument of a construction or of `Some`, `Ok` and `Err`, a preset of a local partial, the value of an `if` branch or a `match` arm, an assignment to an existing name, field or element, an assignment to a local that holds one, a parameter of a callable value, and a parameter or return of any other function declared in the project or in a library. A named function and a closure that captures nothing are accepted in every function-typed slot.

```incan
def apply(f: (int) -> int, x: int) -> int:
    return f(x)

def keep(f: (int) -> int) -> list[(int) -> int]:
    return [f]

def make_adder(n: int) -> (int) -> int:
    return (x) => x + n                 # accepted

def main() -> None:
    n = 5
    g: (int) -> int = (x) => x + n      # accepted
    apply(g, 1)                         # accepted
    keep((x) => x + n)                  # refused: keep stores its parameter (INCAN-T0001)
    fs: list[(int) -> int] = [g]        # refused: a list element (INCAN-T0001)
```

### `mut` parameters

`mut` on a parameter makes it a mutable binding in the function's body. A parameter of type `int`, `float` or `bool`, under any spelling of the type (`i64`, `long` and `bigint` are `int`; `f64`, `double` and `fp64` are `float`) and also through a type alias, is the function's own copy: the body may change and rebind it, its changes stay local, and it is not marked in the function type. A parameter of any other type, except a Rust type and `*args` or `**kwargs`, is marked: the function's changes to it reach the caller, and the function type marks it, `(mut T, ...) -> R`.

| Declaration                                     | Function type                  |
| ----------------------------------------------- | ------------------------------ |
| `def grow(mut counter: Counter) -> int`         | `(mut Counter) -> int`         |
| `def append(mut xs: list[int], x: int) -> None` | `(mut list[int], int) -> None` |
| `def bump(mut n: int) -> int`                   | `(int) -> int`                 |

| Rule                | Contract |
| ------------------- | -------- |
| Where it is written | On a parameter of an arrow-form function type. In a tuple type, a parenthesized type, or the parameter list of `Callable[...]` it is syntax error `INCAN-P0001`. |
| Copied scalars      | On an `int`, `float` or `bool` parameter of a function type, also through a type alias, the marker is refused with `INCAN-T0001`. |
| Type identity       | The marker is part of the function type. Two function types match only when they mark the same parameters; a mismatch in either direction is refused with `INCAN-T0001`. |
| `def` parameters    | A `def` parameter declared `mut` is marked, except a parameter of type `int`, `float` or `bool`, a parameter of a Rust type, and `*args` or `**kwargs`. |
| Parameters without `mut` | The body does not change a parameter declared without `mut`: a field or element write through it, a call of a method that changes it, and a change through the variable of a `for` loop over it are refused with `INCAN-T0001` (see [Assignments](assignments.md#rules)). |
| Rebinding           | The body does not assign a new value to a marked parameter: `items = []` and `label += "!"` are refused with `INCAN-T0001`. |
| Holding             | The body does not hold a marked parameter in another name or value. Each form in the table below is refused with `INCAN-T0001`. Passing the parameter as an argument to any other call is accepted, `rows.append(items)` included. |
| Changing calls      | A call changes a marked parameter in the cases that [Changing calls](#changing-calls) lists. |
| Arguments           | For a marked parameter that the call changes, the argument is a `mut` binding or parameter, `self` in a `mut self` method, a field of one of those, or a temporary such as a literal or a call result. An immutable binding or a field of one, an element of a list or a value of a dict, a static, and the variable of a `for` loop are refused with `INCAN-T0117`. For a marked parameter that the call does not change, any argument is accepted, except one of those whose type cannot be copied, such as a `Generator`, which is refused with `INCAN-T0117`. |
| Libraries           | A published function keeps its marked parameters: a consumer sees the function type the producer checked. |
| Closures            | A closure checked against a function type has each parameter that type marks marked in its own type. |

A marked parameter is held, and refused with `INCAN-T0001`, by each of these:

| Form | Example |
| --- | --- |
| A new binding, or an assignment to an existing name | `other = items`, `let other = items`, `mut other: list[int] = items` |
| A tuple, list, set or dict literal, or a comprehension | `(items, 1)`, `[items]`, `{"k": items}`, `[items for _ in rows]` |
| A field or element store | `holder.items = items`, `table["k"] = items` |
| An argument of a model, class, newtype or enum-variant construction, of `Some`, `Ok` or `Err`, or a `partial` preset | `Holder(items=items)`, `Wrap.Held(items)`, `Some(items)`, `partial extend(items=items)` |
| The value of a `match` arm, an `if` branch, a `break` or a `yield` | `0 => items`, `break items`, `yield items` |
| A `match` arm pattern that binds the whole value to a name | `match items: xs => ...` |
| A closure that returns it, changes it, or passes it to a parameter that a call may change | `() => items`, `() => items.append(1)` |

#### Changing calls

A call changes a marked parameter when:

- the body that runs assigns to the parameter's elements or fields, passes it to a marked parameter that a call it makes changes, or calls on it a method that takes `mut self` (declared by the parameter's type, named through a method alias, or provided by a trait the type adopts), a `list` method other than `clone`, `contains`, `count` and `index`, a `dict` method other than `keys`, `values`, `get` and `contains_key`, or a `set` method other than `contains`;
- the parameter is a `Generator`, and the body that runs uses it: iterating it, calling one of its methods and passing it to a call each advance it;
- the body that runs changes an element through the variable of a `for` loop over the parameter, over a field of it, or over the variable of an enclosing such loop, for a list whose elements are not `int`, `float` or `bool` (`for row in items: row.append(3)`);
- the body that runs changes a value through a name that a `match`, `if let` or `while let` pattern binds from the parameter, from a field of it, or from such a loop variable or name (`match box: Some(xs) => xs.append(1)`);
- the call is a method call through a type parameter's bound, on `self` in a trait's default method, or on a trait-typed value;
- the callee is known only by a function type that marks the parameter, such as a parameter of function type or a function of a compiled library;
- the call goes through a local bound to a function, and the local is reassigned in the module.

```incan
def extend(mut items: list[int]) -> None:
    items.append(9)

def reset(mut items: list[int]) -> None:
    items = []                          # refused: rebinds a marked parameter (INCAN-T0001)

def keep(mut items: list[int]) -> int:
    other = items                       # refused: holds a marked parameter in another name (INCAN-T0001)
    return len(other)

def main() -> None:
    mut kept: list[int] = [1]
    fixed: list[int] = [1]
    extend(kept)                        # accepted
    extend([1])                         # accepted
    extend(fixed)                       # refused: fixed is not declared mut (INCAN-T0117)
```

```incan
step: (mut Counter) -> int = grow   # accepted
step: (Counter) -> int = grow       # refused: the function type does not mark the parameter (INCAN-T0001)
twice: (mut int) -> int = bump      # refused: `mut` cannot mark the `int` parameter (INCAN-T0001)
```

A `mut self` method's decorators spell the receiver with this marker; see [Method decorators](language.md#method-decorators). A worked program: [Pass a function that changes its argument](../how-to/decorators.md#task-pass-a-function-that-changes-its-argument). The `mut` parameters of trait methods are in [Derives and traits](derives_and_traits.md#mut-parameters-of-trait-methods).

### Rest-aware function values

A function value keeps the rest parameters of the function it names.

| Rule              | Contract                                                                                                                                                                                                               |
| ----------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `*args: T`        | Through the value, the call accepts extra positional arguments of type `T` and `*list_value` unpacking; inside the function, `args` is a `List[T]`.                                                                     |
| `**kwargs: T`     | Through the value, the call accepts extra keyword arguments of type `T` and `**dict_value` unpacking; inside the function, `kwargs` is a `Dict[str, T]`.                                                                |
| Fixed-arity types | A function type written with a trailing `List[T]` or `Dict[str, T]` parameter has no rest parameter: that parameter takes one list or dictionary argument, and extra arguments are refused with `INCAN-T0001`. |
| Errors            | A call through the value binds like a direct call and is refused in the cases [Type Errors](#type-errors) lists.                                                                                                       |

With `def collect(prefix: str, *items: int, **labels: str) -> int` and `f = collect`:

```incan
f("event", 0, *[1, 2], **{"kind": "demo"})   # accepted
```

## Ordinary Call Binding

Arguments bind to normal parameters in this order:

1. Positional arguments bind left to right.
2. Named arguments bind by exact parameter name.
3. Binding a parameter twice is error `INCAN-T0001`.
4. A required parameter that remains unbound is error `INCAN-T0001`.
5. A named argument that names no parameter is error `INCAN-T0001`, unless the callee declares `**kwargs`.
6. A positional argument beyond the positional parameters is error `INCAN-T0001`, unless the callee declares `*args`.
7. A defaulted parameter that remains unbound takes its declared default. A name in a default resolves in the module that declares the callable, whichever module the call is written in: a name that module declares resolves to that declaration, private or public, and a name it imports resolves to the declaration the import names. The same holds for the presets of a method partial.
8. In a package other than the callable's, a call may leave a defaulted parameter unbound only when the default is one of the following, and is not a preset of a method partial:
    - a literal, or a negated number literal;
    - a list or dict whose elements are such defaults;
    - the name or path of a const, a static or a function, or an enum variant without a payload, such as `LIMIT` or `Mode.Fast`;
    - a call of a function the package declares, named directly, through a public symbol alias of it or through its module, whose arguments are such defaults;
    - a construction of a newtype the package declares, or of a model or class the package exports, whose arguments are such defaults.

    Otherwise the parameter is required in that package, and a call there that leaves it unbound is error `INCAN-T0001`.

```incan
def connect(host: str, port: int) -> str:
    return f"{host}:{port}"

def main() -> str:
    a = connect("localhost", 5432)             # accepted
    b = connect(host="localhost", port=5432)   # accepted
    c = connect("localhost", host="db")        # refused: INCAN-T0001, host is bound twice
    return connect("localhost")                # refused: INCAN-T0001, port is unbound
```

Defaults that name a private const and call a private function of their module, called from another module. `describe()` in `main.incn` passes `CHUNK` and `_unit()` of `helpers.incn`:

```incan
# helpers.incn
const CHUNK: int = 4

def _unit() -> str:
    return "bytes"

pub def describe(n: int = CHUNK, unit: str = _unit()) -> str:
    return f"{n} {unit}"
```

```incan
# main.incn
from helpers import describe

def main() -> None:
    println(describe())   # accepted
    println(describe(8))  # accepted
```

A default that names an imported const, called from a module that declares a const of the same name. `box_size()` in `main.incn` passes the `SIZE` of `sizes.incn`, which `boxes.incn` imports:

```incan
# sizes.incn
pub const SIZE: int = 3
```

```incan
# boxes.incn
from sizes import SIZE

pub def box_size(n: int = SIZE) -> int:
    return n
```

```incan
# main.incn
from boxes import box_size

const SIZE: int = 9

def main() -> None:
    println(box_size())  # accepted
```

## Rest Positional Parameters

A parameter `*name: T` captures the positional arguments beyond the ordinary parameters. Inside the function, `name` has type `List[T]`.

```incan
def sum_all(label: str, *values: int) -> int:
    mut total: int = 0
    for value in values:
        total = total + value
    return total

def main() -> int:
    return sum_all("scores", 10, 20, 30)
```

The annotation is the element type: `*values: int` captures `List[int]`.

A call with no extra positional arguments binds an empty list:

```incan
def count(*items: str) -> int:
    return len(items)

def main() -> int:
    return count()
```

## Rest Keyword Parameters

A parameter `**name: T` captures the named arguments that name no other parameter. Inside the function, `name` has type `Dict[str, T]`.

```incan
def request(path: str, **headers: str) -> int:
    return len(headers)

def main() -> int:
    return request("/status", accept="json", trace="enabled")
```

The keys are the argument names. The annotation is the value type: `**headers: str` captures `Dict[str, str]`.

A call with no extra named arguments binds an empty dictionary:

```incan
def request(path: str, **headers: str) -> int:
    return len(headers)

def main() -> int:
    return request("/status")
```

## Combining `*args` and `**kwargs`

A function may declare both rest forms:

```incan
def record(event: str, *values: int, **tags: str) -> int:
    return len(values) + len(tags)

def main() -> int:
    return record("startup", 1, 2, source="cli", mode="debug")
```

The rest values are independent:

- `values` is `List[int]`
- `tags` is `Dict[str, str]`

## Placement Rules

Within one parameter list:

- At most one `*name: T` parameter is allowed.
- At most one `**name: T` parameter is allowed.
- Every normal parameter comes before the rest parameters.
- `**name: T`, when present, is the last parameter.
- Rest parameters cannot have default values.

Breaking one of these rules is error `INCAN-T0001`.

```incan
def ok(a: int, b: int, *rest: int, **opts: str) -> int:   # accepted
    return a + b + len(rest) + len(opts)

def bad_order(*rest: int, value: int) -> int:             # refused: INCAN-T0001
    return value

def also_bad(**opts: str, *rest: int) -> int:             # refused: INCAN-T0001
    return len(rest) + len(opts)
```

## Call-Site Unpacking

`*expr` at a call site extends the callee's positional rest parameter with the elements of a list:

```incan
def sum_all(*values: int) -> int:
    mut total: int = 0
    for value in values:
        total = total + value
    return total

def main() -> int:
    extra = [2, 3]
    return sum_all(1, *extra, 4)
```

For the callee's `*name: T` parameter, the unpacked expression has type `List[T]`.

`**expr` extends the callee's keyword rest parameter with the entries of a dictionary:

```incan
def request(path: str, **headers: str) -> int:
    return len(headers)

def main() -> int:
    defaults = {"accept": "json"}
    return request("/status", **defaults, trace="enabled")
```

For the callee's `**name: T` parameter, the unpacked expression has type `Dict[str, T]`.

Unpacking also binds ordinary fixed parameters when the unpacked expression's length or key set is known from the expression itself:

```incan
def fixed(x: int, y: int) -> int:
    return x + y

def needs_rest(*values: int) -> int:
    return len(values)

def main() -> int:
    ok_fixed = fixed(*[1, 2])
    ok_rest = needs_rest(*[3, 4])
    return ok_fixed + ok_rest
```

For fixed positional parameters, the unpacked expression is a tuple expression or an inline list literal. A value of type `List[T]` binds only a positional rest parameter.

For fixed keyword parameters, the unpacked expression is an inline dictionary literal with string literal keys:

```incan
def route(path: str, method: str) -> str:
    return f"{method} {path}"

def main() -> str:
    return route(**{"path": "/status", "method": "GET"})
```

A value of type `Dict[str, T]` binds only a keyword rest parameter.

## List and Dictionary Literal Spread

`*expr` inside a list literal inserts the elements of a list:

```incan
def main() -> List[int]:
    middle = [2, 3]
    return [1, *middle, 4]
```

List spread preserves source order. Direct elements and spread elements must all be compatible with the resulting list element type.

`**expr` inside a dictionary literal inserts the entries of a dictionary:

```incan
def main() -> Dict[str, str]:
    defaults = {"trace": "off"}
    return {**defaults, "trace": "enabled"}
```

Dictionary spread preserves source order, and a later key replaces an earlier one. That example returns a dictionary whose `"trace"` value is `"enabled"`.

- `[*xs]` and `{**xs}` are accepted.
- `[**xs]` in a list literal and `{*xs}` in a braced literal are syntax error `INCAN-P0001`.
- A spread in a const initializer is error `INCAN-T0001`.

## Source Order and Duplicate Keys

Positional unpacking preserves source order. This call:

```incan
sum_all(1, *extra, 4)
```

builds a rest list equivalent to:

```incan
[1] + extra + [4]
```

Keyword rest values and dictionary spread entries are inserted into a dictionary in source order. A named argument written twice is error `INCAN-T0001`; a key that arrives again through `**dict_value` replaces the earlier entry.

```incan
def request(path: str, **headers: str) -> int:
    return len(headers)

def main() -> int:
    overrides = {"trace": "off"}
    return request("/status", trace="on", **overrides)
```

In that example, the captured `headers["trace"]` value is `"off"`.

## Methods

Methods support the same rest syntax. The receiver is not part of the rest capture:

```incan
class Collector:
    def collect(self, *items: int, **labels: str) -> int:
        return len(items) + len(labels)

def main() -> int:
    collector = Collector()
    xs = [1, 2]
    labels = {"kind": "demo"}
    return collector.collect(0, *xs, **labels)
```

## Function Values

Named functions are first-class values of a [function type](#function-types); a value of a rest-aware function keeps its rest parameters ([Rest-aware function values](#rest-aware-function-values)).

## Type Errors

Each of these is error `INCAN-T0001`:

- Extra positional arguments without `*args`.
- Unknown named arguments without `**kwargs`.
- `*expr` when the callee has no positional rest parameter and the value cannot bind fixed positional parameters.
- `**expr` when the callee has no keyword rest parameter and the value cannot bind fixed keyword parameters.
- `*expr` for fixed parameters when the value's length is not statically known.
- `**expr` for fixed parameters when the value's string-key set is not statically known.
- A direct rest argument whose type is incompatible with the rest element type.
- A `*expr` argument whose type is incompatible with `List[T]`.
- A direct keyword rest value whose type is incompatible with the rest value type.
- A `**expr` argument whose type is incompatible with `Dict[str, T]`.
- Duplicate direct named arguments.
- Duplicate fixed bindings across direct and unpacked arguments.
- Missing required normal parameters.

## Rust Interop

A function imported from a Rust crate has no rest parameters.
