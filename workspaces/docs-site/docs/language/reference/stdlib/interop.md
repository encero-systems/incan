# `std.interop`: checked C bindings

`std.interop` activates the checked C binding vocabulary. This page is the exact contract: it lists accepted declaration forms, current execution limits, and verification behavior. Start with the [tutorial](../../tutorials/checked_c_binding.md), use the [how-to guide](../../how-to/checked_c_bindings.md) for modelling and diagnostics, and read the [architecture explanation](../../explanation/checked_c_interop.md) before choosing C over Rust interop.

The surface lets a module declare a small, explicit C ABI contract and call supported scalar functions, opaque resources, and output positions without writing a Rust wrapper first. The compiler verifies declared signatures, enum carriers, and listed plain-structure layouts with Clang before generating Rust.

This is a deliberately narrow foundation. It is useful for direct scalar C functions, opaque resource ownership, output positions, and ABI verification; bounded C strings and views, native artifact resolution, shims, and platform packaging have separate RFC 116 slices.

## Activate the vocabulary

Import the C namespace explicitly. The import activates `binding` only in that module; it does not make C syntax a global language keyword.

```incan
from std.interop import c
```

`binding` is vocabulary surface. It lowers to an ordinary private class decorated with `@c.binding(...)` and extending `BindingDeclaration`. That keeps the ABI declaration inspectable as ordinary language data while making the source read like the contract it describes.

## Declare a binding

Each binding supplies one explicit header and logical system-library link name. `symbol`, `enum`, and `struct` bodies are declarations, not executable method bodies.

```incan
from std.interop import c

binding LibC:
    header = "fixture.h"
    link = c.system_library("fixture")

    resource Handle:
        native = "fixture_handle"
        release = close

    symbol close(handle: c.Owned[Handle]) -> None:
        native = "fixture_close"

    symbol open(output: c.Out[c.Owned[Handle]]) -> c.i32:
        native = "fixture_open"

        outcome Status.OK:
            initializes = [output]

    enum Status:
        OK: c.i32 = FIXTURE_OK

def open_handle() -> int:
    unsafe:
        output = c.out[c.Owned[Handle]]()
        status = LibC.open(output)
        if status == LibC.Status.OK:
            handle = output.take()
            LibC.close(handle)
        return status

def is_success(status: int) -> bool:
    return status == LibC.Status.OK
```

The raw call remains inside `unsafe:`. A public façade can live in the same module and call the private binding; the façade is where an API gives native failure codes, argument validity, and domain meanings their ordinary Incan shape.

The compiler emits the private `extern "C"` declaration from the checked binding descriptor. It does not rediscover the function signature from generated Rust.

## Checked declaration facts

The current surface accepts these declaration forms:

- Exact C scalar spellings: `c.i8`, `c.u8`, `c.i16`, `c.u16`, `c.i32`, `c.u32`, `c.i64`, `c.u64`, `c.Size`, `c.c_char`, and `c.c_int`.
- Read-only and mutable pointer descriptions: `c.ConstPtr[T]` and `c.MutPtr[T]`.
- `enum` variants with one explicit scalar carrier and a native constant name.
- Plain `struct` declarations with an explicit native C type name and listed fields.
- `resource` declarations that associate an opaque native type with one `c.Owned[...]` release symbol.
- `c.Owned[T]`, `c.Borrowed[T]`, and `c.BorrowedMut[T]` resource parameters and owned or nullable-owned resource results.
- `c.Out[T]` and `c.InOut[T]` parameters for scalar values and owned resources, plus an `outcome` declaration that makes output initialization explicit.

For the executable subset, Incan carries scalar values as `int`, releases owned resources through their declared native operation, and keeps output storage in compiler-generated private slots. Generated wrappers range-check every scalar conversion; a value that cannot be represented by Incan `int` is not silently truncated. A verified enum constant is available as an ordinary integer expression such as `LibC.Status.OK`. `c.Out[...]` is readable only after its declared outcome, while a consumed `c.Owned[...]` resource cannot be used again. Pointer and by-value structure contracts are verified declarations in this slice, but calls using them stay rejected until the later view design is implemented.

## Verification and diagnostics

Before code generation, the compiler renders a non-executable C probe from the binding descriptor and invokes a Clang-compatible toolchain for the selected ABI. A normal invocation selects the host ABI; `incan check --native-target <triple>` instead requires and selects the matching `[native].targets` declaration. The probe checks the exact free-function signature, every enum constant's declared carrier, and the size, alignment, and field offsets of every listed plain structure. A mismatch is reported at the binding declaration before native execution.

Headers and native names are explicit in source. The verifier neither scans arbitrary headers to infer an API nor searches for a library that happens to provide a symbol. The logical library name records the link capability. A package may separately declare package-relative target-native inputs and a mobile platform profile in `incan.toml`; `incan lock` records their content-derived identities. Android arm64 profiles name an NDK API level, and iOS arm64 profiles name an Apple deployment target. This experimental slice still does not download artifacts or compile shims. See the [checked C binding how-to](../../how-to/checked_c_bindings.md#freeze-native-inputs-for-a-target) for the current schema and its limits.

The repository verifies the pure checked-ABI fixture in Linux x86-64 and macOS arm64 Clang target modes. Declared Android arm64 verification uses `aarch64-linux-android<api-level>` and the selected target's definitions; until Oven manages the toolchain, `INCAN_C_ABI_CLANG` must point at the corresponding NDK Clang executable. Declared iOS arm64 verification uses `arm64-apple-ios<deployment-target>` with Xcode's iPhoneOS SDK sysroot. These checks do not cross-compile generated Rust, link or stage artifacts, or attest logical toolchain/SDK identities.

`incan inspect native-plan --target <triple>` requires a current `incan.lock` and emits a deterministic `schema_version: 1` handoff from the selected locked target. The report preserves package-relative input digests, include roots, definitions, dependency-ordered deployment actions, runtime names, placement, platform constraints, shim build inputs, and provenance. It contains no machine-local package paths, signing identities, or platform command protocol.

## Not included yet

Do not use this surface for:

- C strings, spans, caller-owned buffers, scoped foreign views, pointer arithmetic, casts, dereferences, or dynamic symbol lookup;
- callbacks, variadics, unions, and bitfields;
- native artifact downloads, C/C++ shim compilation, `incan.pub` publication policy, or final application assembly;
- cross-target compiler provisioning, generated-Rust cross-compilation, artifact staging, Gradle/Xcode command execution, or signing.

Those boundaries will build on the checked descriptor rather than adding a second source of ABI truth.

## Related guidance

- [Write your first checked C binding](../../tutorials/checked_c_binding.md)
- [Work with checked C bindings](../../how-to/checked_c_bindings.md)
- [How checked C interop is structured](../../explanation/checked_c_interop.md)
