# RFC 120: canonical source symbol identity

- **Status:** Planned
- **Created:** 2026-08-19
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 022 (stdlib namespacing and soft keywords)
    - RFC 025 (multi-instantiation trait dispatch)
    - RFC 083 (symbol and method aliases)
    - #546 (`pub`/`rust`/`std` namespace-root import syntax)
    - #699 (v0.4 symbol-identity pass)
    - #1072 (plain-assignment scope-lookup bug)
- **Issue:** [#1042](https://github.com/encero-systems/incan/issues/1042)
- **RFC PR:** —
- **Written against:** v0.5
- **Shipped in:** —

## Summary

Every named source object — a local declaration, an import, an alias, a re-export, a generic binder, or a member — gets one canonical symbol identity, established once at its declaration site. A resolved reference carries that identity through every later stage: HIR, Body IR, diagnostics, the LSP, Oven inspection, codegraph export, and backend emission. Source spelling and any emitted Rust name are projections of that identity for a given consumer; neither is ever the source of truth for what a reference means.

## Motivation

The direct-HIR v0.6 backend cutover removes generated Rust as the normal semantic handoff between compiler stages. Up to now, several tools and stages have been able to lean on emitted Rust names, directly or indirectly, to recover what a piece of source actually referred to. That option goes away once the replacement backend is the normal path: a diagnostic, an LSP hover, a codegraph edge, and the compiled artifact all need to agree on what a name means using only the compiler's own identity model, not by comparing generated output.

Today that model is incomplete. The earlier v0.4 symbol-identity pass intentionally excluded new namespace features — it settled identity for what existed then, not for how imports, aliases, re-exports, generic binders, and members should all resolve onto one shared identity space. Without that, it is possible for two compiler stages to reasonably disagree about whether two references mean the same thing, which is exactly the class of defect this RFC exists to close off before the backend cutover makes generated Rust an unavailable fallback.

This RFC deliberately does not introduce new binding syntax. `let name = value` already exists in Incan (see `scopes_and_name_resolution.md`) as the explicit form for intentionally shadowing an outer binding; plain assignment already documents that it should reassign the nearest active binding rather than create a new one. A real gap between that documented contract and the current implementation exists — plain assignment inside a nested block currently creates an undiagnosed shadow instead of reassigning the outer binding — but that gap is a bug against an already-decided contract, not a new design question, and is tracked separately (see Related). This RFC assumes that bug gets fixed to match the documented contract, and builds canonical identity on top of the binding model as already specified.

## Guide-level explanation

Canonical identity is mostly invisible when things work — that is the point. It becomes visible whenever more than one compiler surface needs to agree about what a name refers to.

```incan
# lib.incn
def helper() -> int:
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
from lib import helper as h
# re-export: `h` is now also part of api's public surface

# consumer.incn
from api import h as run

def main() -> int:
    return run()
```

`run` in `consumer.incn`, `h` in `api.incn`, and `helper` in `lib.incn` are three bindings to one canonical identity. A diagnostic raised while type-checking `run()` names the original declaration's identity, not an intermediate alias hop; an LSP "go to definition" on any of the three spellings lands on the same declaration.

## Reference-level explanation

### Canonical identity

Every named source declaration is assigned one canonical symbol identity at its declaration site. That identity does not change based on how the declaration is later referenced. This covers: a function, model, class, trait, enum (and each of its variants), field, method, generic type parameter, a module-level or block-level binding introduced by `let` or plain assignment under its documented reassignment rules, a `for` loop variable, a `with ... as` binding, and an `except ... as` binding. All of these are ordinary bindings into a lexical namespace; none gets a separate identity model just because of which statement form introduced it.

A decorator's own name (for example `derive` in `@derive(CliArgs)`, or a trait name passed as its argument) is an ordinary reference and resolves through the same canonical identity as any other reference; a decorator application does not introduce a new kind of binding itself.

An import, an alias, and a re-export are bindings to an existing canonical symbol. None of the three creates a second canonical identity for the thing they name. A local declaration and an imported declaration differ only in how their binding enters scope, not in what kind of identity they carry.

A generic binder (a type parameter) has its own canonical identity, scoped to the declaration that introduces it, distinct from any concrete type that instantiates it.

A member (a field or method reached via `.`) resolves to the canonical identity of its declaration on the owning type, not to a fresh identity per access site.

### One shared namespace/binding-resolution mechanism

Today, collision and ambiguity detection for named things is scattered: `duplicate_library_export`, `duplicate_rust_module`, `duplicate_trait_instantiation`, `duplicate_alias`, `pub_library_import_name_collision`, and `pub_library_module_member_ambiguous` each independently implement their own narrow check for their own construct, in different files (`types.rs`, `modules.rs`, `check_stmt.rs`, `check_decl.rs`). That fragmentation is itself a source of the defects this RFC exists to close: two constructs can reasonably disagree about whether a collision is a collision, because each is running its own logic instead of consulting one shared answer.

This RFC requires one shared namespace/binding-resolution mechanism that every binding kind — local declarations, imports, aliases, re-exports, library exports, trait instantiations, generic binders, and members — registers into and is checked against. Collision and ambiguity detection is a property of that one mechanism, not something each construct's typechecker code independently decides. A specific diagnostic's wording may still vary by construct for a good user-facing message (an ambiguous `pub::` import and an ambiguous generic instantiation don't need identical phrasing), but the underlying question — "does this binding collide with, or ambiguously resolve against, an already-active binding for this spelling in this namespace" — is answered once, by one mechanism, for every construct.

### Namespace rule

At every program point, one spelling resolves to at most one active binding in a given ordinary lexical namespace. This RFC does not redefine how a binding becomes active — that is the existing `let`/plain-assignment/import contract in `scopes_and_name_resolution.md`, once the known scope-lookup bug against that contract is fixed (see Related). This RFC defines what a binding carries once it is resolved (its canonical identity) and requires that "is this binding active, and does it collide with another" be answered by the one shared mechanism above, not reimplemented per construct.

### Propagation through the pipeline

A resolved reference's canonical identity must be preserved, not recomputed from spelling, at every stage that consumes it:

- **HIR / Body IR** — a reference node carries the canonical identity, not only a source span or spelling.
- **Diagnostics** — an error or warning naming a symbol names its canonical identity's declaration site, regardless of which binding (local, import, alias, re-export) the offending reference used.
- **LSP** — "go to definition," "find references," and hover all resolve through canonical identity, so every binding to one declaration reports the same definition site.
- **Oven inspection / codegraph export** — an edge or fact referencing a symbol identifies it by canonical identity, so tooling can determine that two differently-spelled references mean the same thing without string comparison.
- **Backend emission** — the emitted Rust name for a canonical identity is a projection for that backend; it must not become a second identity another stage compares against.

### Diagnostics

A duplicate declaration or import that would introduce a second active binding for the same spelling in the same namespace at the same program point, without going through the explicit `let` shadowing form, is a diagnostic, raised by the one shared namespace/binding-resolution mechanism rather than by construct-specific checks. An import whose target cannot be resolved to exactly one canonical identity — because more than one visible declaration could satisfy it — is a diagnostic naming the candidates, raised by that same mechanism.

Existing construct-specific diagnostics that already implement a narrower version of this check (for example `duplicate_alias`, `duplicate_trait_instantiation`, `duplicate_library_export`) are expected to become call sites of the shared mechanism rather than parallel implementations; their user-facing wording may stay construct-specific, but the collision logic behind them should not. `pub_library_import_name_collision` remains tied to the separate `pub`/`rust`/`std` namespace-root import syntax (see Related) and is out of this RFC's scope to consolidate.

## Design details

### Interaction with the scope-lookup bug

This RFC's identity model assumes plain assignment and `let` behave as already documented once the known scope-lookup bug (see Related) is fixed: plain assignment reassigns the nearest active binding (requiring `mut` and a type match); `let` is the only form that introduces a new binding over an already-active one. This RFC does not depend on that fix landing first — the identity model applies equally to whichever binding is active under the corrected contract — but the two should be validated together, since a canonical-identity conformance fixture that exercises shadowing will only pass once the bug is fixed.

### Interaction with existing features

- **Imports and modules** — RFC 022's stdlib namespacing and canonical `std` root are unaffected; this RFC's identity model applies to `std` symbols the same as any other declaration.
- **Aliases** — RFC 083 already establishes that an alias preserves its target's semantic identity for imports, diagnostics, documentation, and metadata rather than acting as a copy or wrapper. This RFC generalizes that same principle to the full identity model rather than introducing a competing one.
- **Generic binders** — RFC 025's multi-instantiation trait dispatch resolves a call to one of several adopted instantiations by argument/return type; this RFC's canonical identity for a generic binder is what that dispatch resolves against, not a per-instantiation identity.
- **Rust interop** — an emitted Rust name remains a backend-specific projection of a canonical identity; this RFC does not change what interop code can call, only how the compiler tracks what a reference means before emission.

### Compatibility / migration

This RFC does not change accepted source syntax and is not expected to be a breaking change for correctly-typed existing programs. Programs that were relying on the undocumented, buggy shadowing behavior described above may see new diagnostics once that bug is fixed to match its own documented contract; that migration story belongs to the bug fix, not this RFC.

## Alternatives considered

### Let the backend or generated Rust resolve collisions

Rejected because source meaning would then vary by backend, and tooling could not inspect one authoritative result — exactly the dependency on generated Rust the v0.6 backend cutover is removing.

### Reopen the earlier v0.4 identity pass unchanged

Rejected. That pass was completed and explicitly excluded new namespace syntax; its scope does not cover imports, aliases, re-exports, or generic binders as one identity space.

### Use Rust-style separate type and value namespaces

Rejected because it introduces a second mental model that does not match Incan's Python-like ordinary lexical lookup, and nothing about the canonical-identity problem requires splitting the namespace to solve.

## Drawbacks

- Threading one canonical identity through every pipeline stage (HIR, Body IR, diagnostics, LSP, codegraph, backend) is real engineering work across most of the compiler, not confined to one layer.
- Conformance fixtures for the full matrix of local/import/alias/re-export/generic-binder/member combinations add meaningful test surface.

## Layers affected

- **Parser** — must attach enough declaration-site information for every named source object to be assigned a canonical identity later; must not itself resolve identity.
- **Typechecker / resolver** — must resolve every import, alias, re-export, generic binder, and member reference to the canonical identity of its underlying declaration; must consolidate collision and ambiguity detection for all binding kinds into one shared namespace/binding-resolution mechanism rather than construct-specific checks, and migrate existing construct-specific diagnostics to call sites of that mechanism.
- **HIR / Body IR** — must carry canonical identity on reference nodes rather than recomputable spelling alone.
- **Diagnostics** — must report a symbol's canonical declaration site regardless of which binding a reference used to reach it.
- **LSP** — must resolve "go to definition," "find references," and hover through canonical identity.
- **Codegraph / Oven inspection** — must key symbol edges and facts by canonical identity, not string spelling.
- **Backend emission** — must treat an emitted Rust name as a projection of canonical identity, never a second identity another stage compares against.

## Inspectability and tooling surface

- **Artifact or metadata:** codegraph export already reports declarations, references, and calls; this RFC requires those records to carry canonical identity so two differently-spelled references to one declaration are visibly the same fact, not two.
- **Inspection command:** `incan inspect codegraph --format jsonl` is the existing surface; no new command is introduced.
- **Diagnostics:** duplicate-binding and ambiguous-import diagnostics name the conflicting declarations by canonical identity and source span.
- **Provenance:** every canonical identity anchors to the source span of its one declaration site.
- **Not implicit:** an alias, re-export, or generic instantiation never silently becomes a second identity; a rename at a declaration site is visible as one consistent change everywhere the compiler reports that identity.

## Design decisions

- **Canonical identity covers every ordinary binding form, not just the originally-proposed set:** locals, imports, aliases, re-exports, generic binders, and members, plus `for` loop variables, `with ... as` bindings, and `except ... as` bindings. All are ordinary bindings into a lexical namespace; none earns a separate identity model just because of which statement form introduced it. A decorator's own name reference resolves through the same general mechanism rather than needing special-casing.
- **Collision and ambiguity detection consolidates into one shared namespace/binding-resolution mechanism, not construct-specific diagnostics:** today's fragmented approach (`duplicate_library_export`, `duplicate_rust_module`, `duplicate_trait_instantiation`, `duplicate_alias`, `pub_library_import_name_collision`, `pub_library_module_member_ambiguous`, each independently implemented in a different file) is itself a source of the defects this RFC exists to close — different constructs can disagree about what counts as a collision because each runs its own logic. Every binding kind registers into and is checked against one shared mechanism; existing construct-specific diagnostics migrate to call sites of it, keeping their own user-facing wording where that's genuinely useful, but not their own collision logic. `pub_library_import_name_collision` stays separate, tied to the `pub`/`rust`/`std` namespace-root import syntax (see Related).
- **Fixture sequencing relative to the scope-lookup bug fix is not an RFC design question.** The bug (plain assignment silently shadowing instead of reassigning inside a nested block, see Related) is against an already-documented contract; it gets fixed independently, on its own timeline, before or after this RFC's own implementation work as a practical sequencing call, not something this RFC needs to decide.
