// Mergesort Benchmark
// Sort 1,000,000 random integers (with allocation)
// NOTE: This uses a simple recursive mergesort with per-split allocations.
// It is a reasonable reference implementation, but not the most allocation-efficient Rust mergesort.

fn mergesort(arr: Vec<i64>) -> Vec<i64> {
    let n = arr.len();
    if n <= 1 {
        return arr;
    }
    
    let mid = n / 2;
    let left = mergesort(arr[..mid].to_vec());
    let right = mergesort(arr[mid..].to_vec());
    
    merge(left, right)
}

fn merge(left: Vec<i64>, right: Vec<i64>) -> Vec<i64> {
    let mut result = Vec::with_capacity(left.len() + right.len());
    let mut i = 0;
    let mut j = 0;
    
    while i < left.len() && j < right.len() {
        if left[i] <= right[j] {
            result.push(left[i]);
            i += 1;
        } else {
            result.push(right[j]);
            j += 1;
        }
    }
    
    // Append remaining elements
    result.extend_from_slice(&left[i..]);
    result.extend_from_slice(&right[j..]);
    
    result
}

fn generate_random_array(size: usize, seed: i64) -> Vec<i64> {
    let mut arr = Vec::with_capacity(size);
    let mut state = seed;
    for _ in 0..size {
        state = (state.wrapping_mul(1103515245).wrapping_add(12345)) % 2147483648;
        arr.push(state % 1000000);
    }
    arr
}

fn main() {
    let size = 1_000_000;
    let arr = generate_random_array(size, 42);
    let sorted_arr = mergesort(arr);
    println!("Sorted {} elements", size);
    println!("First 5: {}, {}, {}, {}, {}", 
             sorted_arr[0], sorted_arr[1], sorted_arr[2], sorted_arr[3], sorted_arr[4]);
}
