# Author Library DSLs with `incan_vocab`

This guide is for library authors who want to ship import-activated DSL syntax such as `route`, `GET`, or `middleware:` without changing the core Incan compiler.

Use this path when the syntax belongs to one library and should only become active after importing that library. If you are changing the language itself, follow [Extending the language](extending_language.md) instead.

## The public contract

A vocab companion crate is a small Rust crate that lives next to your Incan library and exports one canonical Rust entrypoint:

```rust
pub fn library_vocab() -> VocabRegistration
```

That registration is the source of truth for three things:

- activated DSL surfaces
- machine-readable library metadata
- an optional Rust desugarer

The intended author-facing surface is:

- `VocabRegistration`
- `DslSurface`
- `DeclarationSurface`
- `ClauseSurface`
- `LibraryManifest`
- `VocabDesugarer`
- `VocabSyntaxNode`
- `DesugarOutput`

`KeywordRegistration` and `VocabMetadata` still exist, but they are lower-level transport and escape-hatch types. They are not the standard starting point for companion-crate authoring.

## When to use this path

- Use a vocab companion crate when your library wants import-activated DSL syntax.
- Use a plain library API when ordinary functions, models, or classes are enough.
- Use the compiler contributor path only when the feature should become part of Incan itself.

## Recommended layout

This is what the recommended layout looks like for an imaginary library called `routekit`:

```text
routekit/
├── incan.toml
├── src/
│   └── lib.incn
└── vocab_companion/
    ├── Cargo.toml
    └── src/
        ├── desugar.rs
        └── lib.rs
```

`src/lib.incn` is your actual Incan library. `vocab_companion/` is the Rust crate that describes its DSL surface.

## 1. Point `incan.toml` at the companion crate

Add a `[vocab]` section to the library project:

```toml title="routekit/incan.toml"
[project]
name = "routekit"
version = "0.1.0"

[vocab]
crate = "vocab_companion"
```

`[vocab].crate` is a path to the companion crate directory, relative to the project root unless you make it absolute.

## 2. Create the companion crate

Start with a normal Rust library crate:

```toml title="routekit/vocab_companion/Cargo.toml"
[package]
name = "routekit_vocab_companion"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
crate-type = ["rlib", "cdylib"]

[dependencies]
incan_vocab = "0.3"
```

Keep the companion crate as a real Rust crate with `Cargo.toml` and `src/lib.rs`, even when the DSL description itself is quite small.

Both crate types are intentional:

- `rlib` keeps the crate usable as an ordinary Rust library during extraction, so the compiler-owned helper can call `library_vocab()` directly and serialize the resulting metadata.
- `cdylib` produces the packaged WASM artifact that the consumer compiler can execute later when it needs to desugar imported DSL nodes.

The generated `.incnlib` manifest is Incan's library artifact, but it is not a Rust compilation target. It records the derived metadata plus references to packaged outputs such as the desugarer WASM module. We still need Cargo to build the Rust companion crate itself.

## 3. Describe the DSL in `library_vocab()`

Put the registration in `src/lib.rs`:

```rust title="routekit/vocab_companion/src/lib.rs"
mod desugar;

use incan_vocab::{ClauseSurface, DeclarationSurface, DslSurface, LibraryManifest, VocabRegistration};

pub use desugar::RoutekitDesugarer;

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

Key rules:

- `DslSurface::on_import("routekit")` must match the consumer-facing import spelling after `pub::`.
- Declarations own their clause grammar directly, so nested DSL structure stays close to the declaration that introduces it.
- Use `DeclarationSurface::desugars_to_expression()` when the DSL declaration produces a value. Expression-desugaring declarations can be used in ordinary expression positions such as assignment values and returns, and the compiler desugars them before typechecking.
- Use `ClauseSurface::expr_list("SELECT")` for SQL-shaped projection clauses that accept entries such as `sum(amount) as total`; add declared item modifiers with `ExpressionItemModifierSurface::expr("for")` or similar when a projection item needs metadata such as `sum(amount) for customer with context`. The desugarer receives structured expression-list items with alias and modifier metadata, while `ClauseSurface::fields(...)` remains for config-style `name = value` sections.
- `LibraryManifest` is where you describe exported module metadata plus any Cargo dependencies or stdlib features that must travel with the library artifact.
- `KeywordRegistration` remains available only as a lower-level escape hatch for especially simple or incremental cases.

## 4. Add scoped surface forms

Scoped DSL surface forms can be registered alongside the declaration that owns them. Use scoped surfaces when a glyph or expression shape should have meaning only inside an explicit DSL block, while remaining ordinary syntax or an error elsewhere:

Start with the consumer syntax you want to enable:

```incan
from pub::querykit import querykit_name

def main() -> None:
    query:
        .amount > 100
        .customer_id
        orders |> paid_orders
        orders.filter(.status == "paid").select(.region)
```

That surface has four distinct jobs:

- `query:` introduces the owning DSL block.
- `.amount` and `.customer_id` are expression-form surfaces owned by the `query:` block body.
- `orders |> paid_orders` is an operator-like surface owned by the same block body.
- `.status` and `.region` are expression-form surfaces owned by query method arguments, not by every method call in the file.

The registration describes those jobs directly:

| Consumer surface           | Descriptor shape                                           | Eligibility                           | Receiver               |
| -------------------------- | ---------------------------------------------------------- | ------------------------------------- | ---------------------- |
| `query:`                   | `DeclarationSurface::named("query")`                       | import-activated by `pub::querykit`   | none                   |
| `.amount`                  | `ScopedSurfaceDescriptor::leading_dot_path("query.field")` | `in_declaration_body("query")`        | owning declaration     |
| `orders |> paid_orders`    | `ScopedSurfaceDescriptor::operator("query.pipe", "|>")`    | `in_declaration_body("query")`        | none                   |
| `.status` in `filter(...)` | `leading_dot_path("query.method_field")`                   | `in_call_argument("query", "filter")` | custom method receiver |

The descriptor key is intentionally separate from the glyph or source text. The key is the stable identity that later compiler phases and the desugarer see. For example, `query.pipe` can use `|>` today and still remain a stable semantic concept if the library later adds aliases or richer validation.

Once accepted, the compiler hands the desugarer typed payloads rather than raw source text:

```text
.amount
  descriptor_key: query.field
  payload: leading-dot path ["amount"]

orders |> paid_orders
  descriptor_key: query.pipe
  payload: scoped glyph "|>" with left and right expression operands

.status inside filter(...)
  descriptor_key: query.method_field
  payload: leading-dot path ["status"]
```

This is the point of RFC 040: the DSL author registers where a surface is legal, the parser preserves what it means, and the desugarer consumes structured artifacts instead of guessing by string matching.

Here is the matching companion-crate registration:

```rust
use incan_vocab::{
    DeclarationSurface, DslSurface, ScopedSurfaceDescriptor, ScopedSurfaceDiagnosticKind,
    ScopedSurfaceDiagnosticTemplate, ScopedSurfaceEligibility, ScopedSurfaceMisuseScope, ScopedSurfaceReceiver,
    VocabRegistration,
};

pub fn library_vocab() -> VocabRegistration {
    VocabRegistration::new().with_surface(
        DslSurface::on_import("querykit")
            .with_declaration(
                DeclarationSurface::named("query")
                    .with_statement_body(),
            )
            .with_scoped_surface(
                ScopedSurfaceDescriptor::operator("query.pipe", "|>")
                    .in_declaration_body("query")
                    .pairwise_chain(),
            )
            .with_scoped_surface(
                ScopedSurfaceDescriptor::leading_dot_path("query.field")
                    .in_declaration_body("query")
                    .with_receiver(ScopedSurfaceReceiver::OwningDeclaration)
                    .with_misuse_scope(ScopedSurfaceMisuseScope::ActivatingFile)
                    .with_diagnostic(
                        ScopedSurfaceDiagnosticTemplate::new(
                            "query-field-outside-scope",
                            ScopedSurfaceDiagnosticKind::OutsideScope,
                            "query field shorthand is only valid inside query blocks",
                        )
                        .with_help("move this expression into a `query:` block"),
                    ),
            ),
            .with_scoped_surface(
                ScopedSurfaceDescriptor::leading_dot_path("query.method_field")
                    .with_eligibilities([
                        ScopedSurfaceEligibility::call_argument("query", "filter"),
                        ScopedSurfaceEligibility::call_argument("query", "select"),
                    ])
                    .with_receiver(ScopedSurfaceReceiver::custom("method-receiver")),
            ),
    )
}
```

The descriptor `key` must be stable. The compiler preserves it on accepted surface artifacts and uses it for diagnostics, formatter metadata, and desugarer handoff. Expression-form descriptors such as leading-dot paths must declare receiver derivation; operator-like glyph descriptors can expose formatter hints such as `pairwise_chain()`. RFC 040 supports selected descriptor-gated non-core glyphs such as `|>`, `%>%`, `:=`, and `===`. Language-shaped lexical submodes -- markup, style, raw text, regex/template literals, and type-position syntax -- are RFC 081 work, covered next.

## 5. Add scoped symbols

Scoped symbols are identifier calls owned by a DSL position. They are useful when a DSL needs concise names such as `sum(...)` or `count(...)` without changing ordinary Incan resolution in the rest of the file.

Inside an eligible DSL position, a matching scoped symbol descriptor is the local meaning. It wins over lexical names, imports, module names, and builtin fallback, the same way an inner variable binding wins over an outer binding. Outside the owning DSL scope, the same spelling remains an ordinary call.

```incan
from pub::querykit import querykit_name

def main(values: list[int]) -> int:
    query:
        sum(.amount)                 # querykit's scoped symbol
        std.builtins.sum(values)     # explicit core builtin escape

    return sum(values)               # ordinary Incan call resolution
```

Register scoped symbols on the same `DslSurface` as the declaration that owns them:

```rust
use incan_vocab::{
    ClauseSurface, DeclarationSurface, DslSurface, ScopedSymbolDescriptor,
    ScopedSymbolDiagnosticKind, ScopedSymbolDiagnosticTemplate, ScopedSymbolMisuseScope,
    VocabRegistration,
};

pub fn library_vocab() -> VocabRegistration {
    VocabRegistration::new().with_surface(
        DslSurface::on_import("querykit")
            .with_declaration(
                DeclarationSurface::named("query")
                    .with_clause(ClauseSurface::expr("SELECT")),
            )
            .with_scoped_symbol(
                ScopedSymbolDescriptor::aggregate("query.sum", "sum")
                    .in_clause_body("query", "SELECT")
                    .with_misuse_scope(ScopedSymbolMisuseScope::ActiveDsl)
                    .with_diagnostic(
                        ScopedSymbolDiagnosticTemplate::new(
                            "query-sum-outside-select",
                            ScopedSymbolDiagnosticKind::OutsideEligiblePosition,
                            "query aggregate `sum` is only valid inside SELECT clauses",
                        )
                        .with_help("move `sum(...)` into a SELECT clause"),
                    ),
            ),
    )
}
```

Use `ScopedSymbolMisuseScope::ActiveDsl` only when the spelling is a strong signal of DSL intent inside the owning DSL. That gives authors targeted diagnostics for "active DSL, wrong position" while preserving ordinary Incan behavior outside the DSL scope.

If your desugared output needs extra runtime requirements, declare them in `LibraryManifest`:

```rust
use incan_vocab::{CargoDependency, CargoDependencySource, LibraryManifest};

let manifest = LibraryManifest {
    required_dependencies: vec![CargoDependency {
        crate_name: "axum".to_string(),
        source: CargoDependencySource::Version("0.8".to_string()),
    }],
    required_stdlib_features: vec!["web".to_string()],
    ..LibraryManifest::default()
};
```

If your desugarer needs to call a library helper such as `filter`, bind that helper explicitly instead of hard-coding a bare name:

```rust
use incan_vocab::{HelperBinding, LibraryManifest};

let manifest = LibraryManifest {
    helper_bindings: vec![HelperBinding {
        key: "filter".to_string(),
        exported_name: "filter".to_string(),
    }],
    ..LibraryManifest::default()
};
```

Then the desugarer can emit `IncanExpr::Helper("filter".to_string())`, and the compiler will inject a hidden `pub::` import for the matching library export before lowering the desugared code back into the host AST.

`incan build --lib` validates these bindings structurally:

- each helper `key` must be unique within `helper_bindings`
- each `exported_name` must point at a real public export from the library artifact
- empty keys or export names are rejected before the `.incnlib` artifact is written

## 6. Add descriptor-gated embedded fragments (RFC 081)

Scoped surfaces (section 4) reinterpret ordinary Incan tokens; embedded fragments are for content that looks like an entirely different language -- markup, style rules, regex/template literals, and type-shaped syntax that has no faithful spelling as ordinary Incan operators and identifiers. RFC 081 defines a **fixed, compiler-owned catalog** of six lexical submodes. A descriptor selects which submode kind it claims for which position; it never authors its own grammar, and the compiler never claims compatibility with any real external language. Unrecognized syntax inside a claimed position is always a parse error, never a silent reinterpretation.

Start with the consumer syntax you want to enable:

```incan
from pub::webkit import webkit_name

def render_card(title: str) -> None:
    html:
        <section class="card">
            <h1>{title}</h1>
        </section>
```

`html:` introduces the owning DSL block; everything inside it is the `Markup` submode's grammar, not ordinary Incan syntax reinterpreted after the fact. `{title}` is an expression hole: it re-enters ordinary Incan parsing and typechecks `title` as the real parameter it is.

### The submode catalog

Each submode accepts exactly the constructs listed here -- nothing more. This is the entire accepted surface; do not describe any of these as "CSS-compatible," "HTML-compatible," or similar in your own library's documentation, since the mechanism makes no such guarantee.

| Submode                     | Accepts                                                                                                                 | Does not accept                                                    |
| ---------------------------- | ------------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------- |
| `Markup`                    | Open/close and self-closing tags, attributes (`name`, `name="literal"`, `name={expr}`), text runs, `&name;` entity references, `<!-- ... -->` comments, `{expr}` holes | Namespaces, doctype/processing instructions, unquoted attribute values |
| `Style`                     | Comma-separated selector lists (captured as flat token runs, not parsed further), a `{ property: value; ... }` declaration block, `--custom-property` declarations, `/* ... */` comments | Nested rules, at-rules (`@media`, ...), selector combinator structure  |
| `RawText`                   | Verbatim text runs interleaved with `{expr}` holes, and nothing else                                                    | Any structural parsing beyond hole recognition                       |
| `RegexTemplate`             | Exactly one of: a bare `/pattern/flags` regex literal, or a `` `...${expr}...` `` template string with holes           | Mixing both forms in one fragment; regex flags beyond ASCII letters  |
| `SelectorDeclarationValue`  | Exactly one value token: dimension (`16px`), color (`#1166ff`), `var(--name)` reference, identifier, string, number, or `{expr}` hole | Multiple values, arithmetic between values                           |
| `TypePosition`              | `name`, `a.b.Name` qualified names, `Name<Arg, ...>` generics, `T?` nullable, `T[]` array, `A \| B` union                | Bounds, variance, wildcards, function types                          |

Register the submode with `EmbeddedFragmentDescriptor`:

```rust
use incan_vocab::{
    DeclarationSurface, DslSurface, EmbeddedFragmentDescriptor, EmbeddedFragmentSubmode, LibraryManifest,
    VocabRegistration,
};

pub fn library_vocab() -> VocabRegistration {
    VocabRegistration::new()
        .with_surface(
            DslSurface::on_import("webkit")
                .with_declaration(DeclarationSurface::named("html").with_statement_body())
                .with_embedded_fragment(
                    EmbeddedFragmentDescriptor::new("html.fragment", EmbeddedFragmentSubmode::Markup, "markup_nodes")
                        .in_declaration_body("html"),
                ),
        )
        .with_library_manifest(LibraryManifest::default())
}
```

`EmbeddedFragmentDescriptor::new(key, submode, artifact_key)` takes the same stable `key` convention as scoped surfaces, the fixed `EmbeddedFragmentSubmode` variant it claims, and an `artifact_key` naming the typed-artifact contribution your desugarer or lowering hook should expect. Eligibility (`in_declaration_body`, `in_clause_body`, `in_call_argument`) reuses the same `ScopedSurfaceEligibility` builder methods scoped surfaces use, but only `in_declaration_body` is currently wired into embedded-fragment parsing -- registering `in_clause_body`/`in_call_argument` on an embedded-fragment descriptor compiles but never activates today. If two same-depth descriptors claim the same submode for the same position, the compiler rejects the combination as ambiguous rather than guessing which one wins.

### What the compiler hands back

Accepted fragments are never rediscovered by matching raw source text later. The parser produces a typed `EmbeddedFragmentExpr` artifact: the claimed submode, a structural node tree in source order, and the fragment's verbatim source text (for tooling that needs the untouched original). Every expression hole inside that tree is typechecked and lowered exactly as if it appeared in ordinary Incan expression position -- a hole referencing an undeclared name, or one whose type does not fit its surrounding use, is a real type error, not something a downstream desugarer discovers later.

```text
<h1>{title}</h1>
  submode: Markup
  nodes: [Element { name: "h1", children: [Hole(title: str)] }]
```

The DSL-owned structural content itself (tag names, selectors, declaration properties, regex patterns, type shapes) carries no ordinary Incan type or runtime meaning on its own -- RFC 081 assigns that to the owning DSL's desugarer or lowering hook, which is a separate step from the mechanism this section describes. A fragment with no lowering hook registered still parses, typechecks, and reaches Body IR successfully; only Rust code generation refuses, with a message naming exactly what is missing, never a silent guess.

### Formatter and layout sensitivity

If your fragment's layout should never be reformatted, mark it explicitly:

```rust
EmbeddedFragmentDescriptor::new("html.fragment", EmbeddedFragmentSubmode::Markup, "markup_nodes")
    .in_declaration_body("html")
    .layout_sensitive()
```

The formatter has exactly two states for an embedded fragment: format it structurally from the typed artifact, or preserve its original source layout verbatim. `layout_sensitive()` requests the second -- this is a stable, load-bearing signal for that future formatter work, but the formatter itself does not consume it yet. Today the formatter always writes an embedded fragment's verbatim source text regardless of this flag; `layout_sensitive()` has no observable effect until the structural-formatting path lands.

## 7. Add an optional desugarer

Parser activation alone teaches the compiler how to recognize your DSL surface. If the DSL needs custom lowering, register a Rust desugarer from the same `library_vocab()` bundle.

```rust title="routekit/vocab_companion/src/desugar.rs"
use incan_vocab::{DesugarError, DesugarOutput, IncanExpr, IncanStatement, VocabDesugarer, VocabSyntaxNode};

pub struct RoutekitDesugarer;

impl VocabDesugarer for RoutekitDesugarer {
    fn desugar(&self, node: &VocabSyntaxNode) -> Result<DesugarOutput, DesugarError> {
        let keyword = match node {
            VocabSyntaxNode::Declaration(decl) => &decl.keyword,
            _ => return Err(DesugarError::new("routekit desugarer expected a declaration node")),
        };

        Ok(DesugarOutput::Statements(vec![IncanStatement::Expr(IncanExpr::Call {
            callee: Box::new(IncanExpr::Name("print".to_string())),
            args: vec![IncanExpr::Str(format!("{keyword} block desugared"))],
        })]))
    }
}
```

Use `DesugarOutput::Statements(...)` when the DSL lowers into host statements and `DesugarOutput::Expression(...)` when it lowers into an expression position.

If you need non-default packaging metadata, register the desugarer with `with_desugarer_registration(...)` and override fields on `DesugarerRegistration` or `DesugarerMetadata`. The default packaging profile targets `wasm32-wasip1` in `release` mode.

When you package a desugarer locally, make sure your Rust toolchain has that target installed:

```bash
rustup target add wasm32-wasip1
```

CI jobs that install Incan through the repository's `install-incan` GitHub Action get `wasm32-wasip1` by default, so downstream vocab consumers do not need a separate `rustup target add wasm32-wasip1` step just to run `incan fmt --check`, `incan test`, or `incan build --lib`.

Also export the standard WASM bridge symbols from your companion crate root:

```rust title="routekit/vocab_companion/src/lib.rs"
incan_vocab::export_wasm_desugarer!(RoutekitDesugarer);
```

This emits the `desugar_block` entrypoint and required `__incan_*` memory globals consumed by the compiler runtime.

`incan build --lib` also validates the packaged WASM artifact against the canonical ABI before it is recorded in the library artifact. In practice that means the module must export:

- the standard linear memory export `memory`
- the configured entrypoint, usually `desugar_block() -> i32`
- the required initializer `__incan_init_desugarer()`
- the canonical `__incan_*` runtime cell globals

Malformed artifact paths, invalid checksums, or missing ABI exports fail the producer build early instead of surfacing later in consumer projects.

## 8. Build the library artifact

Run library mode from the Incan project root:

```bash
incan build --lib
```

This requires `src/lib.incn`. During the build, Incan:

1. reads `[vocab].crate`
2. builds the companion crate
3. derives the vocab payload from `library_vocab()`
4. packages the derived metadata and any registered desugarer into `target/lib/<library>.incnlib`

Any serialized JSON sidecars or extraction glue are tooling details rather than part of the standard authoring workflow.

## 9. Consume the DSL from another project

The consumer depends on the built library artifact:

```toml
[dependencies]
routekit = { path = "../routekit/target/lib" }
```

Then import the library. That import both exposes the requested symbols and activates the registered DSL surface for the file:

```incan
from pub::routekit import routekit_name

# Any `pub::routekit` import activates the registered DSL entries for this file.
```

## Common pitfalls

- `[vocab].crate` points to a directory, not a Cargo package name.
- The activation namespace must match the consumer import spelling after `pub::`.
- Do not split the public contract across `build.rs`, convention functions, or hand-maintained `vocab_metadata.json` files.
- Companion crates that package a desugarer must include `cdylib` in `[lib].crate-type`.
- If local desugarer packaging fails with a missing target error, install the required Rust target (`rustup target add wasm32-wasip1`) and rerun `incan build --lib`.
- If desugared code needs Rust crates or stdlib features, declare them in `LibraryManifest` so consumer builds get the same requirements.
- Block or clause-oriented DSL registrations need a desugarer when they cannot continue through the compiler as ordinary Incan syntax on their own.
- An `EmbeddedFragmentDescriptor` claims one fixed submode kind from the RFC 081 catalog; it cannot author its own grammar, and unrecognized syntax inside a claimed position is always a parse error, never a fallback to a broader guess.
- Do not describe an embedded-fragment submode as compatible with a specific external language (CSS, HTML, and so on) in your own library's documentation. The mechanism only claims the constructs each submode's table explicitly enumerates.
- A fragment with no registered lowering hook still parses, typechecks, and reaches Body IR; only Rust code generation refuses, naming exactly what is missing.

## See also

- [Extending the language](extending_language.md)
- [Project configuration reference](../../tooling/reference/project_configuration.md)
- [CLI reference](../../tooling/reference/cli_reference.md)
- [RFC 027: `incan_vocab`](../../RFCs/closed/implemented/027_incan_vocab_crate.md)
- [RFC 040: Scoped DSL surface forms](../../RFCs/closed/implemented/040_scoped_dsl_surface_forms.md)
- [RFC 045: Scoped DSL symbol surfaces](../../RFCs/closed/implemented/045_scoped_dsl_symbol_surfaces.md)
- [RFC 081: Language-shaped DSL embeddings](../../RFCs/081_language_shaped_dsl_embeddings.md)
