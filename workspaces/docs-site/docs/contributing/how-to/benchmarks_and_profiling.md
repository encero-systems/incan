# Benchmarks & profiling (repository)

This page is for contributors working on Incan’s performance: running the repository benchmark suite, adding new benchmarks, and profiling the compiler.

If you want to speed up **your program**, see [Performance (how-to)](../../language/how-to/performance.md).

## Benchmark suite

The repository includes a benchmark suite that compares generated binaries against Rust and Python baselines.

### Available benchmarks

| Category | Benchmark     | Description                           |
| -------- | ------------- | ------------------------------------- |
| Compute  | `fib`         | Iterative Fibonacci (N=1,000,000)     |
| Compute  | `collatz`     | Collatz sequence (1,000,000 numbers) |
| Compute  | `gcd`         | GCD of 10,000,000 pairs               |
| Compute  | `mandelbrot`  | 2000×2000 escape iterations           |
| Compute  | `nbody`       | N-body simulation (500,000 steps)     |
| Compute  | `primes`      | Sieve up to 50,000,000                |
| Sorting  | `quicksort`   | In-place sort (1M integers)           |
| Sorting  | `mergesort`   | Merge sort (1M integers)              |

### Running benchmarks

```bash
# Prerequisites (macOS)
brew install hyperfine jq bc

# Build the compiler
cargo build --release

# Run all benchmarks
make benchmarks

# Or run the benchmark runner directly
./benchmarks/run_all.sh
```

### Running an individual benchmark

```bash
cd benchmarks/compute/fib

# Build Incan version
../../../target/release/incan build fib.incn
cp ../../../target/incan/.cargo-target/release/fib ./fib_incan

# Build Rust baseline
rustc -O fib.rs -o fib_rust

# Compare
hyperfine --warmup 2 --min-runs 5 \
  './fib_incan' \
  './fib_rust' \
  'python3 fib.py'
```

### Results

For current measured results, see `benchmarks/results/results.md` in the repository.

## Profiling

### Profiling generated code (your benchmark/program)

```bash
# Build (incan always builds the generated project in release mode today)
incan build myprogram.incn

# Profile with Instruments (macOS)
xcrun xctrace record --template "Time Profiler" \
  --launch ./target/incan/.cargo-target/release/myprogram

# Profile with perf (Linux)
perf record ./target/incan/.cargo-target/release/myprogram
perf report
```

### Profiling the compiler

```bash
# Profile compilation itself
cargo flamegraph --bin incan -- build examples/advanced/async_await.incn

# Or with samply (macOS)
samply record ./target/release/incan build large_program.incn
```

## Adding a new benchmark

1. Create a directory under `benchmarks/compute/` or `benchmarks/sorting/`.
2. Add three implementations: `name.incn`, `name.rs`, `name.py`.
3. Each should print a single result line for verification.
4. Run `./benchmarks/run_all.sh` to include it in the suite.

