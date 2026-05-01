# Symbol aliases

This page is the reference for top-level symbol aliases and same-type method aliases.

Symbol aliases give an existing declaration another resolved name. They are declarations, not runtime assignments, and they do not create wrapper functions or copied declarations.

## Top-level aliases

Use a top-level alias when a module should expose another name for an existing callable or type-like symbol:

```incan
pub def avg(x: int, y: int) -> int:
    return (x + y) // 2

pub mean = avg
```

The alias can be called wherever the target can be called:

```incan
def main() -> int:
    return mean(10, 20)
```

The explicit spelling is equivalent:

```incan
pub average = alias avg
```

Use the explicit `alias` marker when it improves readability near other declarations. It does not change the type, visibility, lowering, or export behavior.

## Supported targets

A top-level alias target must resolve to an existing declaration symbol supported by the compiler:

- function
- model
- class
- enum
- trait
- newtype
- type alias
- imported public symbol with compatible export metadata
- another acyclic alias to one of the supported target kinds

The target is written as a symbol path, not as an arbitrary expression. These are rejected:

```incan
count = 1           # use const count = 1
later = make_avg()  # calls are not alias targets
```

## Public aliases

A public alias uses `pub`:

```incan
pub mean = avg
pub average = alias avg
```

A public alias may only expose a target that is itself public/exportable. This keeps the public API from hiding a private implementation behind a facade alias:

```incan
def avg(x: int, y: int) -> int:
    return (x + y) // 2

pub mean = avg  # rejected: avg is private
```

When a library is built, public aliases are exported as alias metadata. They are not duplicated as independent function or type declarations.

## Importing aliases

Public aliases participate in normal imports:

```incan
# stats.incn
pub def avg(x: int, y: int) -> int:
    return (x + y) // 2

pub mean = avg
```

```incan
# main.incn
from stats import mean

def main() -> int:
    return mean(10, 20)
```

Import aliases and symbol aliases are separate features and can be combined:

```incan
from stats import mean as average_value
```

`average_value` is an import-local name for the exported alias `mean`; `mean` remains an alias of `avg` in the exporting module metadata.

## Same-type method aliases

Inside a model, class, trait, or newtype body, a method alias gives an existing method another name on the same type:

```incan
model Reading:
    value: int
    mean = avg

    def avg(self) -> int:
        return self.value
```

The alias can be called like the target method:

```incan
def main() -> int:
    reading = Reading(value=10)
    return reading.mean()
```

The explicit marker is also accepted:

```incan
model Reading:
    value: int
    mean = alias avg

    def avg(self) -> int:
        return self.value
```

Method aliases are same-type only. They cannot point at a method on another type, a free function, or a field.

## Overloads and signatures

A method alias projects the target method surface. The alias keeps the target receiver, parameters, return type, async status, generic parameters, and overload set.

If the target method is overloaded, the alias exposes the same overload group under the alias name. Call resolution still uses the target signatures; the alias does not introduce another implementation body.

## Lowering and identity

Aliases preserve language-level identity but do not add runtime behavior:

- calls through a top-level function alias lower to the canonical function target;
- public top-level aliases emit backend re-exports when needed;
- calls through a method alias lower to the canonical method target;
- API metadata and library manifests record aliases as aliases;
- diagnostics report the alias name at the use site and may also name the canonical target.

## Rejected forms

The compiler rejects:

- arbitrary top-level assignment;
- targets that do not resolve;
- unsupported target kinds, such as `const`, `static`, fields, or runtime values;
- duplicate alias names;
- direct or indirect alias cycles;
- public aliases targeting private or non-exportable declarations;
- method aliases targeting missing methods;
- method alias cycles.

Examples:

```incan
left = right
right = left  # rejected: alias cycle
```

```incan
model Reading:
    value: int
    mean = avg  # rejected: avg is not declared on Reading
```

## Choosing between aliases and wrappers

Use an alias when the new name should be the same API surface as the target.

Use a wrapper function or method when the new name changes behavior, adapts parameters, adds validation, changes docs, or should appear as an independent callable:

```incan
def avg(x: int, y: int) -> int:
    return (x + y) // 2

def mean_nonzero(x: int, y: int) -> int:
    assert x != 0 and y != 0
    return avg(x, y)
```
