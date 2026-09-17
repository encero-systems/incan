#!/usr/bin/env python3
"""Keep hand-written version literals in step with the workspace version.

The workspace version in the root ``Cargo.toml`` is the single source of truth: binaries, the release manifest, the
Homebrew formula, and the npm and pip packages all derive from it. Most of that derivation happens at package time
into a staging directory, so the copies committed in the repository are never consulted by a release -- which is
precisely why they drift unnoticed, and why a reader cannot tell whether they are authoritative.

This checks only the files that are *supposed* to mirror the workspace version, in the version format each one
uses. It deliberately ignores version literals in tests and fixtures: several exist to exercise release-candidate
handling itself (PEP 440 normalization of ``0.4.0-rc1``, for instance), and demanding they track the workspace
would make this check something people switch off.

Example ``oven.lock`` files record the compiler that last wrote them and update when the examples are rebuilt, so
they are reported for awareness but never fail the check.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent


def workspace_version() -> str:
    """Read the single source-of-truth version from the root manifest."""
    manifest = (REPO_ROOT / "Cargo.toml").read_text(encoding="utf-8")
    match = re.search(r'^version = "([^"]+)"$', manifest, re.MULTILINE)
    if not match:
        raise SystemExit("check_release_version_consistency: no workspace version in Cargo.toml")
    return match.group(1)


def pep440(version: str) -> str:
    """Convert a Cargo version to the PEP 440 spelling the pip package uses.

    Mirrors ``pep440_version`` in ``workspaces/release/pip/prepare_package.py``; the two must agree or this check
    would report a false difference on every pre-release.
    """
    dev = re.fullmatch(r"(.+)-dev\.(\d+)\.(\d+)", version)
    if dev:
        # Cargo's dotted prerelease identifiers can distinguish `dev.1.1` from the reserved `dev.2`.
        # PEP 440 has only one numeric development-release field, so retain the extra identity as a valid local
        # version segment rather than emitting the invalid `0.6.0.dev1.1` spelling.
        return f"{dev.group(1)}.dev{dev.group(2)}+{dev.group(3)}"
    normalized = version.replace("-dev.", ".dev")
    return re.sub(r"-(a|b|rc)(\d+)$", r"\1\2", normalized)


def release_line_requirement(version: str) -> str:
    """Return the source SDK requirement for this release line, including its development cohort."""
    core = version.split("-", maxsplit=1)[0]
    match = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)", core)
    if not match:
        raise SystemExit(f"check_release_version_consistency: workspace version {version!r} is not complete semver")
    major, minor, _patch = (int(part) for part in match.groups())
    return f">={version},<{major}.{minor + 1}.0"


def mirrors(version: str) -> list[tuple[Path, re.Pattern[str], str]]:
    """Files carrying a literal that must equal the workspace version, with the spelling each one expects."""
    cargo_form = version
    pip_form = pep440(version)
    sdk_requirement = release_line_requirement(version)
    return [
        (Path("workspaces/release/npm/package.json"), re.compile(r'^\s*"version":\s*"([^"]+)"', re.MULTILINE), cargo_form),
        (Path("workspaces/release/pip/pyproject.toml"), re.compile(r'^version = "([^"]+)"', re.MULTILINE), pip_form),
        (
            Path("workspaces/release/pip/src/incan_toolchain/__init__.py"),
            re.compile(r'^__release_version__ = "([^"]+)"', re.MULTILINE),
            cargo_form,
        ),
        (
            Path("workspaces/release/pip/src/incan_toolchain/__init__.py"),
            re.compile(r'^__version__ = "([^"]+)"', re.MULTILINE),
            pip_form,
        ),
        (
            Path("loaves/stdlib/sdk-components.toml"),
            re.compile(r'^version = "([^"]+)"', re.MULTILINE),
            cargo_form,
        ),
        (
            Path("loaves/stdlib/sdk-components.toml"),
            re.compile(r'^compiler-requirement = "([^"]+)"', re.MULTILINE),
            sdk_requirement,
        ),
    ]


def workspace_member_names() -> list[str]:
    """Names of the workspace members that inherit the workspace version, read from their own manifests."""
    manifest = (REPO_ROOT / "Cargo.toml").read_text(encoding="utf-8")
    members = re.search(r"^members = \[(.*?)^\]", manifest, re.MULTILINE | re.DOTALL)
    if not members:
        raise SystemExit("check_release_version_consistency: no [workspace] members in Cargo.toml")
    names = []
    for relative in re.findall(r'"([^"]+)"', members.group(1)):
        member = (REPO_ROOT / relative / "Cargo.toml").read_text(encoding="utf-8")
        if not re.search(r"^version\.workspace = true$", member, re.MULTILINE):
            continue
        name = re.search(r'^name = "([^"]+)"$', member, re.MULTILINE)
        if name:
            names.append(name.group(1))
    return names


def workspace_lock_versions() -> list[tuple[str, str | None]]:
    """The version ``Cargo.lock`` records for every member inheriting the workspace version.

    The workspace root is virtual, so the lock has no single entry to read the version from; every member that
    declares ``version.workspace = true`` must agree with the workspace, and a member the lock does not know is as
    stale as a wrong number.
    """
    lock = REPO_ROOT / "Cargo.lock"
    if not lock.is_file():
        return []
    content = lock.read_text(encoding="utf-8")
    versions = []
    for name in workspace_member_names():
        match = re.search(rf'^name = "{re.escape(name)}"\nversion = "([^"]+)"', content, re.MULTILINE)
        versions.append((name, match.group(1) if match else None))
    return versions


def example_lock_versions() -> list[tuple[Path, str]]:
    """Compiler versions recorded in tracked example lockfiles, which follow whenever examples are rebuilt."""
    listing = subprocess.run(
        ["git", "ls-files", "-z", "*oven.lock"], cwd=REPO_ROOT, capture_output=True, text=True, check=True
    )
    recorded: list[tuple[Path, str]] = []
    for name in listing.stdout.split("\0"):
        if not name:
            continue
        path = REPO_ROOT / name
        match = re.search(r'^incan-version = "([^"]+)"', path.read_text(encoding="utf-8"), re.MULTILINE)
        if match:
            recorded.append((Path(name), match.group(1)))
    return recorded


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--release-branch", default="", help="Pull-request source branch, when checking a release merge")
    parser.add_argument("--base-branch", default="", help="Pull-request destination branch")
    args = parser.parse_args()
    version = workspace_version()
    failures: list[str] = []

    # A development line may integrate work while retaining the preceding released baseline. Its PR into main is
    # the point at which it declares the new release, so mirror agreement alone is insufficient at that boundary.
    if (
        args.base_branch == "main"
        and re.fullmatch(r"\d+\.\d+\.\d+-dev\.\d+", args.release_branch)
        and version != args.release_branch
    ):
        failures.append(
            f"release branch {args.release_branch!r} targets main, but the workspace is {version!r}; "
            "bump the workspace version and its mirrors on the release branch before merging"
        )

    # `Cargo.lock` is not hand-written, but bumping the workspace version without letting Cargo refresh it leaves the
    # two disagreeing, and every release job builds with `--locked`. That fails on the runner rather than here, after
    # a tag is already pushed, so it is checked alongside the hand-written mirrors.
    lock_versions = workspace_lock_versions()
    if not lock_versions:
        failures.append("Cargo.lock: no workspace member version found to compare against the workspace version")
    for name, lock_version in lock_versions:
        if lock_version is None:
            failures.append(f"Cargo.lock: no entry for workspace member {name!r}")
        elif lock_version != version:
            failures.append(
                f"Cargo.lock: records {lock_version!r} for {name!r}, workspace is {version!r}"
                " (run any cargo command to refresh it, then commit the result)"
            )

    for relative, pattern, expected in mirrors(version):
        path = REPO_ROOT / relative
        if not path.is_file():
            failures.append(f"{relative}: expected to mirror the workspace version, but the file is missing")
            continue
        match = pattern.search(path.read_text(encoding="utf-8"))
        if not match:
            failures.append(f"{relative}: no version literal found to compare against {expected}")
        elif match.group(1) != expected:
            failures.append(f"{relative}: reads {match.group(1)!r}, workspace is {version!r} (expected {expected!r})")

    if failures:
        print(f"check_release_version_consistency: workspace is {version}, but version checks failed:")
        for failure in failures:
            print(f"  {failure}")
        print("\nThe npm and pip literals are overwritten during packaging, so a release still publishes the right")
        print("version — but the committed values look authoritative and are not. Cargo.lock is different: every")
        print("release job builds with --locked, so a stale lock fails the build on the runner after a tag is")
        print("already pushed. Update them to match the workspace version.")
        return 1

    print(f"check_release_version_consistency: workspace is {version}, mirrored literals agree")

    stale = [(path, recorded) for path, recorded in example_lock_versions() if recorded != version]
    if stale:
        print(f"  note: {len(stale)} example lockfile(s) still record an older compiler; they update when rebuilt:")
        for path, recorded in stale:
            print(f"    {path}: {recorded}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
