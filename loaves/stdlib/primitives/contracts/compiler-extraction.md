## Compiler functionality that belongs in the foundation

The primitives inventory must also be driven by existing compiler and generated-program runtime needs, not just by today's public stdlib packages. The goal is one reusable implementation of each foundational operation, consumed by both normal programs and compiler code.

This is an extraction audit of the current integration baseline, not a claim that these moves are implemented. Proposed homes describe responsibility; they do not freeze additional public namespaces. Rust interop alone does not decide ownership: a module can combine reusable native operations with compiler policy.

### Concrete extraction candidates

| Current source | Reusable mechanism to extract or reuse | Meaning that stays with its current owner |
| --- | --- | --- |
| incan_native_runtime/src/text_codecs.incn | UTF-8 validation, owned decoding/encoding and explicit replacement decoding | Surface encoding-label validation, argument evaluation order, existing error conversion and frozen-value compatibility |
| incan_native_runtime/src/checked_c_strings.incn | Checked NUL-terminated storage and bounded UTF-8 validation | Foreign-call admission, ABI lowering and proof that a raw pointer cannot outlive its backing allocation |
| incan_native_runtime/src/lists.incn and collections helpers | Storage allocation, append, checked element access, replacement and explicit copying | Language indexing rules, duckborrowing decisions and selection of the checked operation during lowering |
| incan_native_runtime/src/lib.incn and panic_hook.incn | Low-level stdout/stderr writes and native hook access, where justified | Generated entrypoint initialization, panic formatting/exit behavior and compiler diagnostic presentation |
| incan_worker_host/src/endpoint.incn and lifecycle.incn | Host file-identity observation, monotonic time, thread/process mechanisms and existing platform IPC | Worker identity checks, idle/drain decisions, caller-abandonment policy and request protocol |
| internal_models/src/checker_records.incn | Existing primitive SHA-256/hex operations; byte framing only if a genuinely reusable contract is identified | Checker schema, canonical ordering, profile limits, owner/root identity validation and record authority |
| incan_native_runtime/src/generic_decorators.incn | Shared synchronization and initialization mechanisms where they match existing contracts | Closed-instance keys, publisher identity, recursive-initialization behavior and once-per-instance decorator semantics |
| incan_native_runtime/src/model_values.incn | Existing trait-driven formatting/comparison and lower-level serialization support | Derive selection, model semantics, JSON policy and generated adapter entrypoints |

A runtime helper being located under loaves/compiler does not prove it belongs there forever. Conversely, relocation into primitives must not make compiler-internal types into public standard-library contracts.

### First vertical test drive: source bytes to validated text

The source-reader example already separates opening/reading a file from keeping its bytes. The next useful operation is decoding those bytes. Today that implementation is in the compiler's native runtime and calls Rust's UTF-8 validator directly.

The intended arrangement is:

1. A primitive UTF-8 operation validates input and returns owned text or a structured decoding failure.
2. The compiler source loader calls it and adds the source path and diagnostic context.
3. Ordinary bytes.decode behavior calls the same mechanism through its existing language adapter, preserving supported labels, strict/replacement policy and error behavior.
4. The generated program links only the runtime providers it actually needs.

There must not be separate UTF-8 validators for the compiler and user programs. There may be different callers presenting the same underlying failure differently. Moving the mechanism does not silently replace today's ValidationError result with a new unrelated public error.

Acceptance includes valid multibyte input, malformed/truncated sequences, exact position information where the new primitive promises it, existing strict/replacement behavior and independent ownership of returned text. A borrowed text-view operation would be a separate RFC 164/native representation proof, not something implied by an owned decoder.

### The bootstrap recursion trap

The current list helper already documents the issue: empty_list allocates through native Vec so it does not recursively invoke its own language operation.

A primitive implementation cannot blindly use the public operation it implements:

- A text decoder implemented through ordinary bytes.decode may lower back into that same decoder.
- A list allocation primitive implemented with a list literal may call itself through lowering.
- A primitive error formatter that uses unavailable high-level display machinery can pull the higher-level dependency back into the foundation.

The bottom operation must terminate in an admitted native operation or an independently implemented lower-level algorithm. Prefer Incan-authored logic using existing rust:: interop; do not introduce a new authored Rust shim merely to relocate a helper.

Also distinguish a bootstrap compiler's own support closure from the dependencies shipped by the package it builds. Prove both. A package can appear source-clean while its generated code still links a broad runtime provider.

### Mechanism, adapter and compiler decision

There are three useful boundaries:

| Boundary | Example | Owner |
| --- | --- | --- |
| Native/general mechanism | Validate UTF-8, append storage, observe a monotonic clock | Primitive family or existing platform mechanism, according to the established layering |
| Language-runtime adapter | Preserve bytes.decode error behavior or negative-index semantics | Canonical language support; it may become library-authored without becoming a generic host primitive |
| Compiler decision | Resolve the call, check access, select the operation and attach source origins | Compiler implementation / future std.compiler surface |

The platform already owns IPC and tracing mechanisms. Reuse those owners; do not create a competing primitive IPC implementation merely because the compiler currently calls platform code. The precise platform-to-primitives closure still needs verification during migration.

Source spans, checked types, Body IR and compiler diagnostic schemas should not all be put into primitives.models. Pure vocabulary data already has its own extraction path. Compiler semantic facts remain compiler-owned.

### How this changes the implementation work

Add compiler/runtime extraction to the existing slices rather than open a competing implementation lane:

- Slice 02: trace the actual language-core and generated-runtime dependencies. Explain which declaration or emitted operation requested each provider.
- Slice 05: identify canonical shared errors/traits and compatibility conversions required by extracted operations.
- Slice 11: include UTF-8 and checked native text/buffer operations alongside byte algorithms. Preserve the public text-codec contract.
- Slices 12–13: verify vocabulary and runtime mechanism extraction without moving compiler execution or worker policy into the foundation.
- Slice 15: replace compiler/runtime callers only after the new provider is executable and its behavior and ownership are proved.

For each candidate record: current owner, proposed mechanism owner, adapter that remains, real consumers, dependency closure, native representation, failure behavior, bootstrap route and acceptance cases. A move is complete only when consumers use the canonical implementation and the old runtime route forwards or is safely retired.

### Immediate priorities

1. UTF-8 validation/decoding, because it completes the source-reader test drive and has an existing Incan-authored implementation.
2. Core/runtime dependency tracing, because it determines whether a primitive-only consumer really avoids higher-level packages.
3. Checked native storage and C-string boundaries, where ownership and foreign-call lifetimes meet RFC 164.
4. Collection and output mechanisms, preserving language semantics and existing native interop.
5. Worker/process/time reuse after distinguishing existing platform responsibilities from compiler lifecycle policy.

This audit adds no blanket authorization to move all compiler helpers into primitives. It establishes the concrete extraction obligation and the proof required for each move.

Source evidence is under loaves/compiler/incan_native_runtime/src/, loaves/compiler/incan_worker_host/src/, loaves/compiler/internal_models/src/checker_records.incn, loaves/compiler/incan_frontend/src/body_ir/text_codecs.rs and loaves/kernel/incan_lang/src/lang/text_codecs.rs at the integration baseline.
