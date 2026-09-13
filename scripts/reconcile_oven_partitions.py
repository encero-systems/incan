"""Prove that the partitions of one Oven compiler-suite replay jointly cover every root exactly once.

A partition that runs a root as a case slice reports `joint-coverage-partition` and claims no complete-root
evidence of its own (#1549). Completeness is a property of the whole set of partition reports, and this is where it
is established: every partition index must be present, every root run whole must appear once, and every root run
in slices must have each slice index exactly once, agreeing on the slice count and the live inventory size, with the
selected cases disjoint and adding up to that inventory. Any gap, overlap, or disagreement fails the run.

Usage: reconcile_oven_partitions.py <report-or-directory>... [--summary <path>]
Exit status 0 when the set reconciles, 1 when it does not, 2 on unreadable input.
"""

import json
import sys
from collections import defaultdict
from pathlib import Path


def load_reports(arguments):
    """Read every JSON report named, expanding directories, skipping wall-time sidecars."""
    reports = []
    for argument in arguments:
        path = Path(argument)
        candidates = sorted(path.glob("*.json")) if path.is_dir() else [path]
        for candidate in candidates:
            if candidate.name.endswith(".wall-time.json"):
                continue
            try:
                with candidate.open() as handle:
                    reports.append((candidate, json.load(handle)))
            except (OSError, ValueError) as error:
                print(f"unreadable partition report {candidate}: {error}", file=sys.stderr)
                sys.exit(2)
    return reports


def reconcile(reports):
    """Return (summary, problems) for a set of partition reports."""
    problems = []
    partition_count = None
    seen_indices = set()
    whole = defaultdict(list)
    slices = defaultdict(list)
    for path, report in reports:
        selection = report.get("selection") or {}
        count = selection.get("partition_count")
        index = selection.get("partition_index")
        if count is None or index is None:
            problems.append(f"{path.name}: not a partition report (no partition index/count)")
            continue
        if partition_count is None:
            partition_count = count
        elif partition_count != count:
            problems.append(f"{path.name}: partition count {count} disagrees with {partition_count}")
        if index in seen_indices:
            problems.append(f"{path.name}: partition {index} reported twice")
        seen_indices.add(index)
        if not report.get("success", False):
            problems.append(f"{path.name}: partition {index} did not succeed")
        for root in report.get("native_test_roots") or []:
            root_path = root.get("source_relative_path", "?")
            slice_report = root.get("case_slice")
            if slice_report is None:
                whole[root_path].append(index)
            else:
                slices[root_path].append((index, slice_report))
    if partition_count is not None:
        missing = sorted(set(range(partition_count)) - seen_indices)
        if missing:
            problems.append(f"partition report(s) missing for index(es) {missing}")
    for root_path, indices in sorted(whole.items()):
        if len(indices) != 1:
            problems.append(f"{root_path}: run whole on {len(indices)} partitions {sorted(indices)}")
        if root_path in slices:
            problems.append(f"{root_path}: run both whole and as slices")
    sliced_roots = 0
    for root_path, entries in sorted(slices.items()):
        sliced_roots += 1
        counts = {entry["count"] for _, entry in entries}
        inventories = {entry["inventory_count"] for _, entry in entries}
        if len(counts) != 1:
            problems.append(f"{root_path}: slices disagree on slice count {sorted(counts)}")
            continue
        if len(inventories) != 1:
            problems.append(f"{root_path}: slices saw different inventories {sorted(inventories)}")
            continue
        count = counts.pop()
        indices = sorted(entry["index"] for _, entry in entries)
        if indices != list(range(count)):
            problems.append(f"{root_path}: slice indices {indices} do not cover 0..{count - 1} exactly once")
        selected = [name for _, entry in entries for name in entry.get("selected", [])]
        if len(selected) != len(set(selected)):
            problems.append(f"{root_path}: a case was selected by more than one slice")
        inventory_count = inventories.pop()
        if len(set(selected)) != inventory_count:
            problems.append(
                f"{root_path}: slices selected {len(set(selected))} distinct case(s) of an inventory of {inventory_count}"
            )
    summary = {
        "partition_count": partition_count,
        "partitions_seen": sorted(seen_indices),
        "whole_roots": len(whole),
        "sliced_roots": sliced_roots,
        "complete_suite_evidence": not problems and partition_count is not None,
        "problems": problems,
    }
    return summary, problems


def main(argv):
    arguments = []
    summary_path = None
    iterator = iter(argv)
    for argument in iterator:
        if argument == "--summary":
            summary_path = Path(next(iterator, ""))
        else:
            arguments.append(argument)
    if not arguments:
        print(__doc__, file=sys.stderr)
        return 2
    summary, problems = reconcile(load_reports(arguments))
    rendered = json.dumps(summary, indent=2, sort_keys=True)
    if summary_path:
        summary_path.parent.mkdir(parents=True, exist_ok=True)
        summary_path.write_text(rendered + "\n")
    print(rendered)
    for problem in problems:
        print(f"reconciliation: {problem}", file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
