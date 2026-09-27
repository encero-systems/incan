# Functions and calls explained

This page explains the ideas behind rest parameters, call-site unpacking and parameter defaults. The exact rules are in the [Functions and calls reference](../reference/functions.md).

## Rest parameters

Rest parameters are for APIs that accept "zero or more of the same kind of thing." The call site stays convenient:

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
    - Python can unpack any iterable or mapping at runtime; Incan only unpacks into fixed parameters when the length or key set is known from the unpacked expression itself.
    - `*expr` means "positional expansion" and is valid in calls and list literals. `**expr` means "mapping expansion" and is valid in calls and dictionary literals.

### When to use rest parameters

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

The annotation names one element, not the collection: `*values: int` collects a `List[int]`, and `*values: List[int]` collects a list of lists. The same holds for `**labels: str`, which collects a `Dict[str, str]`.

If the repeated unit is heterogeneous, package it first and make the packaged unit variadic:

```incan
model Header:
    name: str
    value: str

def request(path: str, *headers: Header) -> int:
    return len(headers)
```

### What a rest call means

Rest parameters are sugar over explicit container parameters. `*items: T` is a trailing `List[T]` parameter and `**labels: T` a trailing `Dict[str, T]` parameter. A call builds those containers in source order: direct rest arguments are appended, `*expr` extends the list and `**expr` extends the dictionary. This call:

```incan
collect("event", 1, *xs, kind="demo", **labels)
```

means a call with explicit containers:

```text
collect("event", [1] + xs, <dict containing "kind": "demo" plus labels inserted in source order>)
```

The generated Rust builds those containers with ordinary `Vec` and `HashMap` construction. Nothing is inspected at runtime to decide how arguments bind, and rest parameters are not the variadics of C or Rust. That is also why a function type that merely ends in a list or dictionary parameter is not rest-aware: the rest markers belong to the declaration, and a function value made from that declaration keeps them.

### Unpacking into fixed parameters

Unpacking fills ordinary fixed parameters only when the unpacked expression shows its own shape. A value of type `List[T]` is a homogeneous list whose length is not part of its type, so it can feed a `*args` rest parameter but cannot fill a fixed pair such as `fixed(x: int, y: int)`; a tuple expression or an inline list literal such as `fixed(*[1, 2])` can. Likewise a `Dict[str, T]` value can feed `**kwargs` but cannot prove that every fixed keyword parameter is present, while an inline dictionary literal with string literal keys can.

## Aliases and presets for functions

For module-level alternate names such as `mean = avg`, use a [symbol alias](../reference/symbol_aliases.md) rather than a function-local value binding: an alias is a declaration, takes part in imports and exports, and keeps its identity in library metadata. For callable specializations such as `get = partial route(method="GET")`, use a [callable preset](callable_presets.md), whose presets behave like ordinary defaults.

## Defaults across a package boundary

A default is evaluated as if it were written in the module that declares the callable, so it may name that module's private consts, functions and types. A caller in another module of the same package receives such a default when it leaves the parameter unbound.

A caller in another package has only what the package publishes about its defaults. A default reaches that caller when the package can describe it as a value the caller's own program can build: a literal, a list or dictionary of such values, the name of a const, static or function, an enum variant without a payload, a call of a function the package declares, or a construction of a newtype the package declares or of a model or class it exports. Any other default leaves the parameter required for that caller, which must then pass the argument:

- an operator expression such as `"fl" + "at"`, a negated const or an f-string;
- a call of a builtin such as `abs(-3)` or `len("abc")`, `Some(3)`, or a call of a stdlib or other-package function;
- a call of a partial, whose presets the caller would have to apply;
- an enum variant with a payload such as `Shape.Circle(4)`, or a static method call;
- a construction of a private model or class, which the caller cannot name;
- the presets of a method partial.

Writing such a default is still valid; only callers in other packages must pass the argument.
