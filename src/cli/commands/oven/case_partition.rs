//! Partitioning the compiler-suite replay by case where whole roots would set the floor.
//!
//! The replay packs receipt-bound roots into shards greedily by weight. A root is indivisible when it runs whole, so
//! the heaviest root bounds the lane from below however well the rest is packed: on a measured four-shard run two
//! roots held the lane at fifty-three minutes while perfect balance would have been twenty-four. This module lets a
//! root whose **measured** weight exceeds one shard's fair share be divided into case slices, each packed as its own
//! unit. Only a measured root is ever sliced: a weight that is a size proxy or a typical-duration estimate says
//! nothing about which root actually dominates.
//!
//! A slice is a name for a deterministic subset of the root's live inventory (see `oven::native_test::case_slice`);
//! it is chosen here, computed at execution time, and proven jointly by the reconciliation of every shard's report.
//! A shard that ran any slice never claims complete-root evidence; the run does, once the reconciler has seen every
//! shard.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::oven::legacy_cargo::OvenCompilerTestSuiteShardReference;

/// The runner name a receipt-bound native libtest root carries; the only kind of root a slice applies to.
const NATIVE_LIBTEST_RUNNER: &str = "rustc-test";

/// One unit of replay work: a whole root, or one slice of a root divided by case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompilerSuiteShardUnit {
    /// The receipt-bound root the unit executes.
    pub(crate) reference: OvenCompilerTestSuiteShardReference,
    /// `Some((index, count))` when the root is divided and this unit runs slice `index` of `count`.
    pub(crate) case_slice: Option<(usize, usize)>,
}

/// Divide roots whose measured weight exceeds one shard's fair share into slices, keeping every other root whole.
///
/// `weight_of` is the packer's own weight for a root; `is_measured` says whether that weight is a measurement rather
/// than a proxy, because only a measured giant is worth dividing. Only a native libtest root can be divided: a
/// Rustdoc root runs its doctests as one process with no case inventory to slice, so it always stays whole. A giant
/// is cut into `ceil(weight / share)` slices, capped at `partition_count` so no shard is asked to compile one root
/// more often than another shard would. The returned units are in input order, slices ascending, and carry the
/// weight each contributes to packing.
pub(crate) fn divide_giant_roots(
    references: &[OvenCompilerTestSuiteShardReference],
    partition_count: usize,
    weight_of: impl Fn(&OvenCompilerTestSuiteShardReference) -> u64,
    is_measured: impl Fn(&OvenCompilerTestSuiteShardReference) -> bool,
) -> Vec<(CompilerSuiteShardUnit, u64)> {
    let total = references
        .iter()
        .fold(0_u64, |total, reference| total.saturating_add(weight_of(reference)));
    let share = if partition_count == 0 {
        0
    } else {
        total / partition_count as u64
    };
    let mut units = Vec::with_capacity(references.len());
    for reference in references {
        let weight = weight_of(reference);
        let divisible = reference.target.runner == NATIVE_LIBTEST_RUNNER;
        let slices = if divisible && share > 0 && weight > share && is_measured(reference) {
            weight.div_ceil(share).min(partition_count as u64).max(2)
        } else {
            1
        };
        if slices == 1 {
            units.push((
                CompilerSuiteShardUnit {
                    reference: reference.clone(),
                    case_slice: None,
                },
                weight,
            ));
            continue;
        }
        let count = usize::try_from(slices).unwrap_or(partition_count);
        let slice_weight = weight / slices;
        for index in 0..count {
            units.push((
                CompilerSuiteShardUnit {
                    reference: reference.clone(),
                    case_slice: Some((index, count)),
                },
                slice_weight,
            ));
        }
    }
    units
}

/// Pack weighted units into `partition_count` bins greedily, heaviest first, and return the bin at `index`.
///
/// Ties on weight break on the root's source path, then its identity, then the slice index, so the same inputs
/// always produce the same shards on every machine. The selected units come back ordered by source path, identity
/// and slice.
pub(crate) fn pack_units_for_partition(
    mut units: Vec<(CompilerSuiteShardUnit, u64)>,
    index: usize,
    partition_count: usize,
) -> Vec<CompilerSuiteShardUnit> {
    units.sort_by(|(left, left_weight), (right, right_weight)| {
        right_weight
            .cmp(left_weight)
            .then_with(|| {
                left.reference
                    .target
                    .source_relative_path
                    .cmp(&right.reference.target.source_relative_path)
            })
            .then_with(|| left.reference.identity.cmp(&right.reference.identity))
            .then_with(|| left.case_slice.cmp(&right.case_slice))
    });
    let mut partitions = vec![Vec::new(); partition_count];
    let mut totals = vec![0_u64; partition_count];
    for (unit, weight) in units {
        let Some(lightest) = (0..partition_count).min_by_key(|bin| (totals[*bin], *bin)) else {
            break;
        };
        totals[lightest] = totals[lightest].saturating_add(weight);
        partitions[lightest].push(unit);
    }
    let mut selected = partitions.into_iter().nth(index).unwrap_or_default();
    selected.sort_by(|left, right| {
        left.reference
            .target
            .source_relative_path
            .cmp(&right.reference.target.source_relative_path)
            .then_with(|| left.reference.identity.cmp(&right.reference.identity))
            .then_with(|| left.case_slice.cmp(&right.case_slice))
    });
    selected
}

/// Read every root's per-case durations from one partition report, or from every report in a directory.
///
/// The map is keyed by the root's source-relative path, then by exact case name. A case present in two reports keeps
/// the larger measurement, for the same reason the root weights do: under-stating a case is the failure mode that
/// unbalances a slice. A missing or malformed record yields no measurements; the slice then spreads cases evenly by
/// name, which is balanced enough to be correct and is exactly what the first run on a fresh checkout gets.
pub(crate) fn measured_case_millis_from_report(report_path: &Path) -> BTreeMap<String, BTreeMap<String, u64>> {
    if report_path.is_dir() {
        let Ok(entries) = fs::read_dir(report_path) else {
            return BTreeMap::new();
        };
        let mut merged = BTreeMap::<String, BTreeMap<String, u64>>::new();
        let mut paths = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "json"))
            .collect::<Vec<_>>();
        paths.sort();
        for path in paths {
            for (root, cases) in measured_case_millis_from_report(&path) {
                let root_cases = merged.entry(root).or_default();
                for (name, millis) in cases {
                    let entry = root_cases.entry(name).or_insert(0);
                    *entry = (*entry).max(millis);
                }
            }
        }
        return merged;
    }
    let Ok(bytes) = fs::read(report_path) else {
        return BTreeMap::new();
    };
    let Ok(report) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return BTreeMap::new();
    };
    let mut measured = BTreeMap::<String, BTreeMap<String, u64>>::new();
    for root in report
        .get("native_test_roots")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(path) = root.get("source_relative_path").and_then(serde_json::Value::as_str) else {
            continue;
        };
        for timing in root
            .get("case_timings")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            if let (Some(name), Some(millis)) = (
                timing.get("name").and_then(serde_json::Value::as_str),
                timing.get("elapsed_ms").and_then(serde_json::Value::as_u64),
            ) {
                let entry = measured
                    .entry(path.to_string())
                    .or_default()
                    .entry(name.to_string())
                    .or_insert(0);
                *entry = (*entry).max(millis);
            }
        }
    }
    measured
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oven::legacy_cargo::OvenCompilerTestSuiteTargetKey;

    fn reference(path: &str, source_bytes: u64) -> OvenCompilerTestSuiteShardReference {
        OvenCompilerTestSuiteShardReference {
            identity: format!("sha256:{path}"),
            target: OvenCompilerTestSuiteTargetKey {
                package_name: "incan".to_string(),
                target_kind: "test".to_string(),
                target_name: path.trim_start_matches("tests/").trim_end_matches(".rs").to_string(),
                runner: "rustc-test".to_string(),
                source_relative_path: path.to_string(),
            },
            source_bytes,
        }
    }

    #[test]
    fn a_measured_giant_is_divided_and_everything_else_stays_whole() {
        let references = vec![
            reference("tests/giant.rs", 1),
            reference("tests/small_a.rs", 1),
            reference("tests/small_b.rs", 1),
        ];
        let measured = BTreeMap::from([
            ("tests/giant.rs".to_string(), 3_000_u64),
            ("tests/small_a.rs".to_string(), 500),
            ("tests/small_b.rs".to_string(), 500),
        ]);
        let units = divide_giant_roots(
            &references,
            4,
            |reference| measured[&reference.target.source_relative_path],
            |_| true,
        );
        // total 4000, share 1000: the giant is 3× the share → three slices; the small roots stay whole.
        let giant = units
            .iter()
            .filter(|(unit, _)| unit.reference.target.source_relative_path == "tests/giant.rs")
            .map(|(unit, weight)| (unit.case_slice, *weight))
            .collect::<Vec<_>>();
        assert_eq!(
            giant,
            vec![(Some((0, 3)), 1000), (Some((1, 3)), 1000), (Some((2, 3)), 1000)]
        );
        assert_eq!(units.len(), 5);
        assert!(units.iter().filter(|(unit, _)| unit.case_slice.is_none()).count() == 2);
    }

    #[test]
    fn a_rustdoc_root_is_never_divided_however_heavy() {
        let mut doctest = reference("src/lib.rs", 1);
        doctest.target.runner = "rustdoc-test".to_string();
        let references = vec![doctest, reference("tests/tiny.rs", 1)];
        let measured = BTreeMap::from([("src/lib.rs".to_string(), 10_000_u64), ("tests/tiny.rs".to_string(), 1)]);
        let units = divide_giant_roots(
            &references,
            4,
            |reference| measured[&reference.target.source_relative_path],
            |_| true,
        );
        assert!(units.iter().all(|(unit, _)| unit.case_slice.is_none()));
    }

    #[test]
    fn a_proxied_giant_is_never_divided() {
        let references = vec![reference("tests/giant.rs", 900_000), reference("tests/tiny.rs", 1)];
        let units = divide_giant_roots(&references, 2, |reference| reference.source_bytes, |_| false);
        assert!(units.iter().all(|(unit, _)| unit.case_slice.is_none()));
    }

    #[test]
    fn slices_are_capped_at_the_partition_count() {
        let references = vec![reference("tests/giant.rs", 1), reference("tests/tiny.rs", 1)];
        let measured = BTreeMap::from([
            ("tests/giant.rs".to_string(), 10_000_u64),
            ("tests/tiny.rs".to_string(), 1),
        ]);
        let units = divide_giant_roots(
            &references,
            3,
            |reference| measured[&reference.target.source_relative_path],
            |_| true,
        );
        let slices = units.iter().filter(|(unit, _)| unit.case_slice.is_some()).count();
        assert_eq!(slices, 3, "a root cannot be cut into more slices than there are shards");
    }

    #[test]
    fn packing_is_deterministic_and_every_unit_lands_in_exactly_one_partition() {
        let references = vec![
            reference("tests/giant.rs", 1),
            reference("tests/a.rs", 1),
            reference("tests/b.rs", 1),
            reference("tests/c.rs", 1),
        ];
        let measured = BTreeMap::from([
            ("tests/giant.rs".to_string(), 4_000_u64),
            ("tests/a.rs".to_string(), 1_000),
            ("tests/b.rs".to_string(), 1_000),
            ("tests/c.rs".to_string(), 1_000),
        ]);
        let units = || {
            divide_giant_roots(
                &references,
                4,
                |reference| measured[&reference.target.source_relative_path],
                |_| true,
            )
        };
        let mut seen = Vec::new();
        for index in 0..4 {
            let selected = pack_units_for_partition(units(), index, 4);
            assert_eq!(
                selected,
                pack_units_for_partition(units(), index, 4),
                "same inputs, same shard"
            );
            seen.extend(selected);
        }
        assert_eq!(
            seen.len(),
            units().len(),
            "no unit is lost or duplicated across partitions"
        );
        // total 7000, share 1750: the giant divides into ⌈4000/1750⌉ = 3 slices of ~1333 each; every shard then
        // carries about the same weight instead of one shard carrying the whole giant.
        let giant_slices = seen
            .iter()
            .filter(|unit| unit.reference.target.source_relative_path == "tests/giant.rs")
            .count();
        assert_eq!(giant_slices, 3);
    }

    #[test]
    fn per_case_durations_merge_across_partition_reports_keeping_the_larger() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = tempfile::tempdir()?;
        let write = |name: &str, roots: serde_json::Value| -> Result<(), Box<dyn std::error::Error>> {
            fs::write(
                directory.path().join(name),
                serde_json::to_vec(&serde_json::json!({ "native_test_roots": roots }))?,
            )?;
            Ok(())
        };
        write(
            "partition-0.json",
            serde_json::json!([{ "source_relative_path": "tests/one.rs",
                "case_timings": [{ "name": "slow", "elapsed_ms": 900 }, { "name": "fast", "elapsed_ms": 3 }] }]),
        )?;
        write(
            "partition-1.json",
            serde_json::json!([{ "source_relative_path": "tests/one.rs",
                "case_timings": [{ "name": "slow", "elapsed_ms": 1200 }] }]),
        )?;
        let measured = measured_case_millis_from_report(directory.path());
        assert_eq!(measured["tests/one.rs"]["slow"], 1200);
        assert_eq!(measured["tests/one.rs"]["fast"], 3);
        assert!(measured_case_millis_from_report(&directory.path().join("absent.json")).is_empty());
        Ok(())
    }
}
