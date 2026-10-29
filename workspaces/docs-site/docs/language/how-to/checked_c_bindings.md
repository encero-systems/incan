# Work with checked C bindings

Keep the raw foreign boundary private and expose ordinary Incan functions to application code.

## Create the source layout

Create an ordinary manifest as in [Getting started](../../tooling/tutorials/getting_started.md). Put binding declarations under `src/native/`, private raw calls in a bridge module, and public Incan functions in a facade.

## Declare exact native types

Import the vocabulary in the declaring module:

```incan
from std.interop import c

binding LibC:
    header = "stdlib.h"
    link = c.system_library("c")

    symbol absolute(value: c.i32) -> c.i32:
        native = "abs"
```

Use the C scalar type declared by the header. Fixed-width values and platform C types are different contracts. Do not replace unsupported pointers, views or ownership with integers merely to get a declaration accepted.

## Wrap the raw call

```incan
def absolute_bridge(value: int) -> int:
    """Call the checked native symbol at one private boundary."""
    unsafe:
        return LibC.absolute(value)

pub def magnitude(value: int) -> int:
    """Return the native absolute value."""
    return absolute_bridge(value)
```

The raw bridge stays private. Public callers use the ordinary function. For resources and initialized outputs, follow the ownership and outcome rules in [`std.interop`](../reference/stdlib/interop.md).

## Declare package headers

Package-owned header paths are project-relative:

```toml
[interop.c]

[[interop.c.targets]]
target = "aarch64-apple-darwin"
headers = ["interop/include/bridge.h"]
```

Check the binding source:

```sh
incan check src/main.incn --format json
```

A successful check verifies the supported source contract; it does not establish an Android or iOS package. Dev.8 does not accept `--interop-target`, `inspect interop-plan`, interop bake or stage commands.

## Diagnose a refusal

Compare the source's native symbol, scalar types, field order and resource contracts with the header. Keep errors explicit at the facade boundary. Missing native artifacts require a compatible installed closure; there is no automatic package or SDK discovery.
