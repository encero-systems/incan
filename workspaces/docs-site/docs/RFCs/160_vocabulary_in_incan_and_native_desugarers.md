# RFC 160: Vocabulary in Incan and native desugarers

- **Status:** Draft
- **Created:** 2026-10-24
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 027 (`incan-vocab` block registration and desugaring)
    - RFC 040 (scoped DSL surface forms)
    - RFC 045 (scoped DSL symbol surfaces)
    - RFC 081 (language-shaped DSL embeddings; Incan-authored desugarers)
    - RFC 108 (import-scoped extension properties; `pub vocab` blocks)
    - RFC 117 (`loaf.toml` and Oven's language-neutral project model)
    - RFC 124 (Oven store unit identity and cross-plan artifact sharing)
    - RFC 125 (`incan.pub` Loaf registry and baked asset distribution)
    - RFC 161 (the semantic core, SCIR)
    - RFC 162 (source surfaces: vocabularies, dialects and frontends)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.6 (in development)
- **Shipped in:** —

## Summary

This RFC defines how package-owned vocabulary enters Incan's canonical semantic pipeline. A Loaf names one Incan module as its **vocabulary entry**. That module registers the Loaf's DSL surfaces through `std.vocab` and, when a surface needs custom lowering, defines the desugarer that lowers parsed vocabulary syntax into Incan's canonical syntax tree. Oven compiles the entry for the host through the native compilation route, like any other unit, and records the registered descriptors in the Loaf's published metadata. When a consumer's source uses a surface that needs desugaring, the compiler runs the compiled desugarer as a separate worker process. The worker receives the module's vocabulary nodes as structured data and returns canonical syntax with a source map back to the original spans. Generated source text is never the interchange. Desugarers are no longer packaged as WebAssembly, and the compiler no longer embeds a WebAssembly runtime. This replaces the Rust companion crate of RFC 027 and the WebAssembly artifact contract that RFC 081 kept for Incan-authored desugarers. File-scoped activation, alternate grammars and foreign-language frontends are separate surfaces, which a later RFC specifies on top of this contract.

## Core model

1. **Vocabulary is Incan.** Registration and desugaring are written in Incan against `std.vocab`. No Rust companion crate, manifest file or build script is involved.
2. **The vocabulary entry is a unit.** Oven builds it for the host through the native route and identifies, caches and publishes it under RFC 124 and RFC 125 like any compiled unit.
3. **Registration runs at the producer.** Descriptors are computed when the Loaf is built and recorded in its published metadata. A consumer reads descriptors and never runs registration code.
4. **Desugaring runs out of process.** The compiler sends a module's vocabulary nodes to the desugarer as structured data and receives canonical syntax in return. A failing, crashing or overdue desugarer produces a diagnostic, never a compiler failure.
5. **Output carries provenance.** Every node a desugarer returns maps back to the input span it was derived from, so diagnostics point at what the author wrote.
6. **A desugarer is a pure function of its request.** Tooling may reuse a response for the same desugarer unit and the same request, and may keep workers running between requests.
7. **A desugarer is a trusted build-time unit.** It runs only when reached through the consumer's resolved dependencies, and the build's evidence records which unit lowered which module.
8. **There is one execution route.** Desugarers are native programs built by the same compiler as everything else; there is no second target, no WebAssembly runtime and no interpreter.

## Motivation

RFC 027 shipped vocabulary as a Rust companion crate: a `library_vocab()` function built with Cargo, whose desugarer was packaged as a WebAssembly module and run inside the compiler under a sandbox with a fuel budget. RFC 081 moved desugarer authoring to Incan, but kept the WebAssembly artifact as the execution contract, which meant compiling Incan desugarers to a second target and keeping the runtime in the compiler.

Three things have changed since then.

**The native route does not use Cargo.** Projects are `loaf.toml` Loaves that Oven builds itself. A Rust companion crate has no build path, and a library that declares vocabulary today publishes no descriptors at all. Consumers of such libraries fail to parse the blocks the library defines. Every vocabulary example in the repository is in that state.

**A WebAssembly route is a second route.** Keeping it means a second compilation target for Incan code, with its own standard library and runtime support. It also means embedding a WebAssembly engine and code generator in the compiler and the language server. That engine is a large dependency of its own, with its own native build on each host, Windows included, and its own security advisories. Everything else the toolchain runs is a native program built by one compiler. Desugarers are the only exception.

**The sandbox no longer buys what it was chosen for.** RFC 027 chose WebAssembly for portability, isolation and determinism.
- **Portability:** `incan.pub` already builds every Loaf for each host target, so one more compiled unit per target is ordinary.
- **Isolation:** the native route already loads Rust procedural macros from Loaves into the compiler as native code, so compile-time code from dependencies is already trusted by its published identity, and a sandbox around vocabulary alone does not change that.
- **Determinism:** reuse of compiled work already relies on compile-time code being a function of its inputs; desugarers fall under the same rule.

A desugarer written in Incan and compiled like any other Incan program removes the second target, the embedded engine, the Cargo dependency and the Rust authoring surface at once. Library authors write their vocabulary in the language their library is written in.

**Richer source surfaces need native providers.** Vocabulary today means new block forms inside Incan files. The same boundary is where alternate words for Incan constructs, alternate grammars with Incan semantics, and subsets of other languages would enter the compiler. Surfaces like those need real parsers and real lowering logic. In a native worker, such a provider can use an existing parser crate through Incan's Rust interop. In a WebAssembly sandbox, every such dependency has to be ported to the second target first. A native boundary with structured interchange and source maps is the foundation those surfaces can build on.

## Goals

- Let a Loaf register its vocabulary surfaces in Incan.
- Let a Loaf define its desugarer in Incan, against a public syntax model exposed by `std.vocab`.
- Build the vocabulary entry through the native route, as an ordinary unit that Oven caches and `incan.pub` publishes.
- Run desugarers outside the compiler's process, with bounded time and diagnostics for every failure.
- Exchange structured canonical syntax with source maps, never generated source text.
- Allow tooling to batch a module's nodes, reuse desugarer responses and keep workers running, so editing in the language server does not start a process per keystroke.
- Record in the build's evidence which provider unit lowered which module.
- Remove the WebAssembly packaging, the embedded WebAssembly runtime and the fuel budget.

## Non-Goals

- Changing the vocabulary surface model of RFC 027, RFC 040, RFC 045 or RFC 081: declarations, clauses, activation on import and embedded fragments keep their meaning.
- Changing RFC 108's `pub vocab` extension properties. Whether registration should be written as source declarations of that kind is an open question below.
- File-scoped surface activation (a header that selects a surface for a whole file), project-level surface defaults, alternate grammars with Incan semantics, and foreign-language frontends. RFC 162 specifies them; see "Relationship to source surfaces".
- Sandboxing desugarers. A desugarer has the trust of the dependency that ships it; see "Trust and evidence".
- Loading previously published WebAssembly desugarers.
- Defining how a consumer selects among competing vocabularies; activation rules are unchanged.

## Guide-level explanation

### For library authors

A library declares its vocabulary entry in `loaf.toml`:

```toml
[project]
name = "querykit"
version = "0.1.0"

[vocab]
entry = "src/vocab.incn"
```

The entry module registers the surfaces. This one adds a `query` declaration that becomes active where `querykit` is imported and desugars into an expression:

```incan
from std.vocab import ClauseSurface, DeclarationSurface, DslSurface, VocabRegistration


pub def library_vocab() -> VocabRegistration:
    """Register the `query` block, active wherever `querykit` is imported."""
    clauses = [
        ClauseSurface.expr("FROM").required(),
        ClauseSurface.expr_list("SELECT").required().after("FROM"),
    ]
    query = DeclarationSurface.named("query").with_clause_body().desugars_to_expression().with_clauses(clauses)
    return VocabRegistration.new().with_surface(DslSurface.on_import("querykit").with_declaration(query))
```

Because `query` desugars, the same module defines the desugarer. It receives the parsed block as `std.vocab` syntax values and returns canonical Incan syntax values. Nodes built from input values keep those values' spans, so the source map follows without extra work:

```incan
from std.vocab import DesugarError, DesugarOutput, DesugarRequest, DesugarResponse, IncanExpr, VocabSyntaxNode


pub def desugar(request: DesugarRequest) -> Result[DesugarResponse, DesugarError]:
    """Rewrite a `query` block into a call to `querykit.select` over its source."""
    match request.node:
        VocabSyntaxNode.Declaration(declaration) =>
            source = declaration.clause("FROM").head(0)?
            columns = declaration.clause("SELECT").head_list()
            call = IncanExpr.call(IncanExpr.path("querykit.select"), [source, IncanExpr.list(columns)])
            return Ok(DesugarResponse(output=DesugarOutput.Expression(call)))
        _ => return Err(DesugarError.at(request.node.span(), "query expects a declaration"))
```

The library builds as usual. Oven compiles `src/vocab.incn` and what it imports as the Loaf's vocabulary unit, runs `library_vocab()` once and records the descriptors in the Loaf's metadata. A vocabulary entry may call Rust through Incan's ordinary Rust interop, like any other Incan code.

### For library consumers

Nothing changes in consumer source. Importing `querykit` activates `query`, and the compiler desugars each `query` block before checking:

```incan
from querykit import query

rows = query:
    FROM orders
    SELECT id, total
```

If the desugarer reports an error, takes too long or exits abnormally, the consumer sees a diagnostic on the `query` block that names `querykit` as the providing Loaf. The rest of the module is still checked. A type error inside the desugared call points at the `FROM` or `SELECT` expression it came from, not at generated code.

### In the editor

The language server desugars blocks as the consumer edits them. Workers stay running between requests, and responses to unchanged blocks are reused, so editing a module that uses `query` does not start a process for each change.

## Reference-level explanation

### Declaring vocabulary

- A Loaf that provides vocabulary must declare `[vocab] entry = "<path>"`, where the path names an Incan module inside the Loaf.
- The `[vocab] crate` key must be refused with a diagnostic that names `entry` as its replacement.
- A Loaf declares at most one vocabulary entry. The entry and the modules it imports form the Loaf's vocabulary unit, which is distinct from the library's public units.
- The vocabulary unit must not activate the vocabulary it registers. A cycle in which the entry, directly or through its imports, uses its own surfaces must be refused with a diagnostic naming the cycle.

### Registration

- The entry module must export `library_vocab() -> VocabRegistration`.
- Oven must run `library_vocab()` when the Loaf is built, and record the resulting descriptors, with the `std.vocab` schema version they were built against, in the Loaf's published metadata.
- A consumer must obtain descriptors only from that metadata. A consumer build must not run registration code.
- Registration must be deterministic. Oven may reuse the recorded descriptors for an unchanged vocabulary unit.

### Desugaring

- When any registered surface declares custom desugaring, the entry module must export `desugar(request: DesugarRequest) -> Result[DesugarResponse, DesugarError]`. A registration that requires desugaring without that export must be refused when the Loaf is built.
- The compiler must run a desugarer outside its own process.
- A request must contain one or more public syntax nodes from one module, the requesting module's path, and nothing else. The compiler may batch all of a module's nodes for one desugarer into one request. A desugarer's output for a node must not depend on the other nodes in the same request.
- For each node, the response must contain canonical Incan statements or one canonical expression, matching the surface's declared output position. The interchange is the structured `std.vocab` syntax model; source text is not an accepted output.
- The compiler must check desugared output exactly as it checks authored source.

### Source maps

- An output node carries the input span it was derived from. That is either the span of an input value it was built from, or a span the desugarer assigns, which must lie within its input node. An output node without one maps to the span of the vocabulary node that produced it.
- Diagnostics inside desugared output must point at the finest span available.
- Diagnostics a desugarer reports must carry a span within the node it was given, or that node's span.

### Failures

- Each of the following must produce a diagnostic at the vocabulary node's span that names the providing Loaf, and must not stop the checking of the rest of the module:
    - an exit other than success;
    - a malformed response;
    - a response in the wrong output position;
    - a crash;
    - an invocation that exceeds its time bound.
- Every invocation must be bounded in time. The diagnostic for an exceeded bound must state the bound.

### Purity and reuse

- A desugarer's response must be a function of its vocabulary unit's identity and its request.
- Tooling may reuse a response for the same unit identity and the same request, and may keep worker processes running across requests.
- A desugarer must not depend on the environment, the clock, the file system or the network. The toolchain does not prevent such access; it is unsupported, and its results may be reused.

### Trust and evidence

- A desugarer must run only when its Loaf is reached through the consumer's resolved and locked dependencies. Activating a vocabulary adds no trust root beyond that dependency.
- A vocabulary unit declares no capabilities under this RFC. Any capability beyond a pure function of the request, such as declared file inputs, requires a later contract.
- Oven must record in the build's evidence, for each module it desugared, the providing Loaf, the vocabulary unit's identity, and whether each response was computed or reused.

### Building and publication

- Oven must build the vocabulary unit for the host target through the native route, under RFC 124 identity.
- `incan.pub` publishes vocabulary units like other compiled units. A consumer whose host has no published vocabulary unit for that Loaf builds it from the Loaf's source.

### Standard packages

- Standard Loaves that provide vocabulary follow this contract.
- A surface whose meaning the compiler implements itself, such as checked C binding declarations, registers its descriptors and declares no desugarer.

### WebAssembly desugarers

- The compiler must not load WebAssembly desugarer artifacts.
- A Loaf whose published metadata names one must be refused with a diagnostic stating that the Loaf must be rebuilt under this RFC.
- This supersedes RFC 081's requirement that published WebAssembly desugarer artifacts continue to load.

### Schema versions

- The public syntax model in `std.vocab` carries a schema version.
- The compiler must refuse a vocabulary unit built against a schema version it does not support, with a diagnostic naming both versions.

## Design details

### `std.vocab`

`std.vocab` is a standard Loaf. It contains the registration model:
- `VocabRegistration`, `DslSurface`, `DeclarationSurface`, `ClauseSurface` and the low-level keyword metadata of RFC 027;
- the library manifest metadata.

It also contains the public syntax model a desugarer reads and writes:
- `VocabSyntaxNode`, `VocabDeclaration`, `VocabClause`, `IncanExpr`, `IncanStatement`, `Span`;
- `DesugarRequest`, `DesugarResponse`, `DesugarOutput`, `DesugarError`.

These carry the shapes RFC 027 and RFC 081 define, expressed as Incan models and enums, with constructors and accessors for authoring desugarers. The types are part of the public standard library contract and evolve under the schema version above.

### Relationship to source surfaces

This RFC defines package-owned vocabulary registration and lowering. Vocabulary is one of three kinds of alternate source surface:
- **vocabulary:** alternate words, symbols and small forms over Incan's grammar and semantics;
- **dialect:** a different grammar with Incan's semantics;
- **frontend:** another source language, with its own parser, lowerability rules, runtime assumptions and source maps.

File-scoped activation of a surface, project-level surface defaults, dialects and frontends are separate concerns, specified by RFC 162. It selects a provider before ordinary parsing, and it owns the provider's identity, version, compatibility level, lowerability diagnostics, and runtime and ABI requirements. Those providers are expected to reuse this RFC's mechanisms:
- the vocabulary unit;
- out-of-process workers;
- structured canonical output with source maps;
- reuse;
- the trust and evidence rules.

Import-activated vocabulary keeps the surface model of RFC 027, RFC 040, RFC 045 and RFC 081.

The canonical syntax model in `std.vocab` is this RFC's interchange. Incan's semantic pipeline is headed toward a semantic core, which RFC 161 defines. A later schema version may let providers return that representation directly. Generated source text is not an interchange at any point.

### Interaction with existing features

- RFC 040 and RFC 045 surfaces register exactly as before; only the authoring language and the execution artifact change.
- RFC 081 embedded fragments reach desugarers through the same request.
- RFC 108 extension properties are unaffected.
- Formatter and syntax highlighting continue to use the published descriptors and do not run desugarers.

### Compatibility and migration

- A Loaf with a Rust companion crate migrates by moving its `library_vocab()` and its desugarer into an Incan module and naming it as `entry`.
- No published Loaf on `incan.pub` carries a WebAssembly desugarer at the time of writing. Published Loaves that carry vocabulary descriptors without a desugarer keep working once republished with an `entry`.

## Alternatives considered

1. **Compile Incan desugarers to WebAssembly and keep the runtime** (RFC 081's contract). This was rejected because it keeps a second compilation target for Incan, an embedded engine in the compiler and language server, that engine's native build on every host, and a fuel model that nothing else in the toolchain uses.
2. **Load desugarers into the compiler as native libraries.** This was rejected because a library loaded into the compiler must match the exact compiler build, and a panic or endless loop in it would take down the compiler or the language server. A separate process keeps the data contract independent of the compiler build and isolates failures.
3. **Interpret desugarers.** This was rejected because the native route is the only execution route; an interpreter would be a second semantics for Incan.
4. **Keep the Rust companion crate.** This was rejected because RFC 081 retired Rust desugarer authoring, and the native route has no Cargo build for it.
5. **Return generated source text.** This was rejected because text loses spans, comments and structure. Diagnostics would then point at generated code, and every provider would have to reproduce Incan's printer exactly.
6. **Keep a sandbox as the trust boundary.** This was rejected for now. Compile-time code from dependencies already runs natively through Rust procedural macros. The out-of-process boundary already isolates the compiler and the language server from crashes, and the locked dependency graph plus build evidence make the trust explicit. An operating-system sandbox around the worker can be added later without changing the contract.

## Drawbacks

- Desugarers are not sandboxed. A malicious or careless desugarer can read files or the network at compile time. This is the same trust a dependency's Rust procedural macros already receive; the evidence records make it auditable, not prevented.
- A vocabulary unit is compiled per host target, and a consumer on a host without a published unit compiles it from source.
- Starting a worker costs a process launch, which on some hosts includes a system security assessment of a newly built program. Retained workers keep this off the editing path.
- Any WebAssembly desugarer built for RFC 027 or RFC 081 must be rebuilt.

## Layers affected

- **Manifest / Oven**: the `[vocab] entry` key, the vocabulary unit's build and identity, running registration at build time, recording descriptors in published metadata, and keeping desugarer workers running.
- **Stdlib / Runtime**: the new `std.vocab` Loaf with the registration and public syntax models.
- **Desugar pass**: batched requests to out-of-process workers, response validation, source maps, the time bound, reuse, and diagnostics; the WebAssembly runtime and its fuel budget are removed.
- **Build evidence**: records of which vocabulary unit lowered which module, and whether each response was reused.
- **Registry**: publication of vocabulary units per host target.
- **LSP / Tooling**: desugaring through retained workers while editing; diagnostics for desugarer failures.
- **Formatter**: none beyond reading published descriptors.

## Unresolved questions

- What is the default time bound per invocation, and may a Loaf declare a larger one for heavy surfaces?
- Should a desugarer be able to declare file inputs, such as a schema it reads, that become part of the reuse identity, or must every desugarer stay a function of its request alone?
- Should registration be written as source declarations, in the style of RFC 108's `pub vocab` blocks, instead of a `library_vocab()` function that builds a registration value?
- Should workers run under an operating-system sandbox where the host provides one, and if so, should that be the default or an opt-in?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
