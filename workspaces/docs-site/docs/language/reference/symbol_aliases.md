# Symbol aliases (reference)

An alias is a declaration that gives an existing declaration another name. This page specifies top-level aliases, method aliases and enum variant aliases, and how aliases are imported and re-exported.

## Top-level aliases

```text
[pub] NAME = TARGET
[pub] NAME = alias TARGET
```

- `TARGET` is a symbol path. `NAME = alias TARGET` is the same declaration as `NAME = TARGET`.
- The alias has its target's type and signature, and a call through it calls the target.
- A `pub` alias is exported. Its target is public.

Supported targets:

- a function, including an overloaded one;
- a `static`;
- a `model`, `class`, `enum`, `newtype` or type alias;
- a trait;
- an imported public symbol of one of these kinds;
- a member of a `std.*` module, a project source module or a `pub::` package namespace, written through a binding of that module: `math.sqrt` after `import std.math as math`;
- another alias of one of these, without a cycle.

An alias of a module member binds that member under the alias name, with the alias's visibility, as importing the member under that name does.

```incan
import std.math as math

pub def avg(x: int, y: int) -> int:
    return (x + y) // 2

pub mean = avg                   # accepted
pub average = alias avg          # accepted
pub root = math.sqrt             # accepted
common_divisor = math.gcd        # accepted
```

Refused (`INCAN-P0001`): a target that is not a symbol path, such as a literal or a call.

Refused (`INCAN-T0001`):

- a target that does not resolve;
- a target of another kind, such as a `const`, a field or a local value;
- an alias whose name is already declared in the module;
- a cycle of aliases;
- a `pub` alias whose target is private.

```incan
def helper(x: int) -> int:
    return x

const LIMIT: int = 10

count = 1                 # refused: not a symbol path (INCAN-P0001)
later = helper(1)         # refused: not a symbol path (INCAN-P0001)
limit = LIMIT             # refused: a const is not an alias target
left = right              # refused: alias cycle
right = left              # refused: alias cycle
pub exposed = helper      # refused: helper is private
```

## Importing and re-exporting aliases

- A public alias is imported like any public symbol: `from stats import mean`.
- `from stats import mean as average_value` binds `average_value` in the importing module; `mean` stays an alias of its target in `stats`.
- A module can re-export an alias without importing its target (`pub from stats import mean`), and a further module can re-export that under another name. A call through any of these names calls the alias's target.

```incan
# stats.incn
pub def avg(x: int, y: int) -> int:
    return (x + y) // 2

pub mean = avg
```

```incan
# facade.incn
pub from stats import mean
```

```incan
# public_api.incn
pub from facade import mean as average
```

```incan
# main.incn
from facade import mean
from public_api import average

def main() -> None:
    println(mean(10, 20))        # accepted: calls avg
    println(average(2, 4))       # accepted: calls avg
```

## Method aliases

```text
NAME = METHOD
NAME = alias METHOD
```

- A method alias appears in a `model`, `class`, `trait` or `newtype` body and names a method of the same type.
- The alias has the target method's receiver, parameters, return type, async status, generic parameters and overloads. A call through it calls the target.

Refused (`INCAN-T0001`): a target that is not a method of the same type, such as a field, a free function or a method of another type, and a cycle of method aliases.

```incan
def helper() -> int:
    return 1

model Reading:
    value: int
    mean = avg                   # accepted
    v = value                    # refused: value is a field
    h = helper                   # refused: helper is not a method of Reading

    def avg(self) -> int:
        return self.value
```

## Enum variant aliases

```text
NAME = alias VARIANT
```

- A variant alias in an `enum` body names a variant of that enum. It adds no variant.
- In an enum that declares values, the alias has its target's value; it is not a second variant with that value.
- A variant alias can name its variant in a pattern (see [Match patterns](match_patterns.md#variant-patterns)).

Refused (`INCAN-T0001`): a target that is not a variant of the same enum.

```incan
enum Level(str):
    WARN = "WARN"
    FATAL = "FATAL"
    WARNING = alias WARN         # accepted
    CRITICAL = alias FATAL       # accepted
    SEVERE = alias PANIC         # refused: PANIC is not a variant of Level
```

## See also

- [Scopes and name resolution](../explanation/scopes_and_name_resolution.md): aliases and wrappers, and how tools and diagnostics see an alias
