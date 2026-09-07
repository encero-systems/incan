# Work with checked C bindings

Use this guide when you already understand the first checked C binding tutorial and need to model a small C header, keep the raw boundary private, or diagnose a rejected declaration.

## Keep the binding local and expose a façade

Do not publish a raw binding merely because it has a useful native name. Leave the binding unexported, keep the direct call in a private bridge, and export only ordinary Incan functions or models that give the C API application meaning:

```incan
from std.interop import c

binding LibC:
    header = "stdlib.h"
    link = c.system_library("c")

    symbol absolute(value: c.i32) -> c.i32:
        native = "abs"

def absolute_bridge(value: int) -> int:
    unsafe:
        return LibC.absolute(value)

pub def magnitude(value: int) -> int:
    return absolute_bridge(value)
```

The `binding` vocabulary desugars to a `@c.binding(...)` declaration class extending `BindingDeclaration`. That is an implementation detail with a useful consequence: normal module visibility rules still apply, and no special compiler rule turns every native name into public API.

The compiler emits one advisory when a `pub def` directly contains a checked C call. It does not reject that low-level form, but a package that promises a safe API should follow the private-bridge shape above. A declaration-only binding package remains supported because it has no public direct raw call to warn about.

## Start with the binding-package scaffold

Create an ordinary project with `incan new`, then keep the C boundary in the following source layout. The paths are a recommended scaffold, not magic package metadata: imports and the checked descriptor remain authoritative.

```text
src/
  native/
    contract.incn  # `binding` declarations only
    bridge.incn    # private `unsafe:` raw calls and boundary conversion
    api.incn       # public Incan models, errors, and facades
  main.incn        # imports the public API only
interop/
  include/         # package-owned headers
  src/             # governed C/C++ shims when needed
  lib/             # declared package-owned static or bundled artifacts
```

Keep public application code out of `contract.incn` and `bridge.incn`. Put target files under `[interop.c]`, run `incan lock`, and use the binding-use receipt when an audit needs to join checked raw use to a selected target. This layout supports intentionally low-level packages too: they may expose the contract deliberately, but should document that they do not provide a safe facade.

## Choose exact C types

Use the C namespace in the raw declaration, even when a type looks similar to Incan `int`:

| C declaration need | Binding type |
| --- | --- |
| Exact signed or unsigned width | `c.i8` through `c.i64`, or `c.u8` through `c.u64`; the matching Incan carrier is the same-width numeric type |
| Target-verified 128-bit extension | `c.i128` or `c.u128`; available only when the selected Clang target accepts `__int128` |
| Exact binary float | `c.f32` or `c.f64`; the matching Incan carrier is `f32` or `f64` |
| C `int` | `c.c_int` |
| C `char` | `c.c_char` |
| Target-sized byte count | `c.Size`, carried as Incan `usize` |
| Required immutable or mutable pointer | `c.ConstPtr[T]` or `c.MutPtr[T]` |
| Nullable pointer | `Option[c.ConstPtr[T]]` or `Option[c.MutPtr[T]]` |
| NUL-terminated text input | `c.cstr(value)?`, then `text.as_const_ptr()` inside `unsafe:` for an exact `c.ConstPtr[c.c_char]` parameter |
| Temporary `const char *` text result | `view.copy_utf8(max_bytes=<positive int>)` inside the same `unsafe:` region |
| Opaque resource consumed by the native call | `c.Owned[Handle]` |
| Call-scoped shared or exclusive resource access | `c.Borrowed[Handle]` or `c.BorrowedMut[Handle]` |
| Native-written or caller-initialized output storage | `c.Out[T]` or `c.InOut[T]` |

The executable subset admits exact scalar calls, opaque resource calls, scalar or owned-resource output positions, bounded typed spans, caller-owned output buffers, and the bounded text bridge. Pointer and by-value structure declarations are still useful because the compiler verifies their declared shape, but calls that would require pointer arithmetic, arbitrary dereference, a raw view that can escape, or an unimplemented representation rule remain rejected.

## Pass a bounded caller-owned buffer

Use a typed span only where the binding declares the matching pointer and count or capacity parameter through `bounds`. The span owns no raw pointer in source: it produces the paired argument from one compiler-tracked allocation, and a mutable span returns the original allocation only after a native-written count is within the declared capacity.

```incan
binding Vector:
    header = "vector.h"
    link = c.system_library("vector")

    symbol sum(values: c.ConstPtr[c.f32], value_count: c.Size) -> c.f32:
        native = "vector_sum_f32"
        bounds = { values: value_count }

    symbol fill(destination: c.MutPtr[c.f32], destination_capacity: c.Size) -> c.Size:
        native = "vector_fill_f32"
        bounds = { destination: destination_capacity }

def sum(values: list[f32]) -> f32:
    source = c.f32_span(values)
    unsafe:
        return Vector.sum(source.as_const_ptr(), source.element_count())

def fill() -> Result[list[f32], str]:
    mut destination = c.mutable_f32_span([0.0, 0.0, 0.0])
    unsafe:
        written = Vector.fill(destination.as_mut_ptr(), destination.element_capacity())
        return destination.into_f32s(written)
```

Use `c.bytes_span(value)` and `c.mutable_bytes_span(value)` for a `c.u8` buffer. The byte-length methods are named `byte_length`, `byte_capacity`, and `into_bytes`; the `f32` methods use element counts. Mixing a span pointer with the count from another allocation, omitting the declared pairing, reusing a mutable span, or allowing it to escape the `unsafe:` region is rejected. The current bounded representation is intentionally byte and `f32` only; it is not a generic raw-pointer escape hatch.

## Associate one release operation with an opaque resource

Declare a resource once, name the exact native opaque type, and associate it with the binding symbol that consumes it:

```incan
binding Fixture:
    header = "fixture.h"
    link = c.system_library("fixture")

    resource Handle:
        native = "fixture_handle"
        release = close

    symbol close(handle: c.Owned[Handle]) -> None:
        native = "fixture_close"

    symbol inspect(handle: c.Borrowed[Handle]) -> c.i32:
        native = "fixture_inspect"
```

`c.Owned[Handle]` moves into `close`, so use after that call is a type error and generated Rust disarms the last-resort guard before invoking the native release function. `c.Borrowed[Handle]` and `c.BorrowedMut[Handle]` are selected from the parameter declaration at the call site; wrapper authors do not write Rust-shaped borrow calls. A mutable local is required for `c.BorrowedMut[Handle]`.

## Use output positions only through compiler-managed storage

The declaration owns the pointer level, while the bridge owns only the ordinary slot value:

```incan
binding Fixture:
    enum Status:
        OK: c.i32 = FIXTURE_OK

    symbol open(output: c.Out[c.Owned[Handle]], attempts: c.InOut[c.i32]) -> c.i32:
        native = "fixture_open"

        outcome Status.OK:
            initializes = [output]
            updates = [attempts]
```

```incan
unsafe:
    output = c.out[c.Owned[Handle]]()
    attempts = c.inout(0)
    status = Fixture.open(output, attempts)
    if status == Fixture.Status.OK:
        handle = output.take()
        Fixture.close(handle)
    updated_attempts = attempts.take()
```

`c.Out[...]` can be read only on an outcome that declares it initialized. `c.InOut[...]` begins initialized and is readable after a call unless an outcome explicitly invalidates it. Neither slot can be returned from a safe façade or reused for a second raw call.

## Convert text at the boundary, not to a raw pointer

Use `c.cstr(value)?` when a checked declaration takes exactly `c.ConstPtr[c.c_char]`. It rejects interior NUL values and the compiler keeps its private storage alive across the raw call:

```incan
def sqlite_version() -> Result[str, str]:
    unsafe:
        view = SQLite.library_version()
        return view.copy_utf8(max_bytes=256)
```

A `c.ConstPtr[c.c_char]` result is a scoped view. It has no public pointer API: call `copy_utf8(max_bytes=...)` immediately inside the same `unsafe:` region to receive an owned `str`. The argument must be named and positive, and the conversion validates a terminator and UTF-8 within that bound. Do not return, store, capture, or forward `view`; spans, mutable buffers, and zero-copy returned views are not part of this foundation.

## Declare constants and a plain layout

Use a binding enum for a C macro or named constant. Every variant uses the same explicit C scalar carrier:

```incan
binding Fixture:
    header = "fixture.h"
    link = c.system_library("fixture")

    enum Status:
        OK: c.i32 = FIXTURE_OK
        Retry: c.i32 = FIXTURE_RETRY
```

Use a binding structure only for a named plain C layout whose native spelling and fields you can state exactly:

```incan
binding Fixture:
    header = "fixture.h"
    link = c.system_library("fixture")

    struct Pair:
        native = "fixture_pair"
        left: c.i32 = left
        right: c.i32 = right
```

Clang checks each requested field offset, size, and alignment for the selected host target. It does not infer omitted fields or discover a structure from the header. By-value structure and pointer calls are deliberately unavailable in this slice, so do not use a structure declaration as an assertion that you can already pass it across the boundary.

## Freeze Oven interop requirements for a target

The binding remains the authority for the Incan-facing ABI. When a package needs physical interop inputs, declare its target-specific requirements under `[interop.c]` in the package's `loaf.toml`. The declaration names only package-owned files and compatible toolchain or SDK capabilities; it never claims that Oven has selected a local installation or asks the compiler to search the host for headers, libraries, or a C++ installation.

```toml title="loaf.toml"
[interop.c]
schema = 1

[[interop.c.targets]]
target = "aarch64-apple-ios"
toolchain = { capability = "apple-clang", version = ">=17, <18" }
sdk = { capability = "iphoneos", version = ">=18, <19" }
headers = ["interop/include/bridge.h"]
definitions = ["FIXTURE=1"]

[interop.c.targets.platform]
kind = "ios"
deployment-target = "13.0"

[[interop.c.targets.artifacts]]
name = "fixture"
kind = "static"
path = "interop/lib/libfixture.a"
origin = { source = "https://example.invalid/fixture", revision = "v1.0.0", license = "MIT" }

[[interop.c.targets.artifacts]]
name = "foundation"
kind = "system"
capability = "apple.framework.Foundation"

[[interop.c.targets.shims]]
name = "fixture_bridge"
language = "c"
sources = ["interop/src/bridge.c"]
headers = ["interop/include/bridge.h"]
output = "fixture_bridge"
```

`static` artifacts name a package-owned archive. `bundled` artifacts name a package-owned dynamic library or framework and must also specify its `runtime-name`, `placement`, and `minimum-platform`. Static and bundled artifacts may record an upstream HTTPS source, exact revision, and license in `origin`; that provenance is sealed with the selected plan, but it is not a download instruction or publication admission. `system` artifacts instead name a required toolchain or SDK capability. Shims may be authored in C or C++, but Oven exposes C++ only behind the shim's bounded C contract. The explicit Oven Alpha baker seals locked static inputs and selected C/C++ shim output archives into one direct-`rustc` plan, retaining bundled artifacts and selected system/framework capabilities in its immutable provenance.

The target triple still names the CPU and operating-system identity. A mobile `platform` table supplies the additional version constraint that later ABI verification and a platform packager require. `kind = "android"` is valid only for `aarch64-linux-android`; it requires the `android` SDK capability and an `api-level` of 21 or later. `kind = "ios"` is valid only for `aarch64-apple-ios`; it requires the `iphoneos` SDK capability and a numeric `major.minor` `deployment-target`. These are package requirements, not paths or a record of the concrete SDK that Oven has selected.

Every declared package file must be a regular, normalized relative path. Running `incan lock` hashes the exact header, artifact, and shim-source bytes into the semantic lock state with the target, compatibility requirements, definitions, and capability requirements. Changing any declared input makes the lock stale; relocating an unchanged package does not change these package-relative entries.

The locked mobile profile records a package target constraint, not a local-toolchain selection. It preserves the Android API level or iOS deployment target that the explicit Oven selection must use without embedding an NDK directory, Xcode path, Gradle configuration, or signing credential in the package.

## Check a declared mobile ABI target

`incan check` verifies checked C declarations against the compiler host by default. Pass `--interop-target` to select exactly one target declared by the current package instead:

```sh
INCAN_C_ABI_CLANG=/path/to/aarch64-linux-android34-clang \
  incan check --interop-target aarch64-linux-android .
```

The selected target's `definitions` are passed to both the signature/layout probe and the enum-value probe. The command rejects a target that is not declared by the package. It also keeps the boundary narrow: this checks the source-owned C ABI against that target profile, but it does not cross-compile generated Rust, link declared artifacts, build shims, stage a mobile package, or attest that a compatible `toolchain` or `sdk` requirement matches an installed binary. `INCAN_C_ABI_CLANG` provisions the executable for this invocation; it does not replace the manifest as ABI or target authority.

## Inspect a locked platform handoff

After locking, inspect the same target requirements as a deterministic platform handoff:

```sh
incan inspect interop-plan --target aarch64-linux-android --format json
```

The plan gives an Oven, Gradle, or Xcode adapter consistent target, artifact, shim, and placement facts without freezing either adapter's task protocol. It is not an Oven resolution receipt or a deployable application: it contains no local SDK path, selected compiler executable, generated artifact, signing identity, licence admission, or credential.

Use the explicit v0.5 baker after the package lock is current. First materialize the package's sealed base runtime Loafs; this writes the release receipt without selecting an interop execution receipt, so it is the required first half of an interop bake:

```sh
incan oven bake --project .
```

Then bake the checked package inputs against that release receipt:

```sh
incan oven interop bake \
  --project . \
  --target aarch64-linux-android \
  --base-receipt .incan/oven/receipt.json \
  --c-compiler /path/to/aarch64-linux-android-clang \
  --archiver /path/to/llvm-ar
```

The baker does not download artifacts, discover a system library, or define a Gradle/Xcode handover protocol. It verifies locked package-owned inputs, compiles C/C++ shims, seals static link archives, and retains bundled runtime files, declared artifact provenance, and explicit system capabilities in its selected receipt-bound plan. A subsequent `incan oven interop stage` creates only a fixed consumer layout; final application assembly, signing, provenance admission, and license policy remain outside this command boundary.

## Interpret common failures

| Failure | What to check |
| --- | --- |
| `@c.binding requires from std.interop import c` | Import `c` in the declaring module. An alias is allowed; a global activation is not. |
| C symbol has an unsupported parameter or return type | Use the current scalar, opaque-resource, or output forms. Do not weaken an unsupported pointer or view contract into an integer. |
| Clang rejects the signature or layout | Compare the header's exact spelling, calling shape, field order, and scalar category with the binding. Do not change the declaration to make generated Rust compile. |
| Native enum carrier mismatch | Keep one declared `c.*` carrier for all variants and verify what the header exposes after macro expansion. |
| `take()` is rejected | For `Out`, guard the read with the binding outcome that names the initialized parameter. For `InOut`, ensure the selected outcome has not invalidated the slot. |
| C resource was transferred or requires a mutable borrow | Do not reuse a resource passed as `c.Owned[...]`; bind it as `mut` before a call declared `c.BorrowedMut[...]`. |
| `copy_utf8` is rejected | Keep it inside the owning `unsafe:` region and write a named positive bound, for example `view.copy_utf8(max_bytes=4096)`. |
| Missing system library at final link | `c.system_library("name")` records a logical system capability; this slice does not download, vendor, or lock a library for you. |

## Review the checked declaration

Run `incan inspect bindings` after the declaration checks successfully to review the compiler-owned binding contract without reading generated Rust. The text report is suitable for a human review; `--format json` emits a schema-versioned projection for tools.

The command inspects the selected source graph, so pass the same feature and SDK-profile options used by the build when declarations are conditional. Follow the [binding inspection how-to](../../tooling/how-to/inspect_checked_c_bindings.md) for entrypoint selection, JSON use, and failure handling.

## Decide whether C is the right boundary

Choose this surface when the library's supported boundary is a compact C ABI and the part you need fits the verified scalar and opaque-resource subset. Prefer [Rust interop](rust_interop.md) when a maintained Rust crate already offers the safe, resource, callback, or asynchronous API you need. A C ABI may still be the right eventual boundary for a library implemented in another language; the implementation language is not the deciding factor.

If the header depends on callbacks, variadics, unions, bitfields, macros that cannot be represented as constants, or nontrivial lifetime rules, do not fake a scalar declaration. A checked C or C++ shim is the intended later adapter; it is not available in this first release slice.

See [how checked C interop is structured](../explanation/checked_c_interop.md) for the source-of-truth and toolchain boundary, the [binding inspection JSON schema](../../tooling/reference/binding_inspection_schema.md) for tool integration, and the [`std.interop` reference](../reference/stdlib/interop.md) for precise accepted syntax.
