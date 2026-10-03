//! Joint coverage reconciliation for partitioned compiler-suite reports.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{CliError, CliResult, ExitCode};

#[derive(Debug, Deserialize)]
struct PartitionSelection {
    partition_index: Option<usize>,
    partition_count: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct PartitionRoot {
    source_relative_path: String,
    #[serde(default)]
    case_slice: Option<oven_rustc::native_test::OvenNativeTestCaseSliceReport>,
}

#[derive(Debug, Deserialize)]
struct PartitionReport {
    #[serde(default)]
    success: bool,
    selection: PartitionSelection,
    #[serde(default)]
    native_test_roots: Vec<PartitionRoot>,
}

#[derive(Debug, Serialize)]
struct ReconciliationSummary {
    partition_count: Option<usize>,
    partitions_seen: Vec<usize>,
    whole_roots: usize,
    sliced_roots: usize,
    complete_suite_evidence: bool,
    problems: Vec<String>,
}

/// Read partition reports, prove their joint coverage, and optionally publish the JSON summary.
pub fn oven_reconcile_partitions(inputs: &[PathBuf], summary_path: Option<&Path>) -> CliResult<ExitCode> {
    let reports = load_reports(inputs)?;
    let summary = reconcile(&reports);
    let rendered = serde_json::to_string_pretty(&summary)
        .map_err(|error| CliError::failure(format!("could not encode partition reconciliation: {error}")))?;
    if let Some(path) = summary_path {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                CliError::failure(format!(
                    "could not create reconciliation directory {}: {error}",
                    parent.display()
                ))
            })?;
        }
        fs::write(path, format!("{rendered}\n")).map_err(|error| {
            CliError::failure(format!(
                "could not write reconciliation summary {}: {error}",
                path.display()
            ))
        })?;
    }
    println!("{rendered}");
    for problem in &summary.problems {
        eprintln!("reconciliation: {problem}");
    }
    Ok(if summary.problems.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// Expand directory arguments, skip wall-time sidecars, and deserialize the compiler-owned report type.
fn load_reports(inputs: &[PathBuf]) -> CliResult<Vec<(PathBuf, PartitionReport)>> {
    let mut reports = Vec::new();
    for input in inputs {
        let mut candidates = if input.is_dir() {
            fs::read_dir(input)
                .map_err(|error| {
                    CliError::failure(format!("unreadable partition report {}: {error}", input.display()))
                })?
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|extension| extension == "json"))
                .collect::<Vec<_>>()
        } else {
            vec![input.clone()]
        };
        candidates.sort();
        for candidate in candidates {
            if candidate
                .file_name()
                .is_some_and(|name| name.to_string_lossy().ends_with(".wall-time.json"))
            {
                continue;
            }
            let bytes = fs::read(&candidate).map_err(|error| {
                CliError::failure(format!("unreadable partition report {}: {error}", candidate.display()))
            })?;
            let report = serde_json::from_slice(&bytes).map_err(|error| {
                CliError::failure(format!("unreadable partition report {}: {error}", candidate.display()))
            })?;
            reports.push((candidate, report));
        }
    }
    Ok(reports)
}

/// Reconcile every partition coordinate and every whole or sliced native-test root.
fn reconcile(reports: &[(PathBuf, PartitionReport)]) -> ReconciliationSummary {
    let mut problems = Vec::new();
    let mut partition_count = None;
    let mut seen = BTreeSet::new();
    let mut whole = BTreeMap::<String, Vec<usize>>::new();
    let mut slices = BTreeMap::<String, Vec<(usize, oven_rustc::native_test::OvenNativeTestCaseSliceReport)>>::new();
    for (path, report) in reports {
        let Some(count) = report.selection.partition_count else {
            problems.push(format!(
                "{}: not a partition report (no partition index/count)",
                display_name(path)
            ));
            continue;
        };
        let Some(index) = report.selection.partition_index else {
            problems.push(format!(
                "{}: not a partition report (no partition index/count)",
                display_name(path)
            ));
            continue;
        };
        if let Some(expected) = partition_count {
            if expected != count {
                problems.push(format!(
                    "{}: partition count {count} disagrees with {expected}",
                    display_name(path)
                ));
            }
        } else {
            partition_count = Some(count);
        }
        if !seen.insert(index) {
            problems.push(format!("{}: partition {index} reported twice", display_name(path)));
        }
        if !report.success {
            problems.push(format!("{}: partition {index} did not succeed", display_name(path)));
        }
        for root in &report.native_test_roots {
            if let Some(slice) = &root.case_slice {
                slices
                    .entry(root.source_relative_path.clone())
                    .or_default()
                    .push((index, slice.clone()));
            } else {
                whole.entry(root.source_relative_path.clone()).or_default().push(index);
            }
        }
    }
    if let Some(count) = partition_count {
        let missing = (0..count).filter(|index| !seen.contains(index)).collect::<Vec<_>>();
        if !missing.is_empty() {
            problems.push(format!("partition report(s) missing for index(es) {missing:?}"));
        }
    }
    for (root, indices) in &whole {
        if indices.len() != 1 {
            let mut sorted = indices.clone();
            sorted.sort_unstable();
            problems.push(format!("{root}: run whole on {} partitions {sorted:?}", indices.len()));
        }
        if slices.contains_key(root) {
            problems.push(format!("{root}: run both whole and as slices"));
        }
    }
    for (root, entries) in &slices {
        reconcile_slices(root, entries, &mut problems);
    }
    ReconciliationSummary {
        partition_count,
        partitions_seen: seen.into_iter().collect(),
        whole_roots: whole.len(),
        sliced_roots: slices.len(),
        complete_suite_evidence: problems.is_empty() && partition_count.is_some(),
        problems,
    }
}

/// Prove one sliced root has one common inventory, every slice index, and no overlapping selected case.
fn reconcile_slices(
    root: &str,
    entries: &[(usize, oven_rustc::native_test::OvenNativeTestCaseSliceReport)],
    problems: &mut Vec<String>,
) {
    let counts = entries.iter().map(|(_, slice)| slice.count).collect::<BTreeSet<_>>();
    let inventories = entries
        .iter()
        .map(|(_, slice)| slice.inventory_count)
        .collect::<BTreeSet<_>>();
    if counts.len() != 1 {
        problems.push(format!("{root}: slices disagree on slice count {counts:?}"));
        return;
    }
    if inventories.len() != 1 {
        problems.push(format!("{root}: slices saw different inventories {inventories:?}"));
        return;
    }
    let count = counts.iter().next().copied().unwrap_or_default();
    let mut indices = entries.iter().map(|(_, slice)| slice.index).collect::<Vec<_>>();
    indices.sort_unstable();
    if indices != (0..count).collect::<Vec<_>>() {
        problems.push(format!(
            "{root}: slice indices {indices:?} do not cover 0..{} exactly once",
            count.saturating_sub(1)
        ));
    }
    let selected = entries
        .iter()
        .flat_map(|(_, slice)| slice.selected.iter())
        .collect::<Vec<_>>();
    let distinct = selected.iter().copied().collect::<BTreeSet<_>>();
    if selected.len() != distinct.len() {
        problems.push(format!("{root}: a case was selected by more than one slice"));
    }
    let inventory = inventories.iter().next().copied().unwrap_or_default();
    if distinct.len() != inventory {
        problems.push(format!(
            "{root}: slices selected {} distinct case(s) of an inventory of {inventory}",
            distinct.len()
        ));
    }
}

/// Render only the filename in report-specific diagnostics, matching the former automation contract.
fn display_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use oven_rustc::native_test::OvenNativeTestCaseSliceReport;

    /// Construct one report fixture with compiler-owned report types.
    fn report(index: usize, count: usize, roots: Vec<PartitionRoot>) -> (PathBuf, PartitionReport) {
        let selection = PartitionSelection {
            partition_index: Some(index),
            partition_count: Some(count),
        };
        (
            PathBuf::from(format!("partition-{index}.json")),
            PartitionReport {
                success: true,
                selection,
                native_test_roots: roots,
            },
        )
    }

    /// Construct the report fields irrelevant to reconciliation with one chosen source path and slice.
    fn root(path: &str, slice: Option<OvenNativeTestCaseSliceReport>) -> PartitionRoot {
        PartitionRoot {
            source_relative_path: path.to_string(),
            case_slice: slice,
        }
    }

    #[test]
    fn whole_roots_and_complete_slices_reconcile() {
        let reports = vec![
            report(
                0,
                2,
                vec![
                    root("a", None),
                    root(
                        "g",
                        Some(OvenNativeTestCaseSliceReport {
                            index: 0,
                            count: 2,
                            inventory_count: 3,
                            selected: vec!["x".into(), "y".into()],
                        }),
                    ),
                ],
            ),
            report(
                1,
                2,
                vec![
                    root("b", None),
                    root(
                        "g",
                        Some(OvenNativeTestCaseSliceReport {
                            index: 1,
                            count: 2,
                            inventory_count: 3,
                            selected: vec!["z".into()],
                        }),
                    ),
                ],
            ),
        ];
        let summary = reconcile(&reports);
        assert!(summary.problems.is_empty());
        assert!(summary.complete_suite_evidence);
        assert_eq!((summary.whole_roots, summary.sliced_roots), (2, 1));
    }

    #[test]
    fn gaps_overlaps_and_disagreements_are_refused() {
        let missing = reconcile(&[report(0, 2, vec![root("a", None)])]);
        assert!(missing.problems.iter().any(|problem| problem.contains("missing")));
        let overlap = reconcile(&[
            report(
                0,
                2,
                vec![root(
                    "g",
                    Some(OvenNativeTestCaseSliceReport {
                        index: 0,
                        count: 2,
                        inventory_count: 2,
                        selected: vec!["x".into()],
                    }),
                )],
            ),
            report(
                1,
                2,
                vec![root(
                    "g",
                    Some(OvenNativeTestCaseSliceReport {
                        index: 1,
                        count: 2,
                        inventory_count: 2,
                        selected: vec!["x".into()],
                    }),
                )],
            ),
        ]);
        assert!(
            overlap
                .problems
                .iter()
                .any(|problem| problem.contains("more than one slice"))
        );
    }
}
