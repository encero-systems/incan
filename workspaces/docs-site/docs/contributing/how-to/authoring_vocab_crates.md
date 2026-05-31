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

The descriptor `key` must be stable. The compiler preserves it on accepted surface artifacts and uses it for diagnostics, formatter metadata, and desugarer handoff. Expression-form descriptors such as leading-dot paths must declare receiver derivation; operator-like glyph descriptors can expose formatter hints such as `pairwise_chain()`. RFC 040 supports selected descriptor-gated non-core glyphs such as `|>`, `%>%`, `:=`, and `===`; broader language-shaped token forms remain RFC 081 work.

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

## 6. Add an optional desugarer

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

When you package a desugarer, make sure your Rust toolchain has that target installed:

```bash
rustup target add wasm32-wasip1
```

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

## 7. Build the library artifact

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

## 8. Consume the DSL from another project

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
- If desugarer packaging fails with a missing target error, install the required Rust target (`rustup target add wasm32-wasip1`) and rerun `incan build --lib`.
- If desugared code needs Rust crates or stdlib features, declare them in `LibraryManifest` so consumer builds get the same requirements.
- Block or clause-oriented DSL registrations need a desugarer when they cannot continue through the compiler as ordinary Incan syntax on their own.

## See also

- [Extending the language](extending_language.md)
- [Project configuration reference](../../tooling/reference/project_configuration.md)
- [CLI reference](../../tooling/reference/cli_reference.md)
- [RFC 027: `incan_vocab`](../../RFCs/closed/implemented/027_incan_vocab_crate.md)
