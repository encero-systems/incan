#!/usr/bin/env python3
"""Fail when touched Rust source files contain undocumented non-test functions or methods.

This script is intentionally scoped to changed `.rs` files so the branch enforces a boyscout-style documentation
standard without requiring an immediate repo-wide documentation migration.

Eventually, we can replace this script with the following clippy rules:
#![warn(missing_docs)]
#![warn(clippy::missing_docs_in_private_items)]
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]

FN_RE = re.compile(
    r"^(?P<indent>\s*)(?:(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:const\s+)?(?:unsafe\s+)?(?:extern\s+\"[^\"]+\"\s+)?)fn\s+(?P<name>[A-Za-z_][A-Za-z0-9_]*)\b"
)
DOC_RE = re.compile(r"^\s*///|^\s*/\*\*")
ATTR_RE = re.compile(r"^\s*#\s*\[")


def changed_rust_files() -> list[Path]:
    """Return changed Rust source files that should satisfy the rustdoc gate."""
    result = subprocess.run(
        ["git", "diff", "--name-only", "--", "*.rs"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=True,
    )
    files: list[Path] = []
    for raw in result.stdout.splitlines():
        raw = raw.strip()
        if not raw:
            continue
        path = ROOT / raw
        if not path.is_file():
            continue
        if "/tests/" in raw or raw.startswith("tests/"):
            continue
        if "/examples/" in raw or raw.startswith("examples/"):
            continue
        files.append(path)
    return files


def has_doc_comment(lines: list[str], fn_index: int) -> bool:
    """Return whether the function at `fn_index` has a preceding rustdoc block."""
    i = fn_index - 1
    saw_attr = False
    while i >= 0:
        line = lines[i]
        stripped = line.strip()
        if not stripped:
            i -= 1
            continue
        if ATTR_RE.match(line):
            saw_attr = True
            i -= 1
            continue
        if DOC_RE.match(line):
            return True
        if saw_attr and DOC_RE.match(line):
            return True
        return False
    return False


def test_module_lines(lines: list[str]) -> set[int]:
    """Return line numbers that live inside `#[cfg(test)] mod ...` blocks."""
    lines_in_test_modules: set[int] = set()
    brace_depth = 0
    active_test_module_depth: int | None = None
    saw_test_cfg = False

    for index, line in enumerate(lines, start=1):
        stripped = line.strip()
        open_braces = line.count("{")
        close_braces = line.count("}")

        if stripped == "#[cfg(test)]":
            saw_test_cfg = True
        elif saw_test_cfg and stripped.startswith("mod ") and stripped.endswith("{"):
            active_test_module_depth = brace_depth + open_braces
            saw_test_cfg = False
        elif stripped and not stripped.startswith("#["):
            saw_test_cfg = False

        if active_test_module_depth is not None:
            lines_in_test_modules.add(index)

        brace_depth += open_braces
        brace_depth -= close_braces

        if active_test_module_depth is not None and brace_depth < active_test_module_depth:
            active_test_module_depth = None

    return lines_in_test_modules


def missing_docs(path: Path) -> list[tuple[int, str]]:
    """Return undocumented non-test function definitions for one Rust source file."""
    lines = path.read_text().splitlines()
    test_lines = test_module_lines(lines)
    misses: list[tuple[int, str]] = []
    for index, line in enumerate(lines):
        match = FN_RE.match(line)
        if not match:
            continue
        line_no = index + 1
        if line_no in test_lines:
            continue
        name = match.group("name")
        if name == "main":
            continue
        if not has_doc_comment(lines, index):
            misses.append((line_no, name))
    return misses


def main() -> int:
    """Run the touched-file rustdoc gate and print failures in `path:line:name` form."""
    misses: list[tuple[Path, int, str]] = []
    for path in changed_rust_files():
        for line, name in missing_docs(path):
            misses.append((path, line, name))

    if not misses:
        print("rustdoc gate passed")
        return 0

    print("missing rustdoc for changed Rust functions/methods:")
    for path, line, name in misses:
        print(f"{path.relative_to(ROOT)}:{line}: fn `{name}`")
    return 1


if __name__ == "__main__":
    sys.exit(main())
