# RFC 027: `incan-vocab` — Library Vocabulary Registration Crate

- **Status:** In Progress
- **Created:** 2026-03-06
- **Author(s):** Danny Meijer (@dannymeijer)
- **Issue:** [#161](https://github.com/dannys-code-corner/incan/issues/161)
- **RFC PR:**
    - Phases 1-3: [#...](https://github.com/dannys-code-corner/incan/pull/...)
- **Related**:
    - RFC 022 (stdlib namespacing)
    - RFC 023 (std.web migration)
    - RFC 028 (global operator overloading)
    - RFC 040 (scoped DSL glyph surfaces)
- **Written against:** v0.2
- **Shipped in:** —

## Summary

This RFC defines **`incan-vocab`**, a standalone Rust crate that provides the traits and types for Incan's **unified keyword registry** — the single mechanism through which *all* keywords are defined, from core language constructs (`def`, `if`, `for`) to `stdlib` features (`async`, `await`) to third-party DSL extensions (for example `routes` and `machine`).

The crate provides:

- **`KeywordRegistry`** — a cached lookup of every keyword the compiler knows about, built once at startup.
- **`VocabProvider`** — the trait libraries implement to register keywords and manifest metadata.
- **`VocabDesugarer`** — transforms parsed DSL blocks into regular Incan AST before typechecking.

There is no distinction between "hard" and "soft" keywords at the architectural level anymore. All keywords are entries in the same registry — they differ only in their **activation rule** (`always-on` for core, `import-activated` for stdlib and libraries) and their **source** (`compiler-built-in`, `stdlib`, or `third-party`). The compiler, LSP, formatter, and other tools consume the same cached registry.

**Near-term rollout:** first unify today's split core + stdlib keyword infrastructure behind this shared registry and activation model inside the compiler. External library loading, manifest transport, and desugarer execution use the same model once RFC 031-style library artifacts exist.

Scoped glyph surfaces for explicit DSL blocks build on this substrate, but their semantics are specified separately. This RFC provides the block registration, placement, and desugaring machinery those glyph surfaces rely on; it does not define the global meaning of operators.

This yields one stable API that can be used to create third-party libraries and language plugins.

## Goals

- Deliver an internal-first migration that unifies today's core language and stdlib keyword metadata under one registry and one activation model before external library loading exists.
- Define a stable, published `incan-vocab` crate that serves as the single extension point for keyword registration across core language, stdlib, and third-party libraries.
- Replace the hard-keyword `KEYWORDS` table and soft-keyword `info_soft()` path with one `KeywordRegistry` consumed uniformly by the compiler, LSP, formatter, and editor grammar generation.
- Give library authors a typed Rust API (`VocabProvider`, `VocabDesugarer`) to declare keywords and desugar DSL blocks once library build and consumer loading flows exist.
- Establish the manifest schema types (`LibraryManifest`, `TypeRef`, `CargoDependency`) used by the library build and consumer flows defined in RFC 031.

## Non-Goals

- Defining the global meaning of operators (`+`, `>>`, `|>`, etc.) — that belongs to RFC 028.
- Defining scoped glyph surfaces for explicit DSL blocks — that belongs to RFC 040.
- External plugin loading via dynamic libraries (`cdylib`/`libloading`) — desugarers for external libraries use WASM (Phase 4+), not native shared libraries.
- Implementing the `incan.pub` registry or git-based dependency resolution — those are Phase 2/3 concerns addressed by RFC 034.
- Replacing the existing `[rust-dependencies]`/Cargo wiring — that is RFC 031's concern.
- Implementing external library artifact transport or consumer loading ahead of RFC 031 — this RFC defines the contracts those later phases use.

## Motivation

Incan currently maintains **two separate keyword systems**: a compile-time `KEYWORDS` const table of ~40 hard keywords (`def`, `if`, `for`, etc.) recognized directly by the `lexer`, and a small `info_soft()` mechanism for 3 import-activated keywords (`async`, `await`, `assert`). Third-party libraries have no way to participate in either system. This split creates multiple problems:

1. **No stable API surface.** `incan_core::lang::keywords` is internal; breaking changes would cascade to every library.
2. **No manifest schema.** Libraries have no way to declare their exported types, functions, and modules in a machine-readable format that the compiler can consume.
3. **No vocab registration path.** Adding a new keyword currently requires modifying the compiler's `KEYWORDS` const array.
4. **Feature scanning debt.** The compiler uses ad-hoc `needs_web`, `needs_serde`, `scan_for_*` booleans to detect library usage. This doesn't scale beyond the `stdlib`.
5. **No desugaring path.** When a library introduces block-level DSL syntax (e.g., `routes { ... }` or `machine { ... }`), the parser produces a generic `VocabBlock` AST node — but the compiler has no mechanism to transform that block into typecheckable Incan code. Libraries need a way to provide their own AST → AST desugaring.
6. **Two keyword systems where one would do.** Hard and soft keywords share the same data — a name, a parsing shape, and activation rules — yet they're implemented as separate subsystems with different types, lookup paths, and parser dispatch. The stdlib's `async`/`await`/`assert` are further special-cased via `scan_for_*` booleans. A unified registry eliminates this accidental complexity, gives the LSP and formatter a single source of truth, and battle-tests the extension API on the stdlib before any external library exists.

`incan-vocab` solves all six by extracting a minimal, stable crate that models **every** keyword uniformly — core, stdlib, and third-party — differing only in activation rule and source.

## Guide-level explanation

### For library authors

You want to create an Incan library called `routekit` that adds HTTP routing DSL keywords. Here's what you do:

**1. Project structure** — Your Incan library project uses the standard `incan init` layout, plus a `crates/` directory for Rust code (following Rust workspace conventions):

```text
routekit/
├── incan.toml                 # Incan project manifest
├── src/                       # Incan source (.incn files)
│   ├── lib.incn
│   ├── router.incn
│   └── middleware.incn
├── crates/                    # Rust crates (workspace convention)
│   └── routekit-vocab/        # VocabProvider implementation
│       ├── Cargo.toml         # depends on incan-vocab
│       └── src/
│           └── lib.rs         # implements VocabProvider
└── tests/
    └── test_routes.incn
```

Key insight:

- `src/` is for Incan code (created by `incan init`).
- `crates/` is for Rust code. The vocab crate follows the naming convention `<library>-vocab`.

**2. Implement `VocabProvider`** — In `crates/routekit-vocab/src/lib.rs`:

```rust
use incan_vocab::{
    VocabProvider, KeywordRegistration, KeywordSpec, KeywordSurfaceKind,
    KeywordActivation, LibraryManifest, ModuleExport, FunctionExport, TypeRef,
};

pub struct RoutekitVocab;

impl VocabProvider for RoutekitVocab {
    fn keyword_registrations(&self) -> Vec<KeywordRegistration> {
        vec![KeywordRegistration {
            activation: KeywordActivation::OnImport("routekit.routes".into()),
            keywords: vec![
                KeywordSpec::new("routes", KeywordSurfaceKind::BlockDeclaration),
                KeywordSpec::in_block("GET", KeywordSurfaceKind::BlockContextKeyword, &["routes"]),
                KeywordSpec::in_block("POST", KeywordSurfaceKind::BlockContextKeyword, &["routes"]),
                KeywordSpec::in_block("middleware", KeywordSurfaceKind::SubBlock, &["routes"]),
            ],
        }]
    }

    fn manifest(&self) -> LibraryManifest {
        LibraryManifest {
            format_version: ManifestFormatVersion::V1,
            modules: vec![
                ModuleExport {
                    path: "routekit".into(),
                    functions: vec![
                        FunctionExport {
                            name: "serve".into(),
                            params: vec![
                                ("router".into(), TypeRef::named("Router")),
                                ("port".into(), TypeRef::named("int")),
                            ],
                            return_type: None,
                            is_async: true,
                        },
                    ],
                    types: vec![],
                },
            ],
        }
    }
}
```

**3. Wire it up in `incan.toml`**:

```toml
[package]
name = "routekit"
version = "0.1.0"

[vocab]
crate = "crates/routekit-vocab"
```

**4. Build** — When you run `incan build --lib`, the compiler:

1. Reads `incan.toml` and finds the `[vocab]` section
2. Builds the vocab crate via `cargo build`
3. Loads the `VocabProvider` implementation
4. Extracts keyword registrations and manifest metadata
5. Packages everything into the distributable library artifact

### For library consumers

Consumers don't interact with the vocab crate at all. They just use the library:

```incan
import std.async
from routekit.routes import routes, GET, POST
from routekit import Router, serve

app = routes {
    GET "/users" -> list_users
    POST "/users" -> create_user
    middleware:
        auth_required
        log_requests
}

router = Router(app)
await serve(router, port=8080)
```

The compiler resolves `routekit` from the project's dependencies, loads the pre-built vocab metadata, and activates the keywords registered for `routekit.routes`.

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

#### `VocabProvider` trait

The central abstraction. Every library vocab crate exports exactly one implementation. The compiler's core keywords and stdlib keywords are also expressed through this trait internally, making it the single source of truth.

```rust
/// Trait for vocabulary providers.
///
/// Implementations register keywords and provide manifest metadata.
/// This is the universal extension point — the compiler's own core keywords,
/// the stdlib, and third-party libraries all register through the same trait.
pub trait VocabProvider {
    /// Keywords this provider introduces, grouped by activation rule.
    fn keyword_registrations(&self) -> Vec<KeywordRegistration>;

    /// Machine-readable manifest describing the library's public surface.
    fn manifest(&self) -> LibraryManifest;
}
```

#### `KeywordRegistration`

Groups keywords by their activation rule:

```rust
/// A group of keywords with a shared activation rule.
///
/// Core language keywords use `KeywordActivation::Always`. Library keywords use
/// `KeywordActivation::OnImport("routekit.routes")`.
/// The registry treats both uniformly.
pub struct KeywordRegistration {
    /// When these keywords become active.
    ///
    /// `Always` — core language keywords, active in every file.
    /// `OnImport("std.async")` — active when the import path is used.
    pub activation: KeywordActivation,

    /// The keywords in this group.
    pub keywords: Vec<KeywordSpec>,

    /// Decorators that are valid on blocks introduced by these keywords (DD-17).
    ///
    /// When non-empty, the parser checks decorator names against this list and emits
    /// a diagnostic for unrecognized decorators — enabling IDE completion without
    /// loading the desugarer. Semantic validation of decorator *arguments* remains
    /// the desugarer's responsibility. Empty means no registry-level validation.
    pub valid_decorators: Vec<String>,
}
```

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

The second core abstraction. Libraries that introduce `BlockDeclaration` keywords provide a desugarer that transforms the parsed DSL block into regular Incan statements before typechecking.

**Why this is needed:** The parser knows *how* to parse a block (via `KeywordSurfaceKind`), but the compiler doesn't know what the block *means*. Without desugaring, the compiler produces a generic `VocabBlock` AST node that can't be typechecked or lowered to IR. The desugarer bridges this gap by rewriting DSL syntax into standard Incan method calls and expressions.

**Two-tier design:**

1. **Simple keywords** (`StatementKeywordArgs`, `PrefixExpression`, `DeclarationModifier`) — the compiler has built-in handling for these patterns. No desugarer needed.
2. **Block keywords** (`BlockDeclaration` + associated `BlockContextKeyword` / `SubBlock`) — the library provides a `VocabDesugarer` that transforms the block into regular Incan code.

```rust
/// Trait for transforming parsed DSL blocks into regular Incan AST.
///
/// Libraries that register `BlockDeclaration` keywords must also provide
/// a desugarer. The compiler calls this after parsing but before typechecking.
pub trait VocabDesugarer {
    /// Transform a parsed vocab block into regular Incan statements.
    ///
    /// The returned statements replace the original `VocabBlock` in the AST.
    /// They are then typechecked and lowered like any other Incan code.
    fn desugar_block(
        &self,
        block: &VocabBlock,
        ctx: &DesugarContext,
    ) -> Result<Vec<IncanStatement>, DesugarError>;
}
```

The desugarer operates on **public AST types** (defined below) that are stable across compiler versions. It receives a `VocabBlock` (the parsed DSL syntax) and returns `Vec<IncanStatement>` (regular Incan code). The compiler then typechecks the returned statements normally.

#### `VocabRegistration` — linking provider and desugarer

A library that introduces `BlockDeclaration` keywords must supply both a `VocabProvider` (metadata) and a `VocabDesugarer` (transform logic). The `VocabRegistration` struct bundles them together so the compiler knows which desugarer handles which library:

```rust
/// A library's complete vocabulary registration.
///
/// Bundles the metadata provider and the optional desugarer into a single
/// unit. The compiler collects `Vec<VocabRegistration>` at startup and uses
/// the provider to build the `KeywordRegistry` while associating each
/// library's block keywords with the corresponding desugarer.
pub struct VocabRegistration {
    /// Keyword source label (used for diagnostics and LSP).
    pub source: KeywordSource,
    /// The vocabulary provider (keyword registrations + manifest).
    pub provider: Box<dyn VocabProvider>,
    /// The desugarer for `BlockDeclaration` keywords, if any.
    ///
    /// `None` for providers that only register simple keywords
    /// (e.g., `IncanCoreVocab`, `StdlibVocab`).
    pub desugarer: Option<Box<dyn VocabDesugarer>>,
}
```

**Why `Option<Box<dyn VocabDesugarer>>`?** Core and stdlib providers register keywords but don't need desugaring — their keywords have dedicated parser handling. Only library providers that introduce `BlockDeclaration` keywords need a desugarer. This keeps the common case simple.

**Forward compatibility:** For the internal-first architecture (Phases 1–3), all registrations are compiled directly into the compiler binary. When external loading is implemented (Phase 4), a library's compiled plugin simply exports a function returning `VocabRegistration` — same struct, different loading mechanism. This ensures Incan DSL libraries (like routing or state machine frameworks) can be loaded identically regardless of whether they are internal or external.

#### Public AST types (`ast` module)

The `incan-vocab` crate exports a set of **public AST types** that form the contract between the compiler and library desugarers. These are intentionally separate from the compiler's internal AST — they are stable, versioned, and designed for library-author ergonomics.

##### Input types (what the desugarer receives)

```rust
/// A parsed DSL block. This is the input to `VocabDesugarer::desugar_block`.
pub struct VocabBlock {
    /// The block keyword (e.g., "machine", "routes").
    pub keyword: String,
    /// Arguments after the keyword (e.g., `"traffic_light"` in `machine "traffic_light" { ... }`).
    pub arguments: Vec<IncanExpr>,
    /// Decorators applied to this block (e.g., `@quality("strict")`, `@retry(3)`).
    ///
    /// The parser collects any `@decorator` expressions that immediately precede the block
    /// keyword and passes them here. The desugarer can interpret them as metadata, validation
    /// rules, wrappers, or ignore them. Empty if no decorators are present.
    pub decorators: Vec<IncanExpr>,
    /// The block body: context entries, sub-blocks, and plain statements.
    pub body: Vec<VocabBodyItem>,
    /// Functions scoped to this block (available only inside the DSL, not globally).
    pub scoped_functions: Vec<ScopedFunction>,
    /// Source location for error reporting.
    pub span: Span,
}

/// An item inside a vocab block body.
pub enum VocabBodyItem {
    /// A context keyword entry (e.g., `on "timer" -> "green"`).
    ContextEntry(VocabContextEntry),
    /// A named sub-block (e.g., `enter: ...`).
    SubBlock(VocabSubBlock),
    /// A nested block declaration (e.g., `state "red": ...` inside a parent block).
    NestedBlock(VocabBlock),
    /// A regular Incan statement inside the block.
    Statement(IncanStatement),
}

/// A context keyword entry within a block.
///
/// Example: `GET "/users" -> list_users` or `on "timer" -> "green"`
pub struct VocabContextEntry {
    /// The context keyword (e.g., "GET", "on").
    pub keyword: String,
    /// Arguments to the context keyword.
    pub arguments: Vec<IncanExpr>,
    /// Optional nested body for block-style context entries.
    ///
    /// Empty for inline forms like `GET "/users" -> list_users`.
    /// Non-empty for block forms like `GET "/users": ...`.
    pub body: Vec<VocabBodyItem>,
    /// Source location.
    pub span: Span,
}

/// A sub-block within a vocab block.
///
/// Example: `enter: activate_red_light()`
pub struct VocabSubBlock {
    /// The sub-block keyword (e.g., "enter", "exit", "middleware").
    pub keyword: String,
    /// The sub-block body.
    ///
    /// This is recursive so sub-blocks can contain nested DSL structure, not just plain statements.
    pub body: Vec<VocabBodyItem>,
    /// Source location.
    pub span: Span,
}

/// A function scoped to a specific block keyword.
///
/// These functions are only available inside the DSL block, not in normal Incan code.
/// Example: a hypothetical `count()` or `sum()` that only makes sense inside a query block.
pub struct ScopedFunction {
    /// Function name.
    pub name: String,
    /// Parameter types.
    pub params: Vec<(String, String)>,
    /// Return type name.
    pub return_type: Option<String>,
}
```

##### Output types (what the desugarer produces)

The desugarer returns regular Incan expressions and statements. These are a **subset** of the compiler's internal AST, exposed as stable public types:

```rust
/// An Incan expression (public subset).
///
/// Desugarers construct these to represent the Incan code that replaces a DSL block.
#[non_exhaustive]
pub enum IncanExpr {
    /// Integer literal: `42`
    IntLiteral(i64),
    /// Float literal: `3.14`
    FloatLiteral(f64),
    /// String literal: `"hello"`
    StringLiteral(String),
    /// Boolean literal: `True` / `False`
    BoolLiteral(bool),
    /// Variable reference: `x`, `my_handler`
    Name(String),
    /// Member access: `builder.state`
    MemberAccess(Box<IncanExpr>, String),
    /// Method call: `builder.state("idle")`
    MethodCall(Box<IncanExpr>, String, Vec<IncanExpr>),
    /// Function call: `activate_light("red")`
    FunctionCall(String, Vec<IncanExpr>),
    /// Binary operation: `x + 1`
    BinaryOp(Box<IncanExpr>, BinaryOperator, Box<IncanExpr>),
    /// Unary operation: `-x`, `not flag`
    UnaryOp(UnaryOperator, Box<IncanExpr>),
    /// List literal: `[1, 2, 3]`
    List(Vec<IncanExpr>),
    /// Lambda: `|x| x + 1`
    Lambda(Vec<String>, Box<IncanExpr>),
    /// Struct/model construction: `Config(timeout=30)`
    Construct(String, Vec<(String, IncanExpr)>),
    /// Pass-through to the compiler's AST (escape hatch for advanced cases).
    ///
    /// Contains an opaque string that the compiler parses as an Incan expression.
    /// Use sparingly — prefer the typed variants above.
    Passthrough(String),
}

/// An Incan statement (public subset).
///
/// Covers the common control-flow shapes that desugarers need when emitting
/// non-trivial logic (e.g., iteration inside a pipeline step, conditional
/// transitions in a state machine). Using typed variants instead of
/// `Passthrough` strings gives the compiler full visibility for validation.
#[non_exhaustive]
pub enum IncanStatement {
    /// Let binding: `let x = expr`
    Let(String, IncanExpr),
    /// Assignment: `x = expr`
    Assign(String, IncanExpr),
    /// Expression statement: `builder.build()`
    Expr(IncanExpr),
    /// Return: `return expr`
    Return(IncanExpr),
    /// For loop: `for item in collection: ...`
    ForLoop {
        target: String,
        iter: IncanExpr,
        body: Vec<IncanStatement>,
    },
    /// If/else chain: `if cond: ... elif cond: ... else: ...`
    IfElse {
        condition: IncanExpr,
        body: Vec<IncanStatement>,
        elif_branches: Vec<(IncanExpr, Vec<IncanStatement>)>,
        else_body: Option<Vec<IncanStatement>>,
    },
    /// While loop: `while cond: ...`
    WhileLoop {
        condition: IncanExpr,
        body: Vec<IncanStatement>,
    },
    /// Match block: `match expr: case pattern: ...`
    MatchBlock {
        subject: IncanExpr,
        arms: Vec<(IncanExpr, Vec<IncanStatement>)>,
    },
    /// Try/except: `try: ... except SomeError as e: ...`
    TryExcept {
        body: Vec<IncanStatement>,
        /// Each handler: (exception type name, optional binding name, handler body).
        handlers: Vec<(String, Option<String>, Vec<IncanStatement>)>,
    },
}

/// Binary operators available to desugarers.
#[non_exhaustive]
pub enum BinaryOperator {
    Add, Sub, Mul, Div, Mod,
    Eq, NotEq, Lt, LtEq, Gt, GtEq,
    And, Or,
}

/// Unary operators available to desugarers.
#[non_exhaustive]
pub enum UnaryOperator {
    Neg,
    Not,
}
```

##### Support types

```rust
/// Context provided to the desugarer by the compiler.
pub struct DesugarContext {
    /// Variables in scope at the point where the block appears.
    pub locals: Vec<String>,
    /// Path of the source file being compiled.
    pub file_path: String,
    /// Span of the entire block (for error reporting).
    pub span: Span,
}

/// Source location for diagnostics.
#[derive(Debug, Clone, Copy)]
pub struct Span {
    pub line: u32,
    pub column: u32,
}

/// Error returned by a desugarer.
pub struct DesugarError {
    /// Human-readable error message.
    pub message: String,
    /// Source location where the error occurred.
    pub span: Span,
    /// Optional help text ("did you mean...?").
    pub help: Option<String>,
}
```

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
    /// Build a registry from multiple VocabProvider implementations.
    ///
    /// Called once at compiler startup. The compiler provides:
    /// 1. Core language provider (activation: Always)
    /// 2. Stdlib providers (activation: OnImport for each std.* namespace)
    /// 3. Project dependency providers (loaded from incan.toml dependencies)
    pub fn from_registrations(registrations: &[VocabRegistration]) -> Self {
        let mut registry = Self::new();
        for reg in registrations {
            for kw_reg in reg.provider.keyword_registrations() {
                for spec in &kw_reg.keywords {
                    registry.insert(KeywordEntry {
                        name: spec.name.clone(),
                        surface_kind: spec.surface_kind,
                        compound_tokens: spec.compound_tokens.clone(),
                        placement: spec.placement.clone(),
                        activation: kw_reg.activation.clone(),
                        source: reg.source.clone(),
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

**Ambiguity rule:** Multiple providers may register the same keyword text under different parent blocks, but the same `(name, immediate_parent, surface_kind)` combination may appear only once. The registry rejects ambiguous duplicates at load time with a diagnostic naming both sources.

**Decorator collection for vocab blocks:** The parser collects `@expr` tokens preceding a `BlockDeclaration` using the same mechanism it uses for `def`/`class` decorators. Collected decorators are stored in `VocabBlock.decorators` and passed to the desugarer. The desugarer decides what they mean — the parser performs no validation beyond syntactic correctness.

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
        let mut registrations = vec![
            VocabRegistration {
                source: KeywordSource::Core,
                provider: Box::new(IncanCoreVocab),
                desugarer: None,
            },
            VocabRegistration {
                source: KeywordSource::Stdlib,
                provider: Box::new(StdlibVocab),
                desugarer: None,
            },
            // Project dependency registrations loaded from incan.toml...
        ];
        KeywordRegistry::from_registrations(&registrations)
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
| **Go to definition**    | `entry.source` → navigate to the VocabProvider or stdlib module                                                                                |

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

In the unified model, there are **three** internal `VocabProvider` implementations:

**1. `IncanCoreVocab`** — registers all ~40 core language keywords with `KeywordActivation::Always`:

```rust
// Compiler-internal: built into the compiler binary.
struct IncanCoreVocab;

impl VocabProvider for IncanCoreVocab {
    fn keyword_registrations(&self) -> Vec<KeywordRegistration> {
        vec![KeywordRegistration {
            activation: KeywordActivation::Always,
            keywords: vec![
                // Representative entries — one per surface kind:
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
                // ... plus ~27 more (model, trait, enum, elif, else, for,
                //     case, except, finally, etc.)
                // See the KeywordSurfaceKind mapping table for the full list.
            ],
        }]
    }

    fn manifest(&self) -> LibraryManifest {
        // Core language has no manifest (it's not a library)
        LibraryManifest::empty()
    }
}
```

**2. `StdlibVocab`** — registers the 3 stdlib import-activated keywords with `KeywordActivation::OnImport`:

```rust
struct StdlibVocab;

impl VocabProvider for StdlibVocab {
    fn keyword_registrations(&self) -> Vec<KeywordRegistration> {
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
    }

    fn manifest(&self) -> LibraryManifest { ... }
}
```

**3. Library providers** — loaded from dependency artifacts, same trait.

**Migration from current keyword infrastructure:**

| Implementation phase | Current keyword system                                           | New unified model                                                                                            |
| -------------------- | ---------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------ |
| Phase 1              | `KEYWORDS` const table + `info_hard()` / `info_soft()`           | `IncanCoreVocab` + `StdlibVocab` produce equivalent registrations; old tables remain for parity validation   |
| Phase 2              | Hard keyword tokens + `active_soft_keywords: HashSet<KeywordId>` | Parser activation and dispatch become registry-backed while still accepting transitional keyword token forms |
| Phase 3              | Mixed registry/legacy consumers in parser and tooling            | Lexer/parser/tooling complete migration; `KeywordRegistry` becomes the sole source of truth                  |

> **Important:** Phase 1 → Phase 2 can be done incrementally. The parser can accept both `Token::Def` and registry-based `Token::Ident("def")` during the transition. This avoids a flag-day rewrite.

### Extraction flow (`incan build --lib`)

The next two flows describe the full library architecture. The earlier implementation phases establish the shared registry and activation model inside the compiler; the later phases extend that model to library artifacts and consumer loading.

When a library author runs `incan build --lib`:

```text
1. Read incan.toml → find [vocab].crate path
2. cargo build the vocab crate (crates/<name>-vocab/)
3. Run the compiler's metadata extraction step to obtain VocabProvider output
4. Serialize keyword registrations + manifest metadata into the library artifact
5. Package: Incan compiled output + vocab metadata → distributable artifact
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
    ├── lib.rs           # pub trait VocabProvider, VocabDesugarer + re-exports
    ├── keywords.rs      # KeywordRegistration, KeywordSpec, KeywordSurfaceKind
    ├── manifest.rs      # LibraryManifest, ModuleExport, TypeExport, etc.
    ├── ast.rs           # Public AST types: VocabBlock, IncanExpr, IncanStatement
    ├── desugar.rs       # VocabDesugarer trait, DesugarContext, DesugarError
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
| Central trait                 | `VocabProvider`        |
| incan.toml section            | `[vocab]`              |

### Interaction with existing features

**Imports / keyword activation** (RFC 022): `VocabProvider::keyword_registrations()` returns the same activation metadata that the stdlib currently uses. The parser's per-file `active_keywords` set is populated from the `KeywordRegistry` — whether a keyword comes from core, stdlib, or a library is invisible to the parser.

**Rust interop** (RFC 005): The vocab crate *is* Rust code. Library authors write it in Rust, depending only on `incan-vocab`. The `crates/` directory convention aligns with standard Rust workspace practices.

**Typechecker**: Manifest metadata provides function signatures and type definitions that the typechecker uses for imported symbols. This replaces the current approach where the typechecker relies on `stdlib/*.incn` stubs.

**`incan.toml`**: The `[vocab]` section is the only new project-level configuration. It points to the vocab crate directory. Projects without custom vocabulary omit this section entirely.

### Compatibility / migration

This is a new feature, not a breaking change. Existing projects without a `[vocab]` section continue to work exactly as before.

For the stdlib, migration is internal to the compiler:

1. Extract types to `incan-vocab`
2. `incan_core` re-exports them
3. Existing `KEYWORDS` table and `info_soft()` continue to work
4. Gradual migration of stdlib features to `VocabProvider` in later phases

## Examples

### Routekit — HTTP Routing DSL Library

A web routing library that introduces declarative route definitions within Incan.

> Note: this is not an ACTUAL incan library, it's just an example of what a library could do given this feature.

**Folder structure:**

```text
routekit/
├── incan.toml
├── src/
│   ├── lib.incn
│   ├── router.incn
│   └── middleware.incn
├── crates/
│   └── routekit-vocab/
│       ├── Cargo.toml
│       └── src/lib.rs
└── tests/
    └── test_routes.incn
```

**`crates/routekit-vocab/Cargo.toml`:**

```toml
[package]
name = "routekit-vocab"
version = "0.1.0"
edition = "2021"

[dependencies]
incan-vocab = "0.1"
```

**`crates/routekit-vocab/src/lib.rs`:**

```rust
use incan_vocab::*;

pub struct RoutekitVocab;

impl VocabProvider for RoutekitVocab {
    fn keyword_registrations(&self) -> Vec<KeywordRegistration> {
        vec![KeywordRegistration {
            activation: KeywordActivation::OnImport("routekit.routes".into()),
            keywords: vec![
                KeywordSpec::new("routes", KeywordSurfaceKind::BlockDeclaration),
                KeywordSpec::in_block("GET", KeywordSurfaceKind::BlockContextKeyword, &["routes"]),
                KeywordSpec::in_block("POST", KeywordSurfaceKind::BlockContextKeyword, &["routes"]),
                KeywordSpec::in_block("PUT", KeywordSurfaceKind::BlockContextKeyword, &["routes"]),
                KeywordSpec::in_block("DELETE", KeywordSurfaceKind::BlockContextKeyword, &["routes"]),
                KeywordSpec::in_block("middleware", KeywordSurfaceKind::SubBlock, &["routes"]),
            ],
        }]
    }

    fn manifest(&self) -> LibraryManifest {
        LibraryManifest {
            format_version: ManifestFormatVersion::V1,
            modules: vec![
                ModuleExport {
                    path: "routekit".into(),
                    functions: vec![FunctionExport {
                        name: "serve".into(),
                        params: vec![
                            ("router".into(), TypeRef::named("Router")),
                            ("port".into(), TypeRef::named("int")),
                        ],
                        return_type: None,
                        is_async: true,
                    }],
                    types: vec![
                        TypeExport {
                            name: "Router".into(),
                            kind: TypeExportKind::Class,
                            type_params: vec![],
                            fields: vec![],
                            methods: vec![
                                FunctionExport {
                                    name: "mount".into(),
                                    params: vec![
                                        ("prefix".into(), TypeRef::named("str")),
                                        ("routes".into(), TypeRef::named("RouteTable")),
                                    ],
                                    return_type: Some(TypeRef::named("Router")),
                                    is_async: false,
                                },
                            ],
                        },
                        TypeExport {
                            name: "Request".into(),
                            kind: TypeExportKind::Model,
                            type_params: vec![],
                            fields: vec![
                                FieldExport {
                                    name: "method".into(),
                                    field_type: TypeRef::named("str"),
                                    has_default: false,
                                },
                                FieldExport {
                                    name: "path".into(),
                                    field_type: TypeRef::named("str"),
                                    has_default: false,
                                },
                            ],
                            methods: vec![],
                        },
                        TypeExport {
                            name: "Response".into(),
                            kind: TypeExportKind::Model,
                            type_params: vec![],
                            fields: vec![
                                FieldExport {
                                    name: "status".into(),
                                    field_type: TypeRef::named("int"),
                                    has_default: false,
                                },
                                FieldExport {
                                    name: "body".into(),
                                    field_type: TypeRef::named("str"),
                                    has_default: true,
                                },
                            ],
                            methods: vec![],
                        },
                    ],
                },
                ModuleExport {
                    path: "routekit.routes".into(),
                    functions: vec![],
                    types: vec![],
                },
            ],
        }
    }
}
```

**Consumer usage** (`my_app/src/main.incn`):

```incan
import std.async
from routekit.routes import routes, GET, POST
from routekit import Router, Request, Response, serve

def list_users(req: Request) -> Response:
    return Response(status=200, body="[{\"name\": \"Alice\"}]")

def create_user(req: Request) -> Response:
    return Response(status=201, body="created")

app = routes {
    GET "/users" -> list_users
    POST "/users" -> create_user
    middleware:
        auth_required
        log_requests
}

router = Router(app)
await serve(router, port=8080)
```

### Stately — State Machine DSL Library

A library that adds declarative state machine definitions to Incan.

> Note: this is not an ACTUAL incan library, it's just an example of what a library could do given this feature.

**Folder structure:**

```text
stately/
├── incan.toml
├── src/
│   ├── lib.incn
│   ├── machine.incn
│   └── transitions.incn
├── crates/
│   └── stately-vocab/
│       ├── Cargo.toml
│       └── src/lib.rs
└── tests/
    └── test_machines.incn
```

**`crates/stately-vocab/src/lib.rs`:**

```rust
use incan_vocab::*;

pub struct StatelyVocab;

impl VocabProvider for StatelyVocab {
    fn keyword_registrations(&self) -> Vec<KeywordRegistration> {
        vec![KeywordRegistration {
            activation: KeywordActivation::OnImport("stately.machine".into()),
            keywords: vec![
                KeywordSpec::new("machine", KeywordSurfaceKind::BlockDeclaration),
                KeywordSpec::in_block("state", KeywordSurfaceKind::BlockDeclaration, &["machine"]),
                KeywordSpec::in_block("on", KeywordSurfaceKind::BlockContextKeyword, &["state"]),
                KeywordSpec::in_block("enter", KeywordSurfaceKind::SubBlock, &["state"]),
                KeywordSpec::in_block("exit", KeywordSurfaceKind::SubBlock, &["state"]),
            ],
        }]
    }

    fn manifest(&self) -> LibraryManifest {
        LibraryManifest {
            name: "stately".into(),
            version: "0.1.0".into(),
            format_version: ManifestFormatVersion::V1,
            modules: vec![
                ModuleExport {
                    path: "stately".into(),
                    functions: vec![],
                    types: vec![
                        TypeExport {
                            name: "StateMachine".into(),
                            kind: TypeExportKind::Class,
                            type_params: vec![],
                            fields: vec![],
                            methods: vec![
                                FunctionExport {
                                    name: "current_state".into(),
                                    params: vec![],
                                    return_type: Some(TypeRef::named("str")),
                                    is_async: false,
                                },
                                FunctionExport {
                                    name: "send".into(),
                                    params: vec![("event".into(), TypeRef::named("str"))],
                                    return_type: Some(TypeRef::named("str")),
                                    is_async: false,
                                },
                            ],
                        },
                    ],
                },
                ModuleExport {
                    path: "stately.machine".into(),
                    functions: vec![],
                    types: vec![],
                },
            ],
        }
    }
}
```

**Consumer usage:**

```incan
from stately.machine import machine, state, on
from stately import StateMachine

lights = machine "traffic_light" {
    state "red" {
        on "timer" -> "green"
        enter:
            activate_red_light()
    }

    state "green" {
        on "timer" -> "yellow"
        enter:
            activate_green_light()
    }

    state "yellow" {
        on "timer" -> "red"
        enter:
            activate_yellow_light()
    }
}

assert lights.current_state() == "red"
lights.send("timer")
assert lights.current_state() == "green"
```

### Stdlib — Import-activated keywords

The stdlib's import-activated keywords can be expressed using the same types:

```rust
// Conceptual: how the stdlib's keywords map to VocabProvider types
// (Phase 1: compiler constructs these internally from KEYWORDS table)

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

### Design validation: tracing `std.async` through the full pipeline

To verify that `VocabProvider` captures the full surface, let's trace how `async`/`await` — an existing stdlib feature — would flow through every compiler stage if expressed via the vocab system.

**Step 1: VocabProvider registration**:

```rust
KeywordRegistration {
    activation: KeywordActivation::OnImport("std.async".into()),
    keywords: vec![
        KeywordSpec::new("async", KeywordSurfaceKind::DeclarationModifier),
        KeywordSpec::new("await", KeywordSurfaceKind::PrefixExpression),
    ],
}
```

Plus the manifest contribution:

```rust
ModuleExport {
    path: "std.async".into(),
    functions: vec![
        FunctionExport {
            name: "sleep".into(),
            params: vec![("seconds".into(), TypeRef::named("float"))],
            return_type: Some(TypeRef::named("None")),
            is_async: true,  // ← critical: consumer must `await` this
        },
    ],
    types: vec![],
}
```

**Step 2: Loading into the parser**:

Consumer file contains `import std.async`. The parser's per-file `active_keywords` receives:

- `async` → `DeclarationModifier` (can precede `def`, `class`)
- `await` → `PrefixExpression` (wraps an inner expression)

**Step 3: Parsing**:

```incan
import std.async
from std.async import sleep

async def fetch_data(url: str) -> str:
    await sleep(0.5)
    response = await http_get(url)
    return response.body
```

The parser processes:

- `async` → recognized as `DeclarationModifier` → attaches to the following `def` → produces `AsyncFunctionDef` AST node
- `await sleep(0.5)` → recognized as `PrefixExpression` → wraps the call expression → produces `AwaitExpr` AST node

**Step 4: Typechecking**:

The typechecker loads `sleep` from the manifest and checks:

- `sleep` is `is_async: true` → calling it without `await` in a sync context is an error
- `sleep(0.5)` → param type `float` matches ✓
- `await sleep(0.5)` → resolves to return type `None` ✓
- `fetch_data` is declared `async` → body may contain `await` expressions ✓

**Step 5: Lowering (AST → IR)**:

- `AsyncFunctionDef` → `IrDecl::Function { is_async: true, ... }`
- `AwaitExpr(call)` → `IrExpr::Await(IrExpr::Call(...))`

**Step 6: Emission (IR → Rust)**:

- `async def fetch_data(...)` → `async fn fetch_data(...)`
- `await sleep(0.5)` → `sleep(0.5).await`

**What this walkthrough reveals**:

The vocab system captures everything needed for the parser and typechecker stages. Specifically:

| Pipeline stage |                 What vocab provides                 | Sufficient? |
| -------------- | --------------------------------------------------- | ----------- |
| Parser         | `KeywordSurfaceKind` (modifier vs prefix vs block)  | ✅ Yes      |
| Typechecker    | `FunctionExport.is_async` + signatures + types      | ✅ Yes      |
| Lowering       | N/A — lowering works from the AST, not the manifest | ✅ N/A      |
| Emission       | N/A — emission works from the IR, not the manifest  | ✅ N/A      |

**Gaps this walkthrough identified (now addressed)**:

1. **`FunctionExport.is_async`** — Without this field, the typechecker cannot validate `await` usage or warn about missing `await` on async calls. Added to the struct definition above.
2. **`TypeExport.type_params`** — Generic types like `Task[T]` need their type parameters in the manifest for the typechecker to validate generic instantiation. Added to the struct definition above.

**Gaps deferred to future work**:

- **`TypeExport.parent`** (class inheritance) — When a library exports a class that extends another, the manifest needs to carry the parent type for the typechecker to validate inherited method calls. Not needed for scope of this RFC (no known library uses inheritance in its public API).
- **`TypeExport.trait_impls`** — Which traits a type implements affects method resolution and trait-bound validation. Deferring: the typechecker can fall back to structural typing for external library types initially.
- **`FunctionExport`/`TypeExport` docstrings** — The LSP needs docstrings for hover tooltips on library symbols. Not critical for compilation, but important for developer experience. Can be added as `Option<String>` later.

### Design validation: tracing `stately` block desugaring through the pipeline

The `std.async` walkthrough above validates that `VocabProvider` captures everything for **simple keywords** (modifiers and prefix expressions). But what about **block keywords**? Let's trace how Stately's imaginary `machine {}` block flows through the compiler with `VocabDesugarer`.

**Step 1: Incan source**:

```incan
from stately.machine import machine, state, on
from stately import StateMachine

lights = machine "traffic_light" {
    state "red" {
        on "timer" -> "green"
        enter:
            activate_red_light()
    }
    state "green" {
        on "timer" -> "yellow"
    }
}
```

**Step 2: Parsing**:

The compiler loads Stately's `KeywordRegistration` and activates keywords for this file:

- `machine` → `BlockDeclaration` → parser opens a block scope, consumes `"traffic_light"` as argument
- `state` → `BlockDeclaration` → nested block scope with `"red"` as argument
- `on` → `BlockContextKeyword` → parser captures `"timer" -> "green"` as context entry arguments
- `enter` → `SubBlock` → parser opens nested sub-block, captures statements

The parser produces a `VocabBlock` AST node:

```text
VocabBlock {
    keyword: "machine",
    arguments: [StringLiteral("traffic_light")],
    body: [
        NestedBlock(VocabBlock {
            keyword: "state",
            arguments: [StringLiteral("red")],
            body: [
                ContextEntry {
                    keyword: "on",
                    arguments: ["timer", Arrow, "green"],
                    body: [],
                },
                SubBlock {
                    keyword: "enter",
                    body: [Statement(Expr(call activate_red_light()))],
                },
            ]
        }),
        NestedBlock(VocabBlock {
            keyword: "state",
            arguments: [StringLiteral("green")],
            body: [
                ContextEntry {
                    keyword: "on",
                    arguments: ["timer", Arrow, "yellow"],
                    body: [],
                },
            ]
        }),
    ]
}
```

**Step 3: Desugaring (NEW — `VocabDesugarer`)**

The compiler looks up Stately's `VocabDesugarer` and calls `desugar_block()`. The desugarer transforms the block into builder-pattern method calls:

```rust
impl VocabDesugarer for StatelyDesugarer {
    fn desugar_block(
        &self,
        block: &VocabBlock,
        _ctx: &DesugarContext,
    ) -> Result<Vec<IncanStatement>, DesugarError> {
        // Start with StateMachine::builder("traffic_light")
        let name = match &block.arguments[0] {
            IncanExpr::StringLiteral(s) => s.clone(),
            _ => return Err(DesugarError {
                message: "machine keyword requires a string name".into(),
                span: block.span,
                help: Some("e.g., machine \"my_machine\" { ... }".into()),
            }),
        };

        let mut expr = IncanExpr::FunctionCall(
            "StateMachine::builder".into(),
            vec![IncanExpr::StringLiteral(name)],
        );

        // Chain .state() and .on() calls for each sub-block
        for item in &block.body {
            match item {
                VocabBodyItem::SubBlock(sub) if sub.keyword == "state" => {
                    // ... extract state name, transitions, enter/exit actions
                    // ... chain: expr = expr.state("red").on("timer", "green").on_enter(...)
                }
                _ => {}
            }
        }

        // Finish with .build()
        expr = IncanExpr::MethodCall(Box::new(expr), "build".into(), vec![]);

        Ok(vec![IncanStatement::Expr(expr)])
    }
}
```

The desugared output replaces the `VocabBlock` in the AST. Conceptually, this transforms:

```incan
lights = machine "traffic_light" {
    state "red" { on "timer" -> "green" }
    state "green" { on "timer" -> "yellow" }
}
```

Into equivalent Incan:

```incan
lights = StateMachine.builder("traffic_light")
    .state("red").on("timer", "green")
    .state("green").on("timer", "yellow")
    .build()
```

**Step 4: Typechecking**:

After desugaring, the typechecker sees standard method chains on `StateMachine`. It validates:

- `StateMachine.builder` exists and returns a builder type ✓
- `.state("red")` → param type `str` matches ✓
- `.on("timer", "green")` → param types match ✓
- `.build()` → returns `StateMachine` ✓

The typechecker never sees `machine {}` syntax — it only sees the desugared method chains.

**Step 5–6: Lowering and emission proceed normally** — the IR and code generation deal with standard method calls, not DSL syntax.

**What this walkthrough reveals:**

`VocabDesugarer` closes the gap between parsing and typechecking for block-level DSL syntax. The compiler pipeline becomes:

```text
Source → Lexer → Parser → [Desugaring] → Typechecker → Lowering → Emission
                              ↑
                    VocabDesugarer (library-provided)
```

| Pipeline stage | Who handles it?              | Block keywords?     |
| -------------- | ---------------------------- | ------------------- |
| Parsing        | Compiler (via `KeywordSpec`) | ✅ `VocabBlock` AST |
| **Desugaring** | **Library (via desugarer)**  | ✅ → Incan AST      |
| Typechecking   | Compiler                     | ✅ Normal code      |
| Lowering       | Compiler                     | ✅ Normal IR        |
| Emission       | Compiler                     | ✅ Normal Rust      |

### Internal-first architecture: stdlib as the proving ground

A key design principle: **the stdlib uses the same `VocabProvider` / `VocabDesugarer` API as third-party libraries**. This has several benefits:

1. **Battle-tested from day one.** The stdlib exercises the API in real compiler builds before any external library exists. API gaps are discovered early.
2. **One code path.** The compiler doesn't need separate logic for "built-in keywords" vs "library keywords". All keyword loading, manifest resolution, and desugaring flows through the same mechanism.
3. **Eliminates `scan_for_*` debt.** The current ad-hoc `needs_web`, `needs_serde`, `scan_for_*` booleans can be replaced with vocab-based feature detection. If `std.async` provides a `VocabProvider`, the compiler detects async usage by checking whether that provider's keywords are activated — no special-case scanning needed.
4. **Dogfooding.** Any friction library authors experience, the stdlib authors experience first.

**Migration path:**

| Implementation phase | Stdlib keywords                                                                  | `scan_for_*` booleans                           |
| -------------------- | -------------------------------------------------------------------------------- | ----------------------------------------------- |
| Phase 1              | `StdlibVocab` mirrors existing stdlib soft keywords alongside current tables     | Kept as-is                                      |
| Phase 2              | Parser and related tooling consume stdlib registrations from the shared registry | Kept as a compatibility path                    |
| Phase 3              | Core + stdlib keyword handling is fully registry-backed                          | Still present for non-keyword feature detection |
| Phase 7              | Provider-declared dependencies and features are end-to-end                       | Removed                                         |

**Note:** Simple keywords like `async` and `assert` don't need a desugarer — the compiler handles `DeclarationModifier` and `StatementKeywordArgs` natively. The desugarer is only needed for `BlockDeclaration` keywords that introduce custom DSL syntax.

## Alternatives considered

### A. Convention functions instead of a trait

Instead of `VocabProvider`, library authors export bare functions with well-known names:

```rust
pub fn keyword_registrations() -> Vec<KeywordRegistration> { ... }
pub fn manifest() -> LibraryManifest { ... }
```

**Rejected** because: traits provide compile-time verification that all required methods exist. Convention functions can silently fail if the name is misspelled. The trait also enables future extension (default methods, associated types) without breaking existing implementations.

### B. Declarative TOML/YAML instead of Rust

Keywords and manifest declared in a static file rather than Rust code:

```toml
[keywords."routekit.routes"]
routes = "BlockDeclaration"
GET = "BlockContextKeyword"
```

**Rejected** because: this limits expressiveness (no conditional registration, no computed manifests) and adds a custom DSL to learn. Rust code is more flexible and benefits from type checking. The VocabProvider trait can be wrapped by a macro (`vocab!{}`) for the declarative common case in a future iteration.

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

- **New crate (`incan-vocab`)** — introduces all types and traits defined in this RFC (`VocabProvider`, `VocabDesugarer`, `KeywordRegistration`, `KeywordSpec`, `KeywordSurfaceKind`, `KeywordActivation`, `KeywordRegistry`, `LibraryManifest`, public AST types). Published to crates.io independently of the compiler. All other layers depend on it transitively through `incan_core`.
- **Lexer** — transitions from emitting dedicated keyword token variants to emitting `Token::Ident` for all keyword-shaped identifiers. Keyword promotion becomes entirely the parser's responsibility via registry lookup. This can be done incrementally across the internal migration phases.
- **Parser** — replaces per-token-type dispatch with a single `KeywordSurfaceKind`-driven dispatch. Gains `VocabBlock` AST node for `BlockDeclaration` keywords. Per-file `active_keywords` set replaces the narrower `active_soft_keywords` mechanism.
- **Typechecker** — later phases accept library manifest exports loaded by the consumer build flow (RFC 031) so library types become first-class during checking.
- **Lowering / IR** — later phases replace import-activated feature detection (`needs_web`, `needs_async`, etc.) with registry-driven queries against `LibraryManifest.required_dependencies`. A desugaring pass (after parsing, before typechecking) transforms `VocabBlock` nodes into ordinary Incan AST via `VocabDesugarer`.
- **Project generator** — later phases derive required Cargo dependencies from `LibraryManifest.required_dependencies` rather than hard-coded boolean flags.
- **CLI (`incan build --lib`)** — later phases build the library's vocab crate, extract `VocabProvider` output, and serialize keyword registrations and manifest metadata into the library artifact. Parsing and handling of the `[vocab]` section in `incan.toml` is required.
- **LSP** — builds and caches `KeywordRegistry` once per workspace open; rebuilds only on `incan.toml` changes or dependency updates. All keyword-dependent features (completions, diagnostics, hover, go-to-definition) consume the registry uniformly.
- **Formatter** — dispatches formatting rules via `KeywordSurfaceKind` rather than hardcoded keyword names, enabling library keywords to receive correct formatting automatically.
- **Editor grammar** — TextMate grammar for VS Code is generated from `IncanCoreVocab` and `StdlibVocab` at build time, replacing the manually maintained keyword list.

## Implementation Plan

Phases 1-3 establish the compiler's internal registry migration for core + stdlib keywords. Phases 4-7 extend the same architecture to library artifacts, external desugarers, and manifest-driven feature detection.

### Phase 1: Extract `incan-vocab` and validate registry parity

1. Create `crates/incan-vocab/` with the stable traits and data types defined in this RFC: `VocabProvider`, `VocabDesugarer`, `KeywordRegistration`, `KeywordSpec`, `KeywordSurfaceKind`, `KeywordActivation`, `KeywordSource`, `KeywordRegistry`, `KeywordEntry`, manifest types, and public AST types.
2. Add `incan-vocab` as a dependency of `incan_core` and re-export the public surface that compiler crates need.
3. Implement `IncanCoreVocab` and `StdlibVocab` as internal `VocabProvider` implementations derived from the existing core + stdlib keyword metadata.
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

### Phase 4: Add library artifact production (requires RFC 031 library build scaffolding)

1. Add `[vocab]` section parsing to `incan.toml`.
2. Implement the vocab crate build/extraction step in `incan build --lib`.
3. Serialize `VocabProvider` output and `LibraryManifest` into the library artifact.
4. Add integration tests that build a library with a vocab crate and verify metadata extraction.

### Phase 5: Add consumer loading and import/typechecker integration (requires Phase 4 and RFC 031 consumer loading)

1. Implement vocab metadata deserialization during consumer builds.
2. Merge imported library keywords into `KeywordRegistry` activation with `KeywordActivation::OnImport`.
3. Wire deserialized `LibraryManifest` data into import resolution and typechecker symbol loading.
4. Add integration tests that import a library, activate its keywords, and typecheck against its manifest exports.

### Phase 6: Add the desugaring pipeline for block DSLs (requires library artifact transport)

1. Add a desugaring pass to the compiler pipeline after parsing and before typechecking.
2. For each `VocabBlock` AST node, load the corresponding `VocabDesugarer`, call `desugar_block()`, and replace the block with the returned `Vec<IncanStatement>`.
3. Map the public `IncanExpr` / `IncanStatement` types back to the compiler's internal AST types.
4. Add success-path and error-path integration tests for block desugaring.

### Phase 7: Remove feature scanning debt once provider metadata is end-to-end

1. Replace `needs_serde` / `needs_tokio` / `needs_web` booleans with provider-declared features and dependencies derived from registry + manifest data.
2. Remove `scan_for_*` methods from `IrCodegen` once manifest-driven feature detection is fully wired through builds and consumers.
3. Replace `ProjectGenerator` boolean fields with collected feature/dependency sets derived from the loaded providers.

### Compiler touchpoints

| Component                                | Change                                                                         |
| ---------------------------------------- | ------------------------------------------------------------------------------ |
| `crates/incan-vocab/`                    | New trait/type crate defined by this RFC                                       |
| `crates/incan_core/src/lang/keywords.rs` | Phase 1 parity source for `IncanCoreVocab` / `StdlibVocab` extraction          |
| `crates/incan_syntax/src/parser/`        | Active keyword tracking, registry dispatch, and `VocabBlock` parsing           |
| `crates/incan_syntax/src/lexer/`         | Transitional then full `Token::Ident` migration for keyword-shaped words       |
| `src/manifest.rs`                        | Later-phase `[vocab]` parsing and manifest integration                         |
| `src/cli/commands/build.rs`              | Later-phase `incan build --lib` extraction and consumer loading hooks          |
| `src/frontend/typechecker/`              | Later-phase manifest-backed import resolution and symbol loading               |
| `src/backend/ir/codegen.rs`              | Phase 7 removal of `scan_for_*` in favor of provider-declared feature metadata |
| `src/backend/project/`                   | Replace `needs_*` booleans with collected feature/dependency sets              |
| `src/lsp/backend.rs`                     | Registry-backed keyword completions and cache lifecycle                        |
| `src/format/`                            | Registry-backed formatting dispatch                                            |
| `editors/vscode/`                        | Build-time TextMate grammar generation from core + stdlib providers            |

## Progress Checklist

- First PR: ... covers phase 1-3. etc etc

### Phase 1: Registry extraction and parity

- [x] Create `crates/incan-vocab/` with the core registry, manifest, and public AST types defined by this RFC.
- [x] Implement `IncanCoreVocab` and `StdlibVocab` as internal providers.
- [x] Build `KeywordRegistry` alongside the existing keyword tables and add parity checks/tests.

### Phase 2: Parser activation and transitional dispatch

- [x] Replace `active_soft_keywords` with registry-backed active keyword tracking.
- [x] Route parser dispatch through `KeywordRegistry` + `KeywordSurfaceKind` while retaining transitional compatibility with existing keyword token forms.
- [x] Add or adjust AST support for registry-driven surfaces and validate parser behavior with targeted tests.

### Phase 3: Full internal compiler and tooling migration

- [x] Migrate the lexer toward `Token::Ident` for keyword-shaped words and remove legacy keyword-specific dispatch once parity is proven.
- [x] Move LSP keyword consumers to core + stdlib provider output.
- [x] Move formatter dispatch to core + stdlib provider output.
- [x] Move static grammar generation to core + stdlib provider output.
- [x] Remove direct compiler/tooling dependence on the old `KEYWORDS` table; keep `keywords.rs` metadata as a compatibility/parity source until RFC 031+ artifact-based vocab loading is end-to-end.

Phase 3 closure inventory:

- Migrated now (compiler/tooling paths): parser activation/dispatch, lexer keyword tokenization, LSP keyword completion, formatter surface-keyword rendering, VS Code keyword grammar patterns.
- Compatibility retained intentionally (non-primary source-of-truth): `incan_core::lang::keywords` metadata table for parity tests and generated language-reference docs, plus adapter scaffolding until external library vocab manifests land via RFC 031.

### Phase 4: Library artifact production (requires RFC 031 library build scaffolding)

- [ ] Parse `[vocab]` in `incan.toml`.
- [ ] Add vocab crate build and metadata extraction to `incan build --lib`.
- [ ] Serialize `VocabProvider` output and `LibraryManifest` into the library artifact.

### Phase 5: Consumer loading and typechecker integration (requires Phase 4 and RFC 031 consumer loading)

- [ ] Deserialize library vocab metadata during consumer builds.
- [ ] Merge imported library keywords into `KeywordRegistry` activation.
- [ ] Wire `LibraryManifest` into import resolution and typechecker symbol loading.

### Phase 6: Desugaring pipeline (requires library artifact transport)

- [ ] Add a post-parse, pre-typecheck desugaring pass for `VocabBlock`.
- [ ] Load and execute `VocabDesugarer` for block DSLs, then map public AST types back to compiler AST.
- [ ] Add success-path and error-path integration tests for block desugaring.

### Phase 7: Feature detection cleanup

- [ ] Replace `needs_*` booleans with provider-declared feature and dependency data.
- [ ] Remove `scan_for_*` methods from `IrCodegen` once the manifest-driven path is end-to-end.
- [ ] Update project generation to consume collected feature and dependency sets instead of hard-coded booleans.
- [ ] Publish `incan-vocab` v0.1.0 once the public API is stable.

## Design decisions

### DD-1: Build-script extraction for metadata; WASM for desugarers

`VocabProvider` output (keyword registrations + manifest) is serialized to JSON during `incan build --lib` and bundled into the library artifact. The consumer compiler deserializes it at build time — no dynamic linking needed for metadata.

`VocabDesugarer` implementations are compiled Rust code. During the internal compiler migration, desugarers live inside the compiler binary. For external libraries (Phase 4+), desugarers are compiled as WASM modules and loaded via a sandboxed runtime (`wasmtime`). WASM is portable (no platform-specific `cdylib`), sandboxed (can't access the filesystem or network), and deterministic.

This also resolves the desugarer loading mechanism — `cdylib` + `libloading` is rejected in favor of WASM.

### DD-2: `KeywordSpec::new` convenience constructors *(pre-resolved)*

`KeywordSpec::new(name, kind)` / `KeywordSpec::compound(name, rest, kind)` are the standard API for top-level keywords. `KeywordSpec::in_block(name, kind, parents)` / `KeywordSpec::compound_in_block(name, rest, kind, parents)` are the standard API for parent-scoped keywords. Direct struct construction is not recommended.

### DD-3: No macro sugar initially

The `VocabProvider` trait is explicit and debuggable. A `vocab!{}` macro can be added as a non-breaking convenience in a future minor version once real-world usage patterns stabilize across 3+ libraries. Premature abstraction over a 2-method trait is not justified.

### DD-4: Explicit vocab crate path in `incan.toml`

`[vocab].crate` in `incan.toml` is required. Auto-discovery of `crates/*-vocab/` directories is magical, breaks when directory structure varies, and makes it harder to reason about what the compiler will load. Explicit declaration is one line and leaves no ambiguity. Convention-based discovery could be added later as sugar.

### DD-5: One VocabProvider per namespace; stdlib in one crate with scoped modules; load on demand

Each independently-activatable namespace has its own `VocabProvider` implementation. For the stdlib, all providers live together in `crates/incan_stdlib/` as separate modules under `src/vocab/` — one struct per namespace (`StdAsyncVocab`, `StdTestingVocab`, etc.). The crate provides a `provider_for(namespace)` lookup function. The compiler loads only the providers for namespaces the project actually imports — if no file imports `std.async`, the `async`/`await` keywords never enter the registry.

This keeps the stdlib as a single distributable unit (one crate to compile, install, and version) while maintaining per-namespace loading at the API level. External libraries use the subcrate pattern (`crates/<name>-vocab/`) because they are independently distributed. The `VocabProvider` trait doesn't care where the implementation lives — same trait, same registry, same loading interface. The only difference between stdlib and external providers is transport (compiled-in vs deserialized JSON), not architecture.

### DD-6: Vocab crate is a regular Cargo workspace member

No special treatment. The library's `crates/<name>-vocab/` directory is listed in the workspace `Cargo.toml`'s `[workspace].members`. The compiler builds it via `cargo build -p <name>-vocab`. This is standard Rust workspace practice and requires zero custom tooling.

### DD-7: Serde derives behind a feature flag (enabled by default)

`incan-vocab` types derive `Serialize`/`Deserialize` when the `serde` feature is active. The compiler enables this feature. Library authors' vocab crates don't need serde directly — they construct types in Rust code and the compiler serializes them. The feature flag keeps `serde` optional for any hypothetical use of the crate that doesn't need serialization.

### DD-8: `IncanExpr`/`IncanStatement` completeness *(pre-resolved)*

`IncanStatement` has been expanded with `ForLoop`, `IfElse`, `WhileLoop`, `MatchBlock`, and `TryExcept` variants. Together with the 14 `IncanExpr` variants, desugarers can emit non-trivial control flow without falling back to `Passthrough`. If further gaps emerge (e.g., `with` blocks, comprehensions), they can be added incrementally.

### DD-9: AST→AST desugaring only; IR-level desugaring out of scope

`VocabDesugarer` produces Incan AST (`IncanExpr`, `IncanStatement`), which the compiler then lowers to IR through the standard pipeline. Emitting IR directly would couple library code to unstable compiler internals and bypass typechecking — both unacceptable. If AST desugaring becomes a bottleneck, the compiler's AST→IR lowering is the place to optimize. The trait signature is stable: `VocabBlock → Vec<IncanStatement>`.

### DD-10: CLI desugars every build; LSP caches on block content hash

For batch compilation (`incan build/run`), desugaring runs once per build — no caching needed. For the LSP, desugared AST is cached per-block using a content hash of the `VocabBlock`. When the user edits inside a DSL block, the hash changes and the block is re-desugared. When the user edits outside the block, the cached result is reused. This plugs into the LSP's existing incremental analysis infrastructure rather than inventing a separate cache.

### DD-11: `IncanCoreVocab` implements `VocabProvider`

A lightweight `impl VocabProvider for IncanCoreVocab` returns hardcoded keyword registrations with `KeywordActivation::Always`. This means the registry's `load()` method is the only entry point for keywords — core, stdlib, and external all flow through the same function. The implementation is ~50 lines of `vec![...]` construction. It can never be swapped or loaded dynamically, but that's fine — the value is architectural consistency, not pluggability.

### DD-12: Parent-qualified uniqueness with additive extension points

Top-level keywords remain globally unique. Two providers may not register the same top-level keyword name — the registry rejects duplicates at load time with a diagnostic naming both sources ("keyword `routes` is already registered by `routekit`; `my-lib` cannot re-register it").

Parent-scoped keywords are unique per immediate parent block. The same keyword text may appear under different parents, but the same `(name, immediate_parent, surface_kind)` combination may not be registered twice. This enables extension libraries to add new sub-forms to existing DSL blocks (e.g., `routekit-graphql` adding `QUERY`/`MUTATION` context keywords to routekit's `routes` block) without re-registering the block keyword itself, while still preventing ambiguous parser behavior.

If a genuine need for keyword override/shadowing emerges in the future, it can be added as an opt-in mechanism (`allow_override: true` on the registration) — but we don't build that until we have a real use case.

### DD-13: Registry is a plain struct; consumers wrap as needed

`KeywordRegistry` is built once and then immutable. The compiler passes `&KeywordRegistry` through the pipeline. The LSP wraps it in `Arc<KeywordRegistry>` for sharing across concurrent file analyses. Embedding `Arc` inside the type would force an allocation even for single-threaded CLI usage and obscure the ownership model. The type is `Send + Sync` by construction (all fields are owned, immutable data). A type alias `pub type SharedRegistry = Arc<KeywordRegistry>` can live in the LSP module for convenience.

### DD-14: Registry cached in memory; rebuilt on config change

**CLI**: The registry is built once at the start of each invocation from compiled-in stdlib providers + deserialized external manifests, then passed by reference through the pipeline. The cost is dominated by JSON deserialization of external manifests (stdlib is zero-cost — compiled-in). For typical projects (0–5 external deps), this is expected to be sub-millisecond, but we benchmark during Phase 2 implementation and add a registry cache file (alongside the lockfile) if it exceeds 10ms.

**LSP**: The registry is built once on workspace open and cached as `Arc<KeywordRegistry>`. It rebuilds only when `incan.toml` changes or a dependency artifact is modified (file-system watcher). Between rebuilds, all concurrent file analyses share the same `Arc`. The LSP never reads manifests from disk during normal editing — only on workspace-level config changes.

### DD-15: Each VocabProvider declares its own Cargo dependencies via manifest

Two new fields on `LibraryManifest`:

```rust
pub required_dependencies: Vec<CargoDependency>,
pub required_stdlib_features: Vec<String>,
```

Each `stdlib` namespace provider declares only its own dependencies (e.g., `StdSerdeVocab` declares `serde 1.0` + `serde_json 1.0` and stdlib feature `"json"`; `StdWebVocab` declares `axum 0.8` + `inventory 0.3` and `stdlib` feature `"web"`). When the compiler builds the generated project's `Cargo.toml`, it collects `required_dependencies` from all loaded providers, deduplicates by crate name (version conflicts are an error), and forwards the merged set to `ProjectGenerator`.

This replaces both the `scan_for_*` booleans and the `STDLIB_NAMESPACES.extra_crate_deps` mechanism with a single, uniform, per-provider declaration. If the user imports `std.serde` and `std.web`, the compiler merges both sets of deps. If they only import `std.async`, they don't pay for `axum`.

### DD-16: Registry declares valid decorator names; desugarer validates semantics

`KeywordRegistration` gets an optional `valid_decorators: Vec<String>` field. When the parser encounters a decorated vocab block, it checks the decorator name against this list and emits a diagnostic if it doesn't match ("decorator `@cache` is not valid on `routes` blocks; valid decorators: `@auth`, `@middleware`"). This enables IDE completion inside vocab blocks without loading the desugarer.

The desugarer is responsible for semantic validation of decorator *arguments* — the registry only validates that the decorator name is recognized. For keywords without the field (or empty list), no decorator validation is performed at the registry level.

### DD-17: Parent placement is explicit in the registry, not inferred from names

The registry must carry parent-block information as first-class data (`KeywordPlacement`) rather than relying on naming conventions or desugarer-side interpretation. This keeps parsing, formatting, and LSP completion aligned: all three can answer "is this keyword valid here?" without loading library-specific logic.

This also keeps nested block declarations honest. A keyword like `state` is still a `BlockDeclaration` syntactically; it does not become a `SubBlock` just because it appears inside another block. The placement tells the parser that it is nested under `machine`, while the surface kind preserves its declaration-like shape.

### DD-18: Public vocab AST is recursive and preserves nested bodies

`VocabBodyItem` is recursive: it can contain `NestedBlock(VocabBlock)`, and both `VocabContextEntry` and `VocabSubBlock` carry `Vec<VocabBodyItem>` bodies. This preserves the full user-written structure for desugarers instead of collapsing nested forms into ad-hoc placeholders.

The recursion is deliberate. Library DSLs often need mixtures of inline entries, nested declarations, named sub-blocks, and ordinary Incan statements in one tree. Flattening that structure too early would force desugarers to reconstruct intent from lossy parser output.

## Scope boundary: operator and glyph semantics

This RFC covers **keyword** registration, **explicit DSL block structure**, **scoped functions**, and **block-level desugaring**. It does not define the global meaning of operators such as `+`, `>>`, `@`, `|>`, or `<|`; that ordinary operator surface belongs to RFC 028.

However, explicit DSL blocks may also reuse registered glyphs with **block-owned, position-scoped** meaning. Examples include `>>`, `|>`, `->`, `<-`, `+`, or future binding-like glyphs such as `:=`. That scoped-glyph mechanism builds on the vocab/block system defined here because activation, placement, parsing context, and desugaring all depend on the enclosing block registration, but its exact resolution rules and AST contracts are specified separately in RFC 040.

In other words:

- RFC 027 defines how a library introduces an explicit block and desugars it.
- RFC 028 defines ordinary global operator overloading.
- RFC 040 defines how an explicit DSL block may own block-local glyph surfaces without implying global operator support.

Imports alone do not change the meaning of `a >> b` or `a |> b` in ordinary code. Only an explicit registered block and its eligible DSL positions may activate a scoped glyph surface positively, though an activating file/module may also gain targeted misuse diagnostics for that glyph family as specified in RFC 040.

<!-- The "Design decisions" section above was renamed from "Unresolved questions" once all open questions were resolved. If new unresolved questions arise during implementation, add an "Unresolved questions" section above "Design decisions" and move resolved items back down after resolution. -->
