//! Deterministic body-weighted source-module ranges shared by parallel lowering stages.

/// Split source order into at most `worker_limit` nonempty contiguous ranges balanced by body count.
pub fn balanced_module_ranges(weights: &[i64], worker_limit: i64) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    if weights.is_empty() || worker_limit <= 0 {
        return ranges;
    }
    let mut remaining_weight: i64 = weights.iter().sum();
    let mut start = 0usize;
    let mut workers_left = worker_limit;
    if (weights.len() as i64) < workers_left {
        workers_left = weights.len() as i64;
    }
    while start < weights.len() {
        let target_weight = (remaining_weight + workers_left - 1).div_euclid(workers_left);
        let mut selected_weight = 0;
        let mut end = start;
        let maximum_end = (weights.len() as i64 - workers_left + 1) as usize;
        while end < maximum_end && (end == start || selected_weight < target_weight) {
            selected_weight += weights[end];
            end += 1;
        }
        ranges.push((start, end));
        remaining_weight -= selected_weight;
        workers_left -= 1;
        start = end;
    }
    ranges
}

/// The default worker count of lowering's parallel stages.
pub const WORKER_LIMIT: i64 = 4;

#[cfg(test)]
mod tests {
    use super::balanced_module_ranges;

    #[test]
    fn balanced_ranges_are_contiguous_and_cover_source_order() {
        assert_eq!(balanced_module_ranges(&[8, 1, 1, 6, 2, 1], 3), vec![(0, 1), (1, 4), (4, 6)]);
    }

    #[test]
    fn balanced_ranges_never_emit_empty_workers() {
        assert_eq!(balanced_module_ranges(&[], 4), Vec::<(usize, usize)>::new());
        assert_eq!(balanced_module_ranges(&[0, 0], 4), vec![(0, 1), (1, 2)]);
        assert_eq!(balanced_module_ranges(&[3, 1], 0), Vec::<(usize, usize)>::new());
    }
}
