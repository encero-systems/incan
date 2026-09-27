# Symbol aliases (reference)

An alias gives an existing declaration another name. It is a declaration: it creates no wrapper and copies nothing. This page specifies top-level aliases, method aliases and enum variant aliases.

## Top-level aliases

```text
[pub] NAME = TARGET
[pub] NAME = alias TARGET
```

- `TARGET` is a symbol path. The `alias` marker changes nothing.
- The alias has its target's type and signature, and a call through it calls the target.
- A `pub` alias is exported. Its target is public.
- A library exports a public alias as an alias of its target, not as a separate declaration.

Supported targets:

- a function, including an overloaded one;
- a `static`;
- a `model`, `class`, `enum`, `newtype` or type alias;
- a trait;
- an imported public symbol of one of these kinds;
- a member of a `std.*` or project source module, written through a binding of that module: `math.sqrt` after `import std.math as math`;
- another alias of one of these, without a cycle.

An alias of a module member binds that member under the alias name, with the alias's visibility, as importing the member under that name does.

```incan
import std.math as math

pub def avg(x: int, y: int) -> int:
    return (x + y) // 2

pub mean = avg                   # accepted
pub average = alias avg          # accepted
pub root = math.sqrt             # accepted: the binding of `pub from std.math import sqrt as root`
common_divisor = math.gcd        # accepted: the binding of `from std.math import gcd as common_divisor`
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
- A module can re-export an alias without its target (`pub from stats import mean`), and a further module can re-export that under another name. A call through any of these names calls the alias's target, whether or not the re-exporting module also imports the target.

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

```incan
enum Level(str):
    WARN = "WARN"
    FATAL = "FATAL"
    WARNING = alias WARN
    CRITICAL = alias FATAL
```

## Identity

- A diagnostic names the alias at its use site, and may also name its target.
- Checked API metadata and library manifests record an alias as an alias of its target.
