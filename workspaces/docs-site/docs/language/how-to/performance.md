# Performance

This page is a practical guide to profiling and improving the performance of **your Incan programs**.

Repository benchmark suite details (and profiling the compiler) live in
[Benchmarks & profiling (repository)](../../contributing/how-to/benchmarks_and_profiling.md).

## What to expect

Incan compiles to Rust and then to a native binary. Runtime performance can be close to Rust for many workloads, but it
depends on current codegen and library behavior.

Two implications:

- **Builds can be slower** than a single-step native compiler (Incan → Rust → binary).
- **Performance is explainable**: when in doubt, inspect the generated Rust and profile the produced binary.

## Profile your program

### Build a release binary (generated project)

```bash
incan build myprogram.incn
```

### Profile on macOS (Instruments)

```bash
xcrun xctrace record --template "Time Profiler" \
  --launch ./target/incan/.cargo-target/release/myprogram
```

### Profile on Linux (perf)

```bash
perf record ./target/incan/.cargo-target/release/myprogram
perf report
```

## Optimization tips (practical)

- **Prefer the canonical build**: `incan build file.incn` (builds the generated Rust project in release mode today).
- **Avoid unnecessary clones**: the code generator tries to avoid extra copying, but explicit `.clone()` in source
  will be preserved.
- **Prefer iterator-style code**: list comprehensions compile to iterator chains.
- **Inspect generated Rust**: use `incan --emit-rust file.incn` when you’re not sure what the compiler is emitting.
