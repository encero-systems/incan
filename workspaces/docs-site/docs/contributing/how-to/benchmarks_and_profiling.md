# Benchmark and profile a program

Use an installed dev.8 compiler. Keep compiler, source, dependency artifacts and profile fixed between measurements.

## Build and measure

From a project with `src/main.incn`:

```bash
incan build src/main.incn
time incan run src/main.incn
OVEN_TRACE=- incan run src/main.incn
```

The default profile is `debug`. For optimized measurements use `--profile release` with a toolchain containing release-profile standard packages. Do not compare a first compilation with an unchanged executable run.

## Profile the executable

Program binaries reside under `target/incan/<profile>/bin/`. Select the binary for your source, then launch it under Instruments on macOS or `perf` on Linux. Record the source arguments and input data with the measurement.

A profiler around `incan build` measures preparation and compilation. A profiler around the resulting binary measures the application. These answer different questions.

## Compare baselines

The repository keeps Incan, Rust and Python benchmark sources under `workspaces/benchmarks/`. Compare identical workloads and output values. Python baselines are research inputs, not production Incan tooling.

The repository Makefile has no benchmark build target. Use explicit installed-toolchain commands and verify each benchmark script's assumptions before running it.

See [Measure Oven reuse](oven_alpha_benchmark.md) for compilation measurements.
