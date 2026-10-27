# RFC 162: File syntax: vocabularies, dialects and frontends

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

This RFC lets a whole source file be written in a surface other than canonical Incan, while every surface still enters one Incan semantic model. It defines three kinds of **file syntax**, distinguished by how far they are from Incan:
- a **vocabulary** keeps Incan's grammar and semantics with different words and small forms, such as Incan with keywords in another human language;
- a **dialect** has a different grammar with Incan's semantics;
- a **frontend** is another source language, parsed by its own parser and lowered through an explicit compatibility boundary.

An .incn file selects its syntax through an explicit header or a project default. Native-language files keep their extensions and original bytes; loaf.toml explicitly selects their frontend through file/path patterns. The built-in #!syntax incan header restores canonical Incan in an .incn file whose project default is another syntax. Surfaces are provided by Loaves on RFC 160's provider substrate: Incan-authored providers, built natively, admitted by policy, run as out-of-process workers and recorded in build evidence. This RFC adds the file-syntax registration records and the whole-file request and response to that substrate. It also gives each kind its own execution shape:
- a vocabulary can be a static word table that needs no worker;
- a dialect needs a parsing and lowering worker;
- a frontend also needs lowerability diagnostics, a compatibility declaration and runtime requirements.

Every provider's output is RFC 160's canonical surface representation, and later SCIR under RFC 161, with a source map back to the original bytes. A construct a provider cannot lower faithfully is reported as a diagnostic, never as a silent change of meaning.

## Core model

1. **Many surfaces, one semantics.** Every surface lowers into canonical Incan and is checked by the ordinary Incan checker. No surface brings its own type system or backend.
2. **The kind is honest.** A vocabulary changes words, a dialect changes grammar, and a frontend translates a language. A provider declares its kind, and a frontend declares how much of its source language it accepts.
3. **Selection is explicit and file-scoped.** An .incn file uses its explicit header or project default; native files use explicit frontend selection in loaf.toml. Ordinary imports never change how a file is parsed.
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

- Define vocabulary, dialect and frontend as the three kinds of file syntax, with the promise each makes.
- Define file-scoped selection through a header directive, and project defaults in `loaf.toml`.
- Define the file-syntax provider contract on top of RFC 160:
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

An .incn directive must be the first significant header line, allowing preceding blank lines and ordinary comments. Native files do not need or receive Incan directives. It names a surface provided by one of the project's dependencies.

### Project defaults

A project whose files mostly use one surface may set a default:

```toml
[source]
syntax = "vocab:incan_es.espanol"
```

An .incn file header overrides its project default; an .incn file with neither is canonical Incan. The built-in #!syntax incan header explicitly selects canonical Incan without a provider. Native files are selected only through the frontend rules below, not by that .incn default. If the selected surface's provider is missing, not admitted or incompatible, the build stops with an error on that file before its body is read. It never quietly treats the file as canonical Incan.

### Native Python and Java sources remain unchanged

The following manifest addition explicitly selects native-language files. Provider names are illustrative dependencies accepting the shown subsets, not a claim that these packages exist or provide complete language compatibility.

```toml
[[source.frontends]]
provider = "example_frontends.python3_12.subset"
include = ["legacy/python/**/*.py"]
exclude = ["legacy/python/tests/**"]

[[source.frontends]]
provider = "example_frontends.java21.subset"
include = ["legacy/java/**/*.java"]
```

The corresponding complete native files retain ordinary language syntax and need no injected header:

```text
File legacy/python/greeting.py:
    def greeting(name: str) -> str:
        return "Hello, " + name

File legacy/java/Greeter.java:
    public final class Greeter {
        public static String greeting(String name) {
            return "Hello, " + name;
        }
    }
```

An .incn author may deliberately select frontend syntax with #!frontend in that file. That explicit override does not make Incan-specific directives a requirement for native files. A braced-declaration dialect is another distinct use: it may parse `record Point { x: int; y: int; }` into canonical `model Point` fields while retaining Incan meaning, without claiming compatibility with Java or Python.

### What authors see

Errors point at the author's file and use its words. For a frontend, an unsupported construct is an error in the source language's terms:

```text
example.py:12:5: error: metaclasses cannot lower to Incan (frontend example_frontends.python3_12.subset)
```

The editor colors, navigates and diagnoses the file through the provider's descriptors and source map. `incan inspect` shows the canonical form of any file.

### Providing a surface

A surface provider is a Loaf whose RFC 160 vocabulary entry registers a file syntax. `FileSyntax` and `VocabRegistration.with_file_syntax` are this RFC's additions to `std.vocab`:

```incan
from std.vocab import FileSyntax, VocabRegistration


pub def library_vocab() -> VocabRegistration:
    """Register the Spanish vocabulary as a file-scoped surface."""
    syntax = FileSyntax.vocabulary("espanol").with_keyword_map({"definir": "def", "imprimir": "println"})
    return VocabRegistration.new().with_file_syntax(syntax)
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

- An .incn file must accept exactly one explicit header: #!syntax incan for the built-in canonical syntax, or #!vocab NAME, #!dialect NAME or #!frontend NAME for a package-owned provider. It must be the first significant header after optional blank lines and ordinary comments. The declared kind must match the resolved provider. The canonical escape needs no provider.
- A project may set `[source] syntax` to `incan`, `vocab:NAME` or `dialect:NAME` as the default for .incn files. A frontend must not become an implicit .incn project default; its .incn use requires the explicit file header. A file header takes precedence; absent both, the file is canonical Incan. The canonical schema term “canonical surface representation” remains RFC 160's structured interchange, not another authored syntax.
- Native-language sources must retain their native extensions and original bytes. They enter the build through explicit `[[source.frontends]]` tables containing provider and include, with optional exclude. Include/exclude are lists of project-relative path glob patterns: * matches within one path component, while ** matches zero or more directory components. Exclusion removes a path from that rule. Absolute paths and parent traversal must be rejected.
- Multiple patterns selecting the same file for the same canonical provider must not duplicate compilation. A file selected for different providers must receive a configuration diagnostic identifying both rules; table order must not silently choose a winner. Provider identity and selected input evidence must be recorded under RFC 160.
- These native-file rules must not silently reinterpret ordinary .incn files. A frontend inside .incn is an explicit file-header choice. A native extension alone is not permission to execute a frontend; a native file outside selected paths is not a source input of this frontend build.
- Provider names must resolve through the project's checked locked dependencies or its own registration. Missing, unadmitted, kind-mismatched or schema-incompatible selections must fail before body translation, never fall back to another parser. Ordinary imports must not change file syntax.

### Providers

- Surface providers must be built, admitted, run, reused and recorded under RFC 160.
- A whole-file request must contain the file's source bytes, its path relative to the project and the selected surface name.
- A whole-file response must contain the file's canonical surface representation, a source map and any diagnostics, as the provider's kind requires.
- Every output node must retain its responsible original token or construct span through parsing, frontend translation, checked IR and backend lowering. A later diagnostic must point back to that authored span, preserving original type names where available and identifying its provider. Synthetic nodes retain the responsible authored construct plus generation provenance; they must not fabricate handwritten tokens. A whole-file placeholder is insufficient when the responsible expression is known. Canonical Incan, Rust-origin code and custom frontends must meet the same provenance standard.
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
- file-syntax registration records in `std.vocab`;
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

A Loaf may contain unchanged native-language sources selected by the explicit frontend path rules in loaf.toml. Its published metadata records the surface identity and version alongside the lowered Incan API, so consumers see an ordinary Incan library.

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
- **Manifest / Oven**: `[source] syntax`; surface registration in vocabulary entries; frontend source files mapped to surfaces.
- **Desugar pass**: whole-file provider requests and responses under RFC 160.
- **Typechecker**: unchanged semantics; diagnostics rendered through source maps.
- **Formatter**: preserve non-canonical files unless the provider declares a formatter.
- **LSP / Tooling**: provider-driven highlighting; source-mapped diagnostics, navigation and hover; `incan inspect` of a file's canonical form.
- **Registry**: surface identity and version in published metadata.

## Design Decisions

- File syntax is the umbrella term; vocabulary changes words, dialect changes grammar, and frontend translates a declared external-language subset. The built-in #!syntax incan header explicitly selects canonical Incan without a provider. The proposed registration descriptor is FileSyntax, and the project default uses [source] syntax.
- Native files retain their extension and bytes. Explicit loaf.toml provider/include/exclude rules select them without inserting directives; #!frontend remains an explicit override in an .incn file. Recursive ** and component-local * are path globs, and conflicting providers diagnose rather than use rule-order precedence.
- Every AST/IR/backend path preserves the responsible source token or construct. Diagnostics use that same provenance standard for Incan, Rust-origin code and custom frontends; generated nodes retain honest authored origins and generation facts.
- Full SCIR implementation is not a prerequisite: providers use RFC 160's canonical structured representation today and normalize through RFC 161 once that core is delivered. This design does not schedule the compiler-wide migration.
