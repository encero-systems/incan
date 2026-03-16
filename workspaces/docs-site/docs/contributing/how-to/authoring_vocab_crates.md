# Author Library DSLs with `incan_vocab`

This guide is for library authors who want to ship import-activated DSL syntax such as `routes:`, `GET`, or `middleware:` without changing the core Incan compiler.

Use this path when the syntax belongs to one library and should only become active after importing that library. If you are changing the language itself, follow [Extending the language](extending_language.md) instead.

## What a vocab companion crate does

A vocab companion crate is a small Rust crate that lives next to your Incan library and describes three things:

- which keywords your library introduces
- when those keywords become active
- what extra manifest metadata consumer builds need

During `incan build --lib`, the compiler builds that companion crate, reads its `vocab_metadata.json`, and packages the resulting vocab payload into the `.incnlib` artifact for your library.

!!! note "Today’s shipped workflow"
    The stable Rust authoring surface is `incan_vocab`, but the compiler currently consumes the companion crate through a generated `vocab_metadata.json` file at the crate root. The easiest way to keep that file in sync is to generate it from a `VocabProvider` in `build.rs`.

## When to use this path

- Use a vocab companion crate when your library wants import-activated DSL syntax.
- Use a plain library API when ordinary functions, models, or classes are enough.
- Use the compiler contributor path only when the feature should become part of Incan itself.

## Recommended layout

This is what the recommended layout would look like for an imaginary library called `routekit`

```text
routekit/             # the parent folder
├── incan.toml
├── src/
│   └── lib.incn      # libraries need a `lib.incn` file to be considered a library
└── vocab_companion/  # the vocab companion crate
    ├── Cargo.toml
    ├── build.rs
    └── src/
        ├── lib.rs
        └── provider.rs
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

[dependencies]
incan_vocab = "0.1"

[build-dependencies]
incan_vocab = "0.1"
```

The compiler validates that the companion crate has both `Cargo.toml` and `src/lib.rs`, so keep it as a real Rust crate even if most of the interesting logic lives in a shared `provider.rs`.

You do not need to depend on `serde_json` just to emit `vocab_metadata.json`. `incan_vocab` provides a helper for that. Add JSON tooling separately only if your own custom build/desugarer pipeline needs it.

## 3. Describe the DSL with `VocabProvider`

Put the registration logic in `src/provider.rs` so both the library crate and `build.rs` can reuse it:

```rust
use incan_vocab::{
    KeywordActivation, KeywordPlacement, KeywordRegistration, KeywordSpec, KeywordSurfaceKind, LibraryManifest,
    VocabMetadata, VocabProvider,
};

pub struct RoutekitVocab;

impl VocabProvider for RoutekitVocab {
    fn keyword_registrations(&self) -> Vec<KeywordRegistration> {
        vec![KeywordRegistration {
            activation: KeywordActivation::OnImport {
                namespace: "routekit".to_string(),
            },
            keywords: vec![
                KeywordSpec::new("routes", KeywordSurfaceKind::BlockDeclaration),
                KeywordSpec {
                    name: "GET".to_string(),
                    surface_kind: KeywordSurfaceKind::BlockContextKeyword,
                    compound_tokens: Vec::new(),
                    placement: KeywordPlacement::InBlock(vec!["routes".to_string()]),
                },
                KeywordSpec {
                    name: "middleware".to_string(),
                    surface_kind: KeywordSurfaceKind::SubBlock,
                    compound_tokens: Vec::new(),
                    placement: KeywordPlacement::InBlock(vec!["routes".to_string()]),
                },
            ],
            valid_decorators: Vec::new(),
        }]
    }

    fn library_manifest(&self) -> LibraryManifest {
        LibraryManifest::default()
    }
}

pub fn metadata() -> VocabMetadata {
    RoutekitVocab.metadata()
}
```

Key rules:

- `KeywordActivation::OnImport { namespace }` must match the consumer-facing import spelling without the `pub::` prefix. In the current library-system phase, `pub::` imports only accept the library name, so `from pub::routekit import routekit_name` activates `namespace: "routekit"`.
- `KeywordPlacement::InBlock(...)` scopes nested entries to the parent DSL block.
- `library_manifest()` is where you describe any extra exported metadata or build requirements that should ride along with the library artifact.

If your desugared output needs extra runtime requirements, populate them in `library_manifest()`:

```rust
use incan_vocab::{CargoDependency, CargoDependencySource, LibraryManifest};

fn library_manifest(&self) -> LibraryManifest {
    LibraryManifest {
        required_dependencies: vec![CargoDependency {
            crate_name: "axum".to_string(),
            source: CargoDependencySource::Version("0.8".to_string()),
        }],
        required_stdlib_features: vec!["web".to_string()],
        ..LibraryManifest::default()
    }
}
```

## 4. Generate `vocab_metadata.json`

The compiler looks for `vocab_metadata.json` at the companion crate root after `cargo build` finishes. A build script keeps that file synchronized automatically.

`src/lib.rs`:

```rust
mod provider;

pub use provider::RoutekitVocab;
```

`build.rs`:

```rust
use std::path::PathBuf;

#[path = "src/provider.rs"]
mod provider;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("vocab_metadata.json");
    incan_vocab::write_metadata_json(out_path, &provider::RoutekitVocab)?;

    println!("cargo:rerun-if-changed=src/provider.rs");
    println!("cargo:rerun-if-changed=src/lib.rs");
    Ok(())
}
```

This is the most practical pattern today:

- author the metadata once in Rust via `VocabProvider`
- serialize it to the file the compiler actually reads
- avoid hand-editing JSON

## 5. Build the library artifact

Run library mode from the Incan project root:

```bash
incan build --lib
```

This requires `src/lib.incn`. During the build, Incan:

1. reads `[vocab].crate`
2. runs `cargo build` for the companion crate
3. reads `vocab_metadata.json`
4. packages the vocab payload into `target/lib/<library>.incnlib`

## 6. Consume the DSL from another project

The consumer depends on the built library artifact:

```toml
[dependencies]
routekit = { path = "../routekit/target/lib" }
```

Then import the library. That import both exposes the symbols you request and activates the registered keywords for the file:

```incan
from pub::routekit import routekit_name

# Any `pub::routekit` import activates the registered DSL entries for this file.
```

## Block DSLs need a desugarer

Registering `BlockDeclaration`, `BlockContextKeyword`, and `SubBlock` teaches the parser how to recognize your DSL surface. It does not, by itself, make raw DSL blocks typecheck.

For block-style DSLs, you also need to package a desugarer artifact so the compiler can rewrite raw vocab blocks into ordinary Incan statements before typechecking.

Declare that artifact from your provider:

```rust
use incan_vocab::DesugarerMetadata;

fn desugarer_metadata(&self) -> Option<DesugarerMetadata> {
    Some(DesugarerMetadata::default())
}
```

`DesugarerMetadata::default()` means:

- target: `wasm32-wasip1`
- profile: `release`
- output file name: `<package_name>.wasm`
- entrypoint: `desugar_block`

The compiler packages that artifact from `target/<target>/<profile>/` into the library output during `incan build --lib`.

!!! warning "Important"
    External block DSLs are only complete once both pieces exist:

    - the vocab metadata (`keyword_registrations`, manifest requirements, optional desugarer metadata)
    - the desugarer artifact itself

    If you register block keywords but do not package a desugarer artifact, the parser may accept the syntax, but raw vocab blocks will be rejected before typechecking finishes.

## Common pitfalls

- `[vocab].crate` points to a directory, not a Cargo package name.
- `vocab_metadata.json` must live at the companion crate root, not inside `target/`.
- The activation namespace must match the consumer import spelling after `pub::`.
- If desugared code needs Rust crates or stdlib features, declare them in `library_manifest()` so consumer builds get the same requirements.
- Block DSL registrations need desugarer metadata and a packaged Wasm artifact, not just keyword registrations.

## See also

- [Extending the language](extending_language.md)
- [Project configuration reference](../../tooling/reference/project_configuration.md)
- [CLI reference](../../tooling/reference/cli_reference.md)
- [RFC 027: `incan_vocab`](../../RFCs/closed/implemented/027_incan_vocab_crate.md)
