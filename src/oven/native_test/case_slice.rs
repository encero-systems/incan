//! Deterministic slices of one native root's live case inventory.
//!
//! The compiler-suite replay packs whole test roots into shards, and a root is indivisible, so the longest root is a
//! floor no packing can go below: on a measured run two roots alone set a fifty-minute lane while two other shards
//! waited forty. A slice lets one root run on several shards. It is defined from the **live** inventory the compiled
//! libtest binary reports, not from any stored list: every shard that runs a sliced root sorts the same inventory by
//! the same recorded per-case durations and assigns cases to bins with the same greedy rule, so slice `k` of `K`
//! names the same cases everywhere, and a case added since the last record lands in exactly one bin by
//! construction. The record only makes the split balanced; it is never what makes it complete.

use std::collections::BTreeMap;

/// One slice of a root's inventory to execute: `index` of `count`, weighted by any recorded case durations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OvenNativeTestCaseSlice<'a> {
    /// Zero-based slice to run.
    pub index: usize,
    /// Total slices the root is divided into; at least one.
    pub count: usize,
    /// Per-case milliseconds a previous run measured for this root; an unmeasured case weighs nothing.
    pub measured_case_millis: &'a BTreeMap<String, u64>,
}

/// What one sliced execution covered, recorded so a later reconciliation can prove the slices jointly cover the
/// root exactly once.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct OvenNativeTestCaseSliceReport {
    /// Zero-based slice that ran.
    pub index: usize,
    /// Total slices the root was divided into.
    pub count: usize,
    /// Size of the live inventory the slices were computed from.
    pub inventory_count: usize,
    /// Exact case names this slice selected, in inventory order.
    pub selected: Vec<String>,
}

/// Assign every case in `inventory` to exactly one of `count` bins, deterministically.
///
/// Cases are taken heaviest first (recorded milliseconds descending, then name ascending, so an unmeasured case
/// sorts after every measured one) and each goes to the bin with the smallest running total, the lowest index
/// breaking ties. An unmeasured case weighs one millisecond rather than zero, so a whole unmeasured inventory
/// spreads across the bins instead of piling into the first. Every bin is returned sorted by name. A `count` of
/// zero yields no bins.
pub fn assign_case_slices(
    inventory: &[String],
    measured_case_millis: &BTreeMap<String, u64>,
    count: usize,
) -> Vec<Vec<String>> {
    let mut bins = vec![Vec::new(); count];
    if count == 0 {
        return bins;
    }
    let mut ordered = inventory
        .iter()
        .map(|name| (measured_case_millis.get(name).copied().unwrap_or(0).max(1), name))
        .collect::<Vec<_>>();
    ordered.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(right.1)));
    let mut totals = vec![0_u64; count];
    for (millis, name) in ordered {
        let Some(lightest) = (0..count).min_by_key(|bin| (totals[*bin], *bin)) else {
            break;
        };
        totals[lightest] = totals[lightest].saturating_add(millis);
        bins[lightest].push(name.clone());
    }
    for bin in &mut bins {
        bin.sort();
    }
    bins
}

/// Select the cases slice `slice.index` runs from a live inventory, with the record that describes the selection.
///
/// An index at or past `count` is a caller error and selects nothing; the report still says which slice was asked
/// for, so the mismatch is visible rather than silently executing everything.
pub fn select_case_slice(inventory: &[String], slice: OvenNativeTestCaseSlice<'_>) -> OvenNativeTestCaseSliceReport {
    let bins = assign_case_slices(inventory, slice.measured_case_millis, slice.count);
    OvenNativeTestCaseSliceReport {
        index: slice.index,
        count: slice.count,
        inventory_count: inventory.len(),
        selected: bins.into_iter().nth(slice.index).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| (*name).to_string()).collect()
    }

    #[test]
    fn every_case_lands_in_exactly_one_bin_and_bins_are_balanced_by_recorded_time() {
        let inventory = names(&["a", "b", "c", "d", "e"]);
        let measured = BTreeMap::from([
            ("a".to_string(), 100),
            ("b".to_string(), 90),
            ("c".to_string(), 10),
            ("d".to_string(), 10),
        ]);
        let bins = assign_case_slices(&inventory, &measured, 2);
        // a→0 (100), b→1 (90), c→1 (100), d ties at 100/100 and takes the lower index, e→1.
        assert_eq!(bins, vec![names(&["a", "d"]), names(&["b", "c", "e"])]);
        let mut all = bins.concat();
        all.sort();
        assert_eq!(all, inventory);
    }

    #[test]
    fn an_unmeasured_inventory_still_splits_deterministically_by_name() {
        let inventory = names(&["zeta", "alpha", "mid"]);
        let bins = assign_case_slices(&inventory, &BTreeMap::new(), 2);
        // Unmeasured cases all weigh one millisecond: heaviest-first order is name order, and each case goes to the
        // bin with the lowest running total, so alpha and mid open the two bins and zeta joins the lighter one.
        assert_eq!(bins, vec![names(&["alpha", "zeta"]), names(&["mid"])]);
        assert_eq!(
            bins,
            assign_case_slices(&inventory, &BTreeMap::new(), 2),
            "same input, same split"
        );
    }

    #[test]
    fn a_new_case_joins_exactly_one_bin_without_moving_the_split_apart() {
        let measured = BTreeMap::from([("heavy".to_string(), 500), ("light".to_string(), 5)]);
        let before = assign_case_slices(&names(&["heavy", "light"]), &measured, 2);
        let after = assign_case_slices(&names(&["heavy", "light", "brand_new"]), &measured, 2);
        assert_eq!(before, vec![names(&["heavy"]), names(&["light"])]);
        assert_eq!(after, vec![names(&["heavy"]), names(&["brand_new", "light"])]);
    }

    #[test]
    fn a_slice_report_names_what_it_selected_and_how_it_was_asked_for() {
        let inventory = names(&["a", "b", "c"]);
        let measured = BTreeMap::new();
        let report = select_case_slice(
            &inventory,
            OvenNativeTestCaseSlice {
                index: 1,
                count: 3,
                measured_case_millis: &measured,
            },
        );
        assert_eq!(report.inventory_count, 3);
        assert_eq!((report.index, report.count), (1, 3));
        assert_eq!(report.selected, names(&["b"]));
        let out_of_range = select_case_slice(
            &inventory,
            OvenNativeTestCaseSlice {
                index: 3,
                count: 3,
                measured_case_millis: &measured,
            },
        );
        assert!(
            out_of_range.selected.is_empty(),
            "an out-of-range slice selects nothing, never everything"
        );
    }

    #[test]
    fn zero_slices_is_no_bins_and_one_slice_is_the_whole_inventory() {
        let inventory = names(&["b", "a"]);
        assert!(assign_case_slices(&inventory, &BTreeMap::new(), 0).is_empty());
        assert_eq!(
            assign_case_slices(&inventory, &BTreeMap::new(), 1),
            vec![names(&["a", "b"])]
        );
    }
}
