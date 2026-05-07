# Callable presets

This page is the reference for callable presets created with `partial`.

Callable presets create a callable surface from an existing callable by supplying named preset values. The projected callable keeps the target return type and exposes preset parameters as ordinary defaulted parameters.

For the mental model and examples, see [Callable presets explained](../explanation/callable_presets.md).

!!! tip "Coming from Python?"
    Callable presets are closest to `functools.partial`, but the contract is stricter.

    - Presets are keyword-only.
    - Preset parameters remain overrideable at the call site.
    - Top-level presets are declarations, not runtime module import work.
    - Public presets are exported as preset metadata, not as opaque function objects.

## Syntax

Top-level declaration:

```incan
pub get = partial route(method="GET")
```

Same-type method declaration:

```incan
model Cell:
    alive: bool

    def set_state(mut self, state: bool) -> None:
        self.alive = state

    set_alive = partial set_state(state=true)
```

Local expression:

```incan
def make_reader(layer: str) -> Callable[[str], Reader]:
    return partial Reader(layer=layer)
```

The explicit grammar shape is:

```text
partial Target(keyword=value, ...)
```

Only named preset arguments are valid. Positional presets are rejected.

## Supported targets

Top-level partial declarations support statically known callable targets:

- functions;
- aliases and partials that resolve to supported callable targets;
- model constructors;
- class constructors;
- newtype constructors.

Method partial declarations are same-type only. They may target another method declared on the same model, class, trait, or newtype.

Local partial expressions support callable targets whose surface can be resolved at the expression site.

## Signature projection

A partial projects the target callable signature:

- unfilled required parameters remain required;
- unfilled defaulted parameters remain defaulted;
- preset parameters become defaulted parameters on the projected callable;
- the return type is the target return type;
- async status follows the target callable;
- receiver kind follows the target method for method partials.

Preset parameters are defaults, not frozen arguments. A call may override a preset by supplying the same keyword:

```incan
pub get = partial route(method="GET")

def submit() -> str:
    return get(method="POST", path="/submit")
```

Default display uses the same visual model as ordinary callable defaults. Tooling may retain separate preset provenance in metadata, but the callable signature does not introduce a separate "partial-only" parameter category.

## Top-level preset values

Top-level partial declarations do not execute the target call during module initialization. Preset values must be declaration-safe so the compiler can represent them without running user code.

Accepted preset value shapes include:

- scalar literals;
- string and bytes literals;
- `None`;
- symbol paths that can be represented as static preset metadata;
- list literals whose elements are declaration-safe;
- dictionary literals whose keys and values are declaration-safe;
- model literals whose field values are declaration-safe.

Rejected top-level preset value shapes include:

- function calls;
- closures;
- comprehensions;
- mutation;
- I/O;
- async operations;
- spread entries in collection literals;
- positional or unpacked model literal arguments.

Local partial expressions are different: their preset expressions evaluate under ordinary runtime expression rules when the partial expression is evaluated.

## Method and trait presets

A method partial creates another method on the same type surface. The generated method keeps the target receiver and return type.

Trait method partials behave like generated trait default methods. They forward to another same-trait method with the preset keyword values applied, and concrete adopters receive the default through the ordinary trait-default expansion path.

## Public API metadata

Public top-level partial declarations are exported as partial metadata. The export records:

- partial name;
- target path;
- target kind;
- preset names, types, and serializable preset values;
- projected callable parameters;
- return type;
- async status.

Public partials import as callable symbols for consumers, while manifests and checked API metadata still preserve their identity as partials.

## Diagnostics

The compiler rejects:

- empty partial templates;
- positional presets;
- duplicate preset names;
- unknown preset names;
- preset values with incompatible types;
- unsupported target kinds;
- rest-parameter targets in the implemented surface;
- top-level preset values that require runtime evaluation.

Use a [symbol alias](symbol_aliases.md) when the new name should be exactly the same callable. Use a wrapper function or method when the new callable should add behavior.
