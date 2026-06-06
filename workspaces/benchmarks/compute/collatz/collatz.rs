// Collatz Conjecture Benchmark
// Count total steps for all numbers 1 to N

fn collatz_steps(n: i64) -> i64 {
    let mut count = 0;
    let mut num = n;
    while num != 1 {
        if num % 2 == 0 {
            num = num / 2;
        } else {
            num = 3 * num + 1;
        }
        count += 1;
    }
    count
}

fn main() {
    let limit = 1_000_000;
    let mut total_steps: i64 = 0;

    for n in 1..=limit {
        total_steps += collatz_steps(n);
    }

    println!("Total Collatz steps for 1..{}: {}", limit, total_steps);
}
