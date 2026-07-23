# RFC 116: Typed C ABI interop

- **Status:** Draft
- **Created:** 2026-07-23
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 005 (Rust interop)
    - RFC 020 (offline and locked reproducible builds)
    - RFC 031 (Incan library system)
    - RFC 041 (first-class Rust interop authoring)
    - RFC 073 (environment matrices and toolchain constraints)
    - RFC 079 (`incan.pub` artifact graph)
    - RFC 091 (constrained integer newtype storage carriers)
    - RFC 094 (context managers)
    - RFC 102 (semantic layer inspection surface)
    - RFC 104 (ambient runtime capabilities and receipts)
    - RFC 112 (crash-safe local publication and file coordination)
    - RFC 114 (compiled providers, SDK components, and package features)
- **Issue:** [#939](https://github.com/encero-systems/incan/issues/939)
- **RFC PR:** —
- **Written against:** v0.5
- **Shipped in:** —

## Summary

This RFC defines a typed, explicit C ABI interop surface for Incan. Packages declare checked C binding modules whose raw symbols are imported through `c::` paths, isolated behind private bridge modules, and normally exposed through safe ordinary Incan APIs. Binding declarations preserve exact ABI types, symbols, layouts, ownership, nullability, output positions, buffers, headers, toolchain constraints, and target-native artifacts without assigning application meaning to foreign status values. The Incan toolchain verifies declarations with a managed Clang-compatible target toolchain, resolves and locks static, bundled, and system-native artifacts through the package graph, and lowers calls from compiler-owned semantic ownership facts rather than backend guesses. The same package, provenance, inspection, and façade boundaries remain extensible to future foreign runtimes without making C pointers or C ownership the universal interop model.

## Core model

1. **`c::` imports checked bindings, not arbitrary headers:** a `c::` module is an Incan-owned semantic contract backed by declared C headers and target artifacts; it never means "find a matching header on this machine."
2. **Bindings have three visible layers:** a declaration-only foreign contract feeds a private bridge containing the narrow unsafe boundary, and the bridge feeds an ordinary public Incan API.
3. **The C ABI declaration stays exact:** raw signatures preserve C scalar identity, pointer shape, output positions, calling convention, symbol spelling, nullability, and by-value layout instead of presenting a prematurely safe projection.
4. **Safe meaning belongs to Incan:** public wrappers own validation, domain errors, retries, cancellation, resource policy, buffer sizing, and conversion into `Result`, models, collections, strings, and bytes.
5. **Ownership is declared and call-site borrowing is inferred:** resources declare one release operation, owned values are non-copyable, and the compiler derives call-scoped shared or mutable borrows from parameter facts without requiring Rust lifetime syntax or repetitive `.borrow()` calls.
6. **Foreign views do not escape:** returned raw views may be copied into Incan-owned storage inside the current unsafe region, while explicit static foreign values are permitted; owner-tied returned zero-copy views require a follow-up lifetime contract.
7. **Buffers are bounded:** pointers do not become Incan strings, bytes, lists, or tensors without a declared length, capacity, terminator policy, output position, or caller-owned span.
8. **Clang verification is target-specific:** the selected target toolchain verifies signatures, layouts, definitions, and supported calling conventions against the resolved headers before the binding is accepted.
9. **Native artifacts are package inputs:** static archives, bundled shared libraries or frameworks, system capabilities, headers, and authored shims are resolved through the package graph, locked by identity, staged by the toolchain, and exposed through inspection.
10. **Shims are first-class:** a checked C or C++ shim is the principled adapter for variadics, function tables, macros, unions, bitfields, callbacks, and lifetime relationships that this RFC intentionally cannot represent directly.
11. **`unsafe:` is explicit and extensible:** this RFC introduces a general scoped acknowledgement surface but authorizes only checked foreign calls and conversions defined here; future low-level RFCs may add operations without changing the block model.
12. **The package envelope is interop-generic:** binding kind, runtime or toolchain requirements, artifacts, locking, provenance, capabilities, bridge ownership, documentation, and inspection are shared concepts, while C-specific types and safety rules remain owned by the C binding kind.

## Motivation

Incan's Rust interop gives applications direct access to Rust crates, but native systems software is not distributed exclusively as Rust. Mobile inference engines, operating-system APIs, media codecs, databases, cryptographic libraries, hardware runtimes, scientific libraries, and long-lived infrastructure commonly expose a C ABI even when their implementation is written in C++, Objective-C, Rust, or another language.

The practical workaround is to author a Rust or C++ adapter, expose Rust-shaped functions, and import those through `rust::`. That remains valuable when a mature safe Rust wrapper already exists, but it also makes another language own boundaries that are often mechanical. It obscures which requirements come from the C ABI, which handles must be released, which values are borrowed, which lengths protect a buffer, which target artifacts are linked, and which semantic decisions belong to the application.

A first-class C boundary must not repeat the least safe parts of traditional foreign-function interfaces. C headers generally do not encode ownership, output initialization, buffer relationships, nullability, thread affinity, error meaning, or release obligations. The same declaration can also change under target definitions, while scalar widths, structure padding, calling convention support, native dependencies, and deployment layout vary by target.

The desired end state is that a binding author performs the low-level work once and publishes a checked, inspectable binding package. The package manager resolves and verifies its target-native inputs. A small private bridge turns exact C signatures into Incan-owned values and domain-neutral boundary results. Application authors consume a safe Incan façade and encounter the raw boundary only when deliberately authoring or auditing an integration.

Mobile inference makes this foundation commercially relevant and technically demanding. A desktop example that happens to link one library is not sufficient. The contract must support Android and Apple target toolchains, static and bundled deployment, model and interpreter lifecycles, bounded tensor buffers, accelerator frameworks, reproducible native artifacts, and a real-device path without hard-coding one inference engine into the language.

## Goals

- Add a distinct `c::` import namespace for checked C ABI binding modules.
- Require an explicit `import std.c as c` for the compiler-known C vocabulary and safe conversion helpers.
- Define a declaration-only Incan binding form mapping logical names to exact C types and symbols.
- Define a clean declaration → private bridge → public façade package structure and make unsafe import edges inspectable.
- Define exact-width and target-width C scalars, pointers, nullability, opaque resources, output positions, spans, and verified plain structures.
- Associate each owned opaque resource with one release operation and make ownership transfer and explicit close semantics deterministic.
- Infer provably call-scoped shared and mutable foreign borrows from semantic parameter facts while rejecting unsupported retained or escaping borrows.
- Define bounded input, output, and in/out buffer contracts, including `c.Out[T]` and `c.InOut[T]`.
- Require raw foreign calls and raw foreign-view conversions to occur inside `unsafe:` without disabling ordinary type, capability, nullability, or ownership checking.
- Keep status interpretation, domain errors, retry behavior, cancellation, and high-level validation in ordinary Incan wrappers.
- Verify signatures and explicit layouts using a managed Clang-compatible toolchain configured for the selected target.
- Resolve static, bundled, and explicit system-native artifacts through the package and provider graph without ambient discovery.
- Make authored C and C++ shims reproducible, governed, locked, documented, and inspectable package assets.
- Define enough target metadata for Android arm64, iOS arm64, macOS arm64, and Linux x86-64 proof paths.
- Preserve one generic interop-package envelope that future Python, Ruby, JVM, Kotlin, or WebAssembly binding kinds may reuse without inheriting C semantics.
- Provide diagnostics, LSP facts, generated reference documentation, lock provenance, compatibility metadata, and package inspection for every checked binding.

## Non-Goals

- This RFC does not define direct C++ ABI interop, C++ name mangling, classes, templates, overload resolution, exceptions, standard-library types, or compiler-specific object layouts.
- This RFC does not promise that arbitrary C headers can be imported without a binding declaration.
- This RFC does not infer ownership, nullability, buffer lengths, thread safety, error meaning, or resource lifecycles from naming conventions or comments.
- This RFC does not support C variadic functions, callbacks from C into Incan, unions, bitfields, flexible array members, `setjmp`/`longjmp`, signal-handler entry into Incan, arbitrary function-pointer calls, or runtime symbol lookup.
- This RFC does not expose pointer arithmetic, arbitrary pointer casts, arbitrary dereference, memory-mapped I/O, integer-to-pointer conversion, or unrestricted raw memory access.
- This RFC does not support returned owner-tied zero-copy views escaping the unsafe region.
- This RFC does not make a native library memory-safe, thread-safe, deterministic, sandboxed, portable, or recoverable from undefined behavior merely because its binding typechecks.
- This RFC does not assign domain meaning to status codes or define application retry and recovery policy.
- This RFC does not define Incan-to-C exports or permit C hosts to call arbitrary Incan functions.
- This RFC does not require MSVC, vendor-specific C compilers, or every desktop and embedded target; follow-up work may add toolchain kinds without weakening the checked binding contract.
- This RFC does not define Python, Ruby, JVM, Kotlin, WebAssembly, or other future binding kinds; it defines only the common package-envelope invariants they must be able to reuse.
- This RFC does not define a second package manager, registry, dependency solver, or ungoverned native build-script system.
- This RFC does not require RFC 094 context-manager syntax to ship with C interop; owned resources remain usable through explicit close and a guaranteed last-resort release guard.

## Guide-level explanation

### Importing the C vocabulary and a raw binding

Code that authors a native bridge imports the C vocabulary explicitly and imports checked raw symbols through `c::`:

```incan
import std.c as c

from c::tflite import (
    Interpreter,
    Model,
    Status,
    interpreter_create,
    interpreter_delete,
    model_create_from_file,
    model_delete,
)
```

This does not search the host for a header or library named `tflite`. The import resolves a checked logical binding from the package graph. Its target variant identifies the headers, definitions, Clang-compatible verifier, native artifacts, deployment class, and lock identities used by the current build.

Raw functions remain visibly foreign:

```incan
def load_raw_model(path: str) -> Result[c.Owned[Model], ModelLoadError]:
    path_view = c.cstr(path)?

    unsafe:
        raw_model = model_create_from_file(path_view.as_const_ptr())

    match raw_model:
        Some(model) =>
            return Ok(model)
        None =>
            return Err(ModelLoadError(path=path))
```

The binding declares that the C function returns a nullable owned model. Nullability is an ABI fact, so the raw function returns `Option[c.Owned[Model]]`. The compiler does not invent a domain error when it encounters `None`; the wrapper authors that meaning.

### Using the safe façade

Application code should normally import an ordinary Incan package API:

```incan
from tflite import LiteModel

model = LiteModel.load("model.tflite")?
result = model.run(input)?
model.close()

return Ok(result)
```

`LiteModel` owns domain validation, error types, buffer sizing, resource sequencing, and any policy around threads or accelerators. Its implementation uses the private bridge, but its public API does not expose raw pointers, `c.Out`, status integers, scoped foreign views, or `unsafe:`.

RFC 094 may later let the same resource participate in a context manager:

```incan
with LiteModel.load("model.tflite")? as model:
    return model.run(input)
```

This RFC does not make that syntax available. Explicit `close()` determines release timing. A compiler-managed last-resort guard releases a still-owned resource on scope exit and must not permit double release.

### Keeping binding concerns separate

A binding package should keep the boundary mechanically visible. This layout is illustrative rather than a required spelling:

```text
bindings/
    c/
        tflite.incn

src/tflite/
    bridge.incn
    api.incn

src/tflite.incn
```

The binding declaration contains declarations and documentation only. The private bridge imports `c::tflite`, contains narrow unsafe regions, copies scoped foreign views, maps exact buffers, and returns private boundary values. The public API contains ordinary Incan models, errors, iterators, validation, retry or cancellation policy, and façade exports.

Inspection must show which modules import `c::`, which bridge modules call each raw symbol, and which public declarations depend on those edges. Tooling should warn when a public module mixes raw binding imports with unrelated application logic. Packages may intentionally publish a low-level checked binding without a safe façade, but that choice must remain explicit in documentation and inspection.

### Declaring owned resources once

A binding author associates the release function with the resource declaration instead of repeating it on every factory:

```incan
import std.c as c

binding c tflite:
    header "tensorflow/lite/c/c_api.h"

    opaque Model = "TfLiteModel":
        release model_delete

    opaque Interpreter = "TfLiteInterpreter":
        release interpreter_delete

    @c.symbol("TfLiteModelCreateFromFile")
    unsafe def model_create_from_file(
        path: c.ConstPtr[c.CChar],
    ) -> Option[c.Owned[Model]]

    @c.symbol("TfLiteModelDelete")
    unsafe def model_delete(model: c.Owned[Model]) -> None

    @c.symbol("TfLiteInterpreterDelete")
    unsafe def interpreter_delete(
        interpreter: c.Owned[Interpreter],
    ) -> None
```

The exact parser spelling may still be refined while this RFC is Draft. The semantic rule is settled: one nominal resource declaration owns one release association, and every function returning `c.Owned[Model]` inherits it.

### Inferred call-scoped borrowing

Binding signatures remain explicit about parameter ownership:

```incan
@c.symbol("sqlite3_step")
unsafe def step(statement: c.Borrowed[Statement]) -> c.CInt

@c.symbol("sqlite3_finalize")
unsafe def finalize(statement: c.Owned[Statement]) -> c.CInt
```

The bridge does not need Rust-shaped borrow syntax:

```incan
unsafe:
    status = step(statement)
    final_status = finalize(statement)
```

The compiler records a shared call-scoped borrow for `step(statement)` and an ownership-consuming move for `finalize(statement)`. Any later use of `statement` is rejected. The inferred borrow is permitted only when the owner is live, no conflicting borrow exists, and the binding guarantees that the C function does not retain the pointer beyond the call.

### Output positions

Many C APIs return values through pointers. SQLite statement preparation, for example, writes both a statement handle and a tail pointer. The raw declaration represents those ABI positions directly:

```incan
@c.symbol("sqlite3_prepare_v2")
unsafe def prepare(
    database: c.Borrowed[Database],
    sql: c.ConstPtr[c.CChar],
    byte_count: c.CInt,
    statement: c.Out[Option[c.Owned[Statement]]],
    tail: c.Out[Option[c.ConstPtr[c.CChar]]],
) -> c.CInt
```

`c.Out[T]` is compiler-managed storage for a value written by the foreign call. It is not an ordinary public container and cannot be returned from a safe façade. The declaration must state whether the position is always written or which raw outcome makes it initialized. Reading an output before the declared write condition is satisfied is rejected. `c.InOut[T]` begins with an initialized value and permits the foreign call to update it.

### Bounded strings and buffers

Incan `str` does not implicitly become `char *`, and a returned `char *` does not implicitly become `str`. A safe conversion creates temporary NUL-terminated storage and rejects interior terminators:

```incan
path_view = c.cstr(path)?
```

The temporary remains live through the foreign call. Passing it to a raw function still requires the unsafe boundary:

```incan
unsafe:
    raw_model = model_create_from_file(path_view.as_const_ptr())
```

Length-delimited data uses Incan-owned spans. Raw declarations preserve the actual C pointer-plus-length signature, while the bridge derives both values from one span:

```incan
def copy_input(
    tensor: Tensor,
    input: FrozenBytes,
) -> Result[None, LiteError]:
    span = c.bytes_span(input)

    unsafe:
        status = tensor_copy_from_buffer(
            tensor,
            span.as_const_ptr(),
            span.byte_length,
        )

    return map_status(status)
```

Application code cannot independently replace a checked span's pointer or length. Caller-owned mutable spans provide the preferred zero-copy path for large output. Returned foreign views are intended for immediate bounded copying, not long-lived tensor access.

### Copying a scoped foreign view

A C function may return text owned by a live foreign handle and invalidated by a later call. The private bridge can copy that value before it escapes:

```incan
def last_error(database: Database) -> Result[str, TextError]:
    unsafe:
        view = sqlite3_errmsg(database)
        return view.copy_utf8(max_bytes=4096)
```

The compiler rejects returning, storing, capturing, or otherwise extending the lifetime of `view`. Diagnostics should recommend an owning conversion such as `copy_utf8(...)` or `copy_bytes(...)`. Large data should instead be written into caller-owned spans or consumed by a native operation that returns a compact owned result.

### Mapping errors in ordinary Incan

Raw status values remain raw:

```incan
from c::sqlite import SQLITE_DONE, SQLITE_ROW, step

def next_step(statement: Statement) -> Result[Step, DatabaseError]:
    unsafe:
        status = step(statement)

    match status:
        SQLITE_ROW =>
            return Ok(Step.Row)
        SQLITE_DONE =>
            return Ok(Step.Done)
        code =>
            return Err(DatabaseError.from_status(code))
```

The compiler knows that `status` is a C integer and preserves named constants from the binding. It does not decide that zero means success, that a particular code is retryable, or which domain error should be constructed. That meaning remains visible, typed Incan code.

### Authored shims

Some legal C APIs cannot be represented directly. ONNX Runtime exposes much of its API through function-pointer tables, libcurl uses option-dependent variadic arguments and callbacks, zlib initialization is macro-mediated, and many C++ libraries expose callback-bearing parameter structures. A binding package may include a narrow authored shim:

```c
int incan_ort_create_session(
    const char *model_path,
    struct incan_ort_session **out_session,
    struct incan_ort_error *out_error
);
```

The shim becomes an ordinary checked native artifact. The package manager builds it with the selected managed toolchain or selects a locked prebuilt variant, verifies its exposed C header, records its source and artifact identity, and links it like any other native input. The safe public API remains authored in Incan.

### Native deployment

Binding artifacts declare one of three deployment classes:

- **Static:** a target archive is linked into the generated native product.
- **Bundled:** a target shared library or framework is staged into the application bundle with validated runtime-link metadata.
- **System:** an explicitly declared target capability supplies the library or framework.

Static linking is the default preference when supported and legally appropriate. Bundled artifacts must be part of the platform packaging plan rather than discovered through an ambient runtime search path. System libraries must be explicit toolchain capabilities with inspectable version or SDK constraints.

For Android, the selected toolchain profile identifies the NDK-compatible Clang toolchain, target ABI, API level, sysroot, native artifact variant, and packaging destination. For Apple targets, Incan manages Clang selection and records the Xcode and SDK identity but does not claim to redistribute the Apple SDK. Incan resolves, verifies, builds, locks, and stages the native inputs; Gradle or Xcode may remain the final application assembler and signer.

The eventual user workflow must not require manual archive copying, ambient library-path variables, handwritten Gradle ABI-directory wiring, or repeated Xcode search-path configuration.

## Reference-level explanation

### Import resolution

The grammar gains a C binding import kind:

```text
c_import = "import" "c" "::" binding_path [ "as" IDENT ]
         | "from" "c" "::" binding_path "import" import_list
```

The first identifier after `c::` names a logical checked binding visible through the current package graph. Following identifiers name logical submodules inside that binding and do not directly identify filesystem directories, headers, libraries, frameworks, or C namespaces.

A `c::` import must resolve a binding declaration, selected target variant, verification record, and native artifact plan. The compiler must reject an unknown binding, unavailable target, stale verification record, unresolved artifact, unsupported toolchain, or symbol absent from the declaration.

Ambient headers and libraries must not satisfy a binding merely because they have matching names. System bindings must be supplied by an explicit package or selected toolchain capability. `c::`, `rust::`, and future foreign namespaces remain distinct; the compiler must not silently route one binding kind through another.

Code using compiler-known C types or conversion helpers must explicitly import `std.c`:

```incan
import std.c as c
```

The `c::` namespace identifies foreign binding modules. The `c` alias identifies the C type and helper vocabulary. Neither introduces a general `feature` or `module` keyword.

### Binding declarations

A C binding declaration must define:

- one stable logical binding identity
- one or more header declarations or an authored shim header
- every imported C type, constant, and symbol
- the exact physical symbol name when it differs from the logical Incan name
- the calling convention when it differs from the target's ordinary C convention
- exact scalar identity, pointer constness, nullability, and output position
- ownership mode for every pointer-bearing resource parameter and result
- one release operation for each owned opaque resource
- declared write conditions for output positions
- bounds, encoding, terminator, and validity rules for strings and buffers
- explicit layouts for by-value C structures
- target availability and required toolchain capabilities

A binding declaration is data-shaped Incan source. It may contain binding declarations, binding constants, documentation, and the imports required to name binding vocabulary, but it must not contain executable application logic or arbitrary build commands.

Tools may generate a draft binding declaration from a header. Generated output must remain incomplete until ownership, nullability, output writes, bounds, validity, release, and target facts are explicit. Header generation is an authoring aid, not a safety proof.

### Private bridges and public façades

A package may publish a raw checked binding intentionally, but a package claiming a safe high-level API must isolate `c::` imports and unsafe calls behind a private bridge. Its public façade must not expose raw pointers, output slots, foreign status integers, unbounded views, undeclared resources, or an unsafe call requirement.

Tooling must be able to distinguish binding declarations, raw-binding-only packages, private bridge modules, and safe public façades. It should diagnose or lint public modules that mix raw foreign imports with unrelated application concerns. The required separation is semantic and inspectable; this RFC does not reserve one filesystem spelling for bridge modules.

### Unsafe regions

`unsafe:` is a scoped language construct:

```incan
unsafe:
    result = foreign_call(...)
```

The construct does not disable type checking, ownership checking, nullability, bounds on Incan-owned values, capability checks, target checks, or diagnostics. It acknowledges that the author accepts preconditions the compiler cannot prove about the foreign implementation.

This RFC permits unsafe regions to contain checked C calls, extraction of compiler-controlled C pointers from temporary views or spans, reads from initialized output positions, and owning copies from scoped foreign views. Pointer arithmetic, arbitrary dereference, arbitrary casts, raw allocation, MMIO, and representation overrides remain unavailable.

The syntax is intentionally extensible. A follow-up RFC may authorize additional low-level operations without changing the block model, but no operation becomes available merely because it appears inside `unsafe:`.

### Scalar types

The compiler-known `c` vocabulary must provide these target-checked scalar categories:

<!-- markdownlint-disable MD060 -->

| Incan C type | C meaning |
| --- | --- |
| `c.Int8` / `c.UInt8` | exact-width 8-bit signed / unsigned integer |
| `c.Int16` / `c.UInt16` | exact-width 16-bit signed / unsigned integer |
| `c.Int32` / `c.UInt32` | exact-width 32-bit signed / unsigned integer |
| `c.Int64` / `c.UInt64` | exact-width 64-bit signed / unsigned integer |
| `c.CChar` / `c.CSChar` / `c.CUChar` | target C `char` categories without assumed signedness |
| `c.CShort` / `c.CUShort` | target C `short` categories |
| `c.CInt` / `c.CUInt` | target C `int` categories |
| `c.CLong` / `c.CULong` | target C `long` categories |
| `c.CLongLong` / `c.CULongLong` | target C `long long` categories |
| `c.Size` / `c.SSize` | target `size_t` / signed size category where available |
| `c.Float32` / `c.Float64` | C `float` / `double` |
| `c.Bool` | C `_Bool` / `bool` when the selected header contract supports it |

<!-- markdownlint-enable MD060 -->

Exact-width types must be rejected on a target where the corresponding width is unavailable. Target-width types must retain their C identity even when their current representation matches an Incan numeric type.

Incan `int` must not implicitly stand for a C integer category. Numeric conversion must be checked for range and sign loss. A raw binding signature must not use semantic Incan numerics in place of exact C parameter types.

### Raw pointers, nullability, and opaque resources

The C binding vocabulary includes:

- `c.ConstPtr[T]` for a non-null pointer to immutable foreign storage
- `c.MutPtr[T]` for a non-null pointer to mutable foreign storage
- `Option[c.ConstPtr[T]]` and `Option[c.MutPtr[T]]` for nullable pointers
- `c.Owned[T]` and `Option[c.Owned[T]]` for non-null or nullable owned opaque resources
- `c.Borrowed[T]` and `c.BorrowedMut[T]` for call-scoped access to opaque resources
- scoped immutable or mutable foreign views that cannot escape the current unsafe region
- explicit static foreign values whose process-lifetime validity is part of the verified declaration

Raw pointers must not support arithmetic, integer conversion, direct dereference, indexing, construction from an address, or storage in an ordinary safe public value. Opaque foreign types are nominal per binding.

Null pointer representation is an ABI fact and must map to `Option` where the declaration permits null. A binding must not claim a non-null pointer unless the native contract guarantees it for every represented outcome or the bridge performs a check before constructing the non-null value.

### Owned resource lifecycle

Each owned opaque resource declaration must name exactly one release operation. Factory functions return `c.Owned[T]` without repeating the release association.

`c.Owned[T]` is non-copyable. Passing it to a `c.Owned[T]` parameter consumes it. Passing it to a call-scoped `c.Borrowed[T]` or `c.BorrowedMut[T]` parameter does not transfer ownership. Explicit `close()` consumes the value and invokes the declared release operation once.

A still-owned value must carry a last-resort release obligation on scope exit. Explicit close determines release timing, while the guard prevents leaks across early return, `?`, or other ordinary control flow. A consumed or closed value must never release twice.

Dynamic boundaries that cannot statically prove consumption may retain a runtime state guard, but ordinary statically visible use after move or close must be a compile-time error.

### Semantic call-site borrowing

The binding parameter mode is the source of truth for a foreign argument:

- `c.Borrowed[T]` requests one shared borrow for the duration of the call.
- `c.BorrowedMut[T]` requests one exclusive mutable borrow for the duration of the call.
- `c.Owned[T]` consumes the resource.

The compiler must infer a call-scoped borrow when the owner is live and the requested access is compatible with every overlapping use. Ordinary call sites must not require explicit Rust-shaped borrow syntax.

The ownership decision must be recorded before backend emission as a stable semantic fact attached to the call and argument. Lowering and emission must consume that fact and must not independently reclassify the argument from generated-language requirements.

A call-scoped borrow is permitted only when the declaration guarantees that the foreign function does not retain the pointer or derive a value that outlives the call. Retained input pointers, escaping owner-tied views, ambiguous aliasing, and unprovable mutation must be rejected or placed behind an authored shim that establishes a supported contract.

### Output and in/out positions

`c.Out[T]` represents compiler-managed storage written by a foreign call. `c.InOut[T]` represents initialized compiler-managed storage that a foreign call may update.

Neither type is an ordinary collection or public API value. They may appear only in binding signatures and private bridge code. They must not be exported through a safe façade.

Each `c.Out[T]` declaration must state whether the output is always initialized or which raw call outcome establishes initialization. Reading or taking the output before that condition is satisfied must be rejected. If the native API has no representable initialization rule, the binding must use a shim.

An output resource becomes `c.Owned[T]` only after the declared initialization and nullability rules are satisfied. Failure paths must not leak an output handle that the native contract requires the caller to release.

### Strings, spans, and scoped foreign views

`std.c` must provide checked helpers for NUL-terminated input, immutable and mutable spans, output buffers, scoped foreign views, encoding validation, and owning copies. These helpers must preserve the corresponding compiler-known C types rather than representing pointers as integers.

Converting `str` into a C string view must validate the declared encoding and reject interior terminators. Temporary encoded storage must remain live through the call.

Raw binding functions must mirror C pointer and length parameters separately. A private bridge may accept one safe span and derive its pointer and length inside `unsafe:`. Application code must not independently replace one component and retain the span's checked status.

Converting foreign bytes into `str` must validate encoding. Invalid text must produce a typed error. A foreign terminator scan requires an explicit maximum bound; an unbounded bare `char *` cannot become an Incan string.

A returned scoped foreign view may be inspected only through compiler-provided bounded operations inside its unsafe region. It must not be returned, stored in a longer-lived model, captured, or retained after consuming its owner. Owning conversions must have diagnostics-friendly names and preserve bounds and encoding.

Large data should cross through caller-owned spans or native operations that return compact owned results. Returned owner-tied zero-copy views require a follow-up lifetime contract.

### Plain C structures

A by-value C structure must have an explicit Incan declaration naming its physical C type and listing every supported field in source order. The selected target verifier must prove total size, alignment, field offsets, field ABI types, fixed array extents, and every by-value calling use.

The compiler must not reorder fields, apply Incan model serialization aliases, infer padding, or use semantic default values to satisfy an ABI structure.

This RFC supports only verified fixed-layout structures composed of supported scalars, pointers, fixed-size arrays, and other verified plain structures. Unions, bitfields, flexible arrays, anonymous target-dependent members, callback fields, and unsupported extensions require an opaque declaration or authored shim.

### Function signatures and calling conventions

The ordinary target C calling convention is the default. A non-default supported calling convention must be explicit and verified for target availability.

Every raw function must have a fully representable parameter and return type. Unsupported by-value types, variadics, callable function pointers, callbacks into Incan, and incomplete structures must produce a source-anchored diagnostic before generated project construction or native linking.

C `void` maps to `None` only as a function return. `void *` must remain an opaque or explicitly typed pointer. Symbol names must be preserved exactly and must not be guessed from Incan casing.

### Status, null, and error facts

A raw C function returns its declared ABI type. Nullability may produce `Option` because it is a representational fact. Integer, enum, or structured status meaning must remain ordinary Incan wrapper code.

A binding may declare that a function writes `errno`, a thread-local status, or a structured output that must be captured immediately. The compiler may preserve that raw fact atomically with the call, but it must not choose a domain error type or recovery policy.

The compiler must not assume that zero means success, negative means failure, non-null means success, or any code is retryable. Named constants and raw status identities should remain available to wrappers and inspection.

Native process termination, segmentation faults, undefined behavior, memory corruption, signals, and C++ exceptions crossing the C boundary cannot be represented as `Result`. Documentation must not imply that `unsafe:` makes them recoverable.

### C++ shims and unwind

A C++ implementation may participate only through an authored `extern "C"` shim with C-representable signatures. The shim must catch every C++ exception before it reaches the C boundary and translate it into a declared raw C outcome.

Without callbacks, the execution shape is Incan → generated adapter → C → return to Incan. Incan code does not execute beneath C-owned stack frames. Incan or backend panics therefore must not be described as recoverable C failures, and C++ exceptions or native crashes must not be allowed to cross or masquerade as ordinary Incan errors.

Callbacks from C into Incan remain excluded because they require separate contracts for calling thread, captured state, reentrancy, panic containment, lifetime, cancellation, and teardown.

### Thread, cancellation, and capability facts

Thread affinity and thread safety are binding facts. A binding may declare a resource thread-confined, transferable, or synchronized only when the native contract supports that claim. Pointer constness does not imply thread safety.

Cancellation must use explicit polling functions, native cancellation handles, or other C-owned mechanisms that do not call into Incan. Callback-based cancellation requires a follow-up RFC.

Importing a binding does not grant runtime authority. A governed runtime may require capabilities before loading a bundled library, mapping an accelerator, opening a device, reading a model, or running a governed native package action. Receipts must identify logical bindings and symbols without exposing pointer values, secret buffers, credentials, or machine-local paths.

### Managed Clang-compatible target toolchains

Binding verification and shim compilation require a Clang-compatible target toolchain selected through Incan's toolchain model. The verified identity must include compiler version, target triple, ABI-relevant options, sysroot or SDK identity, preprocessor definitions, headers, and the binding schema version.

Android profiles must identify the compatible NDK toolchain, ABI, API level, and sysroot. Apple profiles must select the available Xcode-provided Clang and SDK and record their identities without treating the Apple SDK as a redistributable Incan artifact. Linux and other supported targets must likewise resolve an explicit compiler and sysroot contract.

Host verification is insufficient for cross-compilation. A binding accepted for Android arm64 or iOS arm64 must be verified against that selected target environment.

### Signature and layout verification

The toolchain must compile a generated C verification unit against the resolved headers and definitions. It must check every imported symbol signature, supported calling convention, output position type, and explicit by-value structure layout.

The verification result must be keyed by the binding declaration, target and ABI profile, header contents, preprocessor definitions, compiler and SDK identity, native library or shim identity, and binding schema version.

Link-time symbol resolution remains a separate gate. Passing a header probe does not prove that the selected artifact exports the required symbol. Both verification and linking must use the same resolved target plan.

### Native artifact resolution and deployment

The package graph must distinguish semantic binding declarations from physical native artifacts. A target variant may select:

- a static archive linked into the generated product
- a bundled shared library or framework staged for platform packaging
- an explicit system library or framework supplied as a toolchain capability
- an authored shim built through a governed package action
- a locked prebuilt shim or native artifact

Machine-local absolute paths must not become publishable metadata. Explicit local development overrides may exist but must be non-portable, visible in inspection, and excluded from locked publication.

Static linking should be preferred when supported and legally appropriate. Bundled artifacts must have declared runtime link names, placement, transitive native dependencies, minimum platform constraints, and packaging outputs. System artifacts must identify the providing toolchain or SDK capability.

The Incan package manager must resolve, verify, build when declared, lock, cache, and stage native artifacts. It must not invent missing shims, accept licenses, supply signing credentials, infer status meaning, silently discover system libraries, or run arbitrary undeclared shell scripts.

Gradle, Xcode, or another platform packager may remain responsible for final application assembly and signing. Incan must emit one complete target-native plan so users do not manually reproduce include paths, linker flags, ABI directories, runtime search paths, or framework placement.

### Packaging, locking, and reproducibility

A published binding package may include declarations, headers, authored C or C++ shim sources, safe Incan wrappers, and target-specific native artifacts subject to package policy.

The lock graph must record the logical binding version, binding kind, selected target variant, declaration digest, header digests, definitions, shim source or artifact digests, native artifact digests, deployment class, toolchain and SDK constraints, configuration identity, and package provenance.

Offline and locked modes from RFC 020 apply to every binding input. A locked build must not download, discover, or substitute a different native library because the host happens to provide one.

Crash-safe artifact publication and cache coordination must follow RFC 112. Compiled-provider selection and package features must reuse RFC 114 instead of introducing C-only feature activation or provider resolution.

### Inspection, diagnostics, and documentation

Checked package metadata must expose logical and physical symbols, signatures, resource modes, release operations, output positions, buffer bounds, target variants, artifact identities, deployment class, verification records, unsafe requirements, façade relationships, and provenance.

`incan inspect` must show:

- the logical binding and binding kind
- the declaration and selected target variant
- the managed toolchain and SDK identity
- resolved headers, definitions, native artifacts, and deployment classes
- every raw symbol and its physical spelling
- declaration → private bridge → public façade dependency edges
- verification and link status
- lock and package provenance
- capability requirements

Diagnostics must identify the binding, logical and physical symbol, selected target, expected declaration, observed header or linker fact, violated ownership or validity rule, and originating Incan source span. A borrowed-view escape diagnostic should recommend an owning conversion when one exists.

Generated reference documentation must separate the raw binding from the safe façade and must not present the raw module as the recommended application API when a safe façade exists.

### Generic interop package envelope

The package, provider, lock, capability, inspection, documentation, and façade model must represent a binding kind as an explicit discriminator with kind-specific semantic metadata.

The shared envelope may contain logical identity, target or runtime requirements, artifact and provider identity, package features, provenance, capability requirements, bridge ownership, public façade relationships, and inspection facts. It must not require every binding kind to provide C headers, C pointers, C layouts, a Clang verifier, or C resource modes.

Future binding kinds may use the same architectural separation:

- Python or Ruby bindings may own interpreter lifecycle, runtime locks, object handles, reference counts, and language exceptions.
- JVM or Kotlin/JVM bindings may own VM lifecycle, class and method descriptors, JNI reference scopes, threads, and managed exceptions.
- Kotlin/Native libraries may expose a C ABI and use this RFC directly.
- WebAssembly bindings may own WIT or component contracts, canonical ABI values, capabilities, and component resources without being memory-unsafe at the Incan source boundary.

This RFC does not reserve syntax or namespaces for those kinds. It requires only that C remain one kind rather than becoming the universal shape of foreign interoperability.

## Design details

### Binding source units

Binding declarations use Incan-shaped syntax and tooling but are declaration-only units. Keeping executable bridge logic out of the same unit makes generated documentation, compatibility comparison, static inspection, and package review deterministic.

A distinct file extension is not required by the semantic model. Ordinary `.incn` files may be classified as binding declaration units by their top-level `binding c` declaration and package role. The formatter and LSP must treat the contained syntax as Incan rather than embedded C source.

### Why exact raw signatures and safe wrappers are separate

A C function commonly receives a pointer and a separate length, returns a status, and writes one or more outputs. Projecting that immediately into one safe Incan function would require the compiler to invent grouping, initialization, domain errors, retries, and ownership policy.

The raw binding instead mirrors the C function. A private bridge derives exact pointer and length arguments from checked spans, obtains initialized outputs, captures raw status facts, and copies scoped views. The public façade then presents domain-shaped types and policies. This keeps the ABI contract inspectable and makes application meaning ordinary testable Incan.

### Why the unsafe region belongs in this RFC

A checked C signature proves representation and declared boundary facts; it cannot prove that the native implementation respects undocumented preconditions or avoids undefined behavior. Deferring `unsafe:` would either mislabel raw calls as safe or require a C-specific temporary syntax that later low-level work would replace.

This RFC therefore introduces one general scoped acknowledgement construct while authorizing only the checked foreign operations it defines. The coupling is deliberate and bounded: the C feature receives the minimum truthful application surface it needs, while pointer arithmetic, dereference, MMIO, arbitrary casts, and representation control remain unavailable.

### Why call-scoped borrowing is inferred

Incan does not expose Rust ownership syntax as its application language. Requiring `.borrow()` for every native call would leak a generated-language concern and make safe wrapper code needlessly ceremonial.

The binding already declares whether each argument is borrowed, mutably borrowed, or consumed. The compiler therefore has enough information to plan a temporary call-scoped borrow when aliasing and lifetime constraints are satisfied. Making that choice a semantic call fact preserves backend neutrality and gives diagnostics, LSP, codegraph, and future backends the same answer.

### Why returned owner-tied views are deferred

An owner-tied returned view requires the compiler to prove that the owner remains live, is not mutably used in an invalidating way, is not closed, and outlives every use of the view. Some native APIs impose even narrower invalidation rules such as "valid until the next call."

This RFC supports immediate bounded copying and caller-owned buffers because those rules are representable without a general lifetime surface. Native operations should consume large foreign data in place or write into Incan-owned spans when copying would be excessive. A follow-up RFC may define owner-tied zero-copy views once ownership facts can express and diagnose those relationships honestly.

### Why shims are part of the main design

Variadic functions, callbacks, function-pointer tables, macros, unions, bitfields, C++ exceptions, and undocumented memory conventions are ordinary characteristics of important native libraries. Treating a shim as a workaround would make the usable package model fail precisely where native interop becomes valuable.

A narrow shim converts an unsupported physical API into a stable C contract that this RFC can verify. Its source, compiler, outputs, target variants, and provenance are package inputs. It does not own the public semantic API, which remains Incan.

### Prior art

Python `ctypes` demonstrates ergonomic dynamic calls, explicit prototypes, output positions, and user-authored error checks, but its runtime declarations can still corrupt memory or crash the process when wrong. This RFC does not adopt runtime-only ABI trust or ambient dynamic loading.

CFFI's compiler-assisted API mode demonstrates why native declarations should be checked by a real C compiler rather than guessed from binary layout. Its owned pointer destructors and explicit release operations also motivate one resource-level release association plus deterministic close.

Cython demonstrates source-shaped external declarations that remain visible to the C compiler and an effective separation between low-level external declarations and higher-level wrappers. This RFC retains stronger ownership, locking, target verification, and package provenance requirements.

Python's buffer protocol and capsules demonstrate bounded producer-consumer memory views, explicit release obligations, opaque nominal handles, and destructor association. This RFC makes those relationships statically typed and prevents raw handles or scoped views from leaking through safe façades.

### API-shape stress test

The design must accommodate or deliberately reject several different public C API shapes:

<!-- markdownlint-disable MD060 -->

| Library shape | Design pressure | Required outcome |
| --- | --- | --- |
| TensorFlow Lite C API | Opaque models and interpreters, explicit delete functions, nullable factories, status values, pointer-plus-size tensors | Direct checked binding and Android/iOS acceptance proof |
| SQLite | Owned connection and statement handles, multiple output pointers, multi-state statuses, temporary error text | `c.Out[T]`, source-authored status mapping, and immediate scoped-view copying |
| Apple Accelerate | Framework linking, numeric pointer/length/stride signatures, caller-owned buffers | Explicit system framework capability and checked span projection |
| llama.cpp | Opaque model and context resources, by-value parameters, optional callback-bearing configuration, accelerator dependencies | Direct callback-free subset plus an authored shim where unsupported fields appear |
| ONNX Runtime | Versioned function-pointer API tables | Authored flat C shim; no direct arbitrary function-pointer call |
| libcurl | Option-dependent variadic arguments and callback-driven transfers | Typed shim for a selected surface; no direct arbitrary `setopt` projection |
| zlib | Macro-mediated initialization and mutable state containing allocator callbacks | Opaque shim-owned compressor state with span-based Incan operations |

<!-- markdownlint-enable MD060 -->

The required release proof must include one real mobile inference library, one second inference runtime with a cleaner flat C lifecycle, one non-ML stateful C library, one shimmed function-table or variadic API, one Apple framework surface, and one synthetic fixture covering negative ABI diagnostics. Commercial mobile claims require Android arm64 and iOS arm64 packaging and link verification plus one real-device inference on each platform.

### Native deployment contract

The public deployment model is static, bundled, or system. Those categories remain stable even when platform mechanics differ.

Android packaging may place bundled libraries in ABI-specific application inputs, while Apple packaging may consume static archives or bundled frameworks and require a platform signing step. The package manager owns artifact selection, identity, verification, and staging. The final platform packager owns application assembly and signing credentials.

The exact manifest keys and emitted Gradle or Xcode integration format remain implementation details as long as the source contract, lock identity, inspection surface, offline behavior, and user-visible deployment classes remain stable.

### Relationship to Rust interop

RFC 005 remains the preferred path for Rust-native crates and safe Rust APIs. C interop does not route through Rust signature discovery and must not pretend C declarations have Rust ownership semantics.

The current backend may emit private `extern "C"` declarations, representation carriers, resource guards, and narrow unsafe adapters. Those are inspectable generated artifacts, not the source of semantic authority. The stable handoff consists of checked binding facts, semantic ownership decisions, the verified target plan, and ordinary Incan wrapper code.

Rust wrapper crates remain valid when they provide a mature safe API, already own reliable native build discovery, or handle features deliberately excluded here. RFC 116 removes the requirement to author Rust for straightforward C boundaries; it does not prohibit appropriate Rust wrappers.

### Compatibility and migration

This feature is additive. Existing `rust::` imports and Rust wrapper packages remain valid.

Projects may migrate one native symbol group at a time while preserving their public Incan APIs. A package may initially publish only a raw checked binding, then add a private bridge and safe façade without changing the logical binding identity.

The `c::` namespace remains reserved for checked C binding modules. Code must not rely on dot-form aliases, implicit header lookup, ambient linker flags, machine-local library discovery, or runtime string-based symbol resolution.

## Alternatives considered

- **Continue requiring Rust wrappers for every C library** — Mature Rust wrappers remain valuable, but requiring one makes Rust own mechanical C boundaries and hides original ABI, artifact, ownership, and target facts from Incan tooling.
- **Adopt a static form of Python `ctypes`** — Runtime prototypes and dynamic loading are ergonomic but do not provide target compiler verification, locked native artifacts, semantic ownership facts, or safe façade separation.
- **Import headers automatically and expose every declaration** — Headers do not reliably encode ownership, output initialization, buffer relationships, error meaning, lifetimes, or thread rules. Generation remains a draft-authoring aid.
- **Let the compiler generate domain-safe wrappers** — Nullability and exact representation are compiler facts, but domain errors, retries, cancellation, validation, and recovery policy belong in ordinary Incan. Compiler-generated policy would be magical and brittle.
- **Use one universal foreign type system** — C pointers, Python object handles, JVM references, and WebAssembly component resources have different validity and runtime models. Only package-envelope facts should be shared across binding kinds.
- **Require explicit `.borrow()` calls** — Rejected for call-scoped access because the binding parameter already declares the ownership mode and the compiler can record the borrow semantically. Explicit Rust-shaped syntax would add ceremony without adding information.
- **Support returned owner-tied views immediately** — Rejected because truthful support requires invalidation and lifetime facts that the current language does not expose. Caller-owned spans and immediate bounded copies provide a safe foundation.
- **Use runtime dynamic loading and string symbol lookup by default** — Rejected because it moves signature and symbol errors into production and weakens target identity, locking, packaging, and policy.
- **Treat `c::name` as `#include <name.h>`** — Rejected because logical package identity, headers, definitions, linker artifacts, deployment layout, and target variants do not have a one-to-one relationship.
- **Expose raw addresses as integers** — Rejected because it destroys nominal typing, nullability, ownership, provenance, and inspectability.
- **Support C++ directly** — Rejected because compiler-specific ABI, overloads, templates, exceptions, standard-library types, and object lifecycle multiply scope and reduce portability. An authored C shim is the stable boundary.
- **Treat shims as an external escape hatch** — Rejected because important native APIs routinely require them. Shims must participate in the same verification, package, lock, provenance, and deployment contract.

## Drawbacks

- The feature introduces a second native interop kind alongside Rust interop and a general unsafe-region surface.
- Binding declarations require more authored facts than raw header generation.
- Semantic ownership analysis must become authoritative for foreign call arguments rather than leaving decisions to generated-Rust emission.
- Target-equivalent Clang verification and shim compilation increase toolchain size and cross-compilation complexity.
- Native artifacts add platform matrices, deployment rules, license review, signing handoffs, cache pressure, and supply-chain policy.
- The safe package structure uses more files and concepts than a direct FFI call.
- Some performance-sensitive APIs remain behind native operations or caller-owned buffers until owner-tied returned views receive a deliberate lifetime contract.
- Important APIs still require authored C or C++ shims.
- A checked boundary reduces integration mistakes but cannot prevent undefined behavior, process crashes, undocumented native behavior, or defects inside the library.

## Implementation architecture

The recommended architecture represents C as one binding kind inside a generic checked interop-package envelope. Parsing and type checking produce declaration facts independently from target resolution. Semantic ownership analysis records shared borrow, mutable borrow, or ownership transfer on each foreign call argument. Target preparation resolves headers, definitions, managed Clang-compatible toolchains, native artifacts, deployment classes, and shim actions. Verification produces a target-keyed signature and layout record. Lowering consumes only those checked facts.

The current Rust backend may emit private `extern "C"` declarations, representation carriers, compiler-managed output storage, release guards, and narrow unsafe adapters. It must consume semantic call ownership and must not rediscover borrowing from Rust signature requirements. Generated C probes and Rust sources remain inspectable artifacts rather than the public contract.

RFC 114 provider planning should select binding packages and target variants. RFC 020 locking should freeze all semantic and physical inputs. RFC 112 publication should protect generated verification records, shim outputs, and staged artifacts. Documentation, LSP, codegraph, package publication, compatibility tooling, and inspection should consume the same checked descriptor rather than reconstructing foreign facts independently.

## Layers affected

- **Parser and formatter:** parse and format `binding c`, `c::` imports, explicit `unsafe:` regions, opaque resource declarations, C symbol annotations, output positions, and explicit C structures as Incan syntax.
- **Name resolution and package graph:** resolve logical binding identities and bridge/façade relationships through the package graph while keeping binding kinds distinct.
- **Typechecker:** preserve exact C scalar, pointer, nullability, output, opaque resource, span, structure, unsafe-call, and scoped-view facts.
- **Semantic ownership analysis and HIR:** record call-scoped shared or mutable borrows, ownership-consuming moves, close state, and scoped-view validity before lowering.
- **Lowering and backend emission:** consume checked binding and ownership facts, lower calls through contained unsafe mechanics, preserve calling conventions, and emit output storage and release guards.
- **Compiler C vocabulary and `std.c`:** provide checked C types, conversions, spans, output helpers, scoped foreign views, encoding validation, and redaction-safe diagnostics.
- **Target toolchains:** provision or select Clang-compatible target environments, sysroots, Android NDK profiles, Apple Xcode/SDK identities, and target verification inputs.
- **C ABI verification:** compile signature and layout probes, key results by complete semantic and physical identity, and produce source-anchored diagnostics.
- **Provider, package, and lock metadata:** resolve declarations, shims, native artifacts, deployment classes, features, capabilities, provenance, and offline identities through existing provider machinery.
- **CLI and build tooling:** build, check, lock, package, target, cache, and inspect bindings consistently across cold, warm, offline, relocated, and cross-compiled builds.
- **Platform packaging:** emit complete Android and Apple native artifact plans while leaving final application assembly and signing to the selected platform packager where required.
- **LSP, codegraph, and documentation:** expose raw declarations, safety requirements, ownership modes, target availability, bridge edges, safe façades, diagnostics, reference docs, and provenance.
- **Governed runtime and receipts:** identify native binding and symbol use without exposing raw pointers, secrets, credentials, or machine-local paths.

## Design Decisions

- RFC 116 uses explicit `import std.c as c` and a distinct `c::` binding namespace.
- C bindings use a declaration-only contract, private bridge, and ordinary public Incan façade.
- Raw declarations mirror exact C ABI signatures; safe grouping and domain semantics live in Incan wrappers.
- Owned resources associate one release operation with the resource declaration.
- Raw status values remain raw; nullability may map to `Option`, while application `Result` types are Incan-authored.
- Call-scoped foreign borrows are inferred from semantic parameter modes and recorded before backend emission.
- `c.Out[T]` and `c.InOut[T]` represent output positions without enabling arbitrary dereference.
- Returned foreign views may be copied within the current unsafe region or declared static; owner-tied escaping views are excluded.
- Shims are first-class governed package assets.
- Verification requires a managed Clang-compatible target toolchain.
- Native deployment classes are static, bundled, and explicit system capability.
- The package manager owns resolution, verification, locking, caching, and staging, while final platform packaging may own application assembly and signing.
- The shared package envelope is binding-kind-neutral; C-specific type and safety facts remain inside the C binding kind.

## Unresolved questions

- What final parser spelling best associates an opaque resource with its release operation while keeping the declaration readable and extensible?
- What final constructor and initialization-state surface should private bridge code use for `c.Out[T]` and `c.InOut[T]`?
- How should platform-dependent C enum identity and width be represented before a broader representation contract exists?
- What exact manifest schema should represent binding declarations, source-built shims, static artifacts, bundled libraries, system frameworks, and platform packaging outputs?
- Which Android and Apple integration artifacts should `incan package` emit so Gradle and Xcode consume one complete native plan without duplicating source-of-truth configuration?
- Which package signing and license-policy facts are mandatory before prebuilt native artifacts may be published through `incan.pub`?

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
