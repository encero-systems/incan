#!/usr/bin/env python3
"""Fail when a docs-site reference page states more than its contract.

A page under a `reference/` directory states the public contract and nothing else (see *Reference pages state the
contract only* in `.agents/skills/review-docs-claims/SKILL.md`). This gate catches the lexical part of that rule:
comparisons with other languages, time-bound wording, issue and RFC numbers, advice, rationale, walkthrough headings,
quoted diagnostic output, example comments that do more than say accepted or refused, and, on language reference pages,
compiler-internal vocabulary. It cannot judge meaning, so review still reads every changed page; the gate makes the common
failures impossible to miss.

The whole page is checked, not only the changed lines: a change that touches a reference page brings the whole page to
contract. By default the gate checks reference pages with staged or unstaged changes. Pass `--base <ref>` (or set
`INCAN_REFERENCE_CHECK_BASE`) to check every reference page a branch changes against that base, `--all` to report on
every reference page, or explicit paths.

Intentional exceptions live in `scripts/check_reference_contract.allow`, one tab-separated page, exact phrase and
reason per line. The page may be `*`.
"""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DOCS = ROOT / "workspaces" / "docs-site" / "docs"
ALLOW_FILE = ROOT / "scripts" / "check_reference_contract.allow"

# Pages generated from another source; their prose is fixed where it is generated.
GENERATED_PAGES = {
    "contributing/reference/test_corpus_inventory.md",
    "language/reference/language.md",
}

WORD_RULES: list[tuple[str, re.Pattern[str]]] = [
    ("comparison with another language", re.compile(r"\bPython\b|\bRust's\b|\blike Rust\b|\bas in Rust\b")),
    (
        "time-bound wording",
        re.compile(r"\bcurrently\b|\btoday\b|\byet\b|\bfor now\b|\bno longer\b|\bat present\b|\bat the moment\b", re.I),
    ),
    ("issue or RFC number", re.compile(r"(?<![\w`/\[#&-])#\d{3,5}\b|\bRFC[ -]?\d{2,3}\b")),
    (
        "advice",
        re.compile(
            r"\byou should\b|\bprefer\b|\brecommend(?:ed|s)?\b|\bconsider\b|\bmake sure\b|\bkeep in mind\b"
            r"|\bbest practice\b|\bwe suggest\b|\btip:",
            re.I,
        ),
    ),
    ("rationale", re.compile(r"\bbecause\b|\bso that\b", re.I)),
]

# Compiler-internal vocabulary is checked on language reference pages only; tooling and contributor references
# document the toolchain itself.
INTERNAL_VOCABULARY = (
    "compiler-internal vocabulary",
    re.compile(
        r"\blowering\b|\blowers? to\b|\bemitter\b|\bcodegen\b|\bthe checker\b|\btype ?checker\b|\bgenerated Rust\b",
        re.I,
    ),
)

WALKTHROUGH_HEADING = re.compile(r"^#{1,6}\s+(?:Examples?\s*:|Quick start|Walkthrough|Tutorial|Use cases?)", re.I)
QUOTED_DIAGNOSTIC = re.compile(r"^\s*(?:type error|error|warning)(?:\[[\w-]+\])?:\s", re.I)
EXPECTED_LINE = re.compile(r"^\s*Expected:\s", re.I)
INLINE_CODE = re.compile(r"`[^`]*`")
LINK_TARGET = re.compile(r"\]\([^)]*\)|^\[[^\]]+\]:\s*\S+.*$")
FENCE = re.compile(r"^\s*(```|~~~)")
STRING_LITERAL = re.compile(r'"(?:[^"\\]|\\.)*"|\'(?:[^\'\\]|\\.)*\'')
# A comment on a reference example says accepted or refused, with the contract reason if needed; a comment that is only
# a file name labels a multi-file example.
EXAMPLE_COMMENT = re.compile(r"^(?:accepted|refused)\b|^[\w./-]+\.(?:incn|toml)$", re.I)
DIAGNOSTIC_FENCE_LANGUAGES = {"", "bash", "text", "console", "shell", "sh", "txt", "output"}


@dataclass(frozen=True)
class Finding:
    """One sentence on a reference page that goes beyond the contract."""

    page: str
    line: int
    rule: str
    text: str


def load_allow() -> list[tuple[str, str]]:
    """Return the `(page, phrase)` exceptions recorded in the allow file."""
    if not ALLOW_FILE.exists():
        return []
    entries: list[tuple[str, str]] = []
    for raw in ALLOW_FILE.read_text().splitlines():
        if not raw.strip() or raw.startswith("#"):
            continue
        parts = raw.split("\t")
        if len(parts) >= 2:
            entries.append((parts[0].strip(), parts[1]))
    return entries


def is_reference_page(rel: str) -> bool:
    """Return whether a docs-relative path is a reference page this gate owns."""
    return rel.endswith(".md") and "/reference/" in f"/{rel}" and rel not in GENERATED_PAGES


def page_findings(path: Path, rel: str, allow: list[tuple[str, str]]) -> list[Finding]:
    """Check one whole reference page and return every finding not covered by an allow entry."""
    findings: list[Finding] = []
    in_fence = False
    fence_language = ""
    in_see_also = False
    check_internal = rel.startswith("language/reference/")
    for number, line in enumerate(path.read_text().splitlines(), start=1):
        fence = FENCE.match(line)
        if fence:
            if in_fence:
                in_fence = False
            else:
                in_fence = True
                fence_language = line.strip()[3:].strip().split()[0].lower() if line.strip()[3:].strip() else ""
            continue
        if in_fence:
            if fence_language in DIAGNOSTIC_FENCE_LANGUAGES and QUOTED_DIAGNOSTIC.match(line):
                findings.append(Finding(rel, number, "quoted diagnostic output", line.strip()))
            if fence_language == "incan":
                code = STRING_LITERAL.sub('""', line)
                if "#" in code and not EXAMPLE_COMMENT.match(code.split("#", 1)[1].strip()):
                    findings.append(Finding(rel, number, "example comment", line.strip()))
            continue
        if line.startswith("#"):
            in_see_also = bool(re.match(r"^#{1,6}\s+See also\b", line, re.I))
            if WALKTHROUGH_HEADING.match(line):
                findings.append(Finding(rel, number, "walkthrough heading", line.strip()))
        if in_see_also or line.lstrip().startswith("<!--"):
            continue
        if EXPECTED_LINE.match(line):
            findings.append(Finding(rel, number, "quoted diagnostic output", line.strip()))
        prose = LINK_TARGET.sub("", INLINE_CODE.sub("", line))
        rules = WORD_RULES + ([INTERNAL_VOCABULARY] if check_internal else [])
        for rule, pattern in rules:
            if pattern.search(prose):
                findings.append(Finding(rel, number, rule, line.strip()))
    return [
        finding
        for finding in findings
        if not any((page in ("*", finding.page)) and phrase in finding.text for page, phrase in allow)
    ]


def changed_pages(base: str | None) -> list[str]:
    """Return docs-relative reference pages changed against `base`, or in the working tree when `base` is None."""
    commands = (
        [["git", "diff", "--name-only", f"{base}...HEAD"], ["git", "diff", "--name-only", "HEAD"]]
        if base
        else [
            ["git", "diff", "--name-only"],
            ["git", "diff", "--cached", "--name-only"],
            ["git", "ls-files", "--others", "--exclude-standard"],
        ]
    )
    names: set[str] = set()
    for command in commands:
        result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True, check=False)
        if result.returncode != 0:
            sys.stderr.write(result.stderr)
            raise SystemExit(2)
        names.update(line.strip() for line in result.stdout.splitlines() if line.strip())
    prefix = "workspaces/docs-site/docs/"
    return sorted(
        name[len(prefix) :]
        for name in names
        if name.startswith(prefix) and (ROOT / name).exists() and is_reference_page(name[len(prefix) :])
    )


def main() -> int:
    """Run the gate and return its exit code."""
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("--base", default=os.environ.get("INCAN_REFERENCE_CHECK_BASE"))
    parser.add_argument("--all", action="store_true", help="check every reference page")
    parser.add_argument("paths", nargs="*", help="docs-site pages to check (repository-relative or docs-relative)")
    args = parser.parse_args()

    if args.paths:
        pages = []
        for raw in args.paths:
            rel = raw.removeprefix("workspaces/docs-site/docs/")
            pages.append(rel)
    elif args.all:
        pages = sorted(str(p.relative_to(DOCS)) for p in DOCS.rglob("*.md") if is_reference_page(str(p.relative_to(DOCS))))
    else:
        pages = changed_pages(args.base)

    allow = load_allow()
    findings = [finding for rel in pages for finding in page_findings(DOCS / rel, rel, allow)]
    if not findings:
        print(f"reference contract check passed: {len(pages)} reference page(s) checked")
        return 0
    print(f"reference contract check failed: {len(findings)} finding(s) in {len({f.page for f in findings})} page(s)")
    for finding in findings:
        print(f"  {finding.page}:{finding.line}: {finding.rule}: {finding.text[:160]}")
    print(
        "A reference page states the contract only; see *Reference pages state the contract only* in"
        " .agents/skills/review-docs-claims/SKILL.md. Move removed true sentences to the matching how-to or"
        " explanation page. Record a justified exception in scripts/check_reference_contract.allow."
    )
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
