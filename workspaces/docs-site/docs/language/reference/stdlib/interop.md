# `std.interop`: checked C bindings

`std.interop` activates the checked C binding vocabulary. This page is the exact contract: it lists accepted declaration forms, current execution limits, and verification behavior. Start with the [tutorial](../../tutorials/checked_c_binding.md), use the [how-to guide](../../how-to/checked_c_bindings.md) for modelling and diagnostics, and read the [architecture explanation](../../explanation/checked_c_interop.md) before choosing C over Rust interop.

The surface lets a module declare a small, explicit C ABI contract and call supported scalar functions, opaque resources, and output positions without writing a Rust wrapper first. The compiler verifies declared signatures, enum carriers, and listed plain-structure layouts with Clang before generating Rust.

This is a deliberately narrow foundation. It is useful for direct scalar C functions, opaque resource ownership, output positions, bounded NUL-terminated text input, and immediately copied UTF-8 error text. Native artifact resolution, shims, spans and caller-owned buffers, and platform packaging are separate RFC 116 work.

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

- Exact C scalar spellings: `c.i8`, `c.u8`, `c.i16`, `c.u16`, `c.i32`, `c.u32`, `c.i64`, `c.u64`, `c.i128`, `c.u128`, `c.f32`, `c.f64`, `c.Size`, `c.c_char`, and `c.c_int`. The fixed-width entries carry the same-width Incan numeric type; `c.Size` carries `usize`. `c.i128` and `c.u128` require the selected Clang target to accept its `__int128` extension.
- Read-only and mutable pointer descriptions: `c.ConstPtr[T]` and `c.MutPtr[T]`. They remain declaration facts unless the narrow text bridge below admits their use.
- `enum` variants with one explicit scalar carrier and a native constant name.
- Plain `struct` declarations with an explicit native C type name and listed fields.
- `resource` declarations that associate an opaque native type with one `c.Owned[...]` release symbol.
- `c.Owned[T]`, `c.Borrowed[T]`, and `c.BorrowedMut[T]` resource parameters and owned or nullable-owned resource results.
- `c.Out[T]` and `c.InOut[T]` parameters for scalar values and owned resources, plus an `outcome` declaration that makes output initialization explicit.

For the executable subset, fixed-width C scalar values retain their matching fixed-width Incan numeric type, and `c.Size` retains `usize`; they do not widen through Incan `int`. The target-defined `c.c_char` and `c.c_int` aliases retain checked range conversion until a selected target receipt can name their stable numeric layout. Owned resources release through their declared native operation, and output storage remains in compiler-generated private slots. A verified enum constant is available as an ordinary integer expression such as `LibC.Status.OK`. `c.Out[...]` is readable only after its declared outcome, while a consumed `c.Owned[...]` resource cannot be used again. Pointer and by-value structure contracts remain verified declarations rather than a general pointer-call API.

## Use the bounded text bridge

`c.cstr(value)?` creates compiler-private NUL-terminated storage from an Incan `str`. It rejects an interior NUL. Inside `unsafe:`, its `as_const_ptr()` method can supply exactly a declared `c.ConstPtr[c.c_char]` parameter; the temporary stays live through that one call and cannot become a general raw pointer.

```incan
def string_length(value: str) -> Result[usize, str]:
    text = c.cstr(value)?
    unsafe:
        return Ok(LibC.string_length(text.as_const_ptr()))
```

A declared `c.ConstPtr[c.c_char]` result becomes an unnameable scoped text view, not an Incan pointer. Inside the same `unsafe:` region, immediately call `view.copy_utf8(max_bytes=...)`. The named positive bound limits the terminator scan, validates UTF-8, and produces `Result[str, str]`; the view cannot be returned, stored, captured, or passed onward.

```incan
def last_error(database: Database) -> Result[str, str]:
    unsafe:
        view = SQLite.error_message(database)
        return view.copy_utf8(max_bytes=4096)
```

## Verification and diagnostics

Before code generation, the compiler renders a non-executable C probe from the binding descriptor and invokes a Clang-compatible toolchain for the selected ABI. A normal invocation selects the host ABI; `incan check --interop-target <triple>` instead requires and selects the matching `[[interop.c.targets]]` declaration. The probe checks the exact free-function signature, every enum constant's declared carrier, and the size, alignment, and field offsets of every listed plain structure. A mismatch is reported at the binding declaration before native execution.

Headers and native names are explicit in source. The verifier neither scans arbitrary headers to infer an API nor searches for a library that happens to provide a symbol. The logical library name records the link capability. A package may separately declare package-relative interop inputs and compatibility requirements under `[interop.c]` in `loaf.toml`; `incan lock` records those requirements and the content-derived identities of package-owned files. In v0.5, the explicit `incan oven interop bake` command turns those locked facts plus supplied compiler/SDK evidence into a selected direct-`rustc` native plan, and `stage` creates a fixed Android/iOS runtime layout. These commands do not discover host libraries, invoke Cargo or platform build tools, sign applications, or prove physical-device deployment. See the [checked C binding how-to](../../how-to/checked_c_bindings.md#freeze-oven-interop-requirements-for-a-target) for the current declaration schema and limits.

The repository verifies the pure checked-ABI fixture in Linux x86-64 and macOS arm64 Clang target modes. Declared Android arm64 verification uses `aarch64-linux-android<api-level>` and the selected target's definitions; `INCAN_C_ABI_CLANG` supplies the corresponding NDK Clang executable for this check. Declared iOS arm64 verification uses `arm64-apple-ios<deployment-target>` with Xcode's iPhoneOS SDK sysroot. `incan inspect interop-plan --target <triple>` separately projects the current lock into a deterministic, schema-versioned adapter input. It retains package-relative inputs, dependency-ordered deployment classes, explicit checked-binding-to-artifact-name correspondences, runtime and placement facts, shim inputs, and platform constraints. The explicit Oven baker, rather than this inspection command, attests selected compiler/SDK evidence, seals declared archives into its native search closure, and may stage bundled runtime files; neither route produces a signed or physical-device application.

## Not included yet

Do not use this surface for:

- arbitrary pointer arguments or results, pointer arithmetic, casts, dereferences, or dynamic symbol lookup;
- callbacks, variadics, unions, and bitfields;
- interop artifact downloads, selected system/framework or bundled-artifact materialization, `incan.pub` publication policy, or final application assembly;
- cross-target toolchain provisioning, generated-Rust cross-compilation, artifact staging, Gradle/Xcode command execution, or signing.

Those boundaries will build on the checked descriptor rather than adding a second source of ABI truth.

## Related guidance

- [Write your first checked C binding](../../tutorials/checked_c_binding.md)
- [Work with checked C bindings](../../how-to/checked_c_bindings.md)
- [How checked C interop is structured](../../explanation/checked_c_interop.md)
