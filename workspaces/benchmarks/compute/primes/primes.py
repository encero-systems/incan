# Prime Sieve Benchmark
# Sieve of Eratosthenes up to N = 50,000,000

def sieve(limit: int) -> int:
    # Create boolean array, True = prime candidate
    # is_prime = [True] * (limit + 1)
    is_prime = [True for _ in range(limit + 1)]
    is_prime[0] = False
    is_prime[1] = False

    p = 2
    while p * p <= limit:
        if is_prime[p]:
            # Mark multiples of p as not prime
            multiple = p * p
            while multiple <= limit:
                is_prime[multiple] = False
                multiple += p
        p += 1

    # Count primes
    return sum(is_prime)

def main():
    limit = 50_000_000
    count = sieve(limit)
    print(f"Primes up to {limit}: {count}")

if __name__ == "__main__":
    main()
