// Fibonacci Benchmark
// Compute fib(N) iteratively, mod 10^9+7 to avoid overflow
// N = 1,000,000

fn fib_mod(n: i64, modulo: i64) -> i64 {
    if n <= 1 {
        return n;
    }

    let mut prev: i64 = 0;
    let mut curr: i64 = 1;

    for _ in 2..=n {
        let next = (prev + curr) % modulo;
        prev = curr;
        curr = next;
    }

    curr
}

fn main() {
    let n = 1_000_000;
    let modulo = 1_000_000_007;
    let result = fib_mod(n, modulo);
    println!("fib({}) mod {} = {}", n, modulo, result);
}
