# RFC 161: The semantic core (SCIR)

- **Status:** Planned
- **Created:** 2026-10-24
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 027 (`incan-vocab` block registration and desugaring)
    - RFC 081 (language-shaped DSL embeddings)
    - RFC 102 (semantic layer inspection surface)
    - RFC 104 (ambient runtime capabilities and receipts)
    - RFC 106 (compiler-backed agent context graph)
    - RFC 120 (canonical source symbol identity)
    - RFC 129 (shared or copied bindings and the `mut` parameter contract)
    - RFC 160 (vocabulary in Incan and native desugarers)
    - RFC 162 (source surfaces: vocabularies, dialects and frontends)
- **Issue:** [#2462](https://github.com/encero-systems/incan/issues/2462)
- **RFC PR:** —
- **Written against:** v0.6 (in development)
- **Shipped in:** —

## Summary

This is a strategic RFC. It defines SCIR, the semantic core of Incan: a small, checked, explicitly owned core language that every Incan program normalizes into before backend lowering. SCIR has records, enums, traits, implementations and functions. Its bodies are made of places, explicit copies, moves and loans, calls and structured control flow. Its operations that need authority name the capability they require. Surface constructs are named normalizations onto that core:
- a `model` becomes a record with derived behavior and a constructor;
- a `class` becomes a record with its implementations and trait adoptions;
- an `error` becomes an enum or record with its error implementation.

Compiler-owned vocabulary normalizes the same way, and library vocabulary under RFC 160 may target SCIR in a later schema version. RFC 104's capabilities are part of the core, not a layer on top. A normalization cannot introduce, hide or drop authority-bearing work. A function's capability requirements are derived from its SCIR. RFC 102's inspection surface and RFC 106's graph read declarations, edges and authority from one place. SCIR is not part of 0.6. The 0.7 program settles this semantic contract; full implementation is unscheduled and is not promised for 0.7, 0.8 or 1.0. This RFC also lists the constraints 0.6 work keeps so the design stays open.

## Core model

1. **One core, many surfaces.** Every checked program has exactly one SCIR form, and backends consume SCIR. Surface constructs, compiler-owned vocabulary and library vocabulary all normalize into it.
2. **A surface construct is a normalization rule.** The meaning of `model`, `class`, `error`, newtypes and frozen declarations is a named, inspectable rule from surface syntax to SCIR. It is not separate checker and backend logic for each construct.
3. **Ownership is explicit in the core.** Every value use in SCIR is a copy, move, shared loan or exclusive loan, decided once by checking and never re-derived by a backend.
4. **Authority is a core fact.** Every SCIR operation that needs authority carries the identity of the capability it requires, a checked symbol under RFC 104.
5. **Provenance is complete.** Every SCIR node carries:
    - its canonical identity under RFC 120;
    - its source span;
    - the surface construct or vocabulary it came from.

    Diagnostics are therefore always rendered in the terms the author wrote.
6. **One authority per fact.** SCIR does not sit beside the checked representation as a second opinion. Wherever SCIR decides a fact, no other stage decides it again.

## Motivation

Today each Incan construct has its own path. The checker knows `model`, `class`, `error`, newtypes and frozen declarations directly. The backend lowering re-derives the layout, receivers, derived methods and display of each one. Vocabulary desugarers produce surface syntax that the checker then has to understand. Every construct is therefore implemented twice: once to check it and once to lower it. Each new surface feature widens both.

Rust's compiler shows the alternative. Its surface language lowers to a smaller internal form before type checking:
- `for` becomes a `loop` with a `match` on an iterator;
- `?` becomes a `match` on a branch operation;
- `async fn` becomes a state machine.

Each desugared span records which surface construct produced it, so diagnostics still speak in surface terms. The checker and backend only see the small form.

An earlier prototype tested this shape for Incan. Class bodies, factory functions and the standard `Result` combinators normalized into shared checked primitives: records, enum layouts, variant construction, guarded projections and calls. No feature needed its own native operation. It also showed the costs:
- a second representation is harmful while it duplicates the checked one;
- the core must reproduce resolution, ownership and diagnostics before it can replace anything.

The prototype was parked until the native route existed. The native route now exists, and its checked body representation already records explicit ownership facts and canonical identities.

Capabilities belong in the same place. RFC 104 makes authority explicit at boundaries: importing a module is not permission, and governed runs grant, deny and record authority-bearing operations. RFC 104 also asks that a typed action's declared capabilities be checked against what its body actually calls. That check needs one representation in which every authority-bearing operation is visible, whatever surface construct or vocabulary produced it. Otherwise a vocabulary desugarer, a derived method or a generated constructor could perform authority-bearing work that no checker sees. SCIR is that representation.

## Goals

- Define a small, checked core with explicit ownership into which all Incan surfaces normalize.
- Express surface constructs as named normalization rules with complete provenance.
- Make capability identity part of every authority-bearing core operation, and derive capability requirements from the core.
- Give RFC 102 inspection, RFC 104 enforcement and receipts, and RFC 106 graph queries one source of declarations, edges and authority.
- Let compiler-owned vocabulary, and later library vocabulary, normalize to the same core.
- Keep backends construct-agnostic: they lower SCIR, not `model` or `class`.

## Non-Goals

- Shipping SCIR in 0.6. This RFC sets its direction and the constraints 0.6 keeps.
- A user-writable SCIR syntax. The versioned inspection view is read-only; a future separate proposal may define a writable dialect once the semantic contract stabilizes.
- Changing the meaning of any surface construct. Normalization must preserve today's semantics exactly.
- Turning capabilities into effect types in user signatures. RFC 104's usability rule stands: requirements are derived and inspected, not annotated on every function.
- A new backend. SCIR lowers to the existing native plan and the pinned compiler adapter.

## Guide-level explanation

### What a normalization looks like

A model:

```incan
model Point:
    x: int
    y: int = 0
```

normalizes to a record with its derived behavior and constructor. The inspection view shows the core form with each part attributed to its rule:

```text
record Point                          # from model Point
    x: int
    y: int
impl Eq, Clone, Display for Point     # derived by the model rule
fn Point.new(x: int, y: int = 0) -> Point   # constructor from the model rule
```

A class normalizes to the same kind of record, plus the implementations its methods, trait adoptions and inheritance define. An `error` normalizes to an enum or record with its error implementation and display. Nothing in the backend knows which of these it is lowering.

### What capabilities look like in the core

```incan
from std.fs import Path

def load(path: Path) -> str:
    return path.read_text()?
```

The core form of `load` contains a call whose operation carries `host.fs.read`. `load`'s capability requirements, `{host.fs.read}`, are derived from its body and from what it calls. They are recorded in its published facts, so:
- a typed action whose declared capabilities omit `host.fs.read` but which calls `load` gets a compile-time diagnostic;
- a governed run checks the grant at the operation;
- the receipt names the operation's capability and span.

A vocabulary block that desugars into a file read carries the same capability in its core form. That read is visible to the action check, to receipts and to `incan inspect`, exactly as if the author had written the call.

### What diagnostics look like

A type error inside a model's generated constructor points at the field the author wrote, and names the model, not the record:

```text
error: default for field `y` of model `Point` has type `str`, expected `int`
```

## Reference-level explanation

### The core

- Every checked Incan program must have exactly one SCIR form. Backends must consume only SCIR once it ships.
- SCIR declarations are records, enums, traits, implementations, functions and capability declarations.
- SCIR bodies are made of places, operations and structured control flow.
- Every value use in a body must be one of copy, move, shared loan or exclusive loan. Checking decides this, and later stages must not re-derive it.
- Every SCIR node must carry:
    - its canonical identity under RFC 120;
    - its source span;
    - the normalization rule or vocabulary that produced it.

### Normalization

- Each surface construct must be defined by a named normalization rule onto SCIR. A rule must preserve the construct's checked meaning exactly.
- Diagnostics raised while checking or lowering SCIR must be rendered through provenance, in the terms of the surface construct the author wrote.
- Compiler-owned vocabulary must normalize through the same rules.
- Library vocabulary under RFC 160 may target SCIR in a later `std.vocab` schema version. Until then, it continues to produce canonical surface syntax that normalizes like authored source.

### Capabilities

- Every SCIR operation that performs authority-bearing work must carry the identity of its required capability, as a checked symbol under RFC 104.
- A function's capability requirements must be the union of the capabilities its operations carry and the requirements of the functions it calls. They must be recorded in the function's published facts.
- No normalization rule or vocabulary may produce an authority-bearing operation without its capability identity.
- RFC 104's check of a typed action's declared capabilities against its body must use these derived requirements.
- Receipts must identify the SCIR operation, its capability and its source span.

### Inspection

- RFC 102's semantic package and RFC 106's graph must read declarations, call edges and capability requirements from SCIR facts, not from surface syntax or generated output.
- `incan inspect` must be able to show any declaration's SCIR form with its provenance. The inspection form is versioned and is not a source language.

### Generic definitions and derived instances

SCIR must retain each checked generic definition and its constraints as the semantic authority. Concrete instantiations needed by backend lowering or inspection must be derived facts with explicit links to that definition and its type substitution. They must not independently reinterpret ownership, capability requirements or source meaning. Inspection must distinguish a generic definition such as identity[T] from derived identity[int] and identity[str] instances while preserving each originating source location. This contract does not prescribe internal storage or the optimization stage that specializes a body.

### Public authority and compatibility

A public function's effective derived capability requirements must be part of its checked API facts. Adding a required capability at that boundary must be reported as an API compatibility change, identifying the operation and call path that introduced it. Consumer policy may deliberately accept that change; the compiler or runtime must not silently expand an old grant. Argument and result types remaining unchanged do not hide the authority change. Compare effective public requirements rather than reporting every internal call edge as an independent public break. RFC 104 retains actual grant enforcement and requirements remain inferred, not authored effect annotations on every function.

### Single authority

- A fact decided in SCIR must not also be decided by another stage. Introducing SCIR must replace, not shadow, the stages whose facts it takes over.

## Design details

### Relationship to the checked body representation

The native route already has a checked body representation with explicit Copy, Move, Clone, Borrow and MutBorrow facts and canonical declaration identities. The intended path is convergence: that representation becomes SCIR's body half, and a definition arena supplies SCIR's records, enums and implementations. A first production step would translate one existing checked workload into SCIR and compare its native plan and execution with the current lowering, while the current path stays available. Widening follows only when ownership, unwind behavior, diagnostics and package identity match.

### Constraints 0.6 keeps (non-normative)

So that 0.6 does not foreclose SCIR, 0.6 work keeps these properties:
1. A fact lives once, in the checker or the checked body representation. The lowering reads it and does not re-derive or compensate for it.
2. Checked body nodes keep canonical identities and source spans. Work that touches span carriers adds the surface construct that produced a node rather than dropping it.
3. Ownership decisions stay explicit facts in the checked body representation, never inferred by the lowering.
4. Capability-bearing standard operations are identified by their RFC 104 capability identity at the declaration, never by name matching in a backend.
5. The native plan stays construct-agnostic: it gains no operation specific to `model`, `class` or `error`.
6. RFC 160's interchange stays a versioned, structured syntax model, so a later schema version can target SCIR.

### Relationship to RFC 160 and source surfaces

RFC 160 desugarers return canonical surface syntax. Vocabulary, dialects and frontends under RFC 162 enter the compiler the same way. SCIR is where all of them meet. A future separate proposal could define a writable SCIR dialect for code that needs precise control of ownership and layout. This RFC commits to inspectable facts, not another authored source language.

## Alternatives considered

1. **Keep per-construct checking and lowering.** This was rejected because each construct stays implemented twice, each new surface feature widens both paths, and authority-bearing work produced by desugaring or derivation has no single place to be checked.
2. **Use Rust's internal representation as the core.** This was rejected because Incan's ownership rules (RFC 129), receivers and authority model differ from Rust's. The core must express Incan semantics, then lower to the pinned compiler.
3. **Capabilities as effect types in signatures.** This was rejected per RFC 104: requirements are derived and inspected, so ordinary code stays free of capability annotations.
4. **Expose SCIR as user syntax from the start.** This was deferred because a writable core is a long-term compatibility commitment, and the core should stabilize before users depend on it.

## Drawbacks

- Converging the checked body representation into SCIR is a large change across the checker, lowering and inspection.
- Diagnostics depend on complete provenance. A rule that loses it produces errors in core terms.
- Normalization adds a phase that must stay fast enough for the edit loop.
- Until convergence finishes, there is a period in which some constructs normalize and others do not. The single-authority rule must hold throughout.

## Layers affected

- **Typechecker / Symbol resolution**: checks SCIR; surface-specific checking moves into normalization rules.
- **Checked body representation**: converges into SCIR's body half; gains complete provenance.
- **IR Lowering**: lowers SCIR to the native plan; construct-specific lowering is removed.
- **Stdlib / Runtime**: capability identities on authority-bearing standard operations; receipts keyed by SCIR operations.
- **Vocabulary**: compiler-owned surfaces normalize to SCIR; a later `std.vocab` schema version lets library desugarers target it.
- **LSP / Tooling**: `incan inspect` shows SCIR forms; RFC 102 and RFC 106 read SCIR facts; diagnostics render through provenance.

## Design Decisions

- SCIR is the retained name. The 0.7 scope is design closure; full compiler migration remains unscheduled.
- Inspection is versioned and read-only. A writable core dialect requires a separate source-language proposal; exposing complete facts to people and tools does not itself define an authored syntax.
- Checked generic definitions own semantics. Concrete instances are derived, origin-linked facts; inspection distinguishes both without creating a second semantic authority.
- Effective public capability requirements are compatibility facts. New required authority must be reported with provenance and deliberately accepted; existing grants are not silently expanded.
- Selecting the first migration workload is engineering preparation, not an unresolved language policy. Each migrated boundary must replace the authority it assumes and demonstrate ownership, execution, unwind, diagnostics and identity parity before widening. No particular first construct or release date is promised by this design.
