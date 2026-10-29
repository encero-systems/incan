# Examples

Runnable Incan programs organized by complexity. The examples serve as both documentation and smoke tests (`make examples` runs them all).

## Structure

### `simple/`

Single-file programs that showcase core language features: hello world, fibonacci, factorials, string operations, and model declarations.

### `intermediate/`

Programs that exercise richer language surfaces: collections, comprehensions, decorators, data-carrying enums, error handling, inheritance, traits, supertraits, newtypes, public visibility, and while loops.

### `advanced/`

Programs and small projects that combine multiple features or rely on advanced capabilities: async/await, synchronization primitives, file I/O, bytes, JSON serialization, custom traits, iterators, Rust crate interop, reusable libraries, multi-file projects, and nested projects.

Notable advanced Rust interop examples:

- `advanced/using_rust_crates.incn` - broad Rust crate integration patterns.
- `advanced/async_worker_pipeline.incn` - task fan-out, typed join results, and a deadline.
- `advanced/typed_data_processor` - a multi-module JSON-in/JSON-out project with tests.
- `advanced/library_package` - a reusable Incan library and a path-dependent consumer.

### `pro/`

Projects aimed at library and tooling authors: companion crates, vocab/desugaring flows, and integrations that exercise the compiler's authoring surface, and not only end-user language features.

Notable pro Rust interop example:

- `pro/rust_interop_pro.incn` - RFC 041 authoring surface (`rusttype`, `interop`, `std.rust` bounds, async wrappers). Includes a "new to Rust" mental model that explains `rusttype`, `...` method declarations, and `interop:` edges.
- `pro/codegraph_importer` - runnable RFC 106 example that consumes compiler-exported codegraph JSONL without becoming a semantic authority.
- `pro/vocab_querykit` - runnable RFC 040 vocab companion example for query blocks, leading-dot fields, and leading-dot fields in registered method arguments.
- `pro/vocab_routekit` - runnable RFC 040 vocab companion example for block headers, nested block-context clauses, and scoped operator-like glyphs.
- `pro/vocab_studiokit` - runnable RFC 040 vocab companion example for workflow-shaped blocks and scoped fallback glyphs.

### `web/`

Web application examples using the Incan web framework surface.

### Top-level files

`consts.incn` — standalone example for constant declarations.

## Running examples

```bash
# Run all examples with an installed dev.8 toolchain
make examples

# Run a single example
incan run examples/simple/hello.incn

# Build without running
incan build examples/advanced/async_await.incn
```

Oven prepares dependencies automatically. There is no bake or prewarm step. The runner reads `check-only.txt`: the markform, scriptkit and styleforge consumers check their descriptor syntax without a runtime lowering hook, and the declaration-only rusttype example checks without an entry point. Other entry points run, including async functions. Supporting modules and producer libraries are compiled through their entry points' dependency graphs.

`INCAN_BIN` selects an installed command. `INCAN_EXAMPLES_ONLY` selects colon-separated repository-relative entry paths. `INCAN_EXAMPLES_OUT` selects the log directory (default `target/examples`), containing `examples.tsv` and one log per command. GNU `timeout` from coreutils bounds each command to `INCAN_EXAMPLES_TIMEOUT` seconds (default 120); timeouts and compiler errors fail the gate. Project examples run from their nearest manifest directory so bundled data files resolve.

## Adding examples

When adding a new example:

1. Place it in the appropriate difficulty folder.
2. Include a `def main():` entrypoint if the example should be runnable.
3. Run `make examples` to verify it compiles and runs within the timeout.
4. Add an intentionally frontend-only entry to `check-only.txt` with its reason. Supporting modules need no separate entry.
5. Declare project dependencies in `loaf.toml`; Oven prepares producer libraries when their consumers run or check.
