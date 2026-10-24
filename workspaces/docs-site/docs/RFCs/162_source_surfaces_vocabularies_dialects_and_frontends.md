# RFC 162: Source surfaces: vocabularies, dialects and frontends

- **Status:** Draft
- **Created:** 2026-10-24
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 027 (`incan-vocab` block registration and desugaring)
    - RFC 040 (scoped DSL surface forms)
    - RFC 081 (language-shaped DSL embeddings)
    - RFC 104 (ambient runtime capabilities and receipts)
    - RFC 117 (`loaf.toml` and Oven's language-neutral project model)
    - RFC 160 (vocabulary in Incan and native desugarers)
    - RFC 161 (the semantic core, SCIR)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.6 (in development)
- **Shipped in:** —

## Summary

This RFC lets a whole source file be written in a surface other than canonical Incan, while every surface still enters one Incan semantic model. It defines three kinds of **source surface**, distinguished by how far they are from Incan:
- a **vocabulary** keeps Incan's grammar and semantics with different words and small forms, such as Incan with keywords in another human language;
- a **dialect** has a different grammar with Incan's semantics;
- a **frontend** is another source language, parsed by its own parser and lowered through an explicit compatibility boundary.

A file selects its surface with a header directive (`#!vocab`, `#!dialect` or `#!frontend`) on its first line, and a project may set a default in `loaf.toml`. Surfaces are provided by Loaves on RFC 160's provider substrate: Incan-authored providers, built natively, admitted by policy, run as out-of-process workers and recorded in build evidence. This RFC adds the source-surface registration records and the whole-file request and response to that substrate. It also gives each kind its own execution shape:
- a vocabulary can be a static word table that needs no worker;
- a dialect needs a parsing and lowering worker;
- a frontend also needs lowerability diagnostics, a compatibility declaration and runtime requirements.

Every provider's output is RFC 160's canonical surface representation, and later SCIR under RFC 161, with a source map back to the original bytes. A construct a provider cannot lower faithfully is reported as a diagnostic, never as a silent change of meaning.

## Core model

1. **Many surfaces, one semantics.** Every surface lowers into canonical Incan and is checked by the ordinary Incan checker. No surface brings its own type system or backend.
2. **The kind is honest.** A vocabulary changes words, a dialect changes grammar, and a frontend translates a language. A provider declares its kind, and a frontend declares how much of its source language it accepts.
3. **Selection is explicit and file-scoped.** A file's surface is chosen by its header or its project's default, never as a side effect of an ordinary import.
4. **Providers are package-owned.** A surface comes from a Loaf, written in Incan, on RFC 160's substrate for building, admission, execution, reuse and evidence.
5. **A surface that cannot be resolved stops the file.** A missing, unadmitted or incompatible provider fails before the body is parsed. A file is never silently read as canonical Incan instead.
6. **Source maps are mandatory.** Every lowered node maps to a byte span in the original file, so diagnostics, the editor and receipts point at what the author wrote.
7. **Unsupported means a diagnostic.** A construct a provider cannot lower faithfully is reported in the source language's terms. It is never approximated.

## Motivation

RFC 027, RFC 040 and RFC 081 let libraries add block-shaped vocabulary inside Incan files. Some surfaces need a wider boundary than a block:
- A teaching community may want Incan with keywords and builtins in its own human language. The whole file uses those words, but nothing about its meaning differs.
- A team may want an Incan dialect whose declarations and annotations read the way the language they come from does, while keeping Incan's semantics.
- A migration may want to bring a typed subset of another language's code into an Incan project, lowered to Incan with every construct it cannot carry named explicitly.

Making ordinary imports change how a whole file parses would make a file's meaning depend on import order and on dependency internals. A file-level header is visible before the body is parsed. It tells every tool, from the compiler to the editor to a code reviewer, how to read the file.

The three kinds are kept distinct because they make different promises. A vocabulary that renames `def` promises nothing about another language. A dialect that reads like TypeScript must not be mistaken for TypeScript compatibility. A frontend that accepts a Python subset must say which subset, and must refuse what it cannot lower rather than guess.

RFC 160 provides the substrate such surfaces need:
- Incan-authored providers, built natively and admitted by policy;
- out-of-process workers with a structured representation and source maps;
- reuse and build evidence.

Native workers are what make real frontends practical, because a provider can use an existing parser for its source language through Incan's Rust interop. RFC 161 gives all surfaces one destination: the semantic core, where ownership and authority are explicit whatever surface produced the code. That single-authority rule is why this RFC exists. However a file is written, its meaning enters one semantic representation, and the backend never re-derives surface meaning.

## Goals

- Define vocabulary, dialect and frontend as the three kinds of source surface, with the promise each makes.
- Define file-scoped selection through a header directive, and project defaults in `loaf.toml`.
- Define the source-surface provider contract on top of RFC 160:
    - registration records;
    - the whole-file request and response;
    - each kind's execution shape;
    - identity, version and compatibility level;
    - source maps and lowerability diagnostics.
- Use RFC 160's substrate for building, admitting, running, reusing and recording providers, without redefining it.
- Fail before parsing whenever a selected surface cannot be resolved.
- Keep checking, ownership and authority in Incan's semantics, so RFC 104 capabilities apply to code from any surface.

## Non-Goals

- Shipping any particular vocabulary, dialect or frontend. Providers are packages.
- Promising compatibility with any external language. A frontend states its own compatibility level; Incan guarantees only the contract.
- Letting ordinary imports change how a file parses.
- Mixing surfaces within one file. Block vocabulary under RFC 027 and RFC 040 continues to work inside a file of any surface whose grammar admits it.
- Replacing RFC 081. Embedded fragments inside DSL positions keep RFC 081's contract in files of every surface.
- Foreign runtime semantics. A frontend lowers to Incan semantics; source-language behavior Incan does not have must be refused or provided explicitly by the provider's library.

## Guide-level explanation

### Choosing a surface for a file

A vocabulary that spells Incan's keywords and builtins in Spanish:

```incan
#!vocab incan_es.espanol

definir saludar(nombre: str) -> None:
    imprimir(f"Hola, {nombre}")
```

It is the same program as:

```incan
def saludar(nombre: str) -> None:
    println(f"Hola, {nombre}")
```

A dialect and a frontend are chosen the same way:

```text
#!dialect example_dialects.typescript_like
#!frontend example_frontends.python3_12.subset
```

The directive must be the file's first line that is neither blank nor a comment. It names a surface provided by one of the project's dependencies.

### Project defaults

A project whose files mostly use one surface may set a default:

```toml
[source]
surface = "vocab:incan_es.espanol"
```

A file header overrides the project default, and a file with neither is canonical Incan. If the selected surface's provider is missing, not admitted or incompatible, the build stops with an error on that file before its body is read. It never quietly treats the file as canonical Incan.

### What authors see

Errors point at the author's file and use its words. For a frontend, an unsupported construct is an error in the source language's terms:

```text
example.py:12:5: error: metaclasses cannot lower to Incan (frontend example_frontends.python3_12.subset)
```

The editor colors, navigates and diagnoses the file through the provider's descriptors and source map. `incan inspect` shows the canonical form of any file.

### Providing a surface

A surface provider is a Loaf whose RFC 160 vocabulary entry registers a source surface. `SourceSurface` and `VocabRegistration.with_source_surface` are this RFC's additions to `std.vocab`:

```incan
from std.vocab import SourceSurface, VocabRegistration


pub def library_vocab() -> VocabRegistration:
    """Register the Spanish vocabulary as a file-scoped surface."""
    surface = SourceSurface.vocabulary("espanol").with_keyword_map(SPANISH_KEYWORDS)
    return VocabRegistration.new().with_source_surface(surface)
```

A vocabulary whose mapping is a plain table of words needs no provider code: the compiler applies the table while lexing. A dialect or frontend defines the provider function that receives the whole file and returns canonical surface representation with a source map.

## Reference-level explanation

### Kinds

- A **vocabulary** must accept Incan's grammar with substituted words, symbols and small forms, and must lower to the same canonical surface representation that the corresponding canonical Incan would produce.
- A **dialect** may define its own grammar, and must lower to canonical Incan whose semantics the dialect documents as the meaning of its constructs.
- A **frontend** parses another language. It must declare:
    - the source language and version it accepts;
    - its compatibility level, one of `skin`, `subset`, `migration` or `compat`;
    - the runtime, standard-library and ABI requirements of its output.

  A frontend's surface name must include a version or a compatibility marker such as `subset`.

### Provider shapes

Each kind has a fixed execution shape. A provider must register as exactly one kind and meet that kind's requirements:

| Kind | Registration | Worker | Response must contain | Provider must declare |
| --- | --- | --- | --- | --- |
| vocabulary | a complete word and symbol table, optionally with small forms | none for a complete table; required for small forms | for small forms: canonical surface representation and a source map. For a table, the compiler produces both. | identity and version |
| dialect | a grammar provider | required | canonical surface representation, a source map for every node, diagnostics | identity, version, and the documented Incan meaning of each construct |
| frontend | a language provider | required | canonical surface representation, a source map for every node, lowerability diagnostics | identity, version, source language and version, compatibility level, runtime, standard-library and ABI requirements |

A response that lacks a field its kind requires must be refused as malformed under RFC 160's failure rules.

### Selection

- A file selects its surface with exactly one of `#!vocab NAME`, `#!dialect NAME` or `#!frontend NAME` as its first line that is neither blank nor a comment. The directive kind must match the provider's declared kind.
- `NAME` must resolve to a surface registered by a dependency in the project's resolved and locked dependencies, or by the project itself.
- A project may set `[source] surface = "KIND:NAME"` in `loaf.toml`. A file header must take precedence over the project default; a file with neither is canonical Incan.
- If the selected surface cannot be resolved, the file must fail with a diagnostic before its body is parsed. This applies to a surface selected by header or by project default, and covers a missing provider, one the build's provider policy does not admit, a kind mismatch, and an unsupported schema version. The compiler must not fall back to canonical Incan.
- An ordinary import must not change the surface of a file.

### Providers

- Surface providers must be built, admitted, run, reused and recorded under RFC 160.
- A whole-file request must contain the file's source bytes, its path relative to the project and the selected surface name.
- A whole-file response must contain the file's canonical surface representation, a source map and any diagnostics, as the provider's kind requires.
- Every output node must map to a byte span of the original file. Diagnostics must be rendered at the original span, naming the surface and its provider.
- A construct the provider cannot lower faithfully must produce a diagnostic. A provider must not substitute different semantics for it.
- A vocabulary whose registration is a complete word table may omit the provider function. The compiler then applies the table, with the same source map guarantees.

### Checking, authority and inspection

- Output from every surface must be checked by the ordinary Incan checker, under RFC 161 once the semantic core exists.
- Authority-bearing operations in output from any surface must carry their RFC 104 capability identities exactly as authored Incan would.
- `incan inspect` must be able to show the canonical form of any file, with its source map.

### Tooling

- Formatting a file in a non-canonical surface must preserve it unchanged unless its provider declares a formatter.
- The language server must use the provider's descriptors for highlighting, and the source map for diagnostics, navigation and hover.

## Design details

### Relationship to RFC 160 and RFC 161

RFC 160 is the substrate: how a provider Loaf is built, admitted, run, reused and recorded, and the versioned canonical surface representation it returns. This RFC adds to it, without redefining it:
- source-surface registration records in `std.vocab`;
- the whole-file request and response;
- each kind's execution shape;
- selection, compatibility levels and lowerability.

RFC 161 is the common destination: once the semantic core exists, every surface's output normalizes into it, and authority and ownership are explicit whatever surface produced the code.

### Relationship to block vocabulary

Block vocabulary activated by import, under RFC 027 and RFC 040, continues to work. A file-scoped vocabulary and a block vocabulary may share one provider; the difference is activation scope, not the authoring API.

### Relationship to RFC 081 embedded fragments

This RFC's whole-file provider model is distinct from RFC 081's block-local lexical modes, and does not replace them. RFC 081 remains the contract for embedded language-shaped fragments in eligible DSL positions:
- descriptor-gated submodes;
- expression holes;
- ownership boundaries;
- formatter and LSP behavior.

A file selected with `#!dialect`, or a file in any surface whose grammar admits DSL positions, may contain such fragments, and they follow RFC 081 there exactly as in canonical Incan. A whole-file provider lowers the file around its fragments. It does not re-parse or reinterpret a fragment.

### Packaging a frontend's sources

A Loaf may contain sources in a frontend's language with their native extension, mapped to a surface in `loaf.toml`. Its published metadata records the surface identity and version alongside the lowered Incan API, so consumers see an ordinary Incan library.

## Alternatives considered

1. **Let imports switch a file's grammar.** This was rejected because a file's meaning would depend on import order and dependency internals, and tools could not know how to read a file before resolving its imports.
2. **One directive with a kind field.** This was rejected in favor of three directives, because the kind is the promise a surface makes and belongs where a reader sees it first.
3. **Make Incan's core bilingual.** This was rejected because each human language would become permanent core complexity. A package-owned vocabulary keeps the core small and puts its words in the hands of the people who speak the language.
4. **Frontends that emit generated Incan source.** This was rejected per RFC 160: text loses spans and structure, and diagnostics would point at generated code.

## Drawbacks

- A file can no longer be read without knowing its surface. The header makes that visible, but a project default makes it implicit.
- Frontends invite compatibility expectations that no subset meets. Compatibility levels and lowerability diagnostics are there to keep claims honest, and a provider can still overclaim.
- Editor quality for a surface depends on its provider's descriptors and source maps.

## Layers affected

- **Lexer / Parser**: header detection; word-table vocabularies applied while lexing.
- **Manifest / Oven**: `[source] surface`; surface registration in vocabulary entries; frontend source files mapped to surfaces.
- **Desugar pass**: whole-file provider requests and responses under RFC 160.
- **Typechecker**: unchanged semantics; diagnostics rendered through source maps.
- **Formatter**: preserve non-canonical files unless the provider declares a formatter.
- **LSP / Tooling**: provider-driven highlighting; source-mapped diagnostics, navigation and hover; `incan inspect` of a file's canonical form.
- **Registry**: surface identity and version in published metadata.

## Unresolved questions

- How is canonical Incan selected explicitly in a project whose default is another surface: `#!surface incan`, `#!vocab incan`, or another spelling?
- Should a frontend's files keep their native extension, use `.incn`, or allow both?
- What is the minimum source-map granularity a dialect or frontend must meet before its diagnostics are user-facing?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
