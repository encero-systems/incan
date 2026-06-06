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
# or: cargo install hyperfine

# Used by the benchmark runner for parsing/formatting results
brew install jq bc  # macOS

# Ensure Incan CLI is built
cargo build --release
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
cd workspaces/benchmarks/compute/fib
../../../../target/release/incan build fib.incn
cp ../../../../target/incan/.cargo-target/release/fib ./fib_incan
rustc -O fib.rs -o fib_rust
hyperfine --warmup 2 --min-runs 5 './fib_incan' './fib_rust' 'python3 fib.py'
```

## Results

Results are written to `results/results.md` after running the benchmark suite.

## Metrics

- **Time**: Wall clock time via `hyperfine` (warmup + repeated runs)
