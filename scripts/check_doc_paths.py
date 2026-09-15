#!/usr/bin/env python3
"""Check concrete repository paths in contributor Markdown against the current tree.

Fenced examples and diagrams are checked too: they often name real source files. Runtime and example paths
require an explicit document-scoped exception in scripts/check_doc_paths.allow. Each non-comment allowlist
line has three tab-separated fields: document (or *), path (exact or a /** subtree), and a reason.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
from pathlib import Path
import re
import sys
from typing import Iterator


DOCUMENTS = ("AGENTS.md", "CONTRIBUTING.md", "README.md", "src/README.md")
DOCUMENT_TREES = ("workspaces/docs-site/docs/contributing", ".agents")
PATH = re.compile(r"(?<![\w/.-])((?:src|crates|loaves|workspaces|tests|scripts|\.agents|\.github)/[A-Za-z0-9_./*{},?-]*)")


@dataclass(frozen=True)
class ExceptionRule:
    """A documented exception, scoped to a document and an exact path or named subtree."""

    document: str
    token: str
    reason: str

    def matches(self, document: str, token: str) -> bool:
        """Keep subtree exceptions beneath their slash boundary, never sibling prefixes."""
        if self.document not in ("*", document):
            return False
        if self.token.endswith("/**"):
            return token.startswith(self.token[:-2])
        return self.token == token


def documents(root: Path) -> list[Path]:
    """Inventory contributor Markdown deterministically without private runtime state."""
    result = {root / name for name in DOCUMENTS if (root / name).is_file()}
    for tree in DOCUMENT_TREES:
        result.update(path for path in (root / tree).rglob("*.md") if path.is_file())
    result.update(path for path in (root / "loaves").rglob("README.md") if path.is_file())
    return sorted(path for path in result if not path.is_relative_to(root / ".agents/state"))


def mentions(text: str) -> Iterator[tuple[int, str]]:
    """Read concrete tokens in prose and code, excluding embedded URL path segments."""
    for number, line in enumerate(text.splitlines(), 1):
        for match in PATH.finditer(line):
            token = match.group(1).rstrip(".,")
            if token:
                yield number, token


def expand_braces(token: str) -> Iterator[str]:
    """Expand the small brace lists used in module tables, checking each alternative."""
    match = re.search(r"\{([^{}]+)\}", token)
    if match is None:
        yield token
        return
    for alternative in match.group(1).split(","):
        yield from expand_braces(token[:match.start()] + alternative + token[match.end():])


def resolves(root: Path, token: str) -> bool:
    """Require a concrete path or nonempty glob inside the repository boundary."""
    if not (root / token).resolve().is_relative_to(root):
        return False
    if "*" in token or "?" in token:
        return any(path.exists() for path in root.glob(token))
    return (root / token).exists()


def read_allowlist(path: Path) -> list[ExceptionRule]:
    """Reject malformed configuration instead of silently disabling intended checks."""
    result = []
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        fields = line.split("\t", 2)
        if len(fields) != 3 or not all(field.strip() for field in fields):
            raise ValueError(f"{path}:{number}: expected document, path and reason separated by tabs")
        document, token, reason = (field.strip() for field in fields)
        if document != "*" and not document.endswith(".md"):
            raise ValueError(f"{path}:{number}: document must be a Markdown path or *")
        result.append(ExceptionRule(document, token, reason))
    return result


def main(argv: list[str] | None = None) -> int:
    """Report missing paths with source locations; configuration failures use exit status 2."""
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--allow", type=Path, help="override the repository's scripts/check_doc_paths.allow")
    parser.add_argument("--list", action="store_true", help="also print each checked mention")
    args = parser.parse_args(argv)
    root = args.root.resolve()
    try:
        allowed = read_allowlist(args.allow or root / "scripts/check_doc_paths.allow")
        missing = []
        checked = exempted = 0
        for document in documents(root):
            relative = document.relative_to(root).as_posix()
            for number, original in mentions(document.read_text(encoding="utf-8")):
                for token in expand_braces(original):
                    checked += 1
                    if args.list:
                        print(f"{relative}:{number}: {token}")
                    if any(rule.matches(relative, token) for rule in allowed):
                        exempted += 1
                    elif not resolves(root, token):
                        missing.append((relative, number, token))
        if missing:
            print(f"{len(missing)} of {checked} repository paths do not exist ({exempted} explicit exceptions):")
            for document, number, token in missing:
                print(f"  {document}:{number}: {token}")
            return 1
        print(f"doc path audit passed: {checked} mentions, {exempted} explicit exceptions")
        return 0
    except (OSError, ValueError) as error:
        print(f"doc path audit: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
