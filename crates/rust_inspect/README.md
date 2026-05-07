# rust_inspect

Rust item inspection substrate for Incan.

This crate is the compiler-side boundary for looking up Rust function/type metadata needed by RFC 041 interop. It is not a general Rust reflection library and it is not part of the generated-program runtime. Its job is narrower:

- load a Rust workspace rooted at a generated Cargo project, typically `target/incan_lock`
- extract `RustItemMetadata` for canonical Rust paths
- cache extracted items so compiler hot paths can read metadata without re-extracting it

## Purpose

Incan needs Rust-side signatures and item shapes for some interop checks:

- imported Rust free-function validation
- associated function and method validation
- Rust-backed type shape inspection
- borrow-aware interop codegen

`rust_inspect` exists so that work is isolated from the rest of the compiler. The `incan` crate should depend on a small inspection surface, not own rust-analyzer/Cargo loading details directly.

## Current API

The main entrypoint is `Inspector`:

```rust
use rust_inspect::{Inspector, InspectorConfig};

let inspector = Inspector::new(InspectorConfig::new("target/incan_lock"));

inspector.prewarm(
    ["demo::consumer::consume".to_string()],
    &|path| eprintln!("warming {path}"),
)?;

let hit = inspector.get("demo::consumer::consume")?;
```

The intended contract is:

- `prewarm(...)` may perform expensive extraction
- `get(...)` should be cache-only
- compiler/typechecker hot paths should prefer cached reads over fresh extraction

## Architecture Notes

### Generated workspace boundary

This crate inspects Rust items through a generated Cargo workspace rather than the user's live application tree. In practice that means a lock workspace such as:

```text
<project>/target/incan_lock
```

That workspace gives Incan a stable, compiler-owned place to:

- pin dependency resolution
- alias Cargo package names to Rust-safe crate names when needed
- load external crates consistently for inspection

### Why a separate crate?

This code has a different responsibility from parsing, typechecking, or emitting Incan itself:

- it talks to rust-analyzer internals
- it loads Cargo workspaces
- it manages inspection caches
- it may evolve into a long-lived sidecar or other dedicated architecture

Keeping it separate makes that boundary explicit.

### Not a stable public contract

This crate is an internal compiler subsystem. The API may change as Incan settles the right architecture for Rust inspection. In particular, cache layout, fidelity reporting, and workspace loading strategy should be treated as implementation details unless explicitly documented otherwise.

### Internal module layout

`lib.rs` wires crate-root modules so `cache.rs` does not need `#[path]` shims:

- `cache.rs`: cache orchestration and extraction flow
- `cache_resolve.rs`: dependency/source-root resolution helpers
- `cache_timing.rs`: optional timing instrumentation (still uses `eprintln!` when `INCAN_RUST_INSPECT_TIMING` is set)

Structured logging for durable diagnostics uses `tracing` (for example disk-cache parse failures and failed persists).

The on-disk cache filename is `.incan_rust_inspect_cache.json`. The cache loader still reads the older
`.incan_rust_metadata_cache.json` filename for backward compatibility.

## Limitations

- It is currently optimized for Incan's compiler use case, not arbitrary Rust analysis.
- It depends on rust-analyzer implementation crates (`ra_ap_*`), which are powerful but not a stable embedding surface.
- Performance is still under active work. The long-term goal is to keep extraction out of compiler hot loops entirely.

## Development

When changing this crate, hold the line on one architectural rule:

- expensive extraction belongs in explicit preparation paths
- cache-only lookups belong in semantic/codegen hot paths

If a fix requires fresh extraction during type comparison or ordinary call validation, that is usually a design smell and should be challenged first.

## License

Apache 2.0
