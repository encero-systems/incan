"""Unit tests for reconcile_oven_partitions.py; run with `python3 -m unittest scripts/test_reconcile_oven_partitions.py`."""

import unittest
from pathlib import Path

import reconcile_oven_partitions as reconcile


def report(index, count, roots, success=True):
    return (
        Path(f"partition-{index}.json"),
        {
            "success": success,
            "selection": {"partition_index": index, "partition_count": count},
            "native_test_roots": roots,
        },
    )


def whole(path):
    return {"source_relative_path": path}


def sliced(path, index, count, inventory, selected):
    return {
        "source_relative_path": path,
        "case_slice": {"index": index, "count": count, "inventory_count": inventory, "selected": selected},
    }


class ReconcileTests(unittest.TestCase):
    def test_whole_roots_and_complete_slices_reconcile(self):
        reports = [
            report(0, 2, [whole("tests/a.rs"), sliced("tests/giant.rs", 0, 2, 3, ["x", "y"])]),
            report(1, 2, [whole("tests/b.rs"), sliced("tests/giant.rs", 1, 2, 3, ["z"])]),
        ]
        summary, problems = reconcile.reconcile(reports)
        self.assertEqual(problems, [])
        self.assertTrue(summary["complete_suite_evidence"])
        self.assertEqual((summary["whole_roots"], summary["sliced_roots"]), (2, 1))

    def test_a_missing_partition_is_a_gap(self):
        _, problems = reconcile.reconcile([report(0, 2, [whole("tests/a.rs")])])
        self.assertTrue(any("missing" in problem for problem in problems))

    def test_an_overlapping_or_short_slice_set_is_refused(self):
        overlap = [
            report(0, 2, [sliced("tests/g.rs", 0, 2, 2, ["x"])]),
            report(1, 2, [sliced("tests/g.rs", 1, 2, 2, ["x"])]),
        ]
        _, problems = reconcile.reconcile(overlap)
        self.assertTrue(any("more than one slice" in problem for problem in problems))
        short = [
            report(0, 2, [sliced("tests/g.rs", 0, 2, 3, ["x"])]),
            report(1, 2, [sliced("tests/g.rs", 1, 2, 3, ["y"])]),
        ]
        _, problems = reconcile.reconcile(short)
        self.assertTrue(any("distinct case(s)" in problem for problem in problems))

    def test_a_root_run_twice_or_both_ways_is_refused(self):
        twice = [report(0, 2, [whole("tests/a.rs")]), report(1, 2, [whole("tests/a.rs")])]
        _, problems = reconcile.reconcile(twice)
        self.assertTrue(any("run whole on 2" in problem for problem in problems))
        both = [report(0, 2, [whole("tests/a.rs")]), report(1, 2, [sliced("tests/a.rs", 0, 1, 1, ["x"])])]
        _, problems = reconcile.reconcile(both)
        self.assertTrue(any("both whole and as slices" in problem for problem in problems))

    def test_a_failed_partition_never_reconciles(self):
        reports = [report(0, 1, [whole("tests/a.rs")], success=False)]
        summary, problems = reconcile.reconcile(reports)
        self.assertFalse(summary["complete_suite_evidence"])
        self.assertTrue(any("did not succeed" in problem for problem in problems))


if __name__ == "__main__":
    unittest.main()
