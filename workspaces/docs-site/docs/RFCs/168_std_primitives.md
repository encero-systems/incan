# RFC 168: `std.primitives`

- **Status:** Planned
- **Created:** 2026-10-31
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 056 (binary I/O)
    - RFC 064 (binary-text encoding)
    - RFC 065 (content hashing)
    - RFC 112 (crash-safe publication and file coordination)
    - RFC 114 (compiled providers and package features)
    - RFC 117 (Loaf project model)
    - RFC 124 (store unit identity and cross-plan artifact sharing)
    - RFC 131 (typed errors)
    - RFC 142 (async iteration and structured execution)
    - RFC 160 (Incan-authored vocabulary)
    - RFC 164 (explicit ownership and borrowing contracts)
    - RFC 165 (typed embedded storage)
- **Issue:** [#2501](https://github.com/encero-systems/incan/issues/2501)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Introduce `std.primitives` as the foundational Incan package beneath the higher-level standard library. It provides the value, resource, error, trait, asynchronous execution and vocabulary-authoring building blocks needed by compiler, platform and library code without dependencies on higher-level standard packages. Higher-level modules compose or re-export these facilities rather than duplicate their implementations. This RFC establishes the architectural direction and compatibility obligations; exact APIs and implementation organization are determined during implementation.

## Core model

A primitive provides a foundational operation or contract with explicit ownership and observable behavior. Higher-level libraries supply convenience, composition and domain policy. Using Rust interop does not by itself make an entire API primitive, and an implementation written entirely in Incan can still be primitive.

## Motivation

Compiler and platform code need to read files, process bytes, coordinate resources and express asynchronous behavior without depending on application-oriented standard-library APIs. Vocabulary authoring also needs foundational contracts beneath the surfaces it defines. Private implementations duplicate those facilities; importing their convenience modules introduces dependencies in the wrong direction.

One foundational package lets Incan author both its language surfaces and its libraries. Applications keep convenient APIs, while low-level consumers use the underlying operations directly.

## Goals

- Establish a dependency direction from higher-level libraries to foundational operations, with no reverse dependency.
- Keep primitive implementations simple and Incan-authored, using existing native libraries through explicit interop boundaries where needed.
- Make ownership, borrowing, resource lifetime and failure behavior explicit at low-level boundaries.
- Preserve existing higher-level imports and observable contracts through delegation or re-export.
- Include foundational errors, traits, async vocabulary and vocabulary-definition contracts.

## Non-Goals

- Specify every function, private type, platform module or implementation step before development begins.
- Replace higher-level standard libraries or require ordinary applications to use primitive APIs.
- Introduce class subtyping or require primitive and convenience types to be interchangeable.
- Move entire domain-specific native backends into primitives merely because they use Rust.
- Define a database, compiler pipeline, task orchestration policy or build engine.

## Guide-level explanation

### Choose the appropriate layer

The primitive path supplies explicit foundational operations. The higher-level path adds conveniences. These examples illustrate the intended relationship; the exact primitive join spelling remains implementation-stage API work:

```incan
# Compiler or platform code.
from std.primitives import Path

path = Path("sources").join("main.incn")
```

```incan
# Application code keeps the higher-level surface.
from std.fs import Path

path = Path("sources") / "main.incn"
```

`std.fs.Path` can extend the primitive `Path` class and add conveniences such as whole-file text reading. The primitive does not need slash notation or every convenience method. Code chooses its layer; the two Path classes need not be interchangeable at parameter boundaries. Higher-level path-building operations retain the behavior their callers expect.

### Read files without the higher-level filesystem library

Compiler code must be able to open, read and release files through primitives alone. The following complete helper uses illustrative proposed `open_read(path: Borrowed[Path]) -> Result[Owned[File], IoError]` and `read_chunk(file: BorrowedMut[File], max_bytes: int) -> Result[bytes, IoError]` contracts; it does not freeze those API names:

```incan
from std.primitives.fs import Path, open_read, read_chunk
from std.primitives.io import IoError

def read_source_chunks(path: Borrowed[Path]) -> Result[list[bytes], IoError]:
    mut file = open_read(path)?
    mut chunks: list[bytes] = []

    loop:
        chunk = read_chunk(file, max_bytes=65536)?
        if chunk == b"":
            break
        chunks.append(chunk)

    return Ok(chunks)
```

For this example, the positive bound limits each read, short reads are permitted and an empty read means EOF. The owned handle is released on return or error propagation. This helper retains the whole source; a streaming consumer could process each chunk immediately. Decoding, source locations and compiler diagnostics are not hidden behavior of opening a file.

### Compose behavior without duplicating mechanisms

| Foundation | Higher-level composition |
| --- | --- |
| File handles and byte operations | Text reading, recursive traversal, globbing and staging workflows |
| Borrowed read cursor and owned write buffer | Combined reading, writing and seeking through `BytesIO` |
| Byte-level hashing and encoding algorithms | Algorithm selection, whole-file hashing and source/sink conveniences |
| Precise environment observations | Defaults, typed configuration conversion and configuration policy |
| Async mechanisms and vocabulary | Task groups, deadlines, concurrency limits and error policies |
| Vocabulary descriptors and structured transformation contracts | Convenient vocabulary authoring and compiler-owned provider execution |

The algorithm boundary is not restricted to SHA-256 or hexadecimal conversion. Supported algorithms can have foundational implementations without pulling file processing or application defaults down with them. Existing native libraries remain valid dependencies; implementations are not duplicated merely to move their ownership.

## Reference-level explanation

### Dependencies and authoring

`std.primitives` must not depend on higher-level standard packages, including indirectly through helpers, trait defaults or vocabulary-definition contracts. Platform code must be able to use it as its sole standard-package dependency. Required language/runtime foundations and native dependencies must be accounted for without hiding dependencies on higher-level facades.

Higher-level modules must reuse primitive implementations for foundational operations. They may compose operations, adapt their presentation or preserve existing convenience contracts. Such composition is not a duplicate implementation of the primitive operation.

Primitive behavior should use simple Incan classes, functions, types, enums and traits as appropriate. Native interop boundaries must have explicit ownership. A domain-specific native backend remains with its domain unless reusable foundational operations can be separated from it. A Rust import is an inventory signal, not an automatic placement rule.

### Ownership and shared contracts

Primitive boundaries must expose ownership and borrowing obligations using RFC 164 where applicable. A read cursor borrows backing bytes while owning its position; a write buffer owns its storage. Borrowed views retain checked source dependencies. Copies, transfers and independent retention must not be silently substituted for one another.

Foundational errors and traits belong in primitives. Existing standard modules must re-export the same declarations where they expose the same contract. Supporting definitions required to avoid reverse dependencies belong with those contracts; not every domain-specific error or derive implementation moves into the foundation. Typed error behavior follows RFC 131's direction.

Host operations must report actual failures and support limits. Platform selection must not silently weaken an operation's meaning. Resource ownership must not imply guarantees about external writers, successful cleanup or durability that an operation does not provide. Exact signatures and error variants are implementation-stage API work within these obligations.

### Async and vocabulary

Foundational async vocabulary and contracts belong alongside primitive async mechanisms. `std.async` must continue to expose the same vocabulary and higher-level behavior. Importing both entry points must not create competing syntax definitions. Importing an unrelated primitive must not itself activate async vocabulary or require an async runtime.

Vocabulary-definition contracts also belong in the foundation, with `std.vocab` supplying re-exports and authoring conveniences. The compiler consumes these contracts; primitives must not depend on the compiler implementation to describe syntax. Provider admission, execution, validation and semantic compilation remain compiler/toolchain responsibilities under the relevant RFCs.

Relocating vocabulary ownership does not remove semantic obligations such as suspension and lifetime checking, nor require replacing an existing async runtime backend. Registration, bootstrap and backend organization are implementation details.

### Organization and compatibility

Definitions must be grouped by responsibility, with selected common declarations re-exported at the package root. The exact export list and private layout are implementation decisions. Platform-specific modules and compile-time re-exports are permitted approaches, not a prescribed hierarchy.

Existing higher-level imports and observable behavior must remain compatible unless a semantic change is explicitly adopted separately. Shared contracts retain identity through re-exports. Convenience types may extend or wrap primitive types; identical nominal types and interchangeability are not blanket requirements. Compatibility wrappers must delegate rather than retain competing primitive implementations.

Primitives are available to authors who need the low-level surface; ordinary application ergonomics remain the purpose of higher-level packages. Low-level status does not waive documentation, safety or compatibility obligations. Direct primitive use does not bypass applicable capability governance.

## Design details

Implementation must trace actual standard-library and native dependencies, including trait defaults and resource lifetimes, and use concrete compiler and library consumers to shape APIs. Detailed inventories, signatures, representation choices and platform adapters belong in implementation design and verification. Resolving them does not require another RFC-level design vote unless the implementation would change this architectural direction or an established semantic contract.

The architectural boundary, rather than the original list of 17 facilities, governs placement. Filesystem, byte, hashing, encoding, environment and temporary-resource operations are motivating examples. Foundational async and vocabulary facilities follow the same rule. An inventory entry does not establish that an API already exists or has shipped.

## Alternatives considered

- Move convenience modules wholesale: brings policy and higher-level dependencies into the foundation.
- Keep private compiler/platform primitives: duplicates behavior and prevents sharing the same foundation.
- Require identical types at both levels: constrains useful conveniences without being necessary for shared implementation.
- Treat every Rust-backed API as primitive: confuses implementation language with architectural responsibility.
- Freeze a complete API and platform-module inventory here: couples the architectural decision to details better tested with concrete consumers.

## Drawbacks

Separating existing modules requires compatibility work where types currently combine resource access, shared state and convenience policy. Relocating traits or vocabulary ownership also affects compiler recognition and package metadata. Implementation must verify those migrations without inventing parallel definitions or silently changing behavior.

## Layers affected

- **Standard library:** foundational definitions, delegation, compatibility re-exports and authoring APIs.
- **Platform:** direct foundational dependencies without higher-level standard packages.
- **Compiler and tooling:** recognition of relocated contracts and vocabulary, dependency metadata and diagnostics.

## Design Decisions

- **Architectural scope:** direction and obligations are settled here; exact APIs and organization are implementation work.
- **One foundation:** compiler, platform, vocabulary and library authors share primitives without reverse dependencies.
- **Separate Path surfaces:** a simple primitive class supports a richer higher-level class; interchangeability and primitive slash notation are not required.
- **Explicit byte ownership:** borrowed read cursors and owned write buffers underpin the combined `BytesIO` convenience surface.
- **Algorithms below orchestration:** foundational algorithms are not restricted to SHA-256 and hex; file processing and application policy stay above them.
- **Shared contracts:** foundational errors and traits retain identity through higher-level re-exports.
- **Async and vocabulary foundations:** async mechanisms and vocabulary use foundational vocabulary-authoring contracts; higher-level entry points remain available.
- **Grouped modules:** common root re-exports are supported; platform backend layout is an implementation choice.
- **Native boundary ownership:** shared low-level interop boundaries do not make all native-backed domain behavior primitive.
