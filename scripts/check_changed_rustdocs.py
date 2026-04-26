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
HUNK_RE = re.compile(r"^@@ -\d+(?:,\d+)? \+(?P<start>\d+)(?:,(?P<count>\d+))? @@")


def changed_rust_files() -> dict[Path, set[int]]:
    """Return changed Rust source files and their changed current-file line numbers."""
    result = subprocess.run(
        ["git", "diff", "--unified=0", "--", "*.rs"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=True,
    )
    files: dict[Path, set[int]] = {}
    current_path: Path | None = None
    for raw in result.stdout.splitlines():
        raw = raw.strip()
        if raw.startswith("+++ b/"):
            rel = raw.removeprefix("+++ b/")
            current_path = ROOT / rel
            if (
                not current_path.is_file()
                or "/tests/" in rel
                or rel.startswith("tests/")
                or rel.endswith("/tests.rs")
                or "/examples/" in rel
                or rel.startswith("examples/")
            ):
                current_path = None
                continue
            files.setdefault(current_path, set())
            continue
        if current_path is None:
            continue
        match = HUNK_RE.match(raw)
        if match is None:
            continue
        start = int(match.group("start"))
        count = int(match.group("count") or "1")
        if count == 0:
            continue
        files[current_path].update(range(start, start + count))
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


def quote_macro_lines(lines: list[str]) -> set[int]:
    """Return line numbers that live inside simple `quote! { ... }` token blocks."""
    quoted: set[int] = set()
    depth = 0
    active = False

    for index, line in enumerate(lines, start=1):
        if not active and "quote!" in line and "{" in line:
            active = True
            after_quote = line.split("quote!", 1)[1]
            depth = after_quote.count("{") - after_quote.count("}")
            quoted.add(index)
            if depth <= 0:
                active = False
            continue

        if active:
            quoted.add(index)
            depth += line.count("{")
            depth -= line.count("}")
            if depth <= 0:
                active = False

    return quoted


def function_end_line(lines: list[str], fn_index: int) -> int:
    """Return the best-effort inclusive end line for a function starting at `fn_index`."""
    depth = 0
    saw_body = False
    for index in range(fn_index, len(lines)):
        line = lines[index]
        depth += line.count("{")
        if "{" in line:
            saw_body = True
        depth -= line.count("}")
        if saw_body and depth <= 0:
            return index + 1
        if not saw_body and line.rstrip().endswith(";"):
            return index + 1
    return len(lines)


def missing_docs(path: Path, changed_lines: set[int]) -> list[tuple[int, str]]:
    """Return undocumented non-test function definitions for one Rust source file."""
    lines = path.read_text().splitlines()
    test_lines = test_module_lines(lines)
    quoted_lines = quote_macro_lines(lines)
    misses: list[tuple[int, str]] = []
    for index, line in enumerate(lines):
        match = FN_RE.match(line)
        if not match:
            continue
        line_no = index + 1
        if line_no in test_lines:
            continue
        if line_no in quoted_lines:
            continue
        end_line = function_end_line(lines, index)
        if not any(line_no <= changed <= end_line for changed in changed_lines):
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
    for path, changed_lines in changed_rust_files().items():
        for line, name in missing_docs(path, changed_lines):
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
