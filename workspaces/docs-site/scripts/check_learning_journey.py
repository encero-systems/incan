#!/usr/bin/env python3
"""Validate the public learning routes and their canonical content contracts."""

from __future__ import annotations

import re
from pathlib import Path


DOCS_SITE = Path(__file__).resolve().parents[1]
DOCS = DOCS_SITE / "docs"
MKDOCS = DOCS_SITE / "mkdocs.yml"

ROUTES = {"evaluate", "try", "learn", "build", "bridge", "reference"}
PROJECT_TUTORIALS = (
    "tooling/tutorials/getting_started.md",
    "tooling/tutorials/your_first_project.md",
    "tooling/tutorials/build_and_consume_library.md",
    "tooling/tutorials/pipeline_mini_project.md",
    "tooling/tutorials/project_to_ci_artifact.md",
    "tooling/tutorials/diagnose_failed_build.md",
    "language/tutorials/typed_data_processor.md",
    "language/tutorials/build_your_first_api.md",
    "language/tutorials/async_worker_pipeline.md",
    "language/tutorials/add_a_rust_crate.md",
    "language/tutorials/guided_project.md",
)
STABLE_EXECUTABLE_TUTORIALS = {
    "tooling/tutorials/getting_started.md",
}
RELEASE_EXECUTABLE_TUTORIALS = {
    "tooling/tutorials/your_first_project.md",
    "tooling/tutorials/build_and_consume_library.md",
    "tooling/tutorials/pipeline_mini_project.md",
    "tooling/tutorials/diagnose_failed_build.md",
    "language/tutorials/typed_data_processor.md",
    "language/tutorials/build_your_first_api.md",
    "language/tutorials/async_worker_pipeline.md",
    "language/tutorials/add_a_rust_crate.md",
    "language/tutorials/guided_project.md",
}
SOURCE_VERIFIED_PREVIEWS = {
    "tooling/tutorials/project_to_ci_artifact.md",
}
VERIFIED_RANGES = {
    "tooling/tutorials/getting_started.md": "Incan <code>&gt;=0.5.0-0,&lt;0.6.0</code> release line",
}
DEFAULT_VERIFIED_RANGE = "Incan <code>&gt;=0.5.0-0,&lt;0.6.0</code>"
BRIDGES = (
    "start_here/coming_from_python.md",
    "start_here/coming_from_rust.md",
    "start_here/coming_from_typescript_javascript.md",
    "start_here/pipelines_and_automation.md",
)
CANONICAL_SETUP_REFERENCES = {
    "_snippets/learning/install_toolchain.md": {
        "tooling/tutorials/getting_started.md",
        "tooling/how-to/install_and_run.md",
    },
    "_snippets/learning/first_project_loop.md": {
        "tooling/tutorials/getting_started.md",
        "tooling/how-to/install_and_run.md",
    },
    "_snippets/learning/development_toolchain.md": {
        "tooling/how-to/install_and_run.md",
    },
}


def main() -> int:
    errors: list[str] = []

    def require(condition: bool, message: str) -> None:
        if not condition:
            errors.append(message)

    learn_hub = (DOCS / "start_here/index.md").read_text(encoding="utf-8")
    route_markers = set(re.findall(r'data-learning-route="([^"]+)"', learn_hub))
    require(route_markers == ROUTES, f"Learn hub routes differ from the six-intent contract: {sorted(route_markers)}")
    require(
        len(re.findall(r'data-learning-project="([^"]+)"', learn_hub)) >= 6,
        "Learn hub must expose at least six representative project paths",
    )
    project_statuses = re.findall(r'data-project-status="([^"]+)"', learn_hub)
    require(project_statuses.count("executable") >= 9, "Learn hub must expose the completed 0.5 executable project paths")
    require(project_statuses.count("preview") == 1, "Learn hub must keep only the hosted-delivery packaging preview")

    chooser = (DOCS / "start_here/incan_or_incql.md").read_text(encoding="utf-8")
    require("Incan-shaped" in chooser and "IncQL-shaped" in chooser, "Incan/IncQL chooser lacks both problem shapes")
    require(
        not re.search(r"https?://[^\s)>\"]*incql", chooser, flags=re.IGNORECASE),
        "Incan/IncQL chooser links to an unpublished IncQL destination",
    )

    for relative_path in BRIDGES:
        page = (DOCS / relative_path).read_text(encoding="utf-8")
        for heading in ("## Install once", "## What transfers", "## What changes", "## What not to expect"):
            require(heading in page, f"{relative_path} is missing bridge section {heading!r}")
        require("incan new " not in page, f"{relative_path} duplicates the canonical first-project commands")
        require("pipx install incan" not in page, f"{relative_path} duplicates an installer command")
        require("direct_install.sh" not in page, f"{relative_path} duplicates the direct installer snippet")

    setup_references: dict[str, set[str]] = {
        snippet: set() for snippet in CANONICAL_SETUP_REFERENCES
    }
    for path in DOCS.rglob("*.md"):
        page = path.read_text(encoding="utf-8")
        for snippet in setup_references:
            if snippet in page:
                setup_references[snippet].add(str(path.relative_to(DOCS)))
    for snippet, references in setup_references.items():
        expected_references = CANONICAL_SETUP_REFERENCES[snippet]
        require(
            references == expected_references,
            f"{snippet} references differ from the canonical setup pages: "
            f"expected {sorted(expected_references)}, found {sorted(references)}",
        )

    for relative_path in PROJECT_TUTORIALS:
        page = (DOCS / relative_path).read_text(encoding="utf-8")
        require('class="inc-tutorial-meta"' in page, f"{relative_path} has no tutorial metadata")
        for field in ("Reader", "Prerequisites", "Time", "Verified", "Status", "Outcome", "Artifacts"):
            require(f"<dt>{field}</dt>" in page, f"{relative_path} metadata is missing {field}")
        expected_range = VERIFIED_RANGES.get(relative_path, DEFAULT_VERIFIED_RANGE)
        require(expected_range in page, f"{relative_path} does not state the verified compiler range")
        require("inc-learning-panel--complete" in page, f"{relative_path} has no explicit completion condition")
        require(
            re.search(r"^## (Continue|Next)", page, flags=re.MULTILINE) is not None,
            f"{relative_path} has no reader-facing next step",
        )
        if relative_path in STABLE_EXECUTABLE_TUTORIALS:
            require("Stable-release executable" in page, f"{relative_path} is not labeled stable-release executable")
        elif relative_path in RELEASE_EXECUTABLE_TUTORIALS:
            require("Release-envelope executable" in page, f"{relative_path} is not labeled release-envelope executable")
        elif relative_path in SOURCE_VERIFIED_PREVIEWS:
            require("Source-verified" in page, f"{relative_path} is not labeled source-verified")
            require(
                "0.5 release envelope" in page or "0.5 release Loaf envelope" in page,
                f"{relative_path} does not explain its 0.5 release-envelope boundary",
            )

    source_setup = (DOCS / "_snippets/learning/development_toolchain.md").read_text(encoding="utf-8")
    require(
        "make test-prewarm-oven-release-loafs" in source_setup,
        "source toolchain setup does not prepare the bounded 0.5 release envelope",
    )

    mkdocs = MKDOCS.read_text(encoding="utf-8")
    require("  - Learn:" in mkdocs, "global navigation does not expose the Learn intent")
    require("Learning journey QA: project/learning_journey.md" in mkdocs, "learning QA contract is missing from Project")

    if errors:
        print("Learning journey contract failed:")
        for error in errors:
            print(f"- {error}")
        return 1

    print(
        "Learning journey contract passed "
        f"({len(ROUTES)} intents, {len(BRIDGES)} bridges, {len(PROJECT_TUTORIALS)} project tutorials)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
