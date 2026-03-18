# RFC 027: `incan-vocab` — Library Vocabulary Registration Crate

- **Status:** Implemented
- **Created:** 2026-03-06
- **Author(s):** Danny Meijer (@dannymeijer)
- **Issue:** [#161](https://github.com/dannys-code-corner/incan/issues/161)
- **RFC PR:**
    - Phases 1-3: [#176](https://github.com/dannys-code-corner/incan/pull/176)
    - Phases 4-9: [#178](https://github.com/dannys-code-corner/incan/pull/177)
- **Related**:
    - RFC 022 (stdlib namespacing)
    - RFC 023 (compilable stdlib and Rust module binding)
    - RFC 031 (library system phase 1)
    - RFC 040 (scoped DSL glyph surfaces)
- **Written against:** v0.1
- **Shipped in:** v0.2

> Addendum (2026-03-16): See the addendum at the end of this RFC for the release authoring contract for companion crates. Where producer-side workflow details differ, the addendum governs.

## Summary

This RFC defines **`incan-vocab`**, a standalone Rust crate that gives library authors one stable Rust entrypoint for describing richer DSL surfaces through `VocabRegistration`, `DslSurface`, `DeclarationSurface`, `ClauseSurface`, and `VocabDesugarer`, while preserving one unified activation and registry model across core language, stdlib, and libraries; low-level keyword transport remains part of the system, but it is derived from the richer author-facing surface rather than hand-authored directly, and scoped glyph semantics remain explicitly out of scope for RFC 027.

## Motivation

Incan currently maintains **two separate keyword systems**: a compile-time `KEYWORDS` const table of ~40 hard keywords (`def`, `if`, `for`, etc.) recognized directly by the `lexer`, and a small `info_soft()` mechanism for 3 import-activated keywords (`async`, `await`, `assert`). Third-party libraries have no way to participate in either system. This split creates multiple problems:

1. **No stable API surface.** `incan_core::lang::keywords` is internal; breaking changes would cascade to every library.
2. **No manifest schema.** Libraries have no way to declare their exported types, functions, and modules in a machine-readable format that the compiler can consume.
3. **No vocab registration path.** Adding a new keyword currently requires modifying the compiler's `KEYWORDS` const array.
4. **Feature scanning debt.** The compiler uses ad-hoc `needs_web`, `needs_serde`, `scan_for_*` booleans to detect library usage. This doesn't scale beyond the `stdlib`.
5. **No desugaring path for richer DSLs.** When a library introduces richer syntax such as `query { ... }`, typed `step` declarations, or workflow-like orchestration forms, the compiler needs a stable public AST and desugar boundary. Without that boundary, those declarations and clause-owned grammars cannot be lowered into typecheckable Incan code in a principled way.
6. **Two keyword systems where one would do.** Hard and soft keywords share the same data — a name, a parsing shape, and activation rules — yet they're implemented as separate subsystems with different types, lookup paths, and parser dispatch. The stdlib's `async`/`await`/`assert` are further special-cased via `scan_for_*` booleans. A unified registry eliminates this accidental complexity, gives the LSP and formatter a single source of truth, and battle-tests the extension API on the stdlib before any external library exists.

`incan-vocab` solves all six by extracting a minimal, stable crate that models **every** keyword uniformly — core, stdlib, and third-party — differing only in activation rule and source.

## Goals

- Deliver an internal-first migration that unifies today's core language and stdlib keyword metadata under one registry and one activation model before external library loading exists.
- Define a stable, published `incan-vocab` crate that serves as the single extension point for keyword registration across core language, stdlib, and third-party libraries.
- Replace the hard-keyword `KEYWORDS` table and soft-keyword `info_soft()` path with one `KeywordRegistry` consumed uniformly by the compiler, LSP, formatter, and editor grammar generation.
- Give library authors a typed Rust API (`VocabRegistration`, `DslSurface`, `DeclarationSurface`, `ClauseSurface`, `VocabDesugarer`) to declare richer DSL surfaces and desugar them once library build and consumer loading flows exist.
- Establish the manifest schema types (`LibraryManifest`, `TypeRef`, `CargoDependency`) used by the library build and consumer flows defined in RFC 031.

## Non-Goals

- Defining the global meaning of operators (`+`, `>>`, `|>`, etc.) — that belongs to RFC 028.
- Defining scoped glyph surfaces for explicit DSL blocks — that belongs to RFC 040.
- External plugin loading via dynamic libraries (`cdylib`/`libloading`) — desugarers for external libraries use WASM (Phase 4+), not native shared libraries.
- Implementing the `incan.pub` registry or git-based dependency resolution — those are Phase 2/3 concerns addressed by RFC 034.
- Replacing the existing `[rust-dependencies]`/Cargo wiring — that is RFC 031's concern.
- Implementing external library artifact transport or consumer loading ahead of RFC 031 — this RFC defines the contracts those later phases use.

## Guide-level explanation

### For library authors

For public documentation, this RFC uses an outward-safe surrogate library called `studiokit`. The real internal forcing functions are private DSLs; this surrogate example exists to lock the author-facing surface without exposing them.

**1. Project structure** — Your Incan library project uses the standard `incan init` layout, plus a `crates/` directory for Rust code:

```text
studiokit/
├── incan.toml                 # Incan project manifest
├── src/                       # Incan source (.incn files)
│   └── lib.incn
├── crates/
│   └── studiokit-vocab/
│       ├── Cargo.toml
│       └── src/
│           └── lib.rs         # exports pub fn library_vocab()
└── tests/
```

Key insight:

- `src/` is for Incan code.
- `crates/` is for Rust code.
- the companion crate exports one obvious Rust function: `library_vocab()`

**2. Export `library_vocab()`** — In `crates/studiokit-vocab/src/lib.rs`:

```rust
use incan_vocab::{ClauseSurface, DeclarationSurface, DslSurface, LibraryManifest, VocabRegistration};

pub struct StudioKitDesugarer;

// impl VocabDesugarer for StudioKitDesugarer { ... }

pub fn library_vocab() -> VocabRegistration {
    VocabRegistration::new()
        .with_surface(
            DslSurface::on_import("studiokit")
                .with_declaration(
                    DeclarationSurface::named("query")
                        .with_clause_body()
                        .desugars_to_expression()
                        .with_clauses([
                            ClauseSurface::expr("FROM").required(),
                            ClauseSurface::expr("RELATE").repeating(),
                            ClauseSurface::expr("FILTER").optional().after("FROM"),
                            ClauseSurface::expr_list("GROUP BY").optional().after("FILTER"),
                            ClauseSurface::expr_list("SELECT").required().after("GROUP BY"),
                            ClauseSurface::nested_items("WINDOW BY").optional().after("SELECT"),
                        ]),
                )
                .with_declaration(
                    DeclarationSurface::named("step")
                        .with_signature_head()
                        .with_mixed_body()
                        .with_clauses([
                            ClauseSurface::fields("config").optional(),
                            ClauseSurface::type_ref("input").required(),
                            ClauseSurface::type_ref("output").required().after("input"),
                        ]),
                )
                .with_declaration(
                    DeclarationSurface::named("workflow")
                        .with_header_args()
                        .with_statement_body(),
                ),
        )
        .with_library_manifest(LibraryManifest::default())
        .with_desugarer(StudioKitDesugarer)
}
```

This is the intended design center for the crate:

- authors declare one activated DSL surface at a time
- declarations own their clause grammar directly
- expression-position and statement-position desugaring are part of the surface contract
- low-level keyword transport is derived later by tooling instead of being hand-authored first

**3. Wire it up in `incan.toml`**:

```toml
[package]
name = "studiokit"
version = "0.1.0"

[vocab]
crate = "crates/studiokit-vocab"
```

**4. Build** — When you run `incan build --lib`, the compiler:

1. Reads `incan.toml` and finds the `[vocab]` section
2. Builds the vocab crate via `cargo build`
3. Resolves the canonical `library_vocab()` entrypoint
4. Derives low-level keyword registrations, manifest metadata, and optional desugarer packaging from the returned `VocabRegistration`
5. Packages everything into the distributable library artifact

### For library consumers

Consumers do not interact with the vocab crate at all. They just use the library:

```incan
from pub::studiokit import query, step, workflow

sales_story = query {
    FROM orders
    FILTER .status == "paid"
    GROUP BY .region
    SELECT region, total(.amount) as revenue
}

step normalize_orders(data: Records[RawOrder]) -> Records[Order]:
    config:
        currency: str = "EUR"
    input: Records[RawOrder]
    output: Records[Order]
    return clean_orders(data, currency=currency)

workflow daily_revenue:
    orders = load_orders()
    clean = normalize_orders(orders)
    revenue = query {
        FROM clean
        SELECT .region, total(.amount) as revenue
    }
```

The compiler resolves `studiokit` from the project's dependencies, loads the pre-built vocab metadata, and activates the declarations registered for `studiokit`.

## Reference-level explanation

### The `incan-vocab` crate

Lives at `crates/incan-vocab/` in the compiler repository. Published to crates.io independently from the compiler. Follows the **tower-service pattern**: a tiny, stable trait crate that changes infrequently, while implementations evolve on their own schedule.

#### Dependency graph

```text
incan-vocab          (tiny, stable, published to crates.io)
    ↑
incan_core           (compiler internals, re-exports incan-vocab types)
    ↑
incan_syntax         (parser, typechecker)
    ↑
incan (src/)         (CLI, backend, project generator)
```

Library vocab crates depend only on `incan-vocab`:

```text
routekit-vocab  ──depends──▸  incan-vocab
stately-vocab  ──depends──▸  incan-vocab
```

### Core types

#### `VocabRegistration`

The central author-facing abstraction. A companion crate exports exactly one canonical Rust entrypoint:

```rust
pub fn library_vocab() -> VocabRegistration
```

`VocabRegistration` is the source of truth for one library's activated DSL surfaces, manifest metadata, and optional desugarer. The compiler derives serialized transport metadata from this value; library authors should not have to hand-maintain JSON or artifact paths as part of the standard workflow.

```rust
/// Canonical vocabulary bundle exported by a companion crate.
///
/// Tooling derives serialized metadata from this value and, when present,
/// packages the registered desugarer automatically.
pub struct VocabRegistration {
    /// High-level activated DSL surfaces.
    pub dsl_surfaces: Vec<DslSurface>,
    /// Machine-readable manifest describing the library's public surface.
    pub library_manifest: LibraryManifest,
    /// Optional DSL desugarer plus packaging hints.
    pub desugarer: Option<DesugarerRegistration>,
}
```

#### `DslSurface`

The main author-facing grouping abstraction. A `DslSurface` says "these declarations become active together under this activation rule":

```rust
pub struct DslSurface {
    /// When this surface becomes active.
    pub activation: KeywordActivation,
    /// Declarations contributed by this activated surface.
    pub declarations: Vec<DeclarationSurface>,
}
```

#### `DeclarationSurface`

One top-level DSL declaration such as a query-like form, a step-like form, or a workflow-like form:

```rust
pub struct DeclarationSurface {
    /// Leading declaration keyword.
    pub keyword: String,
    /// Additional tokens for compound declaration spellings.
    pub compound_tokens: Vec<String>,
    /// Where the declaration may appear.
    pub placement: KeywordPlacement,
    /// Structured shape of the declaration head.
    pub head_kind: DeclarationHeadKind,
    /// Structured shape of the declaration body.
    pub body_kind: DeclarationBodyKind,
    /// Whether the declaration desugars to statements or an expression.
    pub desugars_to: DesugarTarget,
    /// Nested clause surfaces owned by this declaration.
    pub clauses: Vec<ClauseSurface>,
}
```

#### `ClauseSurface`

Clauses are owned by a declaration, not registered as a separate top-level thing:

```rust
pub struct ClauseSurface {
    /// Leading clause keyword.
    pub keyword: String,
    /// Additional tokens for compound spellings such as `GROUP BY`.
    pub compound_tokens: Vec<String>,
    /// Structured body payload kind for this clause.
    pub body_kind: ClauseBodyKind,
    /// Whether this clause is required, optional, or repeatable.
    pub cardinality: ClauseCardinality,
    /// Relative ordering guidance within the owning declaration.
    pub placement: ClausePlacement,
}
```

#### Low-level keyword metadata

`KeywordRegistration` and `KeywordSpec` remain part of the crate, but they are now lower-level DTOs used for transport and compiler-facing keyword derivation. They are no longer the intended starting point for companion-crate authoring.

#### `KeywordActivation`

Determines when a keyword becomes active in a source file:

```rust
/// Activation rule for a keyword group.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum KeywordActivation {
    /// Always active — core language keywords (`def`, `if`, `for`, etc.).
    ///
    /// These are recognized in every source file without any import.
    Always,

    /// Activated when a specific import path is used in a file.
    ///
    /// Matching rule: the activation path is compared as a **prefix** of the import path.
    /// `OnImport("std.async")` activates when the file contains `import std.async`,
    /// `from std.async import sleep`, or `from std.async.time import sleep` — any import
    /// whose path starts with `std.async`.
    OnImport(String),
}
```

#### `KeywordSource`

Tracks where a keyword was defined (useful for diagnostics, LSP, and tooling):

```rust
/// Origin of a keyword registration.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum KeywordSource {
    /// Built into the compiler — core language syntax.
    Core,
    /// From the Incan standard library.
    Stdlib,
    /// From a third-party library.
    Library(String),
}
```

#### `KeywordSpec`

Describes a single keyword's name and parser behavior:

```rust
/// Specification for a single keyword.
pub struct KeywordSpec {
    /// The keyword text (e.g., "def", "async", "routes", "GET").
    pub name: String,

    /// How the parser should handle this keyword.
    pub surface_kind: KeywordSurfaceKind,

    /// Additional tokens that form a compound keyword (e.g., `["BY"]` for `ORDER BY`).
    ///
    /// Empty for single-token keywords (the common case). When non-empty, the parser consumes `name` followed by each
    /// token in `compound_tokens` to form the full keyword.
    pub compound_tokens: Vec<String>,

    /// Where this keyword is valid.
    ///
    /// Surface kind answers "what syntactic shape does this keyword have?".
    /// Placement answers "where may that shape appear?".
    pub placement: KeywordPlacement,
}

impl KeywordSpec {
    /// Create a simple (single-token) keyword spec.
    pub fn new(name: &str, surface_kind: KeywordSurfaceKind) -> Self {
        Self {
            name: name.to_string(),
            surface_kind,
            compound_tokens: vec![],
            placement: KeywordPlacement::TopLevel,
        }
    }

    /// Create a keyword spec that is valid only inside specific parent blocks.
    pub fn in_block(name: &str, surface_kind: KeywordSurfaceKind, parents: &[&str]) -> Self {
        Self {
            name: name.to_string(),
            surface_kind,
            compound_tokens: vec![],
            placement: KeywordPlacement::InBlock(parents.iter().map(|s| s.to_string()).collect()),
        }
    }

    /// Create a compound keyword spec (e.g., `ORDER BY`, `GROUP BY`).
    ///
    /// The parser will consume `name` followed by each token in `rest`.
    pub fn compound(name: &str, rest: &[&str], surface_kind: KeywordSurfaceKind) -> Self {
        Self {
            name: name.to_string(),
            surface_kind,
            compound_tokens: rest.iter().map(|s| s.to_string()).collect(),
            placement: KeywordPlacement::TopLevel,
        }
    }

    /// Create a compound keyword spec that is valid only inside specific parent blocks.
    pub fn compound_in_block(
        name: &str,
        rest: &[&str],
        surface_kind: KeywordSurfaceKind,
        parents: &[&str],
    ) -> Self {
        Self {
            name: name.to_string(),
            surface_kind,
            compound_tokens: rest.iter().map(|s| s.to_string()).collect(),
            placement: KeywordPlacement::InBlock(parents.iter().map(|s| s.to_string()).collect()),
        }
    }
}

/// Placement rule for a keyword registration.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum KeywordPlacement {
    /// Valid where a normal statement/declaration may begin.
    TopLevel,

    /// Valid only directly inside one of the listed parent block keywords.
    ///
    /// This is how libraries declare that a keyword belongs to a specific DSL block rather than being globally
    /// meaningful on its own.
    InBlock(Vec<String>),
}
```

#### `KeywordSurfaceKind`

Tells the parser how to handle a keyword when it's encountered. The enum covers **all** keyword shapes in the language — core, stdlib, and library — unified under a single dispatch mechanism.

```rust
/// Parser dispatch shape for a keyword.
///
/// Every keyword in Incan — from `def` to `async` to `routes` — has a surface kind that tells
/// the parser what syntactic shape to expect. The parser dispatches on this enum rather than
/// on individual token types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum KeywordSurfaceKind {
    // ---- Core language shapes (activation: Always) ----

    /// Function declaration: `def name(params) -> type: body`
    FunctionDecl,

    /// Type declaration: `class Name(...)`, `model Name:`, `trait Name:`, `enum Name:`
    ///
    /// The specific type kind (`class` vs `model` vs `trait` vs `enum`) is determined by
    /// the keyword name, not the surface kind. The parser uses one shared code path.
    TypeDecl,

    /// Conditional chain: `if expr: body (elif expr: body)* (else: body)?`
    ConditionalChain,

    /// For loop: `for name in expr: body`
    ForLoop,

    /// While loop: `while expr: body`
    WhileLoop,

    /// Match block: `match expr: (case pattern: body)+`
    MatchBlock,

    /// Try/except/finally: `try: body (except Type as name: body)+ (finally: body)?`
    TryBlock,

    /// Import statement: `import path (as alias)?` / `from path import names`
    ImportStatement,

    /// Control flow jump: `return expr?`, `break`, `continue`, `pass`, `raise expr`, `yield expr`
    ControlFlow,

    /// Binding declaration: `let name: type = expr`
    BindingDecl,

    /// Literal keyword: `True`, `False`, `None`
    LiteralKeyword,

    /// Operator keyword: `and`, `or`, `not`, `is`, `in`, `del`
    OperatorKeyword,

    /// Contextual modifier: `extends`, `with`, `as`, `self`, `super`, `lambda`, `type`
    ///
    /// These keywords are meaningful only in specific syntactic positions (e.g., `extends` only
    /// after a class name). The parser handles them contextually.
    ContextualModifier,

    // ---- Extension shapes (activation: OnImport) ----

    /// Statement-level keyword followed by arguments.
    ///
    /// Example: `assert x == 42` (keyword + expression args)
    StatementKeywordArgs,

    /// Prefix expression keyword.
    ///
    /// Example: `await fetch(url)` (keyword + inner expression)
    PrefixExpression,

    /// Modifier keyword before a declaration.
    ///
    /// Example: `async def fetch():` (keyword + def/class declaration)
    DeclarationModifier,

    /// Block-level declaration keyword that opens a new scope.
    ///
    /// Example: `routes { ... }`, `machine "name" { ... }`
    BlockDeclaration,

    /// Context keyword valid only inside a specific block.
    ///
    /// Example: `GET`, `POST` inside a `routes { }` block
    BlockContextKeyword,

    /// Sub-block keyword that opens a nested block within a declaration.
    ///
    /// Example: `middleware:` inside a `routes` block, `enter:` inside a state
    SubBlock,
}
```

**Mapping to keyword layers:**

|        Variant         |                      Core (`Always`)                       | Stdlib (`OnImport`) |       Library (`OnImport`)       |
| ---------------------- | ---------------------------------------------------------- | ------------------- | -------------------------------- |
| `FunctionDecl`         | `def`                                                      | —                   | —                                |
| `TypeDecl`             | `class`, `model`, `trait`, `enum`                          | —                   | —                                |
| `ConditionalChain`     | `if`, `elif`, `else`                                       | —                   | —                                |
| `ForLoop`              | `for`                                                      | —                   | —                                |
| `WhileLoop`            | `while`                                                    | —                   | —                                |
| `MatchBlock`           | `match`, `case`                                            | —                   | —                                |
| `TryBlock`             | `try`, `except`, `finally`                                 | —                   | —                                |
| `ImportStatement`      | `import`, `from`                                           | —                   | —                                |
| `ControlFlow`          | `return`, `break`, `continue`, `pass`, `raise`, `yield`    | —                   | —                                |
| `BindingDecl`          | `let`                                                      | —                   | —                                |
| `LiteralKeyword`       | `True`, `False`, `None`                                    | —                   | —                                |
| `OperatorKeyword`      | `and`, `or`, `not`, `is`, `in`, `del`                      | —                   | —                                |
| `ContextualModifier`   | `extends`, `with`, `as`, `self`, `super`, `lambda`, `type` | —                   | —                                |
| `StatementKeywordArgs` | —                                                          | `assert`            | —                                |
| `PrefixExpression`     | —                                                          | `await`             | —                                |
| `DeclarationModifier`  | —                                                          | `async`             | —                                |
| `BlockDeclaration`     | —                                                          | —                   | `routes`, `machine`, `state`     |
| `BlockContextKeyword`  | —                                                          | —                   | `GET`, `POST`, `on`              |
| `SubBlock`             | —                                                          | —                   | `middleware:`, `enter:`, `exit:` |

**Design note:** `KeywordSurfaceKind` and `KeywordPlacement` are intentionally separate. The surface kind says what syntax shape the parser should expect; placement says whether that shape is top-level or only valid inside specific parent blocks. The core shapes (`FunctionDecl`, `TypeDecl`, etc.) have dedicated, hand-optimized parsing functions in the compiler. The extension shapes (`BlockDeclaration`, `BlockContextKeyword`, `SubBlock`) use generic, registry-driven parsing. Both are dispatched from the same enum — the parser's `match` on `KeywordSurfaceKind` is the single entry point for all keyword handling.

#### `VocabDesugarer` trait

The second core abstraction. Libraries that introduce richer declaration-owned DSL surfaces provide a desugarer that transforms parsed public DSL syntax into regular Incan syntax before typechecking.

**Why this is needed:** the parser may know how to recognize a `query { ... }` expression, a typed `step ...:` declaration, or a clause-owned subgrammar like `WINDOW BY`, but the compiler still needs a stable public contract that explains what those parsed shapes *mean*. The desugarer bridges that gap by rewriting public DSL syntax into standard Incan expressions and statements.

```rust
pub trait VocabDesugarer {
    /// Transform one library-defined syntax node into ordinary Incan syntax.
    fn desugar(&self, node: &VocabSyntaxNode) -> Result<DesugarOutput, DesugarError>;

    /// Optional context-aware entrypoint used by tooling/runtime glue.
    fn desugar_with_context(&self, request: &DesugarRequest) -> Result<DesugarResponse, DesugarError> {
        // default implementation delegates to `desugar`
    }
}
```

The important design change is that the desugar boundary is no longer statement-block-only:

1. some declarations desugar into **statements**
2. some declarations, such as `query { ... }`, desugar into an **expression**
3. both travel through the same public request/response contract

#### `VocabRegistration` and `DesugarerRegistration`

A library that introduces a richer DSL surface must supply both grammar metadata and optional transform logic. The canonical bundle is data-first: `VocabRegistration` carries activated DSL surfaces and manifest metadata directly, and `DesugarerRegistration` attaches the optional Rust desugarer plus any non-default packaging overrides the tooling needs.

```rust
pub struct VocabRegistration {
    pub dsl_surfaces: Vec<DslSurface>,
    pub library_manifest: LibraryManifest,
    pub desugarer: Option<DesugarerRegistration>,
}

pub struct DesugarerRegistration {
    pub metadata: DesugarerMetadata,
    pub desugarer: Arc<dyn VocabDesugarer>,
}
```

**Why `Option<DesugarerRegistration>`?** Many companion crates only register activated surfaces and manifest metadata. Only libraries that introduce custom lowering need a desugarer, so the common metadata-only case stays small and requires no extra build plumbing.

**Forward compatibility:** For the internal-first architecture (Phases 1–3), the compiler may continue deriving equivalent registration data from its existing core and stdlib metadata internally. For external companion crates, the release contract is the canonical `library_vocab() -> VocabRegistration` export. The public producer contract stays data-first either way.

#### Public AST types (`ast` module)

The `incan-vocab` crate exports a set of **public AST types** that form the contract between the compiler and library desugarers. These are intentionally separate from the compiler's internal AST — they are stable, versioned, and designed for library-author ergonomics.

##### Input types (what the desugarer receives)

```rust
pub enum VocabSyntaxNode {
    Declaration(VocabDeclaration),
    Clause(VocabClause),
    Statement(IncanStatement),
    Expression(IncanExpr),
}

pub struct VocabDeclaration {
    pub keyword: String,
    pub keyword_metadata: Option<VocabKeywordMetadata>,
    pub head: VocabDeclarationHead,
    pub decorators: Vec<Decorator>,
    pub body: Vec<VocabBodyItem>,
    pub span: Span,
}

pub struct VocabClause {
    pub keyword: String,
    pub compound_tokens: Vec<String>,
    pub head: Vec<IncanExpr>,
    pub body: VocabClauseBody,
    pub span: Span,
}
```

##### Output types (what the desugarer produces)

The desugarer returns regular Incan expressions and statements through one explicit output enum:

```rust
pub enum DesugarOutput {
    Statements(Vec<IncanStatement>),
    Expression(IncanExpr),
}

pub struct DesugarRequest {
    pub node: VocabSyntaxNode,
    pub module_path: Option<String>,
}

pub struct DesugarResponse {
    pub output: DesugarOutput,
}
```

##### Support types

```rust
pub struct Span {
    pub start: usize,
    pub end: usize,
}

pub struct DesugarError {
    pub message: String,
    pub span: Option<Span>,
}
```

This is the minimum contract needed for the richer surface locked by this RFC:

- declaration heads survive into the public AST
- clause structure survives into the public AST
- host expressions and statements can appear inside DSL-owned positions
- desugarers can lower either to statements or to an expression

#### Manifest types

The manifest describes a library's public API surface in a machine-readable format:

```rust
/// Format version for manifest evolution.
///
/// The compiler checks this to ensure compatibility. Older compilers reject manifests with unknown format versions
/// (fail-closed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ManifestFormatVersion {
    V1,
}

/// Machine-readable description of a library's public surface.
///
/// Identity metadata (name, version) is intentionally absent — the compiler
/// injects it from `incan.toml` for third-party libraries, or from its own
/// version for stdlib/core. This avoids drift between the manifest and the
/// project file.
pub struct LibraryManifest {
    /// Manifest schema version.
    pub format_version: ManifestFormatVersion,
    /// Exported modules.
    pub modules: Vec<ModuleExport>,
    /// Cargo dependencies required when this library/namespace is used (DD-16).
    ///
    /// The compiler collects these from all loaded providers, deduplicates by crate name,
    /// and adds them to the generated Cargo.toml.
    pub required_dependencies: Vec<CargoDependency>,
    /// `incan_stdlib` feature flags to enable (DD-16).
    ///
    /// E.g., `["json"]` for std.serde, `["web"]` for std.web, `["async"]` for std.async.
    pub required_stdlib_features: Vec<String>,
}

/// A module's exported API surface.
pub struct ModuleExport {
    /// Dot-separated module path (e.g., "routekit.routes").
    pub path: String,
    /// Exported functions.
    pub functions: Vec<FunctionExport>,
    /// Exported types (models, classes, enums, traits).
    pub types: Vec<TypeExport>,
}

/// An exported function's signature.
pub struct FunctionExport {
    /// Function name.
    pub name: String,
    /// Parameter list: (name, type).
    pub params: Vec<(String, TypeRef)>,
    /// Return type, if any.
    pub return_type: Option<TypeRef>,
    /// Whether the function is async.
    pub is_async: bool,
}

/// An exported type's surface.
pub struct TypeExport {
    /// Type name.
    pub name: String,
    /// Kind of type definition.
    pub kind: TypeExportKind,
    /// Type parameters (e.g., `["T"]` for `DataFrame[T]`).
    pub type_params: Vec<String>,
    /// Public fields (for models/classes).
    pub fields: Vec<FieldExport>,
    /// Methods.
    pub methods: Vec<FunctionExport>,
}

/// Kind of exported type.
#[non_exhaustive]
pub enum TypeExportKind {
    Model,
    Class,
    Enum,
    Trait,
    Newtype,
}

/// An exported field.
pub struct FieldExport {
    /// Field name.
    pub name: String,
    /// Field type.
    pub field_type: TypeRef,
    /// Whether the field has a default value.
    pub has_default: bool,
}

/// A type reference in the manifest.
///
/// Supports simple names, generics, optionals, and union types.
#[non_exhaustive]
pub enum TypeRef {
    Named(String),
    Generic(String, Vec<TypeRef>),
    Optional(Box<TypeRef>),
    Union(Vec<TypeRef>),
}

impl TypeRef {
    pub fn named(name: &str) -> Self {
        TypeRef::Named(name.to_string())
    }
}

/// A Cargo dependency required by a library or stdlib namespace (DD-16).
///
/// Mirrors the existing `StdlibExtraCrateDep` / `StdlibExtraCrateSource` types
/// in `incan_core::lang::stdlib`, but lives in `incan-vocab` so library authors
/// can declare their own.
pub struct CargoDependency {
    /// Cargo dependency key (e.g., `"serde"`, `"axum"`).
    pub crate_name: String,
    /// Dependency source.
    pub source: CargoDependencySource,
}

/// Source of a Cargo dependency.
#[non_exhaustive]
pub enum CargoDependencySource {
    /// Registry version (e.g., `"1.0"`, `"0.8"`).
    Version(String),
    /// Path dependency relative to the compiler workspace root.
    Path(String),
}

impl CargoDependency {
    pub fn version(name: &str, version: &str) -> Self {
        Self {
            crate_name: name.to_string(),
            source: CargoDependencySource::Version(version.to_string()),
        }
    }

    pub fn path(name: &str, path: &str) -> Self {
        Self {
            crate_name: name.to_string(),
            source: CargoDependencySource::Path(path.to_string()),
        }
    }
}
```

### Manifest versioning and evolution

The `ManifestFormatVersion` enum controls schema evolution:

- **Adding new optional fields** to existing types is non-breaking (stays V1).
- **Adding new required fields** or **changing field semantics** bumps the version (V1 → V2).
- **Compiler compatibility**: the compiler checks `format_version` and rejects unknown versions with a clear error message directing the user to upgrade.

### The unified keyword registry

The `KeywordRegistry` is the compiler's cached, read-only lookup structure that holds **all** keywords — core language, stdlib, and library. It is built once at startup and shared across all file compilations within a session.

```rust
/// Cached keyword registry. Built once, shared across all file compilations.
///
/// The compiler, LSP, formatter, and all tools that need keyword awareness consume this
/// structure. There is no separate "hard keyword" or "soft keyword" subsystem — just
/// keywords with different activation rules.
pub struct KeywordRegistry {
    /// All known keywords, keyed by name.
    ///
    /// Multiple entries may share the same text when they are qualified by different parent blocks.
    entries: HashMap<String, Vec<KeywordEntry>>,

    /// Activation index: import path → keyword names activated by that import.
    ///
    /// Core keywords are indexed under a synthetic `__always__` key and pre-loaded
    /// into every file's active set. Library keywords are indexed under their
    /// `KeywordActivation::OnImport` path.
    activation_index: HashMap<String, Vec<String>>,
}

/// A single keyword entry in the registry.
pub struct KeywordEntry {
    /// The keyword text (e.g., "def", "async", "routes").
    pub name: String,
    /// How the parser handles this keyword.
    pub surface_kind: KeywordSurfaceKind,
    /// Compound tokens (e.g., `["BY"]` for `ORDER BY`). Empty for single-token keywords.
    pub compound_tokens: Vec<String>,
    /// Where this keyword is valid.
    pub placement: KeywordPlacement,
    /// When this keyword is active.
    pub activation: KeywordActivation,
    /// Where this keyword was defined.
    pub source: KeywordSource,
}
```

**Building the registry:**

```rust
impl KeywordRegistry {
    /// Build a registry from compiler-owned internal sources plus loaded library metadata.
    ///
    /// Called once at compiler startup or workspace load. Conceptually, the compiler provides:
    /// 1. core language keyword registrations (activation: Always)
    /// 2. stdlib keyword registrations (activation: OnImport for each std.* namespace)
    /// 3. loaded library metadata from project dependencies
    pub fn from_sources(sources: &[RegistrySource]) -> Self {
        let mut registry = Self::new();
        for source in sources {
            for kw_reg in source.keyword_registrations() {
                for spec in &kw_reg.keywords {
                    registry.insert(KeywordEntry {
                        name: spec.name.clone(),
                        surface_kind: spec.surface_kind,
                        compound_tokens: spec.compound_tokens.clone(),
                        placement: spec.placement.clone(),
                        activation: kw_reg.activation.clone(),
                        source: source.kind().clone(),
                    });
                }
            }
        }
        registry
    }

    /// Look up all candidate registrations for a keyword text.
    pub fn candidates(&self, name: &str) -> &[KeywordEntry] { ... }

    /// Resolve a keyword in the current parsing context.
    ///
    /// `current_parent` is `None` at top level and `Some("routes")`, `Some("state")`, etc. while parsing inside a DSL
    /// block. Resolution filters by `KeywordPlacement`.
    pub fn resolve(&self, name: &str, current_parent: Option<&str>) -> Option<&KeywordEntry> { ... }

    /// Get all keywords activated by a given import path (prefix match).
    ///
    /// Iterates `activation_index` keys and returns keywords for any key that
    /// is a dot-segment prefix of `path` (e.g., key `"std.async"` matches
    /// `"std.async"`, `"std.async.time"`, but not `"std.asyncio"`).
    pub fn keywords_for_import(&self, path: &str) -> Vec<&str> { ... }

    /// Get all always-active keywords (core language).
    pub fn always_active(&self) -> impl Iterator<Item = &KeywordEntry> { ... }
}
```

**Per-file activation model:**

The registry is the global truth. Each file being parsed maintains its own `active_keywords: HashSet<String>`. At the start of parsing, all `Always`-activated keywords are pre-loaded. As imports are encountered, the parser calls `registry.keywords_for_import(path)` and adds those keywords to the active set:

```rust
impl Parser {
    fn init_keywords(&mut self, registry: &KeywordRegistry) {
        // Core keywords are always active
        for entry in registry.always_active() {
            self.active_keywords.insert(entry.name.clone());
        }
    }

    fn process_import(&mut self, path: &str, registry: &KeywordRegistry) {
        // Activate keywords for this import
        for name in registry.keywords_for_import(path) {
            self.active_keywords.insert(name.clone());
        }
    }

    fn try_keyword(
        &self,
        ident: &str,
        current_parent: Option<&str>,
        registry: &KeywordRegistry,
    ) -> Option<&KeywordEntry> {
        if self.active_keywords.contains(ident) {
            registry.resolve(ident, current_parent)
        } else {
            None
        }
    }
}
```

**Parser dispatch — single code path:**

Instead of matching on individual token types (`Token::Def`, `Token::If`, ...) or checking soft keywords separately, the parser dispatches entirely through `KeywordSurfaceKind`:

```rust
// Simplified: the parser sees an identifier and checks the registry
let current_parent = self.vocab_block_stack.last().map(String::as_str);
if let Some(entry) = self.try_keyword(ident, current_parent, &registry) {
    match entry.surface_kind {
        // Core shapes — dedicated parsing functions
        FunctionDecl => self.parse_function_def(),
        TypeDecl => self.parse_type_decl(ident),  // ident distinguishes class/model/trait/enum
        ConditionalChain => self.parse_conditional(),
        ForLoop => self.parse_for_loop(),
        WhileLoop => self.parse_while_loop(),
        MatchBlock => self.parse_match(),
        TryBlock => self.parse_try(),
        ImportStatement => self.parse_import(),
        ControlFlow => self.parse_control_flow(ident),
        BindingDecl => self.parse_let(),
        LiteralKeyword => self.parse_literal(ident),
        OperatorKeyword => self.parse_operator(ident),
        ContextualModifier => { /* handled in context */ },

        // Extension shapes — generic, registry-driven parsing
        StatementKeywordArgs => self.parse_keyword_statement(ident),
        PrefixExpression => self.parse_keyword_prefix(ident),
        DeclarationModifier => self.parse_keyword_modifier(ident),
        BlockDeclaration => self.parse_vocab_block(ident),
        BlockContextKeyword => self.parse_context_entry(ident),
        SubBlock => self.parse_sub_block(ident),
    }
}
```

**Parent-qualified parsing rule:** The parser tracks a `vocab_block_stack: Vec<String>` rather than a single current block. `KeywordPlacement::TopLevel` entries are only considered when the stack is empty. `KeywordPlacement::InBlock([...])` entries are considered only when the immediate parent block matches one of the registered parent names. This applies uniformly to `BlockContextKeyword`, `SubBlock`, and nested `BlockDeclaration` keywords. Outside a matching parent block, these words are treated as regular identifiers — no collision with user-defined names.

**Ambiguity rule:** Multiple sources may register the same keyword text under different parent blocks, but the same `(name, immediate_parent, surface_kind)` combination may appear only once. The registry rejects ambiguous duplicates at load time with a diagnostic naming both sources.

**Decorator collection for library declarations:** The parser collects `@expr` tokens preceding a library declaration using the same mechanism it uses for `def`/`class` decorators. Collected decorators are stored on the public declaration node and passed to the desugarer. The desugarer decides what they mean; the parser performs no validation beyond syntactic correctness.

This is cleaner than the current two-path approach because related keywords group together. `class`, `model`, `trait`, `enum` all route to `parse_type_decl` — the parser handles differences based on keyword name, not token type.

**Lexer simplification:**

In the unified model, the lexer no longer needs to recognize keywords. It emits `Token::Ident(name)` for everything, and the parser promotes identifiers to keywords via registry lookup + activation check. The lexer becomes simpler; the parser's keyword check becomes the single point of truth.

> **Implementation note:** The transition from `Token::Def` / `Token::If` / etc. to a pure `Token::Ident` lexer can
> happen incrementally. Phase 1 can keep the existing lexer token types while introducing the registry alongside.
> Phase 2 collapses lexer token types into `Token::Ident` once the registry-driven parser is validated.

**Performance:**

The registry is a `HashMap<String, Vec<KeywordEntry>>` — still O(1) for the initial name lookup, followed by a tiny linear scan over context-qualified candidates for that name. In practice these candidate lists are expected to stay very small (usually 1, occasionally 2-3). The per-file `active_keywords` set adds one `HashSet::contains` check per identifier token — also O(1). For the common case (core keywords that are always active), the check succeeds immediately.

### LSP integration

The unified registry is a natural fit for the Language Server Protocol implementation. The LSP builds the registry once when the workspace opens and caches it for the session lifetime, rebuilding only when `incan.toml` changes or dependencies are updated.

```rust
impl LspBackend {
    /// Build the keyword registry for this workspace.
    /// Called once at workspace open; rebuilt on incan.toml change.
    fn build_registry(&self) -> KeywordRegistry {
        let sources = vec![
            core_registry_source(),
            stdlib_registry_source(),
            // Project dependency metadata loaded from incan.toml...
        ];
        KeywordRegistry::from_sources(&sources)
    }
}
```

The LSP uses the registry for:

|       LSP feature       |                                                                 Registry usage                                                                 |
| ----------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------- |
| **Syntax highlighting** | `registry.get(ident)` → keyword vs identifier; `entry.source` for coloring                                                                     |
| **Completions**         | After `from std.` → filter by activation path prefix; inside DSL block → entries whose `KeywordPlacement::InBlock` matches the enclosing block |
| **Diagnostics**         | "`await` is only available when `std.async` is imported" — `entry.activation` is `OnImport` but path not in file's imports                     |
| **Hover info**          | "keyword `async`: declaration modifier, source: std.async"                                                                                     |
| **Go to definition**    | `entry.source` → navigate to the companion crate export, compiler-owned stdlib source, or related module                                       |

All of these queries work uniformly across core, stdlib, and library keywords — no special-case LSP logic.

### Formatter integration

The formatter (`incan fmt`) uses the `KeywordRegistry` to format library-introduced syntax without keyword-specific rules. The key insight is that `KeywordSurfaceKind` already describes the *shape* of the syntax — the formatter dispatches on the shape, not the keyword name.

**Surface-kind → formatting rule mapping:**

|  `KeywordSurfaceKind`  |                             Formatting shape                              |   Core examples   |       Library examples       |
| ---------------------- | ------------------------------------------------------------------------- | ----------------- | ---------------------------- |
| `FunctionDecl`         | `keyword name(params) -> type: body` — wrap params, indent body           | `def`             | `step`, `action`             |
| `TypeDecl`             | `keyword Name(clauses): body` — inheritance/trait clauses, indent body    | `class`, `model`  | —                            |
| `ConditionalChain`     | `keyword expr: body (elif: body)* (else: body)?`                          | `if`              | —                            |
| `ForLoop`              | `keyword binding in expr: body`                                           | `for`             | —                            |
| `WhileLoop`            | `keyword expr: body`                                                      | `while`           | —                            |
| `MatchBlock`           | `keyword expr: (case pattern: body)+`                                     | `match`           | —                            |
| `TryBlock`             | `keyword: body (except: body)+ (finally: body)?`                          | `try`             | —                            |
| `ControlFlow`          | `keyword expr?` — single line                                             | `return`, `break` | —                            |
| `BindingDecl`          | `keyword name: type = expr` — wrap at `=`                                 | `let`             | —                            |
| `StatementKeywordArgs` | `keyword expr` — single line                                              | —                 | `assert`                     |
| `PrefixExpression`     | `keyword expr` — inline, part of expression                               | —                 | `await`                      |
| `DeclarationModifier`  | `keyword` prefix on next declaration                                      | —                 | `async`                      |
| `BlockDeclaration`     | `keyword args: body` — indent body, nested blocks/context keywords inside | —                 | `routes`, `machine`, `state` |
| `BlockContextKeyword`  | `keyword args` or `keyword args: body` — inside parent block              | —                 | `GET`, `POST`, `on`          |
| `SubBlock`             | `keyword: body` — inside parent, indent body                              | —                 | `middleware:`, `enter:`      |

When the formatter encounters an identifier, it checks the registry:

```rust
fn format_statement(&mut self, ident: &str) {
    if let Some(entry) = self.registry.get(ident) {
        match entry.surface_kind {
            FunctionDecl => self.format_function_decl(),
            TypeDecl => self.format_type_decl(),
            BlockDeclaration => self.format_block_decl(),
            BlockContextKeyword => self.format_context_keyword(),
            SubBlock => self.format_sub_block(),
            StatementKeywordArgs => self.format_statement_keyword(),
            DeclarationModifier => self.format_decl_modifier(),
            // ... other shapes handled by existing formatting rules
        }
    }
}
```

When multiple entries share the same keyword text, the formatter uses the current parent block to select the matching `KeywordPlacement`. This keeps reused names unambiguous without hardcoding library-specific rules.

This means a library keyword like `step` registered as `FunctionDecl` gets the **exact same formatting rules** as `def` — parameter wrapping, return type alignment, body indentation — with zero formatter changes.

**Intra-block formatting:** For DSL-specific content inside a `BlockDeclaration`, the formatter applies standard rules: indent body one level, separate top-level items with blank lines when they contain bodies, collapse single-expression items onto one line. This handles 90% of library block formatting. An optional `FormatHint` field on `KeywordRegistration` is reserved for future use (e.g., "always separate context keyword blocks with blank lines", "align string arguments") but is not implemented as part of the scope of this RFC.

### Syntax highlighting

Syntax highlighting uses two layers:

**1. LSP semantic tokens (primary):** When the LSP is running, it queries the `KeywordRegistry` for each identifier and emits semantic token types accordingly. Library keywords like `routes`, `GET`, and `middleware` are highlighted as keywords, just like `def` and `if`. The `KeywordSource` allows the LSP to optionally differentiate coloring — e.g., core keywords in one color, library keywords in another — though a single "keyword" token type is the default. This works for all keywords regardless of origin.

**2. TextMate grammar (fallback):** The `.tmLanguage` grammar used by VS Code (and GitHub rendering) is a static regex file with a hardcoded keyword list. It cannot query a runtime registry. This means:

- **Core keywords** are listed in the grammar, as today.
- **Stdlib soft keywords** (`async`, `await`, `assert`) should be added to the grammar as part of the stdlib migration — these are stable and known at grammar-generation time.
- **Library keywords** (`routes`, `GET`, `machine`, etc.) **cannot** appear in the static grammar. They are only highlighted when the LSP is active and providing semantic tokens.

This is the same trade-off that TypeScript, Rust, and Go make: full highlighting requires the language server; the static grammar provides a reasonable baseline for previews, GitHub rendering, and the brief window before the LSP starts.

### Stdlib migration

In the unified model, the already-landed internal migration keeps compiler-owned sources for core and stdlib keywords, and later layers extend that same registry story to library metadata:

**1. Core language source** — registers all always-active language keywords:

```rust
KeywordRegistration {
    activation: KeywordActivation::Always,
    keywords: vec![
        KeywordSpec::new("def", KeywordSurfaceKind::FunctionDecl),
        KeywordSpec::new("class", KeywordSurfaceKind::TypeDecl),
        KeywordSpec::new("if", KeywordSurfaceKind::ConditionalChain),
        KeywordSpec::new("for", KeywordSurfaceKind::ForLoop),
        KeywordSpec::new("while", KeywordSurfaceKind::WhileLoop),
        KeywordSpec::new("match", KeywordSurfaceKind::MatchBlock),
        KeywordSpec::new("try", KeywordSurfaceKind::TryBlock),
        KeywordSpec::new("import", KeywordSurfaceKind::ImportStatement),
        KeywordSpec::new("return", KeywordSurfaceKind::ControlFlow),
        KeywordSpec::new("let", KeywordSurfaceKind::BindingDecl),
        KeywordSpec::new("True", KeywordSurfaceKind::LiteralKeyword),
        KeywordSpec::new("and", KeywordSurfaceKind::OperatorKeyword),
        KeywordSpec::new("self", KeywordSurfaceKind::ContextualModifier),
    ],
}
```

**2. Stdlib source** — registers the import-activated stdlib keywords:

```rust
vec![
    KeywordRegistration {
        activation: KeywordActivation::OnImport("std.testing".into()),
        keywords: vec![
            KeywordSpec::new("assert", KeywordSurfaceKind::StatementKeywordArgs),
        ],
    },
    KeywordRegistration {
        activation: KeywordActivation::OnImport("std.async".into()),
        keywords: vec![
            KeywordSpec::new("async", KeywordSurfaceKind::DeclarationModifier),
            KeywordSpec::new("await", KeywordSurfaceKind::PrefixExpression),
        ],
    },
]
```

**3. Library metadata** — later phases load derived keyword metadata from companion-crate artifacts.

**Migration from current keyword infrastructure:**

- **Phase 1**: `KEYWORDS` const table plus `info_hard()` / `info_soft()` remains present while compiler-owned core + stdlib registry sources produce equivalent registrations for parity validation.
- **Phase 2**: hard keyword tokens and `active_soft_keywords: HashSet<KeywordId>` transition toward registry-backed activation and dispatch while still accepting transitional keyword token forms.
- **Phase 3**: lexer, parser, and tooling complete the migration so `KeywordRegistry` becomes the sole source of truth.

> **Important:** Phase 1 → Phase 2 can be done incrementally. The parser can accept both `Token::Def` and registry-based `Token::Ident("def")` during the transition. This avoids a flag-day rewrite.

### Extraction flow (`incan build --lib`)

The next two flows describe the full library architecture. The earlier implementation phases establish the shared registry and activation model inside the compiler; the later phases extend that model to library artifacts and consumer loading.

When a library author runs `incan build --lib`:

```text
1. Read incan.toml → find [vocab].crate path
2. cargo build the companion crate (crates/<name>-vocab/)
3. Resolve and invoke the canonical library_vocab() entrypoint through tooling-owned extraction glue
4. Derive serialized keyword registrations + manifest metadata from the returned VocabRegistration
5. If present, package the registered desugarer automatically using standard target/profile defaults plus any explicit overrides
6. Package: Incan compiled output + derived vocab payload → distributable artifact
```

### Loading flow (consumer `incan build`)

When a consumer project builds with a library dependency:

```text
1. Read incan.toml → find [dependencies]
2. Resolve library artifact (registry, path, or git)
3. Deserialize vocab metadata from artifact
4. Register keywords in parser's per-file activation table
5. Load manifest for typechecker (function signatures, type definitions)
6. Compile normally — activated keywords parse as expected
```

### Compiler debt: feature scanning

The current compiler uses `needs_web`, `needs_serde`, and various `scan_for_*` booleans to detect which features a program uses. This approach doesn't extend to third-party libraries.

With `incan-vocab`, the compiler can replace these ad-hoc scans with a unified mechanism:

1. **Phases 1-3**: extract `incan-vocab`, migrate core + stdlib keyword registration, and keep `scan_for_*` as a compatibility path.
2. **Phases 4-6**: once library artifacts and manifests exist, move dependency and feature information onto provider output and consumer loading.
3. **Phase 7**: remove `scan_for_*` and `needs_*` booleans when manifest-driven feature detection is end-to-end.

The serde fallback (automatic `#[derive(Serialize, Deserialize)]` on models) is a special case that may remain as a compiler built-in, since it's not a keyword feature but a codegen behavior.

## Design details

### Crate structure

```text
crates/incan-vocab/
├── Cargo.toml
└── src/
    ├── lib.rs           # VocabRegistration, VocabDesugarer + re-exports
    ├── keywords.rs      # KeywordRegistration, KeywordSpec, KeywordSurfaceKind
    ├── manifest.rs      # LibraryManifest, ModuleExport, TypeExport, etc.
    ├── ast.rs           # Public AST types: VocabSyntaxNode, IncanExpr, IncanStatement
    ├── desugar.rs       # VocabDesugarer trait, DesugarRequest, DesugarOutput
    └── version.rs       # ManifestFormatVersion
```

The crate has **zero dependencies** (or at most `serde` behind a feature flag for serialization). This keeps compile times minimal for library authors.

### Naming conventions

|            Concept            |          Name          |
| ----------------------------- | ---------------------- |
| Compiler-side trait crate     | `incan-vocab`          |
| Rust module path              | `incan_vocab`          |
| Library author's vocab crate  | `<library>-vocab`      |
| Example: Routekit vocab crate | `routekit-vocab`       |
| Example: Stately vocab crate  | `stately-vocab`        |
| Vocab crate directory         | `crates/<name>-vocab/` |
| Canonical entrypoint          | `library_vocab()`      |
| Author-facing bundle          | `VocabRegistration`    |
| incan.toml section            | `[vocab]`              |

### Interaction with existing features

**Imports / keyword activation** (RFC 022): keyword activation metadata is derived from the registered DSL surfaces and consumed through the shared `KeywordRegistry`. The parser's per-file `active_keywords` set is populated from that registry; whether a keyword comes from core, stdlib, or a library is invisible to the parser.

**Rust interop** (RFC 005): The vocab crate *is* Rust code. Library authors write it in Rust, depending only on `incan-vocab`. The `crates/` directory convention aligns with standard Rust workspace practices.

**Typechecker**: Manifest metadata provides function signatures and type definitions that the typechecker uses for imported symbols. This replaces the current approach where the typechecker relies on `stdlib/*.incn` stubs.

**`incan.toml`**: The `[vocab]` section is the only new project-level configuration. It points to the vocab crate directory. Projects without custom vocabulary omit this section entirely.

### Compatibility / migration

This is a new feature, not a breaking change. Existing projects without a `[vocab]` section continue to work exactly as before.

For the stdlib, migration is internal to the compiler:

1. Extract types to `incan-vocab`
2. `incan_core` re-exports them
3. Existing `KEYWORDS` table and `info_soft()` continue to work
4. Gradual migration of stdlib features to the shared registration model in later phases

## Examples

The examples below are intentionally split into two categories:

- a **richer surrogate** that acts as the design harness for the public authoring surface
- a **simpler secondary DSL** that proves the richer surface still reads cleanly for smaller libraries

### Studiokit — richer surrogate query/workflow surface

`studiokit` is an outward-safe surrogate used to pressure-test the final producer-side API. It models the kinds of shapes the public surface must support: a query expression with owned clauses, typed `step` declarations, and a statement-oriented `workflow`.

**Companion crate surface:**

```rust
use incan_vocab::{ClauseSurface, DeclarationSurface, DslSurface, LibraryManifest, VocabRegistration};

pub struct StudioKitDesugarer;

pub fn library_vocab() -> VocabRegistration {
    VocabRegistration::new()
        .with_surface(
            DslSurface::on_import("studiokit")
                .with_declaration(
                    DeclarationSurface::named("query")
                        .with_clause_body()
                        .desugars_to_expression()
                        .with_clauses([
                            ClauseSurface::expr("FROM").required(),
                            ClauseSurface::expr("FILTER").optional().after("FROM"),
                            ClauseSurface::expr_list("GROUP BY").optional().after("FILTER"),
                            ClauseSurface::expr_list("SELECT").required().after("GROUP BY"),
                            ClauseSurface::nested_items("WINDOW BY").optional().after("SELECT"),
                        ]),
                )
                .with_declaration(
                    DeclarationSurface::named("step")
                        .with_signature_head()
                        .with_mixed_body()
                        .with_clauses([
                            ClauseSurface::fields("config").optional(),
                            ClauseSurface::type_ref("input").required(),
                            ClauseSurface::type_ref("output").required().after("input"),
                        ]),
                )
                .with_declaration(
                    DeclarationSurface::named("workflow")
                        .with_header_args()
                        .with_statement_body(),
                ),
        )
        .with_library_manifest(LibraryManifest::default())
        .with_desugarer(StudioKitDesugarer)
}
```

**Consumer usage:**

```incan
from pub::studiokit import query, step, workflow

sales_story = query {
    FROM orders
    FILTER .status == "paid"
    GROUP BY .region
    SELECT region, total(.amount) as revenue
}

step normalize_orders(data: Records[RawOrder]) -> Records[Order]:
    config:
        currency: str = "EUR"
    input: Records[RawOrder]
    output: Records[Order]
    return query {
        FROM data
        SELECT normalize(.amount, currency=currency) as amount
    }

workflow daily_revenue:
    orders = load_orders()
    clean = normalize_orders(orders)
    revenue = query {
        FROM clean
        SELECT .region, total(.amount) as revenue
    }
```

This example is the design harness for the public surface because it proves all of the following at once:

- declarations may own clause grammars directly
- a DSL declaration may desugar to an **expression**
- another DSL declaration may mix typed/config-like sections with host statements
- the library authoring surface stays inside one obvious `library_vocab()` function

### Routekit — simpler secondary DSL

The same surface must still read well for smaller DSLs. A simpler routing DSL can use the richer registration model without forcing authors back down into low-level keyword DTOs:

```rust
use incan_vocab::{ClauseSurface, DeclarationSurface, DslSurface, LibraryManifest, VocabRegistration};

pub struct RoutekitDesugarer;

pub fn library_vocab() -> VocabRegistration {
    VocabRegistration::new()
        .with_surface(
            DslSurface::on_import("routekit").with_declaration(
                DeclarationSurface::named("route")
                    .with_header_args()
                    .with_mixed_body()
                    .with_clause(ClauseSurface::nested_items("middleware").optional()),
            ),
        )
        .with_library_manifest(LibraryManifest::default())
        .with_desugarer(RoutekitDesugarer)
}
```

### Internal-first architecture: phases 1–3 remain public history

The RFC keeps the already-landed public history intact:

- phases 1–3 cover the compiler's internal registry migration for core + stdlib keyword handling
- those phases are already public and should remain documented as completed work
- the producer-side surface described above becomes the design center for later phases rather than an afterthought layered onto the earlier internal migration

That distinction is important: the public authoring contract is now `library_vocab() -> VocabRegistration` with `DslSurface` / `DeclarationSurface` / `ClauseSurface`, even though the compiler reached that point through an earlier internal registry migration first

## Alternatives considered

### A. Convention functions returning raw pieces instead of one registration bundle

Instead of `library_vocab() -> VocabRegistration`, library authors export bare functions with well-known names:

```rust
pub fn keyword_registrations() -> Vec<KeywordRegistration> { ... }
pub fn manifest() -> LibraryManifest { ... }
pub fn desugarer() -> Option<Box<dyn VocabDesugarer>> { ... }
```

**Rejected** because: a single registration bundle is easier to discover, easier to version, and keeps metadata plus optional desugarer wiring in one place. Multiple convention functions can silently drift apart or fail if one name is misspelled.

### B. Declarative TOML/YAML instead of Rust

Keywords and manifest declared in a static file rather than Rust code:

```toml
[keywords."routekit.routes"]
routes = "BlockDeclaration"
GET = "BlockContextKeyword"
```

**Rejected** because: this limits expressiveness (no conditional registration, no computed manifests) and adds a custom DSL to learn. Rust code is more flexible and benefits from type checking. `VocabRegistration` can be wrapped by a macro (`vocab!{}`) for the declarative common case in a future iteration.

### C. `src/plugin.rs` alongside Incan code

Put the vocab Rust code directly in `src/` alongside `.incn` files.

**Rejected** because: `src/` is the Incan source directory created by `incan init`. Mixing Rust and Incan files in the same directory is confusing and breaks the mental model. The `crates/` convention follows Rust workspace practices and keeps the separation clean.

### D. `vocab/` directory instead of `crates/`

Use `vocab/` as the directory name instead of `crates/`.

**Rejected** because: the target audience is Rust developers. `crates/` is immediately recognizable to Rust developers and could host additional Rust crates in the future (e.g., a proc-macro crate, a native-extension crate). It's more general-purpose and follows established Rust conventions.

## Drawbacks

- **Adds a Rust dependency for library authors.** Libraries that want custom keywords must write a small Rust crate. This is inherent to the design — keywords affect the parser, which is written in Rust. The `incan-vocab` dependency is tiny (no transitive deps).
- **One more crate to maintain.** The compiler repo gains another crate. However, `incan-vocab` is intentionally minimal and stable — changes should be rare.
- **Artifact and runtime plumbing.** Library vocab metadata must be emitted into build artifacts, and external desugarers need a portable execution format. This RFC chooses build-time metadata extraction plus WASM for external desugarers, which adds a dedicated artifact/runtime layer to the system.

## Layers affected

Phases 1-3 focus on the compiler's internal core + stdlib migration. The library-facing portions of the layers below land later, once RFC 031-style library build and consumer artifacts exist.

- **New crate (`incan-vocab`)** — introduces all public types and traits defined in this RFC (`VocabRegistration`, `DesugarerRegistration`, `VocabDesugarer`, `KeywordRegistration`, `KeywordSpec`, `KeywordSurfaceKind`, `KeywordActivation`, `KeywordRegistry`, `LibraryManifest`, public AST types). Published to crates.io independently of the compiler. All other layers depend on it transitively through `incan_core`.
- **Lexer** — transitions from emitting dedicated keyword token variants to emitting `Token::Ident` for all keyword-shaped identifiers. Keyword promotion becomes entirely the parser's responsibility via registry lookup. This can be done incrementally across the internal migration phases.
- **Parser** — replaces per-token-type dispatch with a single `KeywordSurfaceKind`-driven dispatch. Later phases extend the public desugar boundary from simple keyword shapes to richer declaration- and clause-shaped syntax. Per-file `active_keywords` set replaces the narrower `active_soft_keywords` mechanism.
- **Typechecker** — later phases accept library manifest exports loaded by the consumer build flow (RFC 031) so library types become first-class during checking.
- **Lowering / IR** — later phases replace import-activated feature detection (`needs_web`, `needs_async`, etc.) with registry-driven queries against `LibraryManifest.required_dependencies`. A desugaring pass (after parsing, before typechecking) transforms public library syntax nodes into ordinary Incan AST via `VocabDesugarer`.
- **Project generator** — later phases derive required Cargo dependencies from `LibraryManifest.required_dependencies` rather than hard-coded boolean flags.
- **CLI (`incan build --lib`)** — later phases build the library's vocab crate, resolve the canonical `library_vocab()` entrypoint, derive serialized keyword registrations and manifest metadata from the returned `VocabRegistration`, and package any registered desugarer into the library artifact. Parsing and handling of the `[vocab]` section in `incan.toml` is required.
- **LSP** — builds and caches `KeywordRegistry` once per workspace open; rebuilds only on `incan.toml` changes or dependency updates. All keyword-dependent features (completions, diagnostics, hover, go-to-definition) consume the registry uniformly.
- **Formatter** — dispatches formatting rules via `KeywordSurfaceKind` rather than hardcoded keyword names, enabling library keywords to receive correct formatting automatically.
- **Editor grammar** — TextMate grammar for VS Code is generated from the compiler-owned core + stdlib registry sources at build time, replacing the manually maintained keyword list.

## Implementation Plan

Phases 1-3 are already public history and remain unchanged here. They established the internal registry migration for core + stdlib keyword handling. The phases below supersede the older exploratory future plan and reflect the accepted public design center: companion crates expose `library_vocab()`, describe richer DSLs through `DslSurface` / `DeclarationSurface` / `ClauseSurface`, and rely on tooling-owned metadata/desugarer packaging.

### Phase 1: Extract `incan-vocab` and validate registry parity

1. Create `crates/incan-vocab/` with the stable public types and traits defined in this RFC: `VocabRegistration`, `DesugarerRegistration`, `VocabDesugarer`, `KeywordRegistration`, `KeywordSpec`, `KeywordSurfaceKind`, `KeywordActivation`, `KeywordSource`, `KeywordRegistry`, `KeywordEntry`, manifest types, and public AST types.
2. Add `incan-vocab` as a dependency of `incan_core` and re-export the public surface that compiler crates need.
3. Implement internal core + stdlib registration sources derived from the existing keyword metadata.
4. Build `KeywordRegistry` alongside the old keyword tables and add parity checks/tests so both sources produce the same effective keyword set before any parser behavior changes.
5. Verify with `cargo test`; publish `incan-vocab` once the public API stabilizes.

### Phase 2: Migrate parser activation and dispatch with transitional lexer compatibility

1. Replace `active_soft_keywords: HashSet<KeywordId>` with registry-backed active keyword tracking while still accepting the current keyword token forms during migration.
2. Route parser keyword activation and dispatch through `KeywordRegistry` + `KeywordSurfaceKind` instead of hard-coding separate hard/soft keyword paths.
3. Introduce any AST support needed for registry-driven surfaces, while keeping compatibility paths for the existing core syntax until parity is proven.
4. Add targeted parser tests for hard/soft keyword parity and import-activated behavior.

### Phase 3: Complete the internal compiler and tooling migration

1. Move the lexer incrementally toward emitting `Token::Ident` for keyword-shaped words, starting with extension shapes and then core shapes once the parser path is validated.
2. Migrate formatter dispatch, LSP keyword consumers, and static grammar generation to core + stdlib provider output.
3. Remove the old `KEYWORDS` table and `KeywordId`-specific dispatch only after all compiler and tooling consumers use the registry.
4. Re-run full compiler and tooling tests to confirm that the registry-backed path is the only remaining keyword source of truth.

### Phase 4: Lock the richer public surface and examples

1. Finalize the author-facing registration model around `VocabRegistration`, `DslSurface`, `DeclarationSurface`, and `ClauseSurface`.
2. Finalize the public AST around `VocabSyntaxNode`, `VocabDeclaration`, `VocabClause`, and `DesugarOutput`.
3. Replace older provider-first and block-only examples with surface-first companion-crate examples.
4. Align rustdocs, examples, and RFC prose so the accepted public surface is stated once and consistently.

### Phase 5: Producer extraction from the canonical Rust entrypoint (requires RFC 031 library build scaffolding)

1. Add `[vocab]` section parsing to `incan.toml`.
2. Implement companion-crate build and extraction in `incan build --lib`.
3. Resolve and invoke `pub fn library_vocab() -> VocabRegistration` as the canonical producer entrypoint.
4. Derive low-level keyword transport, manifest payloads, and desugarer packaging metadata from the returned registration.
5. Add diagnostics that clearly explain extraction or packaging failures.

### Phase 6: Consumer loading and richer syntax activation (requires Phase 5 and RFC 031 consumer loading)

1. Implement library vocab metadata deserialization during consumer builds.
2. Merge import-activated library declarations into the shared registry and activation model.
3. Wire `LibraryManifest` data into import resolution and typechecker symbol loading.
4. Add integration coverage proving that imported companion crates activate the right declarations and exported types.

### Phase 7: End-to-end desugaring for richer DSL declarations (requires Phase 5 transport and Phase 6 loading)

1. Extend the parser/runtime boundary so library syntax reaches desugarers as `VocabSyntaxNode`, not a block-only transport type.
2. Support both statement-valued and expression-valued desugaring through `DesugarOutput`.
3. Map public `IncanExpr` / `IncanStatement` outputs back into the compiler's internal syntax pipeline.
4. Add integration tests for richer declaration-, clause-, and expression-shaped DSLs.

### Phase 8: Tooling-owned dependency and feature integration

1. Replace `needs_*` booleans and `scan_for_*` feature detection where vocab metadata can provide the same information declaratively.
2. Collect required dependencies and stdlib feature flags from loaded library manifests.
3. Update project generation and backend wiring to consume collected dependency/feature sets rather than hard-coded booleans.

### Phase 9: Docs, templates, release polish, and validation

1. Publish canonical docs and templates that present `library_vocab()` plus tooling-owned packaging as the default path.
2. Add end-to-end producer and consumer coverage for both metadata-only and richer DSL companion crates.
3. Release the cleaned public API only after rustdocs, examples, and generated documentation all match the accepted surface.

### Compiler touchpoints

- `crates/incan-vocab/`: defines the final public authoring types, manifest types, public AST, and desugarer contracts.
- `crates/incan_core/`: continues to host the internal core + stdlib registry sources used by the already-landed phases 1-3.
- `crates/incan_syntax/src/parser/`: continues registry-driven activation and later extends parsing to richer declaration/clause surfaces.
- `crates/incan_syntax/src/lexer/`: keeps the phase 1-3 keyword-token migration history and supports later richer-surface parsing.
- `src/manifest.rs`: owns `[vocab]` parsing and later manifest integration for library producer/consumer flows.
- `src/cli/commands/build.rs`: owns companion-crate extraction, metadata derivation, and tooling-owned desugarer packaging.
- `src/frontend/typechecker/`: consumes `LibraryManifest` exports and later richer desugared syntax results.
- `src/frontend/`: hosts the public-AST bridge and later end-to-end desugar runtime integration.
- `src/backend/project/`: replaces `needs_*` booleans with collected dependency/feature sets where vocab metadata suffices.
- `src/lsp/` and `src/format/`: remain registry-backed and later consume the richer declaration metadata where relevant.
- `editors/vscode/`: continues build-time grammar generation for the static fallback story.

## Implementation log

The checklist below records the completed rollout of RFC 027. Earlier phases landed first to establish registry parity; later phases completed the public companion-crate surface, library build/consumer integration, desugaring runtime, and release polish.

### Checklist — Phase 1: Registry extraction and parity

- [x] Create `crates/incan-vocab/` with the core registry, manifest, and public AST types defined by this RFC.
- [x] Implement `IncanCoreVocab` and `StdlibVocab` as internal providers.
- [x] Build `KeywordRegistry` alongside the existing keyword tables and add parity checks/tests.

### Checklist — Phase 2: Parser activation and transitional dispatch

- [x] Replace `active_soft_keywords` with registry-backed active keyword tracking.
- [x] Route parser dispatch through `KeywordRegistry` + `KeywordSurfaceKind` while retaining transitional compatibility with existing keyword token forms.
- [x] Add or adjust AST support for registry-driven surfaces and validate parser behavior with targeted tests.

### Checklist — Phase 3: Full internal compiler and tooling migration

- [x] Migrate the lexer toward `Token::Ident` for keyword-shaped words and remove legacy keyword-specific dispatch once parity is proven.
- [x] Move LSP keyword consumers to core + stdlib provider output.
- [x] Move formatter dispatch to core + stdlib provider output.
- [x] Move static grammar generation to core + stdlib provider output.
- [x] Remove direct compiler/tooling dependence on the old `KEYWORDS` table; keep `keywords.rs` metadata as a compatibility/parity source until RFC 031+ artifact-based vocab loading is end-to-end.

Phase 3 closure inventory:

- Migrated now (compiler/tooling paths): parser activation/dispatch, lexer keyword tokenization, LSP keyword completion, formatter surface-keyword rendering, VS Code keyword grammar patterns.
- Compatibility retained intentionally (non-primary source-of-truth): `incan_core::lang::keywords` metadata table for parity tests and generated language-reference docs, plus adapter scaffolding until external library vocab manifests land via RFC 031.

### Checklist — Phase 4: Lock the richer public surface and examples

- [x] Finalize `VocabRegistration`, `DslSurface`, `DeclarationSurface`, and `ClauseSurface` as the canonical author-facing registration story.
- [x] Finalize the public desugar boundary around `VocabSyntaxNode` and `DesugarOutput`.
- [x] Replace stale provider-first examples with surface-first companion-crate examples throughout the RFC and docs.

### Checklist — Phase 5: Producer extraction from the canonical Rust entrypoint (requires RFC 031 library build scaffolding)

- [x] Parse `[vocab]` in `incan.toml`.
- [x] Resolve and invoke `library_vocab()` during `incan build --lib`.
- [x] Derive packaged metadata and packaging inputs from the returned `VocabRegistration`.
- [x] Add clear diagnostics for extraction and packaging failures.

### Checklist — Phase 6: Consumer loading and richer syntax activation (requires Phase 5 and RFC 031 consumer loading)

- [x] Deserialize library vocab metadata during consumer builds.
- [x] Merge import-activated library declarations into the shared registry and activation model.
- [x] Wire `LibraryManifest` into import resolution and typechecker symbol loading.
- [x] Add integration coverage for imported companion crates and exported library types.

### Checklist — Phase 7: End-to-end desugaring for richer DSL declarations (requires Phase 5 transport and Phase 6 loading)

- [x] Pass richer parsed library syntax to desugarers as `VocabSyntaxNode`.
- [x] Support both statement-valued and expression-valued desugaring through `DesugarOutput`.
- [x] Map public desugarer outputs back into the compiler pipeline.
- [x] Add end-to-end tests for query-like, workflow-like, and other richer declaration families.

### Checklist — Phase 8: Tooling-owned dependency and feature integration

- [x] Replace `needs_*` and `scan_for_*` where vocab metadata can express the same dependency/feature intent declaratively.
- [x] Collect required dependencies and stdlib feature flags from loaded library manifests.
- [x] Update project generation and backend wiring to consume those collected sets.

### Checklist — Phase 9: Docs, templates, release polish, and validation

- [x] Publish canonical docs and templates that present `library_vocab()` plus tooling-owned packaging as the default path.
- [x] Add end-to-end coverage for metadata-only and richer DSL companion crates.
- [x] Release the cleaned API only once rustdocs, examples, and generated docs all agree on the accepted surface.

## Design decisions

### DD-1: Canonical Rust entrypoint, tooling-owned metadata derivation, and WASM desugarers

The public producer contract is a Rust companion crate exporting `pub fn library_vocab() -> VocabRegistration`. `incan build --lib` treats that function as the source of truth, derives serialized metadata from the returned registration, and bundles the derived payload into the library artifact. Any intermediate JSON files or extraction glue are tooling concerns rather than author-facing requirements.

`VocabDesugarer` implementations are authored in Rust. During the internal compiler migration, desugarers may still live inside the compiler binary. For external libraries (Phase 4+), the standard path is for Incan tooling to package those Rust-authored desugarers as WASM modules and load them through a sandboxed runtime (`wasmtime`). WASM is portable (no platform-specific `cdylib`), sandboxed (can't access the filesystem or network), and deterministic.

This resolves both sides of the producer contract: user-authored `build.rs` and hand-maintained `vocab_metadata.json` are not part of the standard authoring story, and `cdylib` + `libloading` is rejected in favor of WASM for packaged desugarers.

### DD-2: The public authoring surface is surface-first, not transport-first

The public design center is `DslSurface` / `DeclarationSurface` / `ClauseSurface`. Low-level keyword transport types such as `KeywordRegistration` and `KeywordSpec` still exist because the compiler, formatter, LSP, and serialization layers need them, but those are derived artifacts rather than the primary authoring story for companion crates.

### DD-3: No macro sugar initially

`VocabRegistration` plus ordinary Rust constructors is explicit and debuggable. A `vocab!{}` macro can be added as a non-breaking convenience in a future minor version once real-world usage patterns stabilize across 3+ libraries. Premature abstraction over the canonical Rust entrypoint is not justified.

### DD-4: Explicit vocab crate path in `incan.toml`

`[vocab].crate` in `incan.toml` is required. Auto-discovery of `crates/*-vocab/` directories is magical, breaks when directory structure varies, and makes it harder to reason about what the compiler will load. Explicit declaration is one line and leaves no ambiguity. Convention-based discovery could be added later as sugar.

### DD-5: The public AST preserves declarations, clauses, and host syntax boundaries

The public desugar boundary must preserve the structure authors actually reason about. That means:

- top-level library syntax reaches desugarers as `VocabSyntaxNode`
- declaration heads and owned clauses survive as first-class structure
- host expressions and statements may appear inside DSL-owned positions without being flattened away too early

This replaces the earlier block-only public story with a richer, loss-minimizing surface that can express query-like and workflow-like DSLs.

### DD-6: Expression-valued and statement-valued desugaring share one contract

Some declarations lower to statements; others, such as `query { ... }`, lower to expressions. The public desugar contract therefore uses `DesugarOutput` rather than assuming every DSL shape expands to `Vec<IncanStatement>`. The compiler must preserve that distinction through parsing, desugaring, and integration back into the host pipeline.

### DD-7: Internal migration history remains documented, but it is not the public contract

Phases 1-3 remain the accurate public history of how the compiler unified its internal keyword registry for core + stdlib behavior. That history stays in the RFC. However, it must not override the producer-side contract. External companion crates target `library_vocab()` and the richer surface-first types, even if the compiler reached that authoring model through an earlier internal migration first.

### DD-8: `incan-vocab` stays lightweight and serializable

`incan-vocab` should remain a small, stable crate that companion crates can depend on without dragging in compiler internals. Serde support may remain feature-gated, but the central requirement is that the public types are portable, versionable, and suitable for tooling-owned serialization.

### DD-9: Feature/dependency metadata belongs to library manifests, not ad-hoc scans

When a library requires extra Cargo dependencies or stdlib feature gates, that information should travel through `LibraryManifest` rather than custom compiler scans. The long-term direction is to replace `needs_*` and `scan_for_*` with declared metadata wherever the vocab system can express the same intent in a uniform way.

### DD-10: Internal registry/loading details are implementation choices, not author-facing design

The compiler may continue to use internal registry sources, caches, and other migration-era implementation techniques for core and stdlib surfaces. Those choices matter for implementation planning, but they should not leak into the normative producer story for external libraries unless they become part of the stable contract.

### DD-11: Clause ordering and cardinality are part of the surface, not hidden parser knowledge

If a declaration owns clauses such as `FROM`, `FILTER`, `GROUP BY`, or typed sections like `input` / `output`, the author should be able to express their required/optional/repeating behavior and relative ordering directly in the registration surface. The grammar contract should be visible in `ClauseSurface`, not buried in parser-side special cases.

### DD-12: Registry-level validation may stay shallow; semantic meaning belongs to desugaring

The registry and parser may validate structural constraints that are cheap and syntax-local, but DSL-specific meaning belongs to the desugarer and later compiler stages. This keeps the public registration surface declarative while still leaving room for richer semantic validation where it actually belongs.

## Scope boundary: operator and glyph semantics

This RFC covers keyword registration, richer declaration- and clause-shaped DSL surfaces, and their desugaring boundary. It does not define the global meaning of operators such as `+`, `>>`, `@`, `|>`, or `<|`; that ordinary operator surface belongs to RFC 028.

Some future DSL surfaces may also reuse glyphs with declaration- or block-owned meaning. That possibility depends on the vocab/desugaring system defined here, but its exact resolution rules and AST contracts are specified separately in RFC 040.

In other words:

- RFC 027 defines how a library introduces an activated DSL surface and desugars it.
- RFC 028 defines ordinary global operator overloading.
- RFC 040 defines how an explicit DSL surface may own scoped glyph behavior without implying global operator support.

Imports alone do not change the meaning of `a >> b` or `a |> b` in ordinary code. Any future glyph support must remain explicitly scoped by the surrounding DSL surface rather than becoming an ambient global operator change.

<!-- The "Design decisions" section above was renamed from "Unresolved questions" once all open questions were resolved. If new unresolved questions arise during implementation, add an "Unresolved questions" section above "Design decisions" and move resolved items back down after resolution. -->

## Addendum (2026-03-16): Rust-First Companion Crate Authoring

This addendum records the release contract for external library authoring. It settles the producer-side design and supersedes older sections that described provider-first extraction or user-managed metadata artifacts as the standard path. The standard producer surface is a Rust companion crate that exposes one canonical entrypoint from `src/lib.rs`:

```rust
pub fn library_vocab() -> VocabRegistration
```

This function is the author-facing source of truth for the library's DSL surface bundle.

### Canonical authoring contract

- A library that exports import-activated vocabulary declares `[vocab].crate` in `incan.toml`.
- The referenced directory is a real Rust crate with `Cargo.toml` and `src/lib.rs`.
- `src/lib.rs` exports `pub fn library_vocab() -> VocabRegistration`.
- `VocabRegistration` directly carries activated `DslSurface` values, manifest metadata, and the optional `DesugarerRegistration`.
- A `DslSurface` groups one activation rule with one or more `DeclarationSurface` definitions.
- A `DeclarationSurface` owns its clause grammar directly through `ClauseSurface` definitions.
- Convenience macros or helper constructors may wrap this shape, but they must preserve the same contract.

### Tooling-owned packaging

`incan build --lib` treats `library_vocab()` as the producer contract.

- The CLI resolves `[vocab].crate`, builds the companion crate, and invokes the canonical entrypoint through Incan-owned extraction glue.
- The CLI derives the serialized vocabulary metadata from the returned registration.
- Any intermediate metadata files used by the implementation are tooling concerns, not part of the public authoring contract.
- The packaged `.incnlib` remains the consumer-facing transport boundary for parser activation, manifest metadata, and DSL lowering.

### DSL packaging

If `library_vocab()` returns a registration with a desugarer, `incan build --lib` packages it automatically.

- The standard path for richer DSLs is: implement the desugarer in Rust and return it from `library_vocab()`.
- The CLI owns the standard build target, entrypoint wiring, and artifact packaging for that desugarer, using Incan defaults unless the registration supplies explicit overrides.
- A user-authored `build.rs` is not required as part of the Incan vocabulary contract.

### Authoring notes

- A companion crate may still include a `build.rs` for ordinary Cargo work, but `build.rs` is not part of the standard vocabulary authoring surface.
- Docs, examples, and templates should present the Rust entrypoint and tooling-owned packaging flow as the default path.
- Where earlier producer-side descriptions in this RFC imply provider-first extraction, block-only public AST shapes, user-managed metadata files, or manually wired desugarer output paths as part of the standard flow, this addendum supersedes those descriptions.
