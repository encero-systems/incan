// Quicksort Benchmark
// Sort 1,000,000 random integers in-place

fn quicksort(arr: &mut [i64], low: isize, high: isize) {
    if low < high {
        let pivot_idx = partition(arr, low as usize, high as usize);
        quicksort(arr, low, pivot_idx as isize - 1);
        quicksort(arr, pivot_idx as isize + 1, high);
    }
}

fn partition(arr: &mut [i64], low: usize, high: usize) -> usize {
    let pivot = arr[high];
    let mut i = low as isize - 1;
    
    for j in low..high {
        if arr[j] <= pivot {
            i += 1;
            arr.swap(i as usize, j);
        }
    }
    
    arr.swap((i + 1) as usize, high);
    (i + 1) as usize
}

fn generate_random_array(size: usize, seed: i64) -> Vec<i64> {
    // Simple LCG random number generator for reproducibility
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
    let mut arr = generate_random_array(size, 42);
    quicksort(&mut arr, 0, (size - 1) as isize);
    println!("Sorted {} elements", size);
    println!("First 5: {}, {}, {}, {}, {}", arr[0], arr[1], arr[2], arr[3], arr[4]);
}
