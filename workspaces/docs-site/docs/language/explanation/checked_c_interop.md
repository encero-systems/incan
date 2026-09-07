# How checked C interop is structured

Checked C interop is a language-owned contract for a deliberately small foreign boundary. It is not a header importer, an ambient linker search, or a claim that C pointers already have Incan ownership semantics.

## One declaration authority

The binding source is the authority for the Incan-facing names, C scalar categories, native spellings, ownership and output contracts, and supported plain layouts. The compiler uses the declaration to construct a target-specific C probe; it does not scrape a header to invent a public API or infer safety from generated Rust.

```mermaid
flowchart LR
  S["Incan binding declaration"] --> T["Typechecked C descriptor"]
  T --> V["Clang target probe"]
  V --> R["Verified ABI facts"]
  R --> L["Generated private Rust C ABI bridge"]
  L --> F["Ordinary Incan facade"]
  F --> A["Application API"]
```

The probe is syntax-only. It checks free-function signatures, folds declared enum constants, and checks requested plain-structure size, alignment, and field offsets for the selected host ABI. It neither links the native library nor executes its code. The later generated build still links the system library named by `c.system_library("name")`.

## Inspection is a projection, not another authority

[`incan inspect bindings`](../../tooling/how-to/inspect_checked_c_bindings.md) runs the ordinary checked compilation analysis and projects its binding descriptors for people and tools. It does not reparse vocabulary syntax, scrape headers, or interpret generated Rust. An invalid declaration therefore produces the normal compiler diagnostic instead of a plausible-looking partial report.

The declaration projection deliberately does not absorb facts with different lifecycles:

| Surface | Question it answers |
| --- | --- |
| Binding inspection | What ABI, ownership, output, enum, and layout contract did the compiler accept from this source graph? |
| `[oven.interop]` and `oven.lock` | What target requirements and package-owned physical inputs did the author declare and lock? |
| Oven receipt and store | Which explicitly selected toolchain and SDK, verified package artifacts, and shim outputs satisfied those requirements? |
| Codegraph and LSP projections | Which checked declarations and explicit unsafe calls occur at these source spans? |

Keeping these projections separate prevents a source inspection from being mistaken for evidence that an artifact was resolved, a shim was built, or a mobile package is ready. They can still share stable binding identities as the tooling vertical grows.

## Why the boundary begins with C declarations

C is an ABI, not a complete ownership model. A header can expose an integer function accurately while saying little about which pointer owns a resource, when a callback expires, or how a caller must size an output buffer. Pretending that a foreign declaration is already a safe Incan API would hide those decisions at exactly the wrong boundary.

The current surface adds one narrow ownership model without widening into general pointers. A binding may declare an opaque resource, one matching release operation, and whether each call consumes, shares, or mutably borrows that resource. It may also declare scalar and owned-resource output positions. For text, `c.cstr(value)?` supplies one compiler-owned NUL-terminated input and a returned `const char *` can only be copied immediately through `copy_utf8(max_bytes=...)`. These are compiler-owned call facts: the source does not expose raw addresses, and generated Rust does not infer ownership from its own requirements. The façade above the binding remains responsible for input validation, native status interpretation, error models, retries, and cancellation.

## C interop and Rust interop solve different problems

Neither choice is universally better:

| Choose | When it is the better fit today |
| --- | --- |
| Checked C binding | The supported foreign boundary is a small, stable C ABI whose scalar calls, opaque handles, and output positions can be declared and verified exactly. |
| Rust interop | A maintained Rust crate already exposes the safe API you need, especially for resources, callbacks, async work, collections, or richer types. |
| A future checked shim | The underlying C API is real but needs an adapter for callbacks, variadics, function tables, bitfields, unions, or lifetime relationships. |

The language in which a library happens to be implemented is not decisive. A C++ engine, a Python extension, or a Rust library may intentionally publish a C ABI; a Rust wrapper can still be preferable when it owns difficult safety and build concerns well. Conversely, a small C ABI can be clearer and more durable than a wrapper when it is the producer's published contract.

## What is deliberately not claimed yet

The checked boundary itself does not discover interop artifacts, provision Android or Apple targets, or hand an application assembly to Gradle or Xcode. It also does not make `c.system_library("name")` a portable library-discovery mechanism. Those jobs need target-specific artifact identity and packaging facts, which are distinct from the source ABI declaration.

A package can declare target-specific, package-relative headers, static or bundled artifacts, system capabilities, C/C++ shim sources, compatible toolchain or SDK capabilities, and an Android API level or iOS deployment target under `[oven.interop]` in `incan.toml`; `incan lock` then records those normalized requirements and the content-derived identities of package-owned files. The declaration is intentionally binding-kind-neutral, so a future JNI, Python-extension, or other interop entry point can consume the same package-level evidence without replacing the language binding as ABI authority.

`incan check --interop-target <triple>` checks the source-owned C ABI against one such declared target. The Android API level or iOS deployment target becomes part of Clang's exact target triple, and the target's definitions apply to every probe. This is verification only: it does not cross-compile generated Rust, link or stage artifacts, or turn a compatibility requirement into a claimed local toolchain selection.

`incan inspect interop-plan --target <triple>` then projects the current locked requirements into one deterministic, versioned handoff. It preserves portable input receipts, include roots, definitions, dependency-ordered static links, bundled placements and runtime names, explicit system capabilities, shim inputs and logical outputs, and platform constraints without hard-coding a Gradle or Xcode command protocol.

#944 completes the v0.5 Oven handoff: the explicit baker turns the locked target, supplied compiler/SDK evidence, and declared package files into a receipt-bound native plan, and the stager emits a narrow Android/iOS consumer layout. It does not reinterpret package declarations, discover libraries from the host, invoke Cargo, or make Gradle/Xcode application assembly and signing compiler authority. Virtual-device consumers verify the handoff; physical-device deployment remains outside the release criterion.

The same restraint still applies to general pointers. The v0.5 foundation supports NUL-terminated text input, an immediately copied bounded UTF-8 `const char *` result, and two caller-owned bounded span forms: bytes and `f32` elements. A declaration must pair the span's compiler-owned pointer with its length or capacity, and a mutable span is returned only after the native write count has been validated against that capacity. These forms do not expose raw addresses or allow a view to escape. Arbitrary pointer operations, other element representations, callbacks, variadics, and context-manager syntax need separate lifetime and bounds contracts. Opaque resources and output storage remain private compiler-managed carriers, while public APIs use ordinary Incan values.

For a working first binding, start with the [tutorial](../tutorials/checked_c_binding.md). For declaration recipes and diagnostics, use the [binding how-to](../how-to/checked_c_bindings.md). To review what the compiler accepted, use the [inspection how-to](../../tooling/how-to/inspect_checked_c_bindings.md). The [`std.interop` reference](../reference/stdlib/interop.md) is the exact syntax and capability contract.
