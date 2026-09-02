# RFC 120: canonical source symbol identity

- **Status:** In Progress
- **Created:** 2026-08-19
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 022 (stdlib namespacing and soft keywords)
    - RFC 025 (multi-instantiation trait dispatch)
    - RFC 083 (symbol and method aliases)
    - #546 (`pub`/`rust`/`std` namespace-root import syntax)
    - #652 / #987 (v0.6 backend cutover and its parity corpus)
    - #699 (v0.4 symbol-identity pass)
    - #1072 (plain-assignment scope-lookup bug)
    - #1116 (builtin-function shadowing contract)
    - #1125 (destructuring `for` patterns in Body IR)
    - #1132 (statement-level tuple unpack of a non-tuple value)
    - #1174 (recoverable emitted-name projection)
    - #560 / RFC 094 (future `with` context-manager bindings)
- **Issue:** [#1042](https://github.com/encero-systems/incan/issues/1042)
- **RFC PR:** —
- **Written against:** v0.5
- **Shipped in:** —

## Summary

Every named source object — a local declaration, an import, an alias, a re-export, a generic binder, or a member — gets one canonical symbol identity, established once at its declaration site. A resolved reference carries that identity through every later stage: HIR, Body IR, diagnostics, the LSP, Oven inspection, codegraph export, and backend emission. Source spelling and any emitted Rust name are projections of that identity for a given consumer; neither is ever the source of truth for what a reference means. Where backend emission creates a linker-visible symbol for an Incan-origin declaration, that projection must also be recoverable to the canonical identity it represents without a separate side-car artifact.

## Motivation

The direct-HIR v0.6 backend cutover removes generated Rust as the normal semantic handoff between compiler stages. Up to now, several tools and stages have been able to lean on emitted Rust names, directly or indirectly, to recover what a piece of source actually referred to. That option goes away once the replacement backend is the normal path: a diagnostic, an LSP hover, a codegraph edge, and the compiled artifact all need to agree on what a name means using only the compiler's own identity model, not by comparing generated output.

Today that model is incomplete. The earlier v0.4 symbol-identity pass intentionally excluded new namespace features — it settled identity for what existed then, not for how imports, aliases, re-exports, generic binders, and members should all resolve onto one shared identity space. Without that, it is possible for two compiler stages to reasonably disagree about whether two references mean the same thing, which is exactly the class of defect this RFC exists to close off before the backend cutover makes generated Rust an unavailable fallback.

This RFC deliberately does not introduce new binding syntax. `let name = value` and `mut name = value` already exist in Incan (see `scopes_and_name_resolution.md`) as the explicit forms for introducing a new binding in the current scope, including one that deliberately shadows an outer binding; plain assignment already documents that it should reassign the nearest active binding rather than create a new one.

Two gaps between that documented contract and the implementation existed when this RFC was drafted, and both were bugs against an already-decided contract rather than new design questions. Plain assignment inside a nested block created an undiagnosed shadow instead of reassigning the outer binding (see Related). The typechecker also did not read the binding form: assignment checking inspected `BindingKind` only to decide mutability, so `let x = ...` and plain `x = ...` were checked identically, and `let`'s shadowing semantics were realized solely by the `let` in the emitted Rust. The shared binding-registration implementation closes both gaps: the frontend distinguishes `let`, `mut`, and plain assignment, resolves a plain assignment outward, and preserves or mints canonical identity according to the binding form. That work is the sharpest available statement of why this RFC comes before the cutover: the source-level binding contract must exist in the frontend rather than depend on emitted Rust as a semantic handoff.

## Guide-level explanation

Canonical identity is mostly invisible when things work — that is the point. It becomes visible whenever more than one compiler surface needs to agree about what a name refers to.

```incan
# lib.incn
pub def helper() -> int:
    return 42

# main.incn
from lib import helper as h

def main() -> int:
    return h()
```

`h` is a binding, not a second declaration. Hovering `h()` in an editor, a diagnostic naming that call, and a codegraph edge for that call all resolve to the same canonical identity — the `helper` declared in `lib.incn` — never a separately tracked identity for the alias `h`. If `helper` is later renamed at its declaration site and `main.incn`'s import is updated to match, every tool that already resolved through the canonical identity sees one consistent rename, not two unrelated symbols that happen to share a spelling.

The same guarantee extends to re-exports and generic binders:

```incan
# api.incn
pub from lib import helper as h

# consumer.incn
from api import h as run

def main() -> int:
    return run()
```

`run` in `consumer.incn`, `h` in `api.incn`, and `helper` in `lib.incn` are three bindings to one canonical identity. A diagnostic raised while type-checking `run()` names the original declaration's identity, not an intermediate alias hop; an LSP "go to definition" on any of the three spellings lands on the same declaration.

## Reference-level explanation

### Canonical identity

Every named source declaration is assigned one canonical symbol identity at its declaration site. That identity does not change based on how the declaration is later referenced. This covers: a function, model, class, trait, enum (and each of its variants), field, method, generic type parameter, a module-level or block-level binding introduced by `let`, `mut`, or a first plain assignment under its documented reassignment rules, and a `for` loop variable. All of these are ordinary bindings into a lexical namespace; none gets a separate identity model just because of which statement form introduced it.

This RFC does not introduce context-manager or exception-handler syntax. RFC 094/#560 owns a future `with` binding, and no accepted RFC currently introduces `except ... as`; if either binding form enters the language, it must register through this same ordinary lexical mechanism rather than invent another identity model.

A decorator's own name (for example `derive` in `@derive(CliArgs)`, or a trait name passed as its argument) is an ordinary reference and resolves through the same canonical identity as any other reference; a decorator application does not introduce a new kind of binding itself.

An import, an alias, and a re-export are bindings to an existing canonical symbol. None of the three creates a second canonical identity for the thing they name. A local declaration and an imported declaration differ only in how their binding enters scope, not in what kind of identity they carry.

A generic binder (a type parameter) has its own canonical identity, scoped to the declaration that introduces it, distinct from any concrete type that instantiates it.

A member (a field or method reached via `.`) resolves to the canonical identity of its declaration on the owning type, not to a fresh identity per access site.

### Namespace table

Incan has three namespaces, distinguished by *how* a name is looked up rather than by what kind of thing it names. This is deliberately not the type/value split rejected under Alternatives: a model name and a function name share one namespace here, exactly as ordinary Python-like lexical lookup expects.

| Namespace | Binds | Lookup | Scope unit |
| --- | --- | --- | --- |
| **Ordinary lexical** | module-level declarations (function, model, class, trait, enum, newtype, `rusttype`, type alias, `const`, `static`, partial); imports, aliases, and re-exports; bare enum variant names; generic binders; parameters and receivers (`self`, `cls`); locals introduced by `let`, `mut`, or a first plain assignment; `for` variables | innermost scope outward through the scope chain, then the builtin fallback tier | module, function/method, closure, block, comprehension — and, for a generic binder, the declaration that introduces it |
| **Members** | fields, methods, computed properties, method aliases, and qualified enum variants | `.`-directed from a resolved owner type; never through the scope chain | the owning nominal type, including its inherited surface |
| **Module and package paths** | project module paths, the `std` root, `rust::` crate roots and item paths, and `pub::` library roots | path-directed from a namespace root | the compilation's module and package graph |

Beneath the ordinary lexical namespace sits one **builtin fallback tier**: core builtin functions such as `len`, `sum`, and `zip`, plus builtin type spellings and their registry aliases. It is consulted only after the whole scope chain misses, so a real lexical binding — declared or imported — wins over an ordinary ambient builtin. That is settled behavior with a documented contract and tests (see Related). Rebinding an ordinary builtin-function spelling is therefore **not** a collision, and `std.builtins.<name>` remains the explicit way to reach it from a scope that has rebound the spelling. The output functions are the narrow exception: `print` and its `println` alias are immutable language functions, are registered as reserved bindings rather than fallback candidates, and cannot be reused by an ordinary lexical declaration or import.

Namespaces do not shadow one another. A field named `items` never shadows a local named `items`, and a module path segment never shadows either. The same rule allows a field or method named `print`: member lookup cannot replace the immutable lexical output function. Generic binders are ordinary lexical bindings with a declaration-bounded scope rather than a fourth namespace: what makes a binder distinct is its identity kind and the scope it is bounded to, not a separate lookup rule. Their symbol-table representation remains builtin-kinded, but the canonical identity now records the generic-binder category and exact declaration span so downstream consumers can distinguish a binder from a concrete builtin type.

### One shared namespace/binding-resolution mechanism

When this RFC was drafted, collision and ambiguity detection for named things was scattered: `duplicate_library_export`, `duplicate_rust_module`, `duplicate_trait_instantiation`, `duplicate_alias`, `pub_library_import_name_collision`, and `pub_library_module_member_ambiguous` each independently implemented a narrow check for one construct. The checks crossed compiler layers, not merely files: the library-export check was decided in the build command, the duplicate-`rust`-module check was raised in the parser, and the rest were spread through declaration checking, import collection, and expression checking. That fragmentation was itself a source of the defects this RFC exists to close: two constructs could disagree about whether a collision was a collision because each ran its own logic instead of consulting one shared answer — and a check living in the CLI could not agree with one living in the frontend by construction. Slice 3 establishes the shared frontend registration answer and routes the in-scope construct checks through it; the two documented exemptions remain separate.

This RFC requires one shared namespace/binding-resolution mechanism that every binding kind — local declarations, imports, aliases, re-exports, library exports, trait instantiations, generic binders, and members — registers into and is checked against. One check is deliberately exempt: the duplicate-`rust`-module check runs in the parser, before any resolver exists to consult, and keeps its own narrow implementation (see Design decisions). Collision and ambiguity detection is a property of that one mechanism, not something each construct's typechecker code independently decides. A specific diagnostic's wording may still vary by construct for a good user-facing message (an ambiguous `pub::` import and an ambiguous generic instantiation don't need identical phrasing), but the underlying question — "does this binding collide with, or ambiguously resolve against, an already-active binding for this spelling in this namespace" — is answered once, by one mechanism, for every construct.

### Namespace rule

At every program point, one spelling resolves to at most one active binding in a given ordinary lexical namespace. This RFC does not redefine how a binding becomes active — that is the existing `let` / `mut` / plain-assignment / import contract in `scopes_and_name_resolution.md`, including the scope-lookup repair implemented with Slice 3 (see Related). This RFC defines what a binding carries once it is resolved (its canonical identity) and requires that "is this binding active, and does it collide with another" be answered by the one shared mechanism above, not reimplemented per construct.

### Canonical identity payload

A canonical identity is a compiler-owned value, not a rendered string. It carries exactly the data a downstream stage needs to answer "do these two references mean the same thing" without comparing spellings.

| Field | Purpose | Notes |
| --- | --- | --- |
| `namespace` | Which of the three namespaces above the binding lives in | Keeps a member and a local that share one spelling from ever comparing equal |
| `origin` | The module, package, `rust::` crate, or builtin registry owning the declaration | An import, alias, or re-export carries its target's `origin`, never the importing module's |
| `declaration_name` | The spelling at the **declaration** site | Never the spelling at the reference site, so `run`, `h`, and `helper` in the guide-level example all carry `helper` |
| `kind` | Declaration category: function, model, class, newtype, `rusttype`, enum, variant, trait, field, method, property, `const`, `static`, local, parameter, receiver, generic binder, module, Rust item, builtin | Slices 1-3 record the frontend declaration and reference kinds; later slices propagate the same value to downstream consumers |
| `scope_discriminant` | Distinguishes bindings that are not unique within their `origin` | Required for locals, parameters, receivers, generic binders, and any block-scoped binding; module-level declarations are already unique within their origin |
| `declaration_span` | Provenance anchor | One span, at the one declaration site |

Two properties are load-bearing. Identity equality must be decidable without string comparison of source spellings or emitted names. And identity must be stable across the stages of **one** compilation; it is not required to be stable across edits to the source, because `declaration_span` moves when the file changes.

At drafting time, the missing `scope_discriminant` exposed the model's most concrete defect: two `x` bindings in sibling blocks collapsed to the same module-path-plus-spelling identity, while an alias could split away from the declaration it named. Slices 1-3 mint scope-aware declaration identities and preserve target identity through aliases and imports in the frontend; downstream propagation remains sequenced in later slices.

### Recoverable emitted-name projection

Emission has two deliberately separate contracts:

1. **Semantic resolution is one-way.** No compiler phase may determine what source binding a reference means by parsing an emitted Rust name. The resolver, HIR, Body IR, diagnostics, LSP, Oven facts, and codegraph use compiler-owned canonical identity facts. An emitted name never becomes semantic authority.
2. **Artifact observation is recoverable.** When an Incan-origin declaration becomes a linker-visible emitted symbol, its emitted identifier carries a versioned, reversible encoding of its canonical identity. A backtrace or artifact inspector can decode that payload to the declaration identity without loading generated Rust, re-resolving source, or consulting a source-map sidecar that could drift from the artifact.

The initial projection format is an Incan-owned `incan-v1` payload, encoded with a Rust-identifier-safe reversible alphabet and carried in the emitted item's unmangled identifier. It contains the complete canonical identity payload — namespace, origin, declaration name, kind, scope discriminant, and declaration span — plus a format version. A decoded payload is therefore the identity for the artifact that carries it, rather than a lookup key that needs a source map or another companion artifact. The payload identifies the source declaration; generic specialization remains in Rust v0's type suffix and is not duplicated inside `incan-v1`.

The implementation separates carrier conformance from compiler-emitter conformance. [DD-0002](../design_records/0002_single_pinned_rust_version.md) selects Rust 1.98.0 and records the artifact-size and identifier-length measurement. `tests/emitted_symbol_projection_tests.rs` rejects any other compiler release and uses a synthetic, independently compiled Rust fixture on Linux and macOS to prove that optimized v0 mangling, native symbol inspection, demangling, and decoding preserve ordinary-function, generic-function, member-method, and source-static identities without a sidecar. Its representative `host_bridge` symbol remains unclassified rather than being guessed as an Incan declaration. Separate direct codegen regressions prove that the Incan compiler attaches the same projection to declarations, call sites, imports, re-exports, top-level partial wrappers, concrete methods, computed properties, source-static declarations and references, and Rust-extern wrappers without decoding an emitted name. A method partial keeps its synthetic wrapper explicitly non-Incan and calls the recoverably projected source method it targets.

This applies only to Incan-origin declarations that materialize as linker-visible symbols. Locals and other source declarations that do not materialize as an emitted symbol are not counterexamples; a future backtrace locates them through their nearest recoverable Incan-origin frame. A frame with no Incan origin is classified as runtime, host, or interop and may be collapsed at normal verbosity or shown at an explicit verbose setting. It is never guessed to be an Incan declaration.

The payload may be compacted only through another reversible encoding that passes the same independent decoding fixture. There is no release-mode setting that silently removes recoverability. The binary-size and emitted-name-length impact must be measured on a representative release artifact before the cutover gate is declared complete.

### Propagation through the pipeline

A resolved reference's canonical identity must be preserved, not recomputed from spelling, at every stage that consumes it:

- **HIR / Body IR** — a reference node carries the canonical identity, not only a source span or spelling.
- **Diagnostics** — an error or warning naming a symbol names its canonical identity's declaration site, regardless of which binding (local, import, alias, re-export) the offending reference used.
- **LSP** — "go to definition," "find references," and hover all resolve through canonical identity, so every binding to one declaration reports the same definition site.
- **Oven inspection / codegraph export** — an edge or fact referencing a symbol identifies it by canonical identity, so tooling can determine that two differently-spelled references mean the same thing without string comparison.
- **Backend emission** — the emitted Rust name for a canonical identity is a projection for that backend; it must not become a second identity another stage compares against, and every linker-visible Incan-origin projection must carry the recoverable `incan-v1` payload.

At drafting time, every one of those stages recomputed from spelling, which is what made this a sequence of real slices rather than a threading exercise. The implementation follows that sequence: Slices 1-3 establish declaration and reference identities plus shared binding registration in the frontend; HIR and Body IR then carry those identities instead of rebuilding them from names; diagnostics retain related declaration identities; the LSP answers definition, references, and hover from the checked identity snapshot; and codegraph exports structured canonical identities while treating `target_id` only as an optional link to a declaration in the same export.

### Diagnostics

A duplicate declaration or import that would introduce a second active binding for the same spelling in the same namespace at the same program point, without going through an explicit `let` or `mut` shadowing form, is a diagnostic, raised by the one shared namespace/binding-resolution mechanism rather than by construct-specific checks. When two imports attempt to introduce the same local spelling, the collision is reported as ambiguous if their proven target identities differ or if either target identity cannot be proven; the diagnostic names every proven candidate declaration. The import target itself is still resolved by the module graph before local binding registration.

Existing construct-specific diagnostics that already implement a narrower version of this check (for example `duplicate_alias`, `duplicate_trait_instantiation`, `duplicate_library_export`) are expected to become call sites of the shared mechanism rather than parallel implementations; their user-facing wording may stay construct-specific, but the collision logic behind them should not. `pub_library_import_name_collision` remains tied to the separate `pub`/`rust`/`std` namespace-root import syntax (see Related) and is out of this RFC's scope to consolidate.

## Design details

### Binding-form decisions

These are the decisions the implementation must encode. None changes accepted source syntax; each states which binding an existing form targets and what identity it produces.

- **`let name = value`** introduces a new binding in the current scope and gives it a fresh canonical identity, even when an outer binding for that spelling is active. This is an explicit shadowing form. Slice 3 makes it a frontend-modeled decision instead of relying on emitted Rust to realize the distinction.
- **`mut name = value`** behaves identically to `let` for binding and identity purposes, and additionally marks the new binding mutable. It is equally a shadowing form. It is **not** "make the existing `name` mutable": it declares a new binding that happens to be mutable, and reading it as a modifier on an already-active name inverts what it does to identity. Any claim that `let` is the *only* form introducing a binding over an active one is imprecise and must not be carried into implementation or tests.
- **Plain `name = value`** resolves outward through the scope chain and reassigns the nearest active binding, preserving that binding's canonical identity and requiring both `mut` and a compatible type. It introduces a new binding, with a new identity, only when no active binding for that spelling exists anywhere in the chain. It is never a silent shadow.
- **A duplicate declaration or import** — a second binding for one spelling in one namespace, in the same scope, through a form that is not `let` or `mut` — is a diagnostic from the shared mechanism. Rebinding an ordinary fallback builtin is explicitly not this case; attempting to redefine immutable `print` or `println` is.
- **An ambiguous import** — two imports attempting to introduce one local spelling whose target identities differ, or cannot both be proven equal — is a diagnostic from the shared mechanism naming the proven candidate declarations by identity and span.
- **A bare enum variant name** binds into the ordinary lexical namespace without displacing an already-active binding for that spelling. The variant keeps its own canonical identity and stays reachable through its qualified member path; only the bare spelling defers. This preserves existing behavior and does not weaken the namespace rule.

### Interaction with the scope-lookup bug

This RFC's identity model assumes plain assignment, `let`, and `mut` behave as decided above. The scope-lookup repair implemented with Slice 3 had two halves that landed together: plain assignment walks outward, while `let` and `mut` remain binding-introducing forms. Shipping only the first half would have turned every in-block explicit binding into a reassignment of the outer binding.

The identity model as *specified* is independent of that repair: it applies equally to whichever binding is active under the corrected contract. Delivery could not be independent, because an identity assigned to a binding the checker resolved to the wrong scope would be a precise name for the wrong thing. Slice 3 therefore includes executable `let`, `mut`, and plain-assignment identity conformance after the repair.

### Reconciliation with in-flight source-semantics work

- **#1072 (assignment semantics)** landed after #1132 as part of the Slice 3 foundation and covers both halves explicitly: plain assignment finds the nearest enclosing binding and reassigns it, and `let` and `mut` introduce a binding that may shadow an active outer one. The repair and identity work ship together because either half alone leaves the documented contract broken in a different way.
- **#1132 (statement-level tuple unpack of a non-tuple value)** finished before the assignment repair. Its fix corrected an arity guard that a non-tuple fallback rendered unreachable; it is independent of identity in semantics and adjacent to it in code. The statement-level unpack now carries the same `let`/`mut`/plain-assignment decision as the single-name path.
- **#1125 (destructuring `for` patterns)** has already landed the loop half. Its loop-pattern bindings are ordinary lexical bindings and need identities like any other, and Body IR already binds them through the flat name-to-local map that the Body IR slice replaces.
- **#1116 (builtin-function shadowing)** has already settled the ordinary fallback contract: a real local or imported binding wins over a fallback builtin, and `std.builtins.<name>` is the explicit escape hatch for reaching that builtin from a scope that has rebound the spelling. That contract becomes a conformance case for the identity work rather than only a thing not to regress — the rebound spelling and the qualified builtin must resolve to two different canonical identities, and the rebinding itself must report nothing. Immutable `print` and `println` do not enter the fallback tier and are tested as reserved-function collisions instead.

### Interaction with existing features

- **Imports and modules** — RFC 022's stdlib namespacing and canonical `std` root are unaffected; this RFC's identity model applies to `std` symbols the same as any other declaration.
- **Aliases** — RFC 083 already establishes that an alias preserves its target's semantic identity for imports, diagnostics, documentation, and metadata rather than acting as a copy or wrapper. This RFC generalizes that same principle to the full identity model rather than introducing a competing one.
- **Generic binders** — RFC 025's multi-instantiation trait dispatch resolves a call to one of several adopted instantiations by argument/return type; this RFC's canonical identity for a generic binder is what that dispatch resolves against, not a per-instantiation identity.
- **Rust interop** — an emitted Rust name remains a backend-specific projection of a canonical identity; this RFC does not change what interop code can call, only how the compiler tracks what a reference means before emission. Native runtime, host, and third-party Rust frames have no Incan identity unless the compiler emitted an Incan-origin projection for them; inspection classifies those frames rather than inventing source provenance.

### Compatibility / migration

This RFC does not change source syntax, but its shared registration rule can reject source that previously passed through fragmented checks. A second declaration or import that introduces the same local spelling in the same scope is now diagnosed instead of silently replacing the first active binding. Two imports with that spelling are reported as ambiguous when they resolve to different identities or either target cannot be proven. Give one import an explicit alias when both bindings are intentional; only standard-library component entrypoints containing genuinely repeated leaf spellings required this migration. Dependency declarations supplied to the compiler for interface checking also no longer leak into the consumer's lexical scope: source that relied on an unimported direct or transitive dependency declaration must add an explicit import. Programs that relied on the former nested-assignment bug receive the documented behavior: plain assignment resolves outward, while `let` and `mut` explicitly introduce a new binding.

## Alternatives considered

### Let the backend or generated Rust resolve collisions

Rejected because source meaning would then vary by backend, and tooling could not inspect one authoritative result — exactly the dependency on generated Rust the v0.6 backend cutover is removing.

### Reopen the earlier v0.4 identity pass unchanged

Rejected. That pass was completed and explicitly excluded new namespace syntax; its scope does not cover imports, aliases, re-exports, or generic binders as one identity space.

### Use Rust-style separate type and value namespaces

Rejected because it introduces a second mental model that does not match Incan's Python-like ordinary lexical lookup, and nothing about the canonical-identity problem requires splitting the namespace to solve.

### Emit a side-car artifact-to-identity map instead

Rejected as the primary recovery mechanism. A side-car can be missing or describe a different artifact, reintroducing a second source of truth precisely where user-facing artifact inspection needs an answer. Compiler facts may enrich a decoded identity with spans or source context, but decoding the emitted symbol itself must establish its identity.

## Drawbacks

- Threading one canonical identity through every pipeline stage (HIR, Body IR, diagnostics, LSP, codegraph, backend) is real engineering work across most of the compiler, not confined to one layer.
- Conformance fixtures for the full matrix of local/import/alias/re-export/generic-binder/member combinations add meaningful test surface.
- A reversible emitted-name payload lengthens symbol names and can increase artifact size. v0.6 accepts that cost only with a recorded measurement and a fixture proving that mangling and demangling preserve recovery.

## Layers affected

- **Parser** — must attach enough declaration-site information for every named source object to be assigned a canonical identity later, including the binding form it already records; must not itself resolve identity.
- **Typechecker / resolver** — resolves frontend declaration and reference identities and owns the shared binding-registration answer implemented by Slices 1-3. Later slices propagate that answer through every import, alias, re-export, generic binder, and member consumer. The duplicate-library-export check moved from its draft-time build-command location into the frontend registry; the two checks recorded as exemptions under Design decisions remain separate.
- **HIR / Body IR** — must carry canonical identity on reference nodes rather than recomputable spelling alone.
- **Diagnostics** — must report a symbol's canonical declaration site regardless of which binding a reference used to reach it.
- **LSP** — must resolve "go to definition," "find references," and hover through canonical identity.
- **Codegraph / Oven inspection** — must key symbol edges and facts by canonical identity, not string spelling.
- **Backend emission** — must treat an emitted Rust name as a projection of canonical identity, never a second identity another stage compares against; must encode the versioned, reversible `incan-v1` identity payload in every linker-visible Incan-origin symbol; and must prove the selected Rust toolchain preserves it through v0 mangling and demangling.

## Inspectability and tooling surface

- **Artifact or metadata:** codegraph export already reports declarations, references, and calls; this RFC requires those records to carry canonical identity so two differently-spelled references to one declaration are visibly the same fact, not two. A linker-visible Incan-origin symbol independently carries a recoverable `incan-v1` projection, so an artifact observer can establish its identity even when no codegraph or source-map side-car is present.
- **Inspection command:** `incan inspect codegraph --format jsonl` is the existing surface; no new command is introduced.
- **Diagnostics:** duplicate-binding and ambiguous-import diagnostics name the conflicting declarations by canonical identity and source span.
- **Provenance:** every canonical identity anchors to the source span of its one declaration site.
- **Not implicit:** an alias, re-export, or generic instantiation never silently becomes a second identity; a rename at a declaration site is visible as one consistent change everywhere the compiler reports that identity.

## Implementation plan

This plan is the governing delivery map for the work. At drafting time it required two changes before any identity slice, in this order; both prerequisites are now complete.

1. **#1132 — statement-level tuple unpack of a non-tuple value.** This finished first because it owned the statement-checker lane. Its fix is independent of identity in semantics but edited the same assignment and unpack region as the binding-form repair.
2. **#1072 — assignment semantics, both halves.** This followed #1132 and established that plain assignment finds the nearest enclosing binding and reassigns it, while `let` and `mut` introduce a binding and may shadow an active outer one. `mut` is not "make an existing binding mutable" — it declares a new mutable binding. Slice 3 carries the executable identity conformance for that settled behavior.

The canonical-identity slices follow that completed foundation because an identity assigned to a binding the checker resolved to the wrong scope would be a precise name for the wrong thing.

The slices below are dependency-ordered and deliberately narrow. Each names the modules that own it and the conformance evidence that proves it. Slices 1-3 are frontend-only and were implemented before anything downstream consumed an identity; slices 4-8 take one consumer each, so a regression in a consumer is attributable to one slice. They may ship together in one reviewed change without weakening that implementation order.

### Slice 1: Canonical identity type and declaration-site assignment

Introduce the identity value and assign one at every declaration site, changing no behavior and no diagnostic. Owning modules: `src/frontend/symbols.rs` for assignment at symbol definition, and `crates/incan_semantics_core/src/facts.rs` for the identity type itself, alongside the existing compiler node identity. Conformance: unit coverage proving that two same-spelled bindings in sibling blocks receive different identities, that a generic binder's identity differs from the concrete type instantiating it, and that a module-level declaration's identity is independent of how often it is referenced.

### Slice 2: Reference-side identity recording

The implementation extends the existing source-target recording so resolved frontend references receive an identity rather than limiting that fact to selected calls and type uses. Owning modules: `src/frontend/typechecker/mod.rs` for recording and symbol-origin helpers, and `src/frontend/typechecker/type_info.rs` for the recorded target shape. The existing string-shaped target remains as a projection so codegraph does not have to move in the same change. Conformance covers the RFC's alias and re-export examples, asserting all three spellings record one identity, plus a case proving imported and local references to one declaration compare equal.

### Slice 3: One shared binding-registration and collision mechanism

The implementation gives symbol definition a single entry point that answers "does this binding collide with, or ambiguously resolve against, an already-active binding for this spelling in this namespace", and in-scope construct checks call it. Owning modules: `src/frontend/symbols.rs` for the mechanism; `src/frontend/typechecker/check_decl.rs` for duplicate alias and trait-instantiation call sites; and `src/frontend/library_exports.rs` for the duplicate-library-export check moved from the CLI into the frontend-owned registry. Explicitly excluded: the duplicate-`rust`-module check, which is raised in `crates/incan_syntax/src/parser/core.rs` before a resolver exists, and the `pub`-library import-collision check owned by #546. Conformance preserves diagnostic wording and spans, keeps an ordinary fallback builtin such as `len` shadowable, and rejects immutable `print` and `println` through the same registry.

### Slice 4: HIR carries identity

Stop deriving declaration ids from module path plus spelling, and give import declarations the name and identity they currently lack. Owning modules: `src/frontend/hir.rs` and `crates/incan_semantics_core/src/facts.rs`. Conformance: a snapshot proving an aliased import and its target declaration share one identity.

### Slice 5: Body IR resolves by identity

Replace name-keyed local resolution so a resolved reference cannot degrade into a synthesized external local with unknown ownership. Owning module: `src/frontend/body_ir.rs`. Conformance: a case proving a shadowed spelling binds the correct local in each scope, and that no reference the resolver resolved arrives as an external local. This slice needs the 0.6 backend line and cannot be developed against a base predating Body IR.

### Slice 6: Diagnostics name declaration sites

Make a diagnostic that names a symbol report its canonical declaration site regardless of which binding the offending reference used. Owning modules: the diagnostics catalog under `crates/incan_syntax/src/diagnostics/`, and the typechecker call sites that build symbol-naming messages. Conformance: a diagnostic raised through an alias naming the original declaration's span.

### Slice 7: LSP resolves through identity

Retain the checked fact snapshot on the LSP document state and answer definition, references, and hover from it, instead of the current first-match declaration scan that consults no typechecker output. Owning module: `src/lsp/backend.rs`. Conformance: definition from each of the three spellings in the re-export example landing on one declaration.

### Slice 8: Codegraph and one-way backend projection

Key codegraph records on identity rather than the `(module path, name, kind)` string triple, and add a guard that no compiler phase recovers a source binding by reading an emitted name. Owning modules: `src/cli/commands/codegraph.rs`, plus the emission path for the projection guard. Conformance: a `jsonl` export in which two differently-spelled references to one declaration are visibly one fact, and a case proving an edge is no longer dropped when its triple is unregistered.

### Slice 9: Recoverable emitted-name projection (#1174)

Define and emit the versioned `incan-v1` payload for every linker-visible Incan-origin declaration, then add the independent artifact decoder that an inspector and a future user-facing backtrace consume. Owning modules: the direct-HIR emission path and `incan_semantics_core::emitted_symbol` as the artifact-inspection boundary. Semantic lowering uses only compiler-carried identities and the encoder; it never invokes the decoder. Rust ABI-constrained method slots retain their required spelling and receive a separate inherent entry point carrying the exact source-method projection. Conformance uses Rust 1.98.0 release-mode artifacts for an ordinary declaration, a generic specialization, a member method, and a source static; decodes after v0 mangling and demangling; classifies a representative host frame as non-Incan; and records the artifact-size delta in DD-0002. This slice does not implement the backtrace UX itself; it provides the contract that consumer needs.

### Cutover conformance

The identity guarantees that must not regress at the v0.6 backend cutover belong in the backend-parity corpus rather than only in frontend unit tests, so a replacement backend cannot silently lose them. The corpus pins a typed set of semantically valid cells rather than pretending every axis forms a literal cross-product. Ordinary lexical declarations are covered through local, import, alias, and re-export bindings at module, function, and block scope. Member targets use the same four owner-binding forms, but their declaration cell is correctly scoped to the owning nominal type; executable member references are then covered at function and block scope. Module-path identities are covered for direct imports and module aliases at the module graph/HIR boundary; they do not have local or re-export declaration forms, and Body IR carries the declaration selected through a qualified path rather than retaining the path prefix as a runtime dispatch identity. Every cell requires the carriers its layer genuinely represents: a checked identity always, HIR for represented module bindings, Body IR for executable references, and an emitted projection only for linker-visible source declarations. Conformance additionally covers explicit shadowing with `let` and with `mut`, one generic-binder case, and #1116's builtin contract: a rebound builtin-function spelling and the same name reached through `std.builtins.<name>` must carry two different canonical identities across the cutover. It also includes a release-artifact decode of the `incan-v1` payload after v0 mangling and demangling, plus classification fixtures proving representative non-Incan frames are never reported as source declarations.

## Progress Checklist

Checked items are implemented by the current merge candidate and count as release-branch evidence only after it merges. Unchecked items remain open. The slice structure above remains the governing map; this checklist is its trackable projection.

### Predecessors (not owned by this RFC)

- [x] #1132 statement-level tuple unpack landed first.
- [x] #1072 assignment semantics, both halves, land together.

### Identity core (Slices 1–2)

- [ ] One compiler-owned identity mint at symbol definition, covering module declarations, locals, consts, statics, parameters, receivers, and generic binders, with scope discriminants.
- [ ] Import, alias, and re-export bindings carry their resolved target's identity; unproven bindings carry none.
- [ ] Builtin registry identities: every alias spelling records the one canonical registry identity.
- [ ] Reference-side identity recording keyed by reference span, with the string-shaped source target retained as a projection.
- [ ] Method declarations carry member-namespace identities; field/property declaration identities generalized.
- [ ] Conformance: sibling-scope distinctness, alias/re-export equality, `let`/`mut` shadowing, builtin-rebinding distinctness, duplicate-declaration identity distinctness.

### One shared binding mechanism (Slice 3)

- [ ] Single binding-registration/collision entry point; duplicate declarations and imports become diagnostics.
- [ ] `duplicate_alias`, `duplicate_trait_instantiation`, and `duplicate_library_export` migrate to frontend-owned call sites of the shared mechanism.
- [ ] Ordinary fallback builtin spellings remain shadowable; immutable `print` and `println` reject replacement.

### Consumers (Slices 4–8)

- [ ] HIR declarations carry canonical identity; single-binding imports carry their target's identity.
- [ ] Body IR callable targets consume the typechecker-minted identity instead of re-deriving one.
- [ ] Body IR resolves locals by identity so no resolver-resolved reference degrades to an external local.
- [ ] Diagnostics name canonical declaration sites regardless of the referencing binding.
- [ ] LSP definition/references/hover resolve through identity.
- [ ] Codegraph keys records on identity rather than the string triple.

### Recoverable projection (Slice 9)

- [ ] `incan-v1` emitted-name payload with DD-0002 toolchain record and decode fixtures (#1174).

### Cutover conformance

- [ ] Typed identity-coverage rows land in the backend-parity corpus for every semantically valid binding/namespace/scope cell and require only the carriers each compiler layer actually represents.

## Design decisions

- **Canonical identity covers every implemented ordinary binding form, not just the originally-proposed set:** locals, imports, aliases, re-exports, generic binders, members, and `for` loop variables all use the same identity model. A future `with` binding from RFC 094/#560, or any separately accepted exception-handler binding, must enter that mechanism when its syntax exists; this RFC does not claim those absent forms are implemented. A decorator's own name reference resolves through the same general mechanism rather than needing special-casing.
- **Collision and ambiguity detection consolidates into one shared namespace/binding-resolution mechanism, not construct-specific diagnostics:** the draft-time fragmented approach (`duplicate_library_export`, `duplicate_rust_module`, `duplicate_trait_instantiation`, `duplicate_alias`, `pub_library_import_name_collision`, `pub_library_module_member_ambiguous`) was itself a source of disagreement across parser, frontend, and build-command layers. Slice 3 gives every in-scope binding kind one shared registration answer; construct-specific call sites may keep useful wording but not independent collision logic. `pub_library_import_name_collision` stays separate with #546, and the duplicate-`rust`-module check stays in the parser for the reason recorded below.
- **Delivery order is settled, and the assignment fix's scope with it.** #1132 finished first; #1072 followed with both halves of assignment semantics; Slices 1-3 followed that foundation. The two halves of #1072 were one change, not a sequencing choice: making plain assignment walk outward without also honoring `let` and `mut` would have converted every in-block explicit binding into a reassignment.
- **`mut` is a shadowing form too, not only `let`.** The originating issue's phrasing that `let` is the only ordinary mechanism introducing a same-spelling binding over an active one does not match the documented binding model, which gives `mut name = value` the same binding-introducing behavior plus mutability. Implementation and conformance fixtures follow the documented model.
- **Ordinary builtin-function spellings remain shadowable; `print` and `println` are immutable.** The ordinary builtin fallback tier sits beneath the whole scope chain, so a declared or imported binding legitimately wins for functions such as `len`, `sum`, and `zip`. Output is the narrow exception: the canonical `print` function and its `println` alias are reserved bindings, and the shared registry rejects attempts to redefine or replace either spelling.
- **The duplicate-`rust`-module check stays in the parser.** It is raised before any resolver has run and cannot consult a shared binding mechanism without inverting the pipeline. It keeps its own narrow check, and the consolidation decision above is amended to exclude it rather than pretending it can migrate.
- **The duplicate-library-export check moved out of the CLI.** At drafting time it was a namespace collision decided in the build command rather than the frontend. Slice 3 migrated it to the shared mechanism as a layering correction, not only a deduplication.
- **Canonical identity is stable within one compilation, not across source edits.** Every identity anchors to its declaration span, so an edit that moves a declaration changes its identity. Consumers that need cross-edit continuity, such as an editor session, must re-resolve rather than cache an identity across versions.
- **Recoverable projection serves artifact observation, never source semantics.** A compiler phase never parses an emitted name to resolve a source reference. An artifact observer may decode the versioned `incan-v1` payload because that operation answers the distinct question of which compiler-owned identity a completed artifact exposes. A side-car may enrich that decoded answer, but cannot be necessary to establish it.
