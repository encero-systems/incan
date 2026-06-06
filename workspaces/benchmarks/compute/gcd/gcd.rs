// Compute GCD for many number pairs.
// Uses Stein's algorithm to give Rust a stronger baseline against std.math.gcd.

fn gcd(a: i64, b: i64) -> i64 {
    let mut m = a;
    let mut n = b;

    if m == 0 || n == 0 {
        return (m | n).abs();
    }

    let shift = (m | n).trailing_zeros();

    if m == i64::MIN || n == i64::MIN {
        return (1_i64 << shift).abs();
    }

    m = m.abs();
    n = n.abs();

    m >>= m.trailing_zeros();
    n >>= n.trailing_zeros();

    while m != n {
        if m > n {
            m -= n;
            m >>= m.trailing_zeros();
        } else {
            n -= m;
            n >>= n.trailing_zeros();
        }
    }

    m << shift
}

fn main() {
    let iterations = 10_000_000i64;
    let mut total: i64 = 0;
    
    // Deterministic pairs based on loop index
    for i in 1..=iterations {
        let a = (i * 17) % 10000 + 1;
        let b = (i * 31) % 10000 + 1;
        total += gcd(a, b);
    }
    
    println!("Sum of {} GCDs: {}", iterations, total);
}
