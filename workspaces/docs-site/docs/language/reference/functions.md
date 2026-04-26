# Functions and Calls

This page defines function signatures, ordinary call binding, rest parameters, and rest-aware call-site unpacking.

For a step-by-step introduction, see [Functions](../tutorials/book/03_functions.md). For callable traits and callable type sugar, see [Callable objects](stdlib_traits/callable.md).

## Function Signatures

Function parameters use `name: Type`, and return types use `-> Type`:

```incan
def add(a: int, b: int) -> int:
    return a + b
```

Use `-> None` for a function that does not return a useful value:

```incan
def log(message: str) -> None:
    println(message)
```

## Ordinary Call Binding

Arguments bind to normal parameters in this order:

1. Positional arguments bind left to right.
2. Named arguments bind by exact parameter name.
3. A parameter cannot be bound twice.
4. Required parameters that remain unbound are reported as missing.
5. Unknown named arguments are rejected unless the callee declares `**kwargs`.
6. Extra positional arguments are rejected unless the callee declares `*args`.

```incan
def connect(host: str, port: int) -> str:
    return f"{host}:{port}"

def main() -> str:
    a = connect("localhost", 5432)
    b = connect(host="localhost", port=5432)
    return a + " " + b
```

## Rest Parameter Mental Model

Rest parameters are for APIs that accept "zero or more of the same kind of thing."

The call site stays convenient:

```incan
log("started", "listening", "ready")
```

The callee still receives one ordinary typed value:

```incan
def log(*messages: str) -> int:
    return len(messages)  # messages is List[str]
```

For keyword rest parameters, the caller writes named options and the callee receives a dictionary:

```incan
def annotate(**labels: str) -> int:
    return len(labels)  # labels is Dict[str, str]

def main() -> int:
    return annotate(source="cli", mode="debug")
```

!!! tip "Coming from Python?"
    The spelling follows Python, but the contract is more static.

    - Python `*args` collects a tuple; Incan `*args: T` collects `List[T]`.
    - Python `**kwargs` collects a dict; Incan `**kwargs: T` collects `Dict[str, T]`.
    - Python `**kwargs` is often used as an untyped escape hatch; Incan keyword captures are typed.
    - Python can unpack into ordinary fixed parameters at runtime; Incan currently allows `*expr` and `**expr` only when
      the callee has a matching rest parameter.

## When to Use Rest Parameters

Use `*args` when each extra positional value has the same role and type:

```incan
def any_true(*checks: bool) -> bool:
    for check in checks:
        if check:
            return true
    return false
```

Use `**kwargs` when the API intentionally accepts an open set of same-typed named values:

```incan
def metric(name: str, value: int, **tags: str) -> int:
    return len(tags)
```

Avoid rest parameters when the names are known and required. Use ordinary parameters:

```incan
def connect(host: str, port: int) -> str:
    return f"{host}:{port}"
```

Avoid `**kwargs` when options have different types or need their own documentation. Use a model:

```incan
model RetryOptions:
    attempts: int
    backoff_ms: int

def fetch(url: str, options: RetryOptions) -> int:
    return options.attempts
```

If the repeated unit is heterogeneous, package it first and make the packaged unit variadic:

```incan
model Header:
    name: str
    value: str

def request(path: str, *headers: Header) -> int:
    return len(headers)
```

## Rest Positional Parameters

Use `*name: T` to capture extra positional arguments. Inside the function, `name` has type `List[T]`.

```incan
def sum_all(label: str, *values: int) -> int:
    mut total: int = 0
    for value in values:
        total = total + value
    return total

def main() -> int:
    return sum_all("scores", 10, 20, 30)
```

The annotation is the element type, not the container type. Write `*values: int`, not `*values: List[int]`.

Calling the function with no extra positional arguments is allowed. The binding is an empty list:

```incan
def count(*items: str) -> int:
    return len(items)

def main() -> int:
    return count()
```

## Rest Keyword Parameters

Use `**name: T` to capture unknown named arguments. Inside the function, `name` has type `Dict[str, T]`.

```incan
def request(path: str, **headers: str) -> int:
    return len(headers)

def main() -> int:
    return request("/status", accept="json", trace="enabled")
```

The keys are strings derived from the argument names. The annotation is the captured value type, not the container type. Write `**headers: str`, not `**headers: Dict[str, str]`.

Calling the function with no extra keyword arguments is allowed. The binding is an empty dictionary:

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
- Normal parameters must appear before any rest parameter.
- `*name: T`, when present, appears after normal parameters.
- `**name: T`, when present, must be the last parameter.
- Rest parameters cannot have default values.

Valid:

```incan
def ok(a: int, b: int, *rest: int, **opts: str) -> int:
    return a + b + len(rest) + len(opts)
```

Invalid:

```incan
def bad_order(*rest: int, value: int) -> int:
    return value

def also_bad(**opts: str, *rest: int) -> int:
    return len(rest) + len(opts)
```

## Call-Site Unpacking

Use `*expr` at a call site to extend the callee's positional rest parameter from an existing list-like value:

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

The unpacked expression must typecheck as `List[T]` for the callee's `*name: T` parameter.

Use `**expr` to extend the callee's keyword rest parameter from an existing dictionary:

```incan
def request(path: str, **headers: str) -> int:
    return len(headers)

def main() -> int:
    defaults = {"accept": "json"}
    return request("/status", **defaults, trace="enabled")
```

The unpacked expression must typecheck as `Dict[str, T]` for the callee's `**name: T` parameter.

The implemented subset is currently rest-directed: the callee must declare the matching rest parameter.

```incan
def fixed(a: int, b: int) -> int:
    return a + b

def needs_rest(*values: int) -> int:
    return len(values)

def main() -> int:
    xs = [1, 2]
    ok = needs_rest(*xs)
    # fixed(*xs) is rejected because fixed has no *values rest parameter.
    return ok
```

RFC 038 also owns the full fixed-parameter unpacking design. That design is not a separate RFC: fixed-parameter unpacking must prove the unpacked value's length, key set, duplicate bindings, defaults, and per-field types before the compiler can lower the call.

This syntax is only for function-call arguments. It does not add collection-literal spread:

- Use `f(*xs)` to unpack a list-like value into a positional rest parameter.
- Use `f(**kw)` to unpack a dictionary into a keyword rest parameter.
- `[*xs]`, `{**kw}`, and `[**xs]` are not part of this feature.

If collection-literal spread is added later, `[*xs]` would be the list-spread form and `{**kw}` would be the dictionary-spread form. `[**xs]` should still be invalid because `**` is mapping or keyword unpacking, not sequence unpacking.

## Source Order and Duplicate Keys

Positional rest values preserve source order. This call:

```incan
sum_all(1, *extra, 4)
```

builds a rest list equivalent to:

```incan
[1] + extra + [4]
```

Keyword rest values are inserted into a dictionary in source order. Duplicate direct named arguments are rejected, but a duplicate key that arrives through `**dict_value` follows ordinary dictionary insertion behavior: later entries replace earlier entries.

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

Named functions are first-class values. When a function value originates from a rest-aware function, the callable metadata preserves the rest markers, so direct rest arguments and unpack arguments still work through the variable:

```incan
def collect(prefix: str, *items: int, **labels: str) -> int:
    return len(items) + len(labels)

def main() -> int:
    f = collect
    xs = [1, 2]
    labels = {"kind": "demo"}
    return f("event", 0, *xs, **labels)
```

A plain fixed-arity function type does not become rest-aware just because one of its parameters is a list or dictionary. Rest behavior comes from rest metadata, not from trailing container types alone.

## Lowering Model

Rest parameters are compile-time sugar over explicit container parameters:

- `*items: T` lowers to a trailing `List[T]` parameter.
- `**labels: T` lowers to a trailing `Dict[str, T]` parameter.
- Direct rest arguments are pushed into the generated list or inserted into the generated dictionary.
- `*expr` extends the generated list.
- `**expr` extends the generated dictionary.

For example:

```incan
collect("event", 1, *xs, kind="demo", **labels)
```

lowers conceptually to a call with explicit rest containers:

```incan
collect("event", [1] + xs, {"kind": "demo"} + labels)
```

The emitted Rust uses ordinary `Vec` and `HashMap` construction; it does not use runtime reflection or Rust variadics.

## Type Errors

The compiler reports errors for these cases:

- Extra positional arguments without `*args`.
- Unknown named arguments without `**kwargs`.
- `*expr` when the callee has no positional rest parameter.
- `**expr` when the callee has no keyword rest parameter.
- A direct rest argument whose type is incompatible with the rest element type.
- A `*expr` argument whose type is incompatible with `List[T]`.
- A direct keyword rest value whose type is incompatible with the rest value type.
- A `**expr` argument whose type is incompatible with `Dict[str, T]`.
- Duplicate direct named arguments.
- Missing required normal parameters.

## Rust Interop

Rest syntax is an Incan call contract. It does not expose C-style variadics and does not automatically apply to arbitrary Rust functions. Rust-backed calls can participate only when the compiler has an Incan-level callable signature that marks the relevant parameter as positional rest or keyword rest.
