# Examples

Runnable Incan programs organized by complexity. The examples serve as both documentation and smoke tests (`make examples` runs them all).

## Structure

### `simple/`

Single-file programs that showcase core language features: hello world, fibonacci, factorials, string operations, and model declarations.

### `intermediate/`

Programs that exercise richer language surfaces: collections, comprehensions, decorators, data-carrying enums, error handling, inheritance, traits, supertraits, newtypes, public visibility, and while loops.

### `advanced/`

Programs that combine multiple features or rely on advanced capabilities: async/await, channels, synchronization primitives, file I/O, bytes, JSON serialization, custom traits, iterators, Rust crate interop, multi-file projects, and nested projects.

Notable advanced Rust interop examples:

- `advanced/using_rust_crates.incn` - broad Rust crate integration patterns.

### `pro/`

Projects aimed at library and tooling authors: companion crates, vocab/desugaring flows, and integrations that exercise the compiler's authoring surface, and not only end-user language features.

Notable pro Rust interop example:

- `pro/rust_interop_pro.incn` - RFC 041 authoring surface (`rusttype`, `interop`, `std.rust` bounds, async wrappers).
  Includes a "new to Rust" mental model that explains `rusttype`, `...` method declarations, and `interop:` edges.
- `pro/vocab_querykit` - runnable RFC 040 vocab companion example for query blocks, leading-dot fields, and
  leading-dot fields in registered method arguments.
- `pro/vocab_routekit` - runnable RFC 040 vocab companion example for block headers, nested block-context clauses, and
  scoped operator-like glyphs.
- `pro/vocab_studiokit` - runnable RFC 040 vocab companion example for workflow-shaped blocks and scoped fallback
  glyphs.

### `web/`

Web application examples using the Incan web framework surface.

### Top-level files

`consts.incn` — standalone example for constant declarations.

## Running examples

```bash
# Run all examples as a smoke test (requires release build)
make examples

# Run a single example
./target/release/incan run examples/simple/hello.incn

# Build without running
./target/release/incan build examples/advanced/async_await.incn
```

## Adding examples

When adding a new example:

1. Place it in the appropriate difficulty folder.
2. Include a `def main():` entrypoint if the example should be runnable.
3. Run `make examples` to verify it compiles and runs within the timeout.
4. Examples without a `main()` function are still typechecked but not executed.
5. If an example contains a library project (`incan.toml` + `src/lib.incn`), the examples runner pre-builds it with `incan build --lib` before checking consumer files.
