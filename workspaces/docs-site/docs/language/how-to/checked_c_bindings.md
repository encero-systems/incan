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

A successful check verifies the supported source contract against the host; it does not establish an Android or iOS package.

## Verify against a declared target

Declare what another target requires next to its headers: the compatible toolchain and SDK capabilities, the preprocessor definitions, and, for Android or iOS, the platform's version fact:

```toml
[[interop.c.targets]]
target = "aarch64-apple-ios"
toolchain = { capability = "apple-clang", version = ">=17" }
sdk = { capability = "iphoneos", version = ">=18" }
platform = { kind = "ios", deployment-target = "13.0" }
headers = ["interop/include/bridge.h"]
definitions = ["BRIDGE_MOBILE=1"]
```

Check the bindings against that target instead of the host:

```sh
incan check --interop-target aarch64-apple-ios src/main.incn
```

The check verifies each binding's signatures, layouts and enum values with Clang for that target and its definitions; a layout that differs on that target is refused. It refuses a target the package does not declare, and a `toolchain` or `sdk` requirement the selected C toolchain or SDK does not meet, by name. On macOS the selected toolchain is Xcode's Clang with the target's SDK; `INCAN_C_ABI_CLANG` selects another Clang explicitly, as an Android NDK Clang for `aarch64-linux-android`, whose NDK supplies the `android-ndk` toolchain and `android` SDK capabilities. `build`, `test` and `run` accept the same option; the program is still built for the host, and nothing is cross-compiled.

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

## Stage bundled libraries for Android or iOS

Declare a dynamic library or framework the application ships as a `bundled` artifact of its mobile target:

```toml
[[interop.c.targets]]
target = "aarch64-linux-android"
sdk = { capability = "android" }
platform = { kind = "android", api-level = 24 }

[[interop.c.targets.artifacts]]
name = "bridge_runtime"
kind = "bundled"
path = "interop/android/libbridge_runtime.so"
runtime-name = "libbridge_runtime.so"
placement = "jniLibs/arm64-v8a"
minimum-platform = "24"
```

Stage it for the platform's packager:

```sh
oven interop stage --target aarch64-linux-android --adapter android --output build/android-stage
```

The new directory holds `jniLibs/arm64-v8a/libbridge_runtime.so` and `incan-interop-adapter.json`, which records each file's digest and declared placement; `--adapter ios` stages `Frameworks/<runtime-name>` for `aarch64-apple-ios` or `aarch64-apple-ios-sim`. Point Gradle or Xcode at that directory. Staging refuses an existing output directory, so stage into a new one each time.

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
