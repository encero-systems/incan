#!/usr/bin/env python3
"""Check that the active release surface is represented in docs, tests, and generated references."""

from __future__ import annotations

import dataclasses
import pathlib
import sys


ROOT = pathlib.Path(__file__).resolve().parents[1]


@dataclasses.dataclass(frozen=True)
class FileRequirement:
    path: str
    snippets: tuple[str, ...]


@dataclasses.dataclass(frozen=True)
class SurfaceRequirement:
    name: str
    files: tuple[FileRequirement, ...]


def main() -> int:
    missing: list[str] = []

    for requirement in RELEASE_0_4_REQUIREMENTS:
        missing.extend(check_surface(requirement))

    if missing:
        print("0.4 release surface gate failed:", file=sys.stderr)
        for item in missing:
            print(f"- {item}", file=sys.stderr)
        return 1

    print(f"0.4 release surface gate passed ({len(RELEASE_0_4_REQUIREMENTS)} surfaces)")
    return 0


def check_surface(requirement: SurfaceRequirement) -> list[str]:
    missing: list[str] = []
    for file_requirement in requirement.files:
        path = ROOT / file_requirement.path
        if not path.exists():
            missing.append(f"{requirement.name}: missing {file_requirement.path}")
            continue
        text = path.read_text(encoding="utf-8")
        for snippet in file_requirement.snippets:
            if snippet not in text:
                missing.append(f"{requirement.name}: {file_requirement.path} does not contain {snippet!r}")
    return missing


RELEASE_0_4_REQUIREMENTS: tuple[SurfaceRequirement, ...] = (
    SurfaceRequirement(
        name="release direction and scope guard",
        files=(
            FileRequirement(
                "workspaces/docs-site/docs/roadmap.md",
                (
                    "0.3 made real programs credible. 0.4 makes the stack tryable.",
                    "Explicit 0.4 exclusions",
                    "Broad language/runtime features that are not required by the installer, starter, diagnostics, inspection, build-report, or codegraph path.",
                ),
            ),
            FileRequirement(
                "workspaces/docs-site/docs/release_notes/0_4.md",
                (
                    "0.4 is a tooling, inspection, installer, starter, and first-contact release.",
                    "## Scope guard",
                    "[#223]",
                ),
            ),
        ),
    ),
    SurfaceRequirement(
        name="boundary parity and symbol identity",
        files=(
            FileRequirement(
                "tests/fixtures/boundary_parity/README.md",
                (
                    "provider-owned union wrappers through facades",
                    "decorated callable identity, aliases, partial presets",
                    "dependency-provided vocab activation",
                ),
            ),
            FileRequirement(
                "tests/integration_tests.rs",
                (
                    "boundary_parity_preserves_dependency_owned_union_helpers_through_facade",
                    "boundary_parity_preserves_decorated_alias_partial_identity_through_facade",
                    "boundary_parity_activates_dependency_vocab_across_check_fmt_and_test",
                ),
            ),
            FileRequirement(
                "tests/cli_integration.rs",
                ("test_partial_constructor_presets_materialize_const_metadata_issue753",),
            ),
            FileRequirement(
                "workspaces/docs-site/docs/release_notes/0_4.md",
                (
                    "Boundary parity fixtures",
                    "Checked API identity reuse",
                    "Partial preset const metadata",
                    "[#699]",
                    "[#753]",
                    "[#760]",
                ),
            ),
        ),
    ),
    SurfaceRequirement(
        name="preheat and test runner observability",
        files=(
            FileRequirement(
                "src/cli/test_runner/execution.rs",
                (
                    "generated_harness_preheat_fingerprint_changes_when_source_changes",
                    "generated_harness_preheat_fingerprint_includes_cargo_flags",
                ),
            ),
            FileRequirement(
                "src/cli/commands/lock.rs",
                (
                    "library_dependency_preheat_fingerprint_uses_separate_profile_domain",
                    "run_generated_library_dependency_preheat",
                ),
            ),
            FileRequirement(
                "tests/integration_tests.rs",
                ("e2e_generated_harness_preheat_is_fingerprinted",),
            ),
            FileRequirement(
                "tests/cli_integration.rs",
                ("build_lib_preheats_dependency_graph_for_generated_library_target",),
            ),
            FileRequirement(
                "workspaces/docs-site/docs/tooling/reference/cli_reference.md",
                (
                    "incan test -v",
                    "dependency preheat uses the generated library Cargo project and the same release-profile Cargo target directory",
                    "INCAN_LOCK_PREHEAT=0",
                ),
            ),
            FileRequirement(
                "workspaces/docs-site/docs/contributing/reference/release_surface_gates.md",
                (
                    "Downstream Timing Evidence",
                    "generated-library dependency preheat targets the generated library Cargo project",
                    "timing and observability evidence, not a downstream acceptance pass",
                ),
            ),
            FileRequirement(
                "workspaces/docs-site/docs/release_notes/0_4.md",
                (
                    "Preheat observability",
                    "Generated-library preheat alignment",
                    "[#707]",
                    "[#723]",
                    "[#697]",
                ),
            ),
        ),
    ),
    SurfaceRequirement(
        name="stable diagnostics and explain",
        files=(
            FileRequirement(
                "tests/cli_integration.rs",
                (
                    "check_json_reports_parser_diagnostics",
                    "check_json_reports_typechecker_diagnostics",
                    "check_json_reports_tooling_diagnostics",
                    "check_json_reports_import_diagnostics",
                    "explain_reports_known_and_unknown_diagnostic_codes",
                ),
            ),
            FileRequirement(
                "workspaces/docs-site/docs/tooling/reference/cli_reference.md",
                (
                    "### `incan check`",
                    "### `incan explain`",
                    "schema_version: 1",
                    "INCAN-T0001",
                ),
            ),
            FileRequirement(
                "workspaces/docs-site/docs/language/reference/feature_inventory.md",
                ("Stable diagnostics commands",),
            ),
            FileRequirement(
                "workspaces/docs-site/docs/release_notes/0_4.md",
                (
                    "Stable diagnostics",
                    "[#589]",
                    "[#590]",
                ),
            ),
        ),
    ),
    SurfaceRequirement(
        name="build reports and generated Rust inspection",
        files=(
            FileRequirement(
                "tests/cli_integration.rs",
                (
                    "build_report_json_describes_executable_build",
                    "build_report_output_file_describes_library_build",
                    "inspect_rust_reports_current_generated_rust_files",
                    "Widget docs survive into generated Rust",
                ),
            ),
            FileRequirement(
                "workspaces/docs-site/docs/tooling/reference/cli_reference.md",
                (
                    "--report json",
                    "--report-output <PATH>",
                    "### `incan inspect rust`",
                    "current backend output",
                    "source declarations carry checked docstrings",
                ),
            ),
            FileRequirement(
                "workspaces/docs-site/docs/language/reference/feature_inventory.md",
                ("Build reports and generated Rust inspection",),
            ),
            FileRequirement(
                "workspaces/docs-site/docs/release_notes/0_4.md",
                (
                    "Build reports and generated Rust inspection",
                    "Generated public Rust items now preserve checked source docstrings",
                    "[#567]",
                    "[#591]",
                ),
            ),
        ),
    ),
    SurfaceRequirement(
        name="codegraph inspection",
        files=(
            FileRequirement(
                "tests/cli_integration.rs",
                (
                    "inspect_codegraph_exports_multifile_imports_and_public_symbols",
                    "inspect_codegraph_tolerant_directory_keeps_parseable_facts_and_diagnostics",
                ),
            ),
            FileRequirement(
                "workspaces/docs-site/docs/tooling/reference/codegraph_inspection.md",
                (
                    "incan inspect codegraph",
                    "JSONL",
                    "std.graph",
                    "runtime library for graph values inside Incan programs",
                ),
            ),
            FileRequirement(
                "workspaces/docs-site/docs/language/reference/feature_inventory.md",
                ("Compiler-backed codegraph inspection",),
            ),
            FileRequirement(
                "workspaces/docs-site/docs/release_notes/0_4.md",
                (
                    "Codegraph inspection",
                    "[#573]",
                    "[#666]",
                ),
            ),
        ),
    ),
    SurfaceRequirement(
        name="sdk installer and starter flow",
        files=(
            FileRequirement(
                "release/sdk/manifest.schema.v1.json",
                (
                    '"schema_version"',
                    '"sdk_version"',
                    '"hosts"',
                    '"archive_sha256"',
                ),
            ),
            FileRequirement(
                "scripts/install-incan-sdk.sh",
                (
                    "Install the Incan SDK from a versioned release manifest.",
                    "native Windows is not supported by the 0.4 SDK installer",
                    "sha256_file",
                ),
            ),
            FileRequirement(
                "tests/sdk_installer_tests.rs",
                (
                    "sdk_installer_dry_run_selects_manifest_target_without_writing",
                    "sdk_installer_verifies_checksum_and_links_commands",
                ),
            ),
            FileRequirement(
                "tests/integration_tests.rs",
                ("zero_clone_starter_project_runs_tests_and_release_builds",),
            ),
            FileRequirement(
                "workspaces/docs-site/docs/tooling/tutorials/getting_started.md",
                (
                    "incan new hello --yes",
                    "incan build --release",
                    "What 0.4 is good for",
                ),
            ),
            FileRequirement(
                "workspaces/docs-site/docs/start_here/encero_stack.md",
                (
                    "Incan",
                    "InQL",
                    "Hees.ai",
                    "Pallay",
                ),
            ),
            FileRequirement(
                "workspaces/docs-site/docs/language/reference/feature_inventory.md",
                (
                    "SDK installer and release manifest",
                    "Zero-clone starter project flow",
                ),
            ),
            FileRequirement(
                "workspaces/docs-site/docs/release_notes/0_4.md",
                (
                    "SDK installer and starter path",
                    "[#428]",
                    "[#551]",
                    "[#553]",
                ),
            ),
        ),
    ),
)


if __name__ == "__main__":
    raise SystemExit(main())
