#!/usr/bin/env python3
"""Emit a strict-surface audit report for generated Rust artifacts.

The report is intentionally objective: it records artifact availability,
surface class, simple marker counts, and structured review placeholders. It
does not score generated Rust quality automatically.
"""

from __future__ import annotations

import argparse
import json
import sys
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Iterable


ROOT = Path(__file__).resolve().parents[1]

DEFAULT_ARTIFACTS = (
    ("program-main", "target/incan/std_encoding_hex_surface/src/main.rs"),
    ("stdlib-copy", "target/incan/std_ordinal_map_surface/src/__incan_std/collections.rs"),
    ("surface-fixture", "target/incan/std_regex_surface/src"),
)

MARKER_PATTERNS = {
    "clone": (".clone(", ".to_owned("),
    "allocation": (
        "Box::new(",
        "BTreeMap::new(",
        "HashMap::new(",
        "HashSet::new(",
        "String::from(",
        "String::new(",
        "Vec::new(",
        "format!(",
        "to_string(",
        "vec![",
    ),
    "eager_collection": (".collect()", ".collect::<", "collect_vec("),
}


@dataclass(frozen=True)
class Marker:
    """One literal marker found in generated Rust source."""

    path: str
    line: int
    pattern: str


@dataclass(frozen=True)
class ReviewNote:
    """Structured placeholder for manual audit notes."""

    status: str
    marker_count: int
    markers: list[Marker]
    notes: str


@dataclass(frozen=True)
class ArtifactReport:
    """Audit report for one generated Rust artifact or artifact directory."""

    surface_class: str
    artifact_path: str
    check_status: str
    strictness_status: str
    rust_files: list[str]
    clone: ReviewNote
    allocation: ReviewNote
    eager_collection: ReviewNote
    message: str


def relative(path: Path) -> str:
    """Return `path` relative to the repository root when possible."""
    try:
        return path.resolve().relative_to(ROOT).as_posix()
    except ValueError:
        return path.as_posix()


def artifact_specs(values: list[str]) -> list[tuple[str, Path]]:
    """Parse `SURFACE_CLASS=PATH` artifact specs."""
    if not values:
        return [(surface_class, ROOT / rel_path) for surface_class, rel_path in DEFAULT_ARTIFACTS]

    specs: list[tuple[str, Path]] = []
    for value in values:
        if "=" not in value:
            raise argparse.ArgumentTypeError(
                f"artifact `{value}` must use SURFACE_CLASS=PATH form"
            )
        surface_class, raw_path = value.split("=", 1)
        surface_class = surface_class.strip()
        raw_path = raw_path.strip()
        if not surface_class:
            raise argparse.ArgumentTypeError("artifact surface class cannot be empty")
        if not raw_path:
            raise argparse.ArgumentTypeError("artifact path cannot be empty")
        path = Path(raw_path)
        if not path.is_absolute():
            path = ROOT / path
        specs.append((surface_class, path))
    return specs


def rust_files_for(path: Path) -> tuple[str, list[Path], str]:
    """Return check status, Rust files, and a message for one artifact path."""
    if not path.exists():
        return "missing", [], "artifact path does not exist"
    if path.is_file():
        if path.suffix == ".rs":
            return "present", [path], "artifact file is available"
        return "no-rust-files", [], "artifact file is not a Rust source file"
    if path.is_dir():
        files = sorted(candidate for candidate in path.rglob("*.rs") if candidate.is_file())
        if files:
            return "present", files, f"found {len(files)} Rust source file(s)"
        return "no-rust-files", [], "artifact directory contains no Rust source files"
    return "unsupported-path", [], "artifact path is neither a file nor a directory"


def find_markers(files: Iterable[Path], patterns: tuple[str, ...]) -> list[Marker]:
    """Find literal marker occurrences in generated Rust source files."""
    markers: list[Marker] = []
    for path in files:
        try:
            lines = path.read_text(encoding="utf-8").splitlines()
        except UnicodeDecodeError:
            markers.append(Marker(path=relative(path), line=0, pattern="<non-utf8>"))
            continue
        for line_number, line in enumerate(lines, start=1):
            for pattern in patterns:
                start = 0
                while (start := line.find(pattern, start)) != -1:
                    markers.append(Marker(path=relative(path), line=line_number, pattern=pattern))
                    start += len(pattern)
    return markers


def review_note(files: list[Path], marker_name: str, check_status: str) -> ReviewNote:
    """Build the structured manual-review placeholder for one marker class."""
    if check_status != "present":
        return ReviewNote(
            status="not_available",
            marker_count=0,
            markers=[],
            notes="artifact not available for manual review",
        )
    markers = find_markers(files, MARKER_PATTERNS[marker_name])
    return ReviewNote(
        status="pending_manual_review",
        marker_count=len(markers),
        markers=markers,
        notes="",
    )


def audit_artifact(surface_class: str, path: Path) -> ArtifactReport:
    """Build an objective audit report entry for one artifact."""
    check_status, files, message = rust_files_for(path)
    strictness_status = "available_for_review" if check_status == "present" else "not_evaluated"
    return ArtifactReport(
        surface_class=surface_class,
        artifact_path=relative(path),
        check_status=check_status,
        strictness_status=strictness_status,
        rust_files=[relative(file) for file in files],
        clone=review_note(files, "clone", check_status),
        allocation=review_note(files, "allocation", check_status),
        eager_collection=review_note(files, "eager_collection", check_status),
        message=message,
    )


def json_report(reports: list[ArtifactReport]) -> str:
    """Render reports as stable JSON."""
    payload = {
        "report": "generated-rust-strict-surface",
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "artifacts": [asdict(report) for report in reports],
    }
    return json.dumps(payload, indent=2, sort_keys=True) + "\n"


def marker_summary(note: ReviewNote) -> str:
    """Render a compact marker summary for markdown tables."""
    if note.status == "not_available":
        return "not available"
    return f"{note.marker_count} marker(s); notes: pending"


def note_summary(note: ReviewNote) -> str:
    """Render the manual-note placeholder text for markdown details."""
    return note.notes if note.notes else "pending"


def markdown_report(reports: list[ArtifactReport]) -> str:
    """Render reports as Markdown for reviewer handoff."""
    lines = [
        "# Generated Rust Strict Surface Report",
        "",
        f"Generated at: `{datetime.now(timezone.utc).isoformat()}`",
        "",
        (
            "| Surface class | Artifact path | Check status | Strictness status | "
            "Clone notes | Allocation notes | Eager collection notes |"
        ),
        (
            "| ------------- | ------------- | ------------ | ----------------- | "
            "----------- | ---------------- | ---------------------- |"
        ),
    ]
    for report in reports:
        lines.append(
            "| "
            + " | ".join(
                [
                    f"`{report.surface_class}`",
                    f"`{report.artifact_path}`",
                    report.check_status,
                    report.strictness_status,
                    marker_summary(report.clone),
                    marker_summary(report.allocation),
                    marker_summary(report.eager_collection),
                ]
            )
            + " |"
        )
    lines.extend(["", "## Artifact Details", ""])
    for report in reports:
        lines.extend(
            [
                f"### `{report.surface_class}`",
                "",
                f"- Artifact path: `{report.artifact_path}`",
                f"- Check status: `{report.check_status}`",
                f"- Strictness status: `{report.strictness_status}`",
                f"- Message: {report.message}",
                f"- Rust files: {len(report.rust_files)}",
                (
                    "- Clone notes: "
                    f"status=`{report.clone.status}`, markers={report.clone.marker_count}, "
                    f"notes={note_summary(report.clone)}"
                ),
                (
                    "- Allocation notes: "
                    f"status=`{report.allocation.status}`, markers={report.allocation.marker_count}, "
                    f"notes={note_summary(report.allocation)}"
                ),
                (
                    "- Eager collection notes: "
                    f"status=`{report.eager_collection.status}`, "
                    f"markers={report.eager_collection.marker_count}, "
                    f"notes={note_summary(report.eager_collection)}"
                ),
                "",
            ]
        )
    return "\n".join(lines)


def parse_args(argv: list[str]) -> argparse.Namespace:
    """Parse command-line arguments."""
    parser = argparse.ArgumentParser(
        description="Emit a generated Rust strict-surface audit report."
    )
    parser.add_argument(
        "--artifact",
        action="append",
        default=[],
        metavar="SURFACE_CLASS=PATH",
        help=(
            "Generated Rust artifact file or directory to audit. "
            "May be repeated. Defaults to representative target/incan fixtures."
        ),
    )
    parser.add_argument(
        "--format",
        choices=("markdown", "json"),
        default="markdown",
        help="Report output format.",
    )
    parser.add_argument(
        "--output",
        type=Path,
        help="Write the report to this path instead of stdout.",
    )
    parser.add_argument(
        "--fail-on-missing",
        action="store_true",
        help="Exit non-zero when any requested artifact is missing or contains no Rust files.",
    )
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    """Run the generated Rust audit reporter."""
    args = parse_args(argv)
    try:
        specs = artifact_specs(args.artifact)
    except argparse.ArgumentTypeError as err:
        print(f"error: {err}", file=sys.stderr)
        return 2

    reports = [audit_artifact(surface_class, path) for surface_class, path in specs]
    rendered = json_report(reports) if args.format == "json" else markdown_report(reports)

    if args.output is not None:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(rendered, encoding="utf-8")
    else:
        print(rendered, end="")

    if args.fail_on_missing and any(report.check_status != "present" for report in reports):
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
