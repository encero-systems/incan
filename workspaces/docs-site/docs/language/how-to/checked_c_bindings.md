# Work with checked C bindings

Use this guide when you already understand the first checked C binding tutorial and need to model a small C header, keep the raw boundary private, or diagnose a rejected declaration.

## Keep the binding local and expose a façade

Do not publish a raw binding merely because it has a useful native name. Leave the binding unexported and export only ordinary Incan functions or models that give the C API application meaning:

```incan
from std.interop import c

binding LibC:
    header = "stdlib.h"
    link = c.system_library("c")

    symbol absolute(value: c.i32) -> c.i32:
        native = "abs"

pub def magnitude(value: int) -> int:
    unsafe:
        return LibC.absolute(value)
```

The `binding` vocabulary desugars to a `@c.binding(...)` declaration class extending `BindingDeclaration`. That is an implementation detail with a useful consequence: normal module visibility rules still apply, and no special compiler rule turns every native name into public API.

## Choose exact C types

Use the C namespace in the raw declaration, even when a type looks similar to Incan `int`:

- Use `c.i8` through `c.i64`, or `c.u8` through `c.u64`, for an exact signed or unsigned width.
- Use `c.c_int` for C `int` and `c.c_char` for C `char`.
- Use `c.Size` for a target-sized byte count.
- Use `c.ConstPtr[T]` or `c.MutPtr[T]` for a required immutable or mutable pointer; wrap either in `Option[...]` for a nullable pointer.
- Use `c.Owned[Handle]` for an opaque resource consumed by the native call.
- Use `c.Borrowed[Handle]` or `c.BorrowedMut[Handle]` for call-scoped shared or exclusive resource access.
- Use `c.Out[T]` or `c.InOut[T]` for native-written or caller-initialized output storage.

The executable subset admits scalar calls, opaque resource calls, scalar or owned-resource output positions, and one checked text-input path. Pointer and by-value structure declarations are still useful because the compiler verifies their declared shape, but calls that would require pointer arithmetic, arbitrary dereference, pointer returns, or unimplemented view rules remain rejected.

## Pass text to a const C character pointer

Use `c.cstr(value)?` when a C function takes a NUL-terminated `const char *`. It constructs a private temporary whose storage remains live for the enclosing raw call and rejects an interior NUL instead of silently truncating the text.

```incan
from std.interop import c

binding LibC:
    header = "string.h"
    link = c.system_library("c")

    symbol string_length(value: c.ConstPtr[c.c_char]) -> c.Size:
        native = "strlen"

def checked_length(value: str) -> Result[int, str]:
    text = c.cstr(value)?
    unsafe:
        return Ok(LibC.string_length(text.as_const_ptr()))
```

`as_const_ptr()` is the only operation that exposes this temporary, and it is accepted only inside `unsafe:` for the exact `c.ConstPtr[c.c_char]` contract. It does not produce an integer address or permit pointer arithmetic or dereference. This is input conversion only: returned strings, byte spans, mutable buffers, and scoped foreign views remain separate bounded-lifetime work.

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

## Keep the raw bridge inside an ordinary Incan façade

The binding and its façade may be authored in the same module. Put the `unsafe:` region around the small set of raw calls, then publish only functions, models, or errors built from ordinary Incan values. Do not return `c.Owned[...]`, `c.Out[...]`, raw status values, or a foreign pointer from that façade.

An owned resource may pass through any number of declared `c.Borrowed[...]` or `c.BorrowedMut[...]` calls while it stays in the façade. The compiler emits the matching Rust borrow for each call and retains the release guard. If a façade needs deterministic early release, pass the resource to its declared `c.Owned[...]` release symbol; otherwise the guard releases it once on normal scope exit or an early return. No separate `with`-style cleanup surface is required for this compiler-managed lifetime.

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

The binding remains the authority for the Incan-facing ABI. When a package needs physical interop inputs, declare its target-specific requirements under `[oven.interop]` in the package's `incan.toml`. The declaration names only package-owned files and compatible toolchain or SDK capabilities; it never claims that Oven has selected a local installation or asks the compiler to search the host for headers, libraries, or a C++ installation.

```toml title="incan.toml"
[oven.interop]
schema = 1

[[oven.interop.targets]]
target = "aarch64-apple-ios"
toolchain = { capability = "apple-clang", version = ">=17, <18" }
sdk = { capability = "iphoneos", version = ">=18, <19" }
headers = ["interop/include/bridge.h"]
definitions = ["FIXTURE=1"]

[[oven.interop.targets.artifacts]]
name = "fixture"
kind = "static"
path = "interop/lib/libfixture.a"

[[oven.interop.targets.artifacts]]
name = "foundation"
kind = "system"
capability = "apple.framework.Foundation"

[[oven.interop.targets.shims]]
name = "fixture_bridge"
language = "c"
sources = ["interop/src/bridge.c"]
headers = ["interop/include/bridge.h"]
output = "fixture_bridge"
```

`static` artifacts name a package-owned archive. `bundled` artifacts name a package-owned dynamic library or framework and must also specify its `runtime-name`, `placement`, and `minimum-platform`. `system` artifacts instead name a required toolchain or SDK capability. Shims may be authored in C or C++, but Oven will expose C++ only behind the shim's bounded C contract.

Every declared package file must be a regular, normalized relative path. Running `incan lock` hashes the exact header, artifact, and shim-source bytes into the semantic lock state with the target, compatibility requirements, definitions, and capability requirements. Changing any declared input makes the lock stale; relocating an unchanged package does not change these package-relative entries.

This declaration and lock slice deliberately does not download artifacts, discover a system library, compile a shim, or define a Gradle/Xcode handover format. Oven will resolve the requirements, select concrete compiler and SDK installations, build shims, cache outputs, and record those choices in its own receipt or store. Do not put signing, provenance admission, or license policy here: publication policy belongs to `incan.pub`.

## Interpret common failures

- If `@c.binding requires from std.interop import c`, import `c` in the declaring module. An alias is allowed; global activation is not.
- If a C symbol has an unsupported parameter or return type, use the current scalar, opaque-resource, or output forms. Do not weaken an unsupported pointer or view contract into an integer.
- If Clang rejects a signature or layout, compare the header's exact spelling, calling shape, field order, and scalar category with the binding. Do not change the declaration merely to make generated Rust compile.
- If an enum carrier mismatches, keep one declared `c.*` carrier for all variants and verify the header's macro-expanded value.
- If `take()` is rejected for `Out`, guard the read with the binding outcome that initializes the parameter. For `InOut`, ensure the selected outcome has not invalidated the slot.
- If a C resource was transferred or requires a mutable borrow, do not reuse a resource passed as `c.Owned[...]`; bind it as `mut` before a call declared `c.BorrowedMut[...]`.
- If the final link misses a system library, remember that `c.system_library("name")` records a logical system capability; this slice does not download, vendor, or lock a library for you.

## Review the checked declaration

Run `incan inspect bindings` after the declaration checks successfully to review the compiler-owned binding contract without reading generated Rust. The text report is suitable for a human review; `--format json` emits a schema-versioned projection for tools.

The command inspects the selected source graph, so pass the same feature and SDK-profile options used by the build when declarations are conditional. Follow the [binding inspection how-to](../../tooling/how-to/inspect_checked_c_bindings.md) for entrypoint selection, JSON use, and failure handling.

## Decide whether C is the right boundary

Choose this surface when the library's supported boundary is a compact C ABI and the part you need fits the verified scalar and opaque-resource subset. Prefer [Rust interop](rust_interop.md) when a maintained Rust crate already offers the safe, resource, callback, or asynchronous API you need. A C ABI may still be the right eventual boundary for a library implemented in another language; the implementation language is not the deciding factor.

If the header depends on callbacks, variadics, unions, bitfields, macros that cannot be represented as constants, or nontrivial lifetime rules, do not fake a scalar declaration. A checked C or C++ shim is the intended later adapter; it is not available in this first release slice.

See [how checked C interop is structured](../explanation/checked_c_interop.md) for the source-of-truth and toolchain boundary, the [binding inspection JSON schema](../../tooling/reference/binding_inspection_schema.md) for tool integration, and the [`std.interop` reference](../reference/stdlib/interop.md) for precise accepted syntax.
