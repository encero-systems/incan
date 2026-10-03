// Step 11's Rust binary, built by plain rustc. It does what the `fib` and `collatz` benchmarks' `main`s do, calling
// the two kernels the driver compiled natively from real Body IR, and prints the benchmarks' own output lines.
use kernels::{collatz_steps, fib_mod};

fn main() {
    let n = 1_000_000;
    let modulo = 1_000_000_007;
    println!("fib({n}) mod {modulo} = {}", fib_mod(n, modulo));
    let limit = 1_000_000;
    let mut total_steps = 0;
    for n in 1..limit + 1 {
        total_steps += collatz_steps(n);
    }
    println!("Total Collatz steps for 1..{limit}: {total_steps}");
}
