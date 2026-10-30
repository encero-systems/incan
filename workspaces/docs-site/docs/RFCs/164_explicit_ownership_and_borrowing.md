# RFC 164: Explicit ownership and borrowing contracts

- **Status:** Draft
- **Created:** 2026-10-30
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 005 (Rust interop)
    - RFC 023 (implicit ownership and borrowing)
    - RFC 097 (Rust-hosted Incan caller)
    - RFC 121 (unified Incan/Rust type substrate)
    - RFC 128 (callable contracts)
    - RFC 129 (inferred bindings and the mut parameter contract)
    - RFC 142 (async iteration and structured execution)
    - RFC 165 (typed embedded storage)
    - RFC 166 (indexed graphs and snapshot-bound access)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.6.0-dev.9
- **Shipped in:** —

## Summary

Add optional, explicit ownership contracts to Incan through `Borrowed[T]`, `BorrowedMut[T]` and `Owned[T]`, with checked relationships between borrowed results and their sources. Ordinary `T` annotations retain duckborrowing. Library authors can require shared access, exclusive access or ownership transfer without exposing backend types, requesting a hidden clone or relying on inference to describe an ambiguous public boundary. The proposed result spelling is `-> Borrowed[T] from parameter`; its extension to multiple sources and aggregate results remains under review. These are proposed language contracts, not implemented APIs.

## Core model

1. Ownership mode and value type are distinct: a `Page` can be owned, borrowed read-only or borrowed exclusively.
2. An explicit mode is an obligation for both caller and implementation. Inference may find a valid realization but may not weaken it.
3. A borrowed result retains checked dependencies on the storage it accesses. Naming a source does not prove that the function actually returns from it.
4. Owning a reader or cursor differs from exclusively owning every byte it can read. A reader may own a lease retaining immutable shared pages.
5. No borrow conflict is repaired by an implicit clone, whole-container copy, new lease or runtime ownership conversion.

## Motivation

RFC 129 establishes inferred local borrowing, write-through assignment and diagnostics for conflicting access. That is appropriate for ordinary code. Public APIs also need to describe retained views, explicit ownership transfer, exclusive mutation and results tied to an input. These relationships must remain inspectable across library boundaries without requiring the caller to inspect an implementation body.

A database column view may borrow from a batch; an in-memory graph reader may borrow its graph; a compiler value may depend on one native compiler session. A persistent cursor may instead own a lease and remain valid after the local snapshot binding ends. Treating all these values as either independent copies or universally shared handles loses important differences in lifetime, cost and mutation rights.

This RFC owns that general language boundary. RFCs 165 and 166 own storage leases, batches, graph scopes and cursor behavior, and consume the ownership contract rather than inventing database-specific borrowing syntax.

## Goals

- Keep duckborrowing concise while making explicit ownership modes available to library and application authors.
- Express shared and exclusive borrows, ownership transfer and checked result dependencies in public signatures.
- Preserve source-visible aliasing, mutation and destruction behavior through inference and native lowering.
- Carry ownership facts through callable contracts, generics, returned values and imported declarations.
- Support borrowed projections and independently retained readers without mandatory per-row materialization or hidden copies.
- Explain failures using the source value, conflicting operation and use that retains the borrow.

## Non-Goals

- Replacing ordinary duckborrowing or requiring explicit annotations at every call.
- Giving arbitrary trait implementations authority to manufacture a valid reference or lifetime.
- Making ownership imply deep copying, exclusive physical allocation, authorization, thread safety or persistence.
- Defining database leases, query execution, graph algorithms or raw backend APIs.
- Claiming that the existing checker or native adapter implements these contracts completely.

## Guide-level explanation

All ownership forms below are proposed compiler-recognized type forms. The examples use them unqualified for readability; their public namespace and import spelling remain open. The `Page` model and every helper used in the examples are defined here. These are design test drives, not runnable compatibility fixtures.

### Return a view of a field

```incan
model Page:
    title: str
    body: str


def title_of(page: Borrowed[Page]) -> Borrowed[str] from page:
    return page.title


def demonstrate() -> None:
    mut page = Page(title="Introduction", body="A long document")
    title = title_of(page)
    print(title)
    # No later use retains the title borrow.
    page.title = "Getting started"
```

Read the signature as: borrow a Page and return a borrowed string from that Page. Calling the function does not require an `&page` expression. The result relationship is part of its interface, not a lifetime extension requested by its implementation.

The following ordering must be rejected:

```incan
def conflicting_access() -> None:
    mut page = Page(title="Introduction", body="A long document")
    title = title_of(page)
    page.title = "Getting started"  # Invalidates the later borrowed read.
    print(title)
```

The diagnostic must connect the call producing `title`, the replacement and the later read. It may suggest finishing the borrowed use first or explicitly materializing an independent value where that is supported. It must not silently change the result into an owned string.

### State exclusive access

```incan
def replace_title(mut page: BorrowedMut[Page], title: str) -> None:
    page.title = title


def edit_page() -> None:
    mut page = Page(title="Draft", body="Contents")
    replace_title(page, "Published")
    print(page.title)
```

The proposed form retains `mut` as writable binding permission and uses `BorrowedMut` to require exclusive borrowed access. `mut page: Borrowed[Page]` cannot upgrade a shared borrow into an exclusive one. The exact combination of these two spellings remains a review question; the exclusivity and write-through requirements do not depend on that choice.

### Require ownership transfer

```incan
def publish_title(page: Owned[Page]) -> None:
    print(page.title)


def transfer_page() -> None:
    page = Page(title="Guide", body="Contents")
    publish_title(page)
    # A later use of this non-copyable Page is rejected after transfer.
```

This contract requires ownership of the argument even though the body only reads one field. An author needing to preserve the original can explicitly supply a supported clone. The compiler cannot silently choose cloning to satisfy an `Owned` boundary. Types with a checked copy contract may be copied under that contract; `Owned` does not make every type move-only.

### Choose a borrowed view or an independently retained reader

| Consumer                            | Borrowing relationship                   | Valid escape                                                                     |
| ----------------------------------- | ---------------------------------------- | -------------------------------------------------------------------------------- |
| Field view                          | Borrows the containing value's storage   | Only while that storage remains valid                                            |
| In-memory graph read view           | Borrows the selected graph state         | Cannot outlive or conflict with that state                                       |
| Reusable-buffer projection          | Borrows the current buffer contents      | Cannot survive a refill that invalidates those contents                          |
| Database cursor owning a lease      | Owns retention of an immutable selection | May outlive the local snapshot binding if all required resources remain retained |
| Column view within a retained batch | Borrows the batch's buffers              | Requires the batch or an explicit independent buffer owner to remain valid       |

An owned batch may own buffer handles and a snapshot lease without owning an eagerly decoded model for every row. Its ordinary field views can still be borrowed. Acquiring independent retention is an explicit library operation or a declared constructor contract; the compiler does not introduce one to rescue an escaping borrow.

For a reusable buffer, a later use of the previous projection prevents a conflicting refill. For an immutable retained batch, advancing the cursor can remain valid when the API proves that the previous batch's buffers remain owned and unchanged. Both designs are legitimate; they must not have indistinguishable lifetime contracts.

## Reference-level explanation

### Modes and inference

`Borrowed[T]` must grant shared read access without transferring the referent's ownership. Multiple shared accesses may coexist subject to the underlying type's checked operations. It must not grant writable access to the referenced place.

`BorrowedMut[T]` must grant exclusive writable access for its required lifetime. The source must permit that access. Reborrowing may temporarily suspend the parent's conflicting access; it must not produce two simultaneously usable exclusive accesses to overlapping storage.

`Owned[T]` must require an owned value of `T`. A non-copyable input is transferred; a copyable input may be copied according to its checked contract. A live conflicting borrow must prevent transfer. Owning a value must not erase borrowing dependencies inside that value, imply independent mutable interiors, or grant thread-safety properties.

Ordinary `T` must retain the inference rules of its surrounding declaration. An explicit mode must constrain inference and must not be an optimization hint. Aliases and generic substitutions must preserve that mode. User-defined types with similar names must not gain compiler privileges by spelling alone. These forms must not require a runtime wrapper allocation solely to represent the annotation.

Shared access must obey the underlying type's checked interior-mutability operations where such operations exist; this RFC does not make shared access a promise of deep immutability. Interior mutation cannot invalidate a live safe projection. Any runtime guard needed by the imported or declared contract must remain attached to that projection's validity.

### Bindings and mutation

Explicit borrowed bindings must preserve RFC 129's target identity and write-through semantics. Assignment through a writable borrowed binding replaces the borrowed value; it does not retarget the binding. Assignment to a mutable owned binding replaces that binding's own value.

Unlike a bare `T` annotation, an explicit `Owned[T]` initializer requests ownership rather than a local alias. Initializing it from a non-copyable place must perform a legal transfer or fail. This is an explicit extension to RFC 129's rule that ordinary type annotations do not request a copy or change alias meaning. It must never be inferred merely because a borrowed annotation would be inconvenient.

For parameters, explicit ownership modes must take precedence over the ordinary inferred passing mode. A writable exclusive borrow writes through to its source; replacing a mutable owned parameter changes the callee's own value. Existing immutable, static and interop access restrictions remain enforced; the annotation cannot create permission to mutate an otherwise forbidden source.

### Result dependencies

The proposed `from parameter` result clause names a source parameter or receiver binding, not a type, imported namespace or runtime expression. The implementation must prove the returned borrowing dependency is permitted by that source. Returning a borrow of a local temporary must not satisfy a clause naming an unrelated input.

The caller must retain the required source validity for every dependent use. Dependencies must propagate through fields, containers, aliases, branch joins, closures and other returned structures; putting a borrowed value inside an owned outer value must not erase its origin. The exact syntax for multiple origins and aggregate results remains unresolved, but implementations must not substitute an unsound single-origin contract.

Lifetime checking must account for control flow and last dependent use. It must not rely on textual last occurrence in one block. A later capture, cursor continuation, task or suspended frame must retain every dependency it uses. Borrowing across `await` or `yield` is permitted only when lifetime, exclusivity and required task-safety properties are proved; neither blanket acceptance nor an implicit copy is a substitute.

A returned shared borrow from an exclusive input must still prevent conflicting access to its source for its required lifetime. Imported native lifetime relationships and reborrows must be preserved rather than reconstructed from parameter names. Unsupported native contracts must produce an actionable diagnostic, not lifetime erasure.

### Owned resources and retained views

An owned handle may retain shared immutable storage through a checked resource contract. Ownership of that handle must preserve its release obligations and all other retained dependencies. It must not imply that a database lease has been acquired unless the library operation actually acquires or transfers one.

A borrowed projection must not outlive its buffers, guards, mappings or required lease. A movable or independently retained projection must explicitly own whatever keeps it valid. Storage compaction, buffer refill, owner replacement and release must respect those dependencies. This RFC supplies the language-level proof; the owning library supplies runtime retention and concurrency guarantees.

No default reference-counting strategy, pinning mechanism or universal lease type is introduced. Stable-address and self-referential cases must be expressed through supported ownership contracts or diagnosed; the compiler must not assume that moving an owner preserves borrowed addresses.

### Public contracts and diagnostics

Callable types and published metadata must preserve explicit modes and result dependencies. Implementations, overrides and adapters must satisfy the declared contract; they must not silently replace a shared parameter with a consuming one or erase a result dependency. The full callable spelling and compatibility rules need review alongside RFC 128.

Diagnostics must identify the requested mode, source place, conflicting action and dependent use or escape. Tooling should distinguish inferred and explicit contracts. An author must be able to inspect whether a result borrows, owns materialized data or owns a handle retaining shared storage. A value's name or a presumed method convention must not supply that evidence.

### Acceptance scenarios

Adoption must demonstrate accepted and refused programs for shared field returns, exclusive write-through, explicit transfer, valid reborrowing, owner replacement, ambiguous result sources and returned aggregates retaining borrows. Library-boundary checks must work without reading provider bodies. Storage and graph consumers must demonstrate borrowed versus retained readers, invalid cursor escape, buffer reuse conflicts and valid retained batches. Native interop must preserve the same source contracts in both calling directions. Source diagnostics and actual execution must agree; successful type checking alone is insufficient.

## Design details

### Relationship to existing borrowing RFCs

RFC 129 remains the authority for ordinary inferred bindings and its `mut` contract. This RFC proposes an optional explicit boundary and names where it extends the ordinary annotation rule. It does not reopen write-through assignment or permit hidden clone repair. RFC 005 and RFC 023 remain historical records of earlier interop and inference contracts; this proposal supplies a future explicit surface without requiring those annotations in ordinary code.

RFC 121 owns canonical Incan/Rust type identity. Borrowing annotations must use that identity rather than introduce lookalike owned and borrowed user types. The precise mapping for native string and slice views needs a checked projection contract; `Borrowed[str]` must not silently equate every native string representation. RFC 128 must preserve the resulting contracts in callable metadata and syntax.

### Storage and graph consumers

RFC 165 owns the typed record and projected-batch contracts; RFC 166 owns snapshot-bound graph reading. Both should use this RFC for language-level borrowing and specify their library-specific retained owners separately. A database-backed cursor may own a lease while an in-memory reader borrows its graph. The common graph interface must preserve that distinction without forcing storage into ordinary memory graphs.

This dependency does not establish a mandatory new spelling for every storage method. Their projection and cursor signatures must be completed against the accepted ownership contract before adoption. Borrowing semantics must not depend on the database, graph or compiler package being imported.

### Compiler authoring

An author may borrow a compiler builder or session while constructing a representation that describes borrowing in the eventual target program. These are separate contracts: host-language borrows govern execution of the compiler; target ownership facts govern execution of the compiled program. Neither may be inferred from the other or from a source-role label.

## Alternatives considered

- **Inference alone:** retains the shortest source, but does not provide the explicit public constraints and ambiguous result relationships needed here. It remains the default for ordinary code.
- **Require native reference and lifetime syntax everywhere:** would expose low-level detail at ordinary call sites. The proposal instead names access modes and retains call-site inference; existing native reference spellings require compatibility review.
- **Freely implementable ownership traits:** cannot establish memory validity or exclusivity. Compiler-checked forms may participate in ordinary generic APIs but their safety meaning cannot be implemented arbitrarily.
- **Always copy returned views:** violates explicit borrowing and projected-access cost contracts. Owned materialization remains an explicit operation.
- **Always retain through shared handles:** changes release timing, allocation and retention costs and cannot express exclusive borrowed access. Libraries may expose owned retention where appropriate.
- **Put borrowing inside the storage RFC:** would make a general language contract appear to depend on one consumer. Storage and graphs instead reference this RFC.

## Drawbacks

Explicit modes add vocabulary and expose lifetime errors that inference cannot hide. Result dependencies must survive separate compilation, generics, callable values and native interop. Mixing explicit modes with `mut` needs careful diagnostics and a consistent authoring rule. Retained readers reduce lexical restrictions but can retain substantial storage; ownership syntax does not replace library-level cost reporting. This draft deliberately keeps the unresolved syntax visible rather than claiming that all difficult cases have been settled.

## Layers affected

- **Parser and declarations:** ownership type forms, result-source clauses and their placement in signatures.
- **Type checking and ownership:** explicit access modes, transfers, result dependencies, reborrowing and escape checks.
- **Callable and library metadata:** preserving contracts across generic, callable and package boundaries.
- **Native interop and lowering:** canonical types, native lifetime relationships and checked representation choices.
- **Standard library consumers:** borrowed projections, buffer views and independently retained readers.
- **Diagnostics, formatter and editor tooling:** readable contract presentation and source-attributed conflict explanations.

## Unresolved questions

- Should `Borrowed`, `BorrowedMut` and `Owned` be prelude type forms or imports from a standard ownership module, and what declaration identity makes their compiler recognition explicit?
- Is `from parameter` the preferred result spelling? How should multiple potential sources, independently borrowed result fields, nested aggregates and callable signatures express their relationships without guessing or excessive annotations?
- Should exclusive parameters require both `mut` and `BorrowedMut`, as illustrated, or can one spelling express writable permission and the exclusive mode consistently with RFC 129?
- What source syntax declares retained borrowed fields and their dependencies in user-authored generic views and cursors? The ownership and escape obligations above are required; field-level lifetime parameterization remains to be designed.
- How do existing explicit native reference annotations coexist with these forms, including string and slice projections, imported lifetime relationships and higher-ranked callable contracts?

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
