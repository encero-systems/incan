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

## Link a package-owned static archive

Declare a prebuilt `lib<name>.a` for the target, and relate the binding to it explicitly:

```toml
[[interop.c.targets.artifacts]]
name = "bridge"
kind = "static"
path = "interop/lib/libbridge.a"

[[interop.c.targets.bindings]]
module = ["main"]
name = "Bridge"
artifacts = ["bridge"]
```

The binding names the archive's link name with `link = c.system_library("bridge")`. A build for that target links the archive statically. See [`[interop.c]`](../../tooling/reference/project_configuration.md#interopc) for `dependencies` and `origin`.

## Build and link your own C

Declare the C library in the same `loaf.toml` with a [`[c]` facet](../../tooling/reference/project_configuration.md#c), which makes the Loaf a Clove:

```toml
[c]
library = "shim"
public-headers = ["include/shim.h"]
include = ["include"]

[[c.compile]]
sources = ["csrc/*.c"]
```

Link it from a binding by the Loaf's name:

```incan
binding Shim:
    header = "include/shim.h"
    link = c.loaf_library("my_project")

    symbol scale(value: c.i32) -> c.i32:
        native = "shim_scale"
```

Build with the host compiler:

```sh
INCAN_C_COMPILER=host incan run src/main.incn
```

The build compiles the C into `libshim.a` and links it; an unchanged library is reused. A path dependency that carries a `[c]` table is linked the same way, by its own Loaf name. `INCAN_C_COMPILER=host` is a local development override: the archive depends on the host compiler and is never published.

## Diagnose a refusal

Compare the source's native symbol, scalar types, field order and resource contracts with the header. Keep errors explicit at the facade boundary. Missing native artifacts require a compatible installed closure; there is no automatic package or SDK discovery.
