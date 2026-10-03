// Step 11's Rust binary, built by plain rustc. It does what the `fib`, `collatz` and `mandelbrot` benchmarks'
// `main`s do, calling the kernels the driver compiled natively from real Body IR, and prints their own output lines.
use kernels::{collatz_steps, fib_mod, mandelbrot_escape};

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
    let (size, max_iter) = (2000, 50);
    let mut total_iter = 0;
    for y in 0..size {
        for x in 0..size {
            let cr = (2.0 * x as f64 / size as f64) - 1.5;
            let ci = (2.0 * y as f64 / size as f64) - 1.0;
            total_iter += mandelbrot_escape(cr, ci, max_iter);
        }
    }
    println!("Total iterations: {total_iter}");
}
