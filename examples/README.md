# Examples

Runnable Incan programs organized by complexity. These serve as both documentation and smoke tests (`make examples` runs them all).

## Structure

### `simple/`

Single-file programs demonstrating basic language features: hello world, fibonacci, factorials, string operations, model declarations.

### `intermediate/`

Programs that exercise more involved features: collections, comprehensions, decorators, enums with data, error handling, inheritance, traits, newtypes, public visibility, while loops.

### `advanced/`

Programs that combine multiple features or use advanced capabilities: async/await, channels, synchronization primitives, file I/O, bytes, JSON serialization, custom traits, iterators, Rust crate interop, multi-file projects, nested projects.

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
