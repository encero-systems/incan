# Incan Benchmarks

Performance comparison between Incan, Rust, and Python.

## Benchmarks

### Compute

- **fib** — Iterative Fibonacci (N=1,000,000, mod 1e9+7)
- **collatz** — Collatz sequence (1,000,000 numbers)
- **gcd** — Greatest common divisor (10,000,000 pairs)
- **mandelbrot** — Mandelbrot escape iterations (2,000 × 2,000)
- **nbody** — N-body simulation (500,000 steps)
- **primes** — Sieve of Eratosthenes (up to 50,000,000)

### Sorting

- **quicksort** — In-place quicksort (1,000,000 random integers)
- **mergesort** — Merge sort with allocation (1,000,000 random integers)

### Collections

- **ordinal_map** — Standalone comparison of builtin Python `dict`, Python `fastconstmap`, Incan `OrdinalMap`, and the prior Rust spike baseline. It is intentionally not part of `run_all.sh` because `fastconstmap` is an optional Python dependency.

## Running Benchmarks

### Prerequisites

```bash
# Install hyperfine (benchmarking tool)
brew install hyperfine  # macOS

# Used by the benchmark runner for parsing/formatting results
brew install jq  # macOS

# Select an installed dev.8 toolchain and Oven
export INCAN_HOME=/path/to/installed/home
export INCAN=/path/to/installed/incan
export INCAN_STDLIB=/path/to/incan/loaves/stdlib
```

### Run All Benchmarks

```bash
./workspaces/benchmarks/run_all.sh
```

Or via Make:

```bash
make benchmarks
```

### Run Individual Benchmark

```bash
./workspaces/benchmarks/run_all.sh fib
```

## Results

Results are written to `results/results.md`, with fresh-project and unchanged release build times in `results/build_times.md`. Each measurement retains `uptime`, compiler logs, exact stdout comparisons, and hyperfine JSON under `target/run.*`. Set `BENCH_SCRATCH` to an existing lane-local scratch directory to keep builds there, and `RESULTS_DIR` to select the report directory. Python 3, hyperfine, and jq must be on `PATH`.

Rust baselines use the installed toolchain's pinned `rustc -C opt-level=3`. Every workload runs once in all three languages and must produce identical output before Incan receives a timing. Build refusals and output mismatches remain visible in the table and make the runner fail. Historical August numbers remain explicitly dated. `check_incan.sh` builds all Incan benchmark sources with the same installed CLI.

## Metrics

- **Time**: Wall clock time via `hyperfine` with two warmups, at least ten measured runs, and no shell. `RUNS` may increase the measured run count.
