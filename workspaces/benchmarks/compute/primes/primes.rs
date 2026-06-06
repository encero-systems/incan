// Prime Sieve Benchmark
// Sieve of Eratosthenes up to N = 50,000,000

fn sieve(limit: usize) -> usize {
    // Create boolean array, true = prime candidate
    let mut is_prime = vec![true; limit + 1];
    is_prime[0] = false;
    is_prime[1] = false;

    let mut p = 2;
    while p * p <= limit {
        if is_prime[p] {
            // Mark multiples of p as not prime
            let mut multiple = p * p;
            while multiple <= limit {
                is_prime[multiple] = false;
                multiple += p;
            }
        }
        p += 1;
    }

    // Count primes
    is_prime.iter().filter(|&&x| x).count()
}

fn main() {
    let limit = 50_000_000;
    let count = sieve(limit);
    println!("Primes up to {}: {}", limit, count);
}
