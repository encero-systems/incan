# RFC 160: Vocabulary in Incan and native desugarers

- **Status:** Draft
- **Created:** 2026-10-24
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 027 (`incan-vocab` block registration and desugaring)
    - RFC 040 (scoped DSL surface forms)
    - RFC 045 (scoped DSL symbol surfaces)
    - RFC 081 (language-shaped DSL embeddings; Incan-authored desugarers)
    - RFC 104 (ambient runtime capabilities and receipts)
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

This RFC defines how package-owned vocabulary enters Incan's canonical semantic pipeline. A Loaf names one Incan module as its **vocabulary entry**. That module registers the Loaf's DSL surfaces through `std.vocab` and, when a surface needs custom lowering, defines the desugarer that lowers parsed vocabulary syntax into Incan's **canonical surface representation**. That representation is a versioned, structured contract; it is not the compiler's private syntax tree. Oven compiles the entry for the host through the native compilation route, like any other unit, and records the registered descriptors in the Loaf's published metadata. When a consumer's source uses a surface that needs desugaring, the compiler runs the compiled desugarer as a separate worker process, once the build's provider policy admits it. The worker receives the module's vocabulary nodes and returns canonical surface representation with a source map back to the original spans. Generated source text is never the interchange. Desugarers are no longer packaged as WebAssembly, and the compiler no longer embeds a WebAssembly runtime. This replaces the Rust companion crate of RFC 027, and RFC 081's desugarer authoring and WebAssembly execution-artifact contract. RFC 081 remains authoritative for descriptor-gated embedded syntax, lexical submodes, expression holes, formatter behavior and LSP ownership. This RFC is the provider substrate: execution, trust, evidence, workers and structured output. RFC 162 builds file-scoped surfaces, alternate grammars and foreign-language frontends on top of it. Both exist for RFC 161's single-authority rule: every surface normalizes into one semantic representation, and the backend never re-derives surface meaning.

## Core model

1. **Vocabulary is Incan.** Registration and desugaring are written in Incan against `std.vocab`. No Rust companion crate, manifest file or build script is involved.
2. **The vocabulary entry is a unit.** Oven builds it for the host through the native route and identifies, caches and publishes it under RFC 124 and RFC 125 like any compiled unit.
3. **Registration runs at the producer.** Descriptors are computed when the Loaf is built and recorded in its published metadata. A consumer reads descriptors and never runs registration code.
4. **Desugaring runs out of process.** The compiler sends a module's vocabulary nodes to the desugarer and receives canonical surface representation in return. A failing, crashing or overdue desugarer produces a diagnostic, never a compiler failure.
5. **Output carries provenance.** Every node a desugarer returns maps back to the input span it was derived from, so diagnostics point at what the author wrote.
6. **A desugarer is a pure function of its request.** Tooling may reuse a response for the same desugarer unit and the same request, and may keep workers running between requests.
7. **A desugarer runs only when admitted.** A locked dependency is necessary but not sufficient. The build's provider policy admits a desugarer by its identity, its declared authority and host policy. A desugarer invocation is granted no capabilities, and the build's evidence records the admission decision and which unit lowered which module.
8. **There is one execution route.** Desugarers are native programs built by the same compiler as everything else; there is no second target, no WebAssembly runtime and no interpreter.

## Motivation

RFC 027 shipped vocabulary as a Rust companion crate: a `library_vocab()` function built with Cargo, whose desugarer was packaged as a WebAssembly module and run inside the compiler under a sandbox with a fuel budget. RFC 081 moved desugarer authoring to Incan, but kept the WebAssembly artifact as the execution contract, which meant compiling Incan desugarers to a second target and keeping the runtime in the compiler.

Three things have changed since then.

**The native route does not use Cargo.** Projects are `loaf.toml` Loaves that Oven builds itself. A Rust companion crate has no build path, and a library that declares vocabulary today publishes no descriptors at all. Consumers of such libraries fail to parse the blocks the library defines. Every vocabulary example in the repository is in that state.

**A WebAssembly route is a second route.** Keeping it means a second compilation target for Incan code, with its own standard library and runtime support. It also means embedding a WebAssembly engine and code generator in the compiler and the language server. That engine is a large dependency of its own, with its own native build on each host, Windows included, and its own security advisories. Everything else the toolchain runs is a native program built by one compiler. Desugarers are the only exception.

**The sandbox no longer buys what it was chosen for.** RFC 027 chose WebAssembly for portability, isolation and determinism.
- **Portability:** `incan.pub` already builds every Loaf for each host target, so one more compiled unit per target is ordinary.
- **Isolation:** a separate worker isolates crashes and lets the toolchain terminate stuck work. Authority is a different boundary: RFC 104's capabilities must be enforced across the selected provider's operations, including native and interop paths. Published identity establishes which provider is admitted, not permission to bypass those checks. This RFC therefore requires correct capability enforcement without requiring an operating-system sandbox.
- **Determinism:** reuse of compiled work already relies on compile-time code being a function of its inputs; desugarers fall under the same rule.

A desugarer written in Incan and compiled like any other Incan program removes the second target, the embedded engine, the Cargo dependency and the Rust authoring surface at once. Library authors write their vocabulary in the language their library is written in.

**Richer source surfaces need native providers.** Vocabulary today means new block forms inside Incan files. The same boundary is where alternate words for Incan constructs, alternate grammars with Incan semantics, and subsets of other languages would enter the compiler. Surfaces like those need real parsers and real lowering logic. In a native worker, such a provider can use an existing parser crate through Incan's Rust interop. In a WebAssembly sandbox, every such dependency has to be ported to the second target first. A native boundary with structured interchange and source maps is the foundation those surfaces can build on.

## Goals

- Let a Loaf register its vocabulary surfaces in Incan.
- Let a Loaf define its desugarer in Incan, against a public syntax model exposed by `std.vocab`.
- Build the vocabulary entry through the native route, as an ordinary unit that Oven caches and `incan.pub` publishes.
- Run desugarers outside the compiler's process, with bounded time and diagnostics for every failure.
- Exchange a versioned canonical surface representation with source maps, never generated source text and never the compiler's private syntax tree.
- Allow tooling to batch a module's nodes, reuse desugarer responses and keep workers running, so editing in the language server does not start a process per keystroke.
- Record in the build's evidence which provider unit lowered which module.
- Remove the WebAssembly packaging, the embedded WebAssembly runtime and the fuel budget.

## Non-Goals

- Changing the vocabulary surface model of RFC 027, RFC 040, RFC 045 or RFC 081: declarations, clauses, activation on import and embedded fragments keep their meaning.
- Changing RFC 108's `pub vocab` extension properties. Whether registration should be written as source declarations of that kind is an open question below.
- File-scoped surface activation (a header that selects a surface for a whole file), project-level surface defaults, alternate grammars with Incan semantics, and foreign-language frontends. RFC 162 specifies them; see "Relationship to source surfaces".
- An operating-system sandbox for desugarers. Provider admission, enforced RFC 104 capabilities, and execution evidence are this RFC's controls; see "Trust and evidence".
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
    query = DeclarationSurface.named("query").with_clause_body().desugars_to_expression()
    query = query.with_clause(ClauseSurface.expr("FROM").required())
    surface = DslSurface.on_import("querykit").with_declaration(query)
    return VocabRegistration.new().with_surface(surface).with_desugarer()
```

Because `query` desugars, the same module defines the desugarer. It receives the parsed block as `std.vocab` values and returns values of the canonical surface representation. Nodes built from input values keep those values' spans, so the source map follows without extra work:

```incan
from std.vocab import DesugarError, DesugarOutput, DesugarRequest, DesugarResponse, VocabSyntaxNode


pub def desugar(request: DesugarRequest) -> Result[DesugarResponse, DesugarError]:
    """Return the FROM expression with its original source range."""
    match request.node:
        VocabSyntaxNode.Declaration(declaration) =>
            clause = declaration.clause("FROM")?
            expression = clause.expression()?
            return Ok(DesugarResponse(output=DesugarOutput.Expression(expression)))
        _ => return Err(DesugarError.at(request.node.span(), "query expects a declaration"))
```

The library builds as usual. Oven compiles `src/vocab.incn` and what it imports as the Loaf's vocabulary unit, runs `library_vocab()` once and records the descriptors in the Loaf's metadata. A vocabulary entry may call Rust through Incan's ordinary Rust interop, like any other Incan code.

### For library consumers

Nothing changes in consumer source. Importing `querykit` activates `query`, and the compiler desugars each `query` block before checking:

```incan
from pub::querykit import marker

def main() -> None:
    value = query:
        FROM marker() + 1
    println(value)
```

Here `marker()` is an ordinary function the library exports from `src/lib.incn`, and the consumer lists `querykit` as a dependency. If the desugarer reports an error, takes too long or exits abnormally, the consumer sees a diagnostic on the `query` block that names `querykit` as the providing Loaf. The rest of the module is still checked. A type error inside the expanded expression points at the `FROM` expression it came from, not at generated code.

### In the editor

The language server desugars blocks as the consumer edits them. Workers stay running between requests, and responses to unchanged blocks are reused, so editing a module that uses `query` does not start a process for each change.

## Reference-level explanation

### Declaring vocabulary

- A Loaf that provides vocabulary must declare `[vocab] entry = "<path>"`, where the path names an Incan module inside the Loaf.
- The `[vocab] crate` key must be refused with a diagnostic that names `entry` as its replacement.
- A Loaf declares at most one vocabulary entry. The entry and the modules it imports form the Loaf's vocabulary unit, which is distinct from the library's public units.
- The vocabulary unit must not activate the vocabulary it registers. An import of the Loaf's own surfaces, in the entry or any module it imports, is refused with a diagnostic naming that Loaf: `vocabulary activation cycle: querykit entry imports its own surfaces`.

### Registration

- The entry module must export `library_vocab() -> VocabRegistration`.
- Oven must run `library_vocab()` when the Loaf is built, and record the resulting descriptors, with the `std.vocab` schema version they were built against, in the Loaf's published metadata.
- A consumer must obtain descriptors only from that metadata. A consumer build must not run registration code.
- Registration must be deterministic. Oven may reuse the recorded descriptors for an unchanged vocabulary unit.
- Producer-side registration must participate in the build's RFC 104 authority boundary, using its selected mode, effective grants, and any host-supplied ceiling. Building or importing the Loaf must not grant the registration function authority by implication. Pure descriptor construction requires no host capability. In reporting modes, attempted authority-bearing operations must retain RFC 104 receipts, including denials; in governed mode, an ungranted operation must fail before its authority-bearing behavior. A grant must not waive deterministic registration or the build's declared-input and identity rules.

### Desugaring

- When any registered surface declares custom desugaring, the entry module must export `desugar(request: DesugarRequest) -> Result[DesugarResponse, DesugarError]`. A registration that requires desugaring without that export must be refused when the Loaf is built.
- The compiler must run a desugarer outside its own process.
- Each `DesugarRequest` must contain the representation's schema version (`schema_version`), one public syntax node (`node: VocabSyntaxNode`), the requesting module's path (`module_path: str`), and nothing else. The compiler may send all of a module's requests for one desugarer as one batch and must receive the responses in the same order. A desugarer's output for a node must not depend on the other nodes in the same batch.
- For each node, the response must contain compilation-unit declarations, statements or one expression of the canonical surface representation, matching the surface's declared output position. `DesugarOutput` has `Declarations(list[IncanDeclaration])`, `Statements(list[IncanStatement])` and `Expression(IncanExpr)`. Source text is not an accepted output.
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
    - a crash.

### Purity and reuse

- A desugarer's response must be a function of its vocabulary unit's identity and its request.
- Tooling may reuse a response for the same unit identity and the same request, and may keep worker processes running across requests.
- A desugarer must not depend on the environment, the clock, the file system or the network. Governed execution must enforce its empty capability grant under RFC 104 rather than merely declare these accesses unsupported. A permitted native dependency must not introduce an unmediated authority-bearing path around that contract. Clearing the environment alone is not evidence of enforcement.
- The compiler clears the worker's inherited environment before starting it. This removes ambient environment inputs; it does not provide an operating-system sandbox.

### Trust and evidence

- A desugarer must run only after the build's provider policy admits it. Admission considers:
    - the provider's identity: its Loaf, reached through the consumer's resolved and locked dependencies, with its checked manifest digest, worker digest and vocabulary unit identity;
    - its declared authority;
    - host policy.

  Being a locked dependency is necessary but not sufficient. The compiler must verify the admitted identity against the worker artifact it starts.
- The default policy admits providers reached through locked dependencies. A host or project policy may restrict admission further, for example to named providers or to providers with published units. The lock and the invocation evidence must record the policy that admitted each provider.
- A vocabulary unit declares no authority under this RFC. Any authority beyond a pure function of the request, such as declared file inputs, requires a later contract.
- A desugarer invocation must run as a governed RFC 104 execution boundary with an empty effective capability grant. A containing build's mode or broader grants must not widen that grant. Any authority-bearing operation a desugarer attempts must be denied before its effect and recorded under RFC 104.
- Capability enforcement, rather than an operating-system sandbox, is the required authority boundary. Enforcement must cover authority-bearing paths through native dependencies and Rust interop as well as ordinary Incan calls; declaring a capability or admitting a provider is not enforcement by itself. If the selected provider path cannot satisfy that contract, governed execution must be refused with a diagnostic identifying the uncovered boundary. The toolchain must not execute that path under an empty grant while claiming its authority is governed. This RFC does not prescribe the internal enforcement mechanism or claim that the required coverage is already implemented.
- Oven must record in the build's evidence, for each module it desugared:
    - the admission decision and the policy that made it;
    - the providing Loaf;
    - the vocabulary unit's identity;
    - whether each response was computed or reused.

### Building and publication

- Oven must build the vocabulary unit for the host target through the native route, under RFC 124 identity.
- `incan.pub` publishes vocabulary units like other compiled units. A consumer whose host has no published vocabulary unit for that Loaf builds it from the Loaf's source.

### Standard packages

- Standard Loaves that provide vocabulary follow this contract.
- A surface whose meaning the compiler implements itself, such as checked C binding declarations, registers its descriptors and declares no desugarer.

### WebAssembly desugarers

- The compiler must not load WebAssembly desugarer artifacts.
- A Loaf whose published metadata names one must be refused with a diagnostic stating that the Loaf must be rebuilt under this RFC.
- This supersedes RFC 081's requirement that published WebAssembly desugarer artifacts continue to load. It supersedes nothing else in RFC 081.

### Embedded fragments (RFC 081)

- An embedded language-shaped fragment in an eligible DSL position remains an owned, structured artifact under RFC 081. That covers its lexical submode, expression holes, ownership boundary, and formatter and LSP behavior.
- When the owning surface provides a lowering hook, the hook is a desugarer under this RFC. It receives the fragment artifact in its request and may lower it into canonical surface representation.
- When no lowering hook exists, RFC 081's behavior is unchanged: the fragment checks and reaches the checked body representation, and emission refuses it explicitly.

### Schema versions

- The public syntax model in `std.vocab` carries a schema version.
- The compiler must refuse a vocabulary unit built against a schema version it does not support, with a diagnostic naming both versions.

## Design details

### `std.vocab`

`std.vocab` is a standard Loaf. It contains the registration model:
- `VocabRegistration`, `DslSurface`, `DeclarationSurface`, `ClauseSurface` and the low-level keyword metadata of RFC 027;
- the library manifest metadata.

It also contains the canonical surface representation a desugarer reads and writes:
- `VocabSyntaxNode`, `VocabDeclaration`, `VocabClause`, `IncanExpr`, `IncanStatement`, `IncanDeclaration`, `Span`;
- `DesugarRequest`, `DesugarResponse`, `DesugarOutput`, `DesugarError`.

These carry the shapes RFC 027 and RFC 081 define, expressed as Incan models and enums, with constructors and accessors for authoring desugarers. The representation is initially syntax-shaped, but it is deliberately not the compiler's private syntax tree. It is a published, versioned contract that the compiler translates from and into its own representations. Changing the compiler's internal syntax tree therefore never changes a package's interface. A later schema version may target RFC 161's semantic core directly.

### Relationship to source surfaces

This RFC defines package-owned vocabulary registration and lowering. Vocabulary is one of three kinds of alternate source surface:
- **vocabulary:** alternate words, symbols and small forms over Incan's grammar and semantics;
- **dialect:** a different grammar with Incan's semantics;
- **frontend:** another source language, with its own parser, lowerability rules, runtime assumptions and source maps.

This RFC is the provider substrate: the vocabulary unit, admission, out-of-process workers, the canonical surface representation with source maps, reuse, and evidence. RFC 162 specifies file-scoped activation, project-level surface defaults, dialects and frontends on top of that substrate. It selects a provider before ordinary parsing, and adds:
- the source-surface registration records;
- the whole-file request and response shapes;
- each provider kind's execution shape;
- compatibility levels, lowerability diagnostics, and runtime and ABI requirements.

Import-activated vocabulary keeps the surface model of RFC 027, RFC 040, RFC 045 and RFC 081.

The four RFCs divide the work:

```text
RFC 081   embedded language-shaped fragments inside DSL positions
RFC 160   package-owned provider registration, execution, provenance and lowering
RFC 162   whole-file and project surface selection; the vocabulary, dialect and frontend taxonomy
RFC 161   the common semantic destination for every lowered surface
```

RFC 160 and RFC 162 exist for RFC 161's single-authority rule. Every surface normalizes into one semantic representation, and the backend never re-derives surface meaning. Generated source text is not an interchange at any point.

### Interaction with existing features

- RFC 040 and RFC 045 surfaces register exactly as before; only the authoring language and the execution artifact change.
- RFC 081 embedded fragments keep their RFC 081 contract; only their lowering hook runs under this RFC, as "Embedded fragments (RFC 081)" states.
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
6. **Require an operating-system sandbox for vocabulary workers.** Rejected. RFC 104 owns the capability and receipt contract, and this RFC requires that contract to be enforced across the selected provider's authority-bearing paths. A separate process isolates worker failures; it is not the authority boundary. An operating-system sandbox may be a host implementation measure, but is not an additional requirement of this RFC or a replacement for capability enforcement.

## Drawbacks

- Enforcing capabilities through native dependencies and Rust interop requires complete boundary coverage. An uncovered path cannot be admitted for governed execution merely because the provider is locked or trusted by identity; until its required coverage exists, that path is unavailable in governed execution. This is an implementation obligation, not a claim that arbitrary native code is already constrained or that receipts themselves prevent access.
- A vocabulary unit is compiled per host target, and a consumer on a host without a published unit compiles it from source.
- Starting a worker costs a process launch, which on some hosts includes a system security assessment of a newly built program. Retained workers keep this off the editing path.
- Any WebAssembly desugarer built for RFC 027 or RFC 081 must be rebuilt.

## Layers affected

- **Manifest / Oven**: the `[vocab] entry` key, the vocabulary unit's build and identity, running registration at build time, recording descriptors in published metadata, and keeping desugarer workers running.
- **Stdlib / Runtime**: the new `std.vocab` Loaf with the registration and public syntax models.
- **Desugar pass**: batched requests to out-of-process workers, response validation, source maps, reuse, and diagnostics; the WebAssembly runtime and its fuel budget are removed.
- **Build evidence**: provider admission decisions, which vocabulary unit lowered which module, and whether each response was reused.
- **Registry**: publication of vocabulary units per host target.
- **LSP / Tooling**: desugaring through retained workers while editing; diagnostics for desugarer failures.
- **Formatter**: none beyond reading published descriptors.

## Design Decisions

- Desugaring remains a function of its vocabulary unit identity and its request. Declared external file inputs and their additional authority, schema, dependency, and reuse contracts are outside this RFC. Naming a file in a syntax node or in `module_path` must not grant permission to read it.
- Keep `[vocab] entry` and the typed `library_vocab() -> VocabRegistration` function. Registration executes at the producer; consumers read its published descriptors. Producer registration uses RFC 104's existing authority and receipt contract, without introducing another grant mechanism or implicit package authority.
- An operating-system sandbox is not required. Correct capability enforcement under RFC 104 is the authority boundary, including native and interop paths. Process separation remains worker-failure isolation. Unknown enforcement coverage must be diagnosed before governed execution rather than hidden behind an empty capability declaration or an audit claim.

- No desugarer-specific timeout, invocation budget, or package timeout declaration is required. Authoring a desugarer in Incan does not introduce an additional timing contract. This does not alter independently configured host policies or RFC 104 capability constraints.
