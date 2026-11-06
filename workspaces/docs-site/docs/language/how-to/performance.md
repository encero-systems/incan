# Profile an Incan program

Measure the program's native executable separately from compilation. Record the compiler version, artifact profile, inputs and host with the result.

## Build an optimized executable

```bash
incan build src/main.incn --profile release
```

The installation must contain release-profile standard packages. A debug-only home cannot supply them. The executable is under `target/incan/release/bin/`.

## Measure unchanged runs

```bash
time incan run src/main.incn --profile release
OVEN_TRACE=- incan run src/main.incn --profile release
```

Use the untraced command for timing. Use the trace to distinguish executable reuse from compilation. On macOS, first execution of a new binary can incur a system assessment independently of compilation.

## Find the expensive operation

Run the selected executable under Instruments on macOS or `perf` on Linux. Keep the same input data for comparisons. Optimize an observed allocation, algorithm or I/O cost, then rerun the workload and check its output.

Native compilation alone does not establish a speed claim. See [Repository benchmarking](../../contributing/how-to/benchmarks_and_profiling.md) for comparing implementations.
