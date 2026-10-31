# RFC 164: Explicit ownership and borrowing contracts

- **Status:** Planned
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
- **Issue:** [#2500](https://github.com/encero-systems/incan/issues/2500)
- **RFC PR:** —
- **Written against:** v0.6.0-dev.9
- **Shipped in:** —

## Summary

Add optional explicit ownership contracts alongside duckborrowing: `Borrowed[T]` for shared access, `BorrowedMut[T]` for exclusive input access, `Owned[T]` for ownership transfer, and `Reference[T] from source` for returned exclusive access to existing storage. Returned references support fluent chains and nested projections without copying or retaining their owners independently. Ordinary annotations and calls retain RFC 129's inference and mutation rules. Result-source relationships survive `Result` and other containers; failures propagate without implicit rollback. Implementation is targeted for 0.6; this specification and its examples do not claim shipped support.

## Core model

1. Ownership mode and value type are distinct: a `Page` can be owned, borrowed read-only or borrowed exclusively.
2. An explicit mode is an obligation for both caller and implementation. Inference may find a valid realization but may not weaken it.
3. A borrowed or reference result retains checked dependencies on the storage it accesses. Naming a source does not prove that the function actually returns from it.
4. Owning a reader or cursor differs from exclusively owning every byte it can read. A reader may own a lease retaining immutable shared pages.
5. No borrow conflict is repaired by an implicit clone, whole-container copy, new lease or runtime ownership conversion.
6. `Reference[T]` is exclusive borrowed access returned for continued use, not a mutation receipt or an independently retained object. Shared returned views use `Borrowed[T]`.
7. Borrow validity follows control flow and dependent uses, including result variants, fields and closures. Returning `Err` is not rollback or an unconditional release of every borrow.

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

`Borrowed`, `BorrowedMut`, `Reference` and `Owned` are compiler-recognized prelude type forms: ordinary use requires no import. Recognition follows their canonical declaration identity, not a matching user-defined name. The examples are design acceptance cases, not reports of successful compiler runs. Application types and helpers are defined below; ordinary `Result`, `Ok`, `Err`, `print` and collection operations use the language's existing surfaces.

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

### Return exclusive access for fluent calls

```incan
class Address:
    city: str
    postcode: str

    def set_city(mut self, city: str) -> Reference[Self] from self:
        self.city = city
        return self

    def set_postcode(mut self, postcode: str) -> Reference[Self] from self:
        self.postcode = postcode
        return self


model Customer:
    name: str
    address: Address


def update_customer() -> None:
    mut customer = Customer(
        name="Sam",
        address=Address(city="Utrecht", postcode="3511"),
    )
    customer.address.set_city("Rotterdam").set_postcode("3011")
    print(customer.address.city)  # Rotterdam; the original nested address.
```

Each call returns access to the same address. The intermediate reference carries exclusive access into the next call; when the chain ends, ordinary access resumes. No extra accessor is needed for a public nested field. Returning `Reference[Self]` from a method on the customer would instead continue the chain on the customer; there is no implicit return to a parent object.

A free function can state the input and output contracts separately:

```incan
def move_city(
    mut address: BorrowedMut[Address],
    city: str,
) -> Reference[Address] from address:
    address.city = city
    return address


def update_through_function() -> None:
    mut customer = Customer(
        name="Sam",
        address=Address(city="Utrecht", postcode="3511"),
    )
    move_city(customer.address, "Rotterdam").set_postcode("3011")
```

`BorrowedMut` requires exclusive borrowed input access; `Reference` describes returned exclusive access. They share exclusive-borrow semantics, with different authoring roles. `mut` remains binding permission: an immutable binding does not gain permission to mutate merely because its type can carry exclusive access. In particular, `mut value: Borrowed[T]` cannot upgrade a shared borrow. When retaining a writable returned reference, use `mut local = operation(...)`.

### Nested references retain the field they came from

The following class deliberately exposes a reference through a method to illustrate a library boundary; direct `customer.address` access remains sufficient for a public field.

```incan
class AddressBookEntry:
    name: str
    address: Address

    def edit_address(mut self) -> Reference[Address] from self.address:
        return self.address


def edit_independent_fields() -> None:
    mut entry = AddressBookEntry(
        name="Sam",
        address=Address(city="Utrecht", postcode="3511"),
    )
    mut address = entry.edit_address()
    entry.name = "Samuel"  # A proven disjoint sibling field.
    address.set_city("Rotterdam")
    print(entry.address.city)  # The retained reference is no longer used.
```

`from self.address` specifies the storage that remains borrowed after the call. The method may need whole-receiver mutable access while executing; its returned reference need only retain the declared field dependency. The caller does not need the implementation body to learn this distinction. Passing `entry.address` to `move_city` likewise substitutes that field path for the source parameter.

The rejected examples below each start with a fresh entry and use the same declarations above:

```incan
def conflicting_field_edit(mut entry: AddressBookEntry) -> None:
    mut address = entry.edit_address()
    entry.address.city = "Amsterdam"  # Rejected: overlaps retained exclusive access.
    address.set_city("Rotterdam")


def conflicting_parent_replacement(mut entry: AddressBookEntry) -> None:
    mut address = entry.edit_address()
    entry = AddressBookEntry(
        name="Alex",
        address=Address(city="Amsterdam", postcode="1011"),
    )  # Rejected: replaces the owner of storage still needed below.
    address.set_city("Rotterdam")
```

The compiler must identify the reference-producing call, overlapping edit or replacement, and dependent later use. It must not follow the replacement owner or copy the old address to repair the conflict. A whole-receiver mutating call overlaps a retained field reference unless its checked public access contract proves otherwise; a result-source clause describes retained result dependencies, not every effect during execution.

### Fallible chains and recovery

```incan
error BufferError:
    Full(capacity: int) = "Buffer capacity {capacity} has been reached"


class SampleBuffer:
    capacity: int
    samples: list[int]

    def append(
        mut self,
        sample: int,
    ) -> Result[Reference[Self], BufferError] from self:
        if len(self.samples) >= self.capacity:
            return Err(BufferError.Full(capacity=self.capacity))
        self.samples.append(sample)
        return Ok(self)

    def append_all(
        mut self,
        samples: list[int],
    ) -> Result[Reference[Self], BufferError] from self:
        for sample in samples:
            self.append(sample)?
        return Ok(self)


def fill(mut buffer: SampleBuffer) -> Result[None, BufferError]:
    (buffer.append(10)?).append_all([20, 30])?
    return Ok(None)


def recover() -> Result[None, BufferError]:
    mut buffer = SampleBuffer(capacity=2, samples=[])
    match fill(buffer):
        Ok(_) =>
            print("All samples added")
        Err(error) =>
            print(error)
            assert len(buffer.samples) == 2
            buffer.capacity = 3
            buffer.append(30)?
    assert len(buffer.samples) == 3
    return Ok(None)
```

Appending `10` and `20` succeeds. Attempting `30` fails before inserting it, leaving `[10, 20]`. `?` propagates that failure through `append_all` and `fill`; it does not undo successful changes. This example uses RFC 131's error declaration for readability; borrowing and `Result` behavior do not require that declaration syntax and must also work with existing error types.

`BufferError` carries an integer, not a reference into the buffer. Once the failed operation and temporary borrows finish, the caller can access the original buffer, enlarge its capacity and retry. In contrast, an error that explicitly retains a borrowed view must retain its checked dependency. An `Err` variant must never unconditionally erase dependencies. Diagnostics must not unexpectedly retain exclusive access to application objects merely to display an error.

An atomic operation must implement its own all-or-nothing contract, for example by validating the complete input first or using an explicit transaction. Neither `Reference`, `Result` nor `?` promises rollback. Discarding an unused returned reference ends that reference's dependent use; it does not undo mutation or suppress a returned error. Existing explicit-discard and must-use rules continue to apply.

### Duckborrowing is the ordinary surface of the same contract

`fill(mut buffer: SampleBuffer)` above uses RFC 129's caller-visible mutation rules. It could instead explicitly declare `mut buffer: BorrowedMut[SampleBuffer]` to require exclusive borrowed input. Both reach the caller's buffer in this example; the explicit form constrains inference rather than introducing another safety system.

Ordinary code keeps bare `T`, `mut`, field access and ordinary calls. No reference wrapper needs to be constructed at a call site. A library can explicitly require shared access, exclusive access or ownership transfer and can publish a returned source relationship. Duckborrowing must satisfy those obligations without changing identity, silently cloning or adding retention. Bare scalar parameters and imported types retain RFC 129's existing rules, so `mut T` must not be described as a universal synonym for `BorrowedMut[T]`.

A retained writable reference uses `mut address = move_city(customer.address, "Rotterdam")`; a bare local binding provides read-only use under RFC 129 and does not automatically surrender the outstanding exclusive dependency. Local assignment through a writable reference writes through to its target; it does not retarget the reference. Constructing or explicitly cloning an independent value remains a separate operation.

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

`BorrowedMut[T]` must require exclusive borrowed access, with mutation through a binding requiring `mut`. `Reference[T]` must carry the same exclusive access semantics as a returned reference, including when nested in a result or stored in a local. Its source relation must remain attached to every derived reference. Neither form grants independent retention or a copy. `Reference[T]` must not be manufactured from a shared-only source. Exclusive access must remain valid for its required lifetime. The source must permit that access. Reborrowing may temporarily suspend the parent's conflicting access; it must not produce two simultaneously usable exclusive accesses to overlapping storage.

`Owned[T]` must require an owned value of `T`. A non-copyable input is transferred; a copyable input may be copied according to its checked contract. A live conflicting borrow must prevent transfer. Owning a value must not erase borrowing dependencies inside that value, imply independent mutable interiors, or grant thread-safety properties.

Ordinary `T` must retain the inference rules of its surrounding declaration. An explicit mode must constrain inference and must not be an optimization hint. Aliases and generic substitutions must preserve that mode. User-defined types with similar names must not gain compiler privileges by spelling alone. These forms must not require a runtime wrapper allocation solely to represent the annotation.

Shared access must obey the underlying type's checked interior-mutability operations where such operations exist; this RFC does not make shared access a promise of deep immutability. Interior mutation cannot invalidate a live safe projection. Any runtime guard needed by the imported or declared contract must remain attached to that projection's validity.

### Bindings and mutation

Explicit borrowed bindings must preserve RFC 129's target identity and write-through semantics. Assignment through a writable borrowed binding replaces the borrowed value; it does not retarget the binding. Assignment to a mutable owned binding replaces that binding's own value.

Unlike a bare `T` annotation, an explicit `Owned[T]` initializer requests ownership rather than a local alias. Initializing it from a non-copyable place must perform a legal transfer or fail. This is an explicit extension to RFC 129's rule that ordinary type annotations do not request a copy or change alias meaning. It must never be inferred merely because a borrowed annotation would be inconvenient.

For parameters, explicit ownership modes must take precedence over the ordinary inferred passing mode. A writable exclusive borrow writes through to its source; replacing a mutable owned parameter changes the callee's own value. Existing immutable, static and interop access restrictions remain enforced; the annotation cannot create permission to mutate an otherwise forbidden source.

### Result dependencies

The `from source` result clause names one source parameter or receiver binding, optionally followed by statically resolved field projections such as `from self.address`. It is not a type, imported namespace, function call or arbitrary runtime expression. A source parameter's field path at the call site must be substituted into the published dependency. The clause requires an existing borrowed source whose storage remains valid after return; naming an owned, consumed local parameter must not legalize a dangling result. The implementation must prove the returned borrowing dependency is permitted by that source. Returning a borrow of a local temporary must not satisfy a clause naming an unrelated input.

The caller must retain the required source validity for every dependent use. Dependencies must propagate through fields, containers, aliases, branch joins, closures and other returned structures; putting a borrowed value inside an owned outer value must not erase its origin. The clause applies through result constructors and nested aggregates, not only to a top-level reference: `Result[Reference[T], E] from source` retains the dependency of its active payload. A value must not lose that dependency merely because it is wrapped, matched, propagated or passed through a callable. Consuming a result in a branch where no payload retains a borrow permits release when no other dependent use remains; an uninspected retained result must conservatively preserve its possible dependencies.

This contract specifies one named source path per result clause. All borrowed components of a result must be justified by that source, with disjoint projections checked individually. A result requiring independent sources that cannot be expressed by this clause must receive a diagnostic identifying those sources; inference must not guess one or invent a public lifetime signature. General multiple-source spelling, independently parameterized borrowed-field declarations and higher-ranked source-level lifetime binders are extensions beyond this syntax, not prerequisites for the accepted single-source surface. Existing checked native or inferred metadata must remain intact; an unrepresentable public contract must be diagnosed rather than erased.

Lifetime checking must account for control flow and last dependent use. It must not rely on textual last occurrence in one block. A later capture, cursor continuation, task or suspended frame must retain every dependency it uses. Borrowing across `await` or `yield` is permitted only when lifetime, exclusivity and required task-safety properties are proved; neither blanket acceptance nor an implicit copy is a substitute.

A returned shared borrow from an exclusive input must still prevent conflicting access to its source for its required lifetime. Imported native lifetime relationships and reborrows must be preserved rather than reconstructed from parameter names. Unsupported native contracts must produce an actionable diagnostic, not lifetime erasure.

### Owned resources and retained views

An owned handle may retain shared immutable storage through a checked resource contract. Ownership of that handle must preserve its release obligations and all other retained dependencies. It must not imply that a database lease has been acquired unless the library operation actually acquires or transfers one.

A borrowed projection must not outlive its buffers, guards, mappings or required lease. A movable or independently retained projection must explicitly own whatever keeps it valid. Storage compaction, buffer refill, owner replacement and release must respect those dependencies. This RFC supplies the language-level proof; the owning library supplies runtime retention and concurrency guarantees.

No default reference-counting strategy, pinning mechanism or universal lease type is introduced. Stable-address and self-referential cases must be expressed through supported ownership contracts or diagnosed; the compiler must not assume that moving an owner preserves borrowed addresses.

### Public contracts and diagnostics

Callable types and published metadata must preserve explicit modes and result dependencies. Implementations, overrides and adapters must satisfy the declared contract; they must not silently replace a shared parameter with a consuming one or erase a result dependency. Callable contracts must retain the source relation by parameter identity or position under renaming, rather than the provider's local spelling. Ordinary callable inference may preserve a relation without rewriting it as a user annotation. An explicitly written callable annotation must not erase a dependency: if its source syntax cannot express the required relation, the checker must diagnose the mismatch and identify the missing source relationship. Overrides and adapters may not require stronger input access or return broader borrowing dependencies than the declared contract permits. This RFC does not introduce new higher-ranked callable syntax.

Diagnostics must identify the requested mode, source place, conflicting action and dependent use or escape. Tooling should distinguish inferred and explicit contracts. An author must be able to inspect whether a result borrows, owns materialized data or owns a handle retaining shared storage. A value's name or a presumed method convention must not supply that evidence.

### Acceptance scenarios

Adoption must demonstrate accepted and refused programs for shared field returns, exclusive write-through, explicit transfer, valid reborrowing, owner replacement, ambiguous result sources and returned aggregates retaining borrows. Library-boundary checks must work without reading provider bodies. Acceptance must also cover fluent chains, nested field-source substitution, disjoint sibling edits, rejected overlapping edits and owner replacement, retained `mut` references, success/error branch sensitivity, partial mutation on failure and recovery through the original owner. Storage-shaped and graph-shaped fixtures must demonstrate borrowed versus retained readers, invalid cursor escape, buffer reuse conflicts and valid retained batches without requiring the complete libraries to ship. Native interop must preserve the same source contracts in both calling directions. Source diagnostics and actual execution must agree; successful type checking alone is insufficient.

## Design details

### Relationship to existing borrowing RFCs

RFC 129 remains the authority for ordinary inferred bindings and its `mut` contract. This RFC defines an optional explicit boundary and names where it extends the ordinary annotation rule. It does not reopen write-through assignment or permit hidden clone repair. RFC 005 and RFC 023 remain historical records of earlier interop and inference contracts; this proposal supplies a future explicit surface without requiring those annotations in ordinary code.

RFC 121 owns canonical Incan/Rust type identity. Borrowing annotations must use that identity rather than introduce lookalike owned and borrowed user types. Existing native reference annotations remain supported and must preserve their canonical access and lifetime contracts. Native shared references correspond to shared borrowed access and native mutable references to exclusive access; no extra wrapper allocation or cloning is implied. String and slice projections require checked representation-compatible conversions: `Borrowed[str]` must not silently equate distinct native string representations. Unsupported native lifetime relationships require diagnostics at the boundary, not guessed ownership. This extends the recognized surface without making Incan users write native lifetime syntax for ordinary code. RFC 128 must preserve the resulting contracts in callable metadata and syntax.

### Storage and graph consumers

RFC 165 owns the typed record and projected-batch contracts; RFC 166 owns snapshot-bound graph reading. Both should use this RFC for language-level borrowing and specify their library-specific retained owners separately. A database-backed cursor may own a lease while an in-memory reader borrows its graph. The common graph interface must preserve that distinction without forcing storage into ordinary memory graphs.

This dependency does not establish a mandatory new spelling for every storage method. Their projection and cursor signatures must satisfy this ownership contract before adoption. Borrowing semantics must not depend on the database, graph or compiler package being imported.

### Compiler authoring

An author may borrow a compiler builder or session while constructing a representation that describes borrowing in the eventual target program. These are separate contracts: host-language borrows govern execution of the compiler; target ownership facts govern execution of the compiled program. Neither may be inferred from the other or from a source-role label.

## Alternatives considered

- **Inference alone:** retains the shortest source, but does not provide the explicit public constraints and ambiguous result relationships needed here. It remains the default for ordinary code.
- **A `Mutated` receipt instead of a reference:** does not provide access for fluent continuation and could incorrectly imply that an operation must change the value. `Reference` states the access returned, including no-op updates.
- **Require native reference and lifetime syntax everywhere:** would expose low-level detail at ordinary call sites. The proposal instead names access modes and retains call-site inference; existing native reference spellings remain compatible through checked access and source relationships.
- **Freely implementable ownership traits:** cannot establish memory validity or exclusivity. Compiler-checked forms may participate in ordinary generic APIs but their safety meaning cannot be implemented arbitrarily.
- **Always copy returned views:** violates explicit borrowing and projected-access cost contracts. Owned materialization remains an explicit operation.
- **Always retain through shared handles:** changes release timing, allocation and retention costs and cannot express exclusive borrowed access. Libraries may expose owned retention where appropriate.
- **Put borrowing inside the storage RFC:** would make a general language contract appear to depend on one consumer. Storage and graphs instead reference this RFC.

## Drawbacks

Explicit modes add vocabulary and expose lifetime errors that inference cannot hide. Result dependencies must survive separate compilation, generics, callable values and native interop. Keeping explicit modes alongside `mut` requires diagnostics that distinguish access mode from binding permission. Retained readers reduce lexical restrictions but can retain substantial storage; ownership syntax does not replace library-level cost reporting. The single-source contract intentionally does not invent syntax for independently parameterized multi-source views; unsupported boundaries must be diagnosed explicitly.

## Layers affected

- **Parser and declarations:** ownership type forms, result-source clauses and their placement in signatures.
- **Type checking and ownership:** explicit access modes, transfers, result dependencies, reborrowing and escape checks.
- **Callable and library metadata:** preserving contracts across generic, callable and package boundaries.
- **Native interop and lowering:** canonical types, native lifetime relationships and checked representation choices.
- **Standard library consumers:** borrowed projections, buffer views and independently retained readers.
- **Diagnostics, formatter and editor tooling:** readable contract presentation and source-attributed conflict explanations.

## Design Decisions

- **Target release: 0.6.** Explicit borrowing is implementable independently of the later database, graph, debugging and primitives proposals. Supporting examples use those domains to test the contract, not to impose their delivery as prerequisites.
- **Duckborrowing remains the default.** Bare annotations, ordinary calls and RFC 129's `mut` rules keep their meanings. Explicit contracts constrain inference; they do not create another ownership system.
- **Prelude forms require no imports.** `Borrowed`, `BorrowedMut`, `Reference` and `Owned` are recognized through canonical declarations, never by arbitrary matching names.
- **Inputs and returned access have readable roles.** `Borrowed[T]` supplies shared access and read-only returned views; `BorrowedMut[T]` explicitly requires exclusive input access; `Reference[T]` expresses returned exclusive access for continued use. `Owned[T]` explicitly requests ownership.
- **Writable binding permission remains explicit.** Exclusive inputs and retained local references use `mut` when mutated. A fluent temporary can continue exclusive access from its mutable source. Shared access cannot be upgraded by adding `mut`.
- **Returned sources are checked.** `from parameter`, `from self` and static field paths such as `from self.address` identify one source of retained borrowing. The implementation must prove the relationship, and callers retain the precise source dependency.
- **Nested access preserves identity.** Direct dotted access needs no accessor. Proven disjoint sibling fields remain accessible; overlapping access, invalidating replacement and whole-parent operations conflict while a retained reference needs the storage.
- **Fluent calls keep the same target.** There is no hidden copy, automatic parent navigation, detached mutation or implicit independently retained handle.
- **Containers preserve borrowing.** `Result`, other aggregate values, aliases and callables retain the dependencies of the values they carry. Branches may release access only when no retained payload or other use needs it.
- **Failure is not rollback.** `Err` and `?` preserve already-performed changes. Errors that retain no borrow permit recovery through the original owner after temporary access ends; explicitly borrowed error payloads retain their dependencies. Atomicity belongs to the operation's documented implementation.
- **Last dependent use governs exclusivity.** Analysis accounts for branches, loops, reborrows, closures and suspension; it is not a textual end-of-block rule.
- **Native compatibility preserves safety.** Existing native references and canonical types retain their actual access and lifetime contracts. Unsupported projections or lifetime relationships receive diagnostics, not erased dependencies or fabricated clones.
- **Additional lifetime syntax is not introduced here.** Independent multiple-source clauses, explicitly lifetime-parameterized user fields and higher-ranked callable binders require a separate syntax extension. The accepted contract must reject a boundary it cannot express rather than pretending it is single-source.
- **Diagnostics explain the relationship.** Identify the requested access, originating source, conflicting operation and dependent use; expose inferred versus explicit contracts to tooling.
