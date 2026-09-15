#!/usr/bin/env python3
"""Keep the rings' version lines and the workspace dependency table consistent.

Cargo allows one ``[workspace.package] version``, which is the toolchain's — what ``incan --version`` reports and what
the release scripts read. A ring that ships on its own carries its own line instead: every crate of that ring
declares the same explicit ``version``, and the root ``[workspace.dependencies]`` entry for each of those crates
carries that line as a requirement beside its ``path``, so the cross-ring edge is a semver requirement Cargo verifies
rather than an equality. Every other workspace crate inherits the workspace version and its table entry names no
requirement.

The table is also the one place a checkout names a workspace crate's directory: every member is listed there, and no
member manifest spells a ``path`` to another workspace crate.

The compiler ring does not link the runtime it generates for, so the emitter declares the stdlib line it generates
code for (``incan_emit::GENERATED_FOR_STDLIB_VERSION``) and this check keeps that declaration equal to the ring line.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

# Rings with their own version line, by crate name. A ring joins this table when its first independent release needs
# it; until then its crates inherit the workspace version. `incan_vocab` is the vocabulary registration contract that
# companion crates compile against, versioned on its own since before the rings existed.
RING_LINES: dict[str, tuple[str, ...]] = {
    "oven": ("oven_model", "oven_store", "oven_rustc"),
    "stdlib": ("incan_std_core", "incan_std_data", "incan_std_async", "incan_std_web", "incan_std_testing", "incan_derive", "incan_web_macros"),
    "vocab contract": ("incan_vocab",),
}

DEPENDENCY_LINE = re.compile(r'^(?P<name>[A-Za-z0-9_-]+) = (?P<spec>\{.*\}|"[^"]*")$', re.MULTILINE)


def table(manifest: str, name: str) -> str:
    """Return the body of the ``[name]`` table, up to the next table header; empty when the table is absent."""
    match = re.search(rf"^\[{re.escape(name)}\]\n(.*?)(?=^\[|\Z)", manifest, re.MULTILINE | re.DOTALL)
    return match.group(1) if match else ""


def workspace_members(root_manifest: str) -> list[str]:
    """The workspace member directories other than the root package itself."""
    members = re.search(r"^members = \[\n(.*?)^\]", root_manifest, re.MULTILINE | re.DOTALL)
    if members is None:
        raise SystemExit("check_ring_versions: Cargo.toml has no workspace member list")
    return [line.strip().strip('",') for line in members.group(1).strip().splitlines() if line.strip() != '".",']


def package_name(manifest: str) -> str | None:
    """The ``[package] name`` of a crate manifest."""
    match = re.search(r'^\[package\]\nname = "([^"]+)"', manifest, re.MULTILINE)
    return match.group(1) if match else None


def spec_value(spec: str, key: str) -> str | None:
    """The string value of ``key`` inside an inline dependency table, if any."""
    match = re.search(rf'\b{re.escape(key)} = "([^"]+)"', spec)
    return match.group(1) if match else None


def check(root: Path) -> list[str]:
    """Return every inconsistency between the ring lines, the crate manifests, and the dependency table."""
    failures: list[str] = []
    root_manifest = (root / "Cargo.toml").read_text(encoding="utf-8")
    ring_of = {crate: ring for ring, crates in RING_LINES.items() for crate in crates}
    entries = {
        match.group("name"): match.group("spec")
        for match in DEPENDENCY_LINE.finditer(table(root_manifest, "workspace.dependencies"))
    }

    # ---- Each member: its version line, and its dependency spellings ----
    lines: dict[str, dict[str, str]] = {ring: {} for ring in RING_LINES}
    names: dict[str, str] = {}
    for member in workspace_members(root_manifest):
        manifest_path = root / member / "Cargo.toml"
        manifest = manifest_path.read_text(encoding="utf-8")
        name = package_name(manifest)
        if name is None:
            failures.append(f"{member}/Cargo.toml: no [package] name")
            continue
        names[name] = member
        package = table(manifest, "package")
        explicit = re.search(r'^version = "([^"]+)"$', package, re.MULTILINE)
        inherits = re.search(r"^version\.workspace = true$", package, re.MULTILINE) is not None
        if name in ring_of:
            if explicit is None:
                failures.append(f"{member}/Cargo.toml: {name} is in the {ring_of[name]} ring and must declare its version line")
            else:
                lines[ring_of[name]][name] = explicit.group(1)
        elif not inherits:
            failures.append(f"{member}/Cargo.toml: {name} has no ring line and must inherit the workspace version")
        entry = entries.get(name)
        if entry is None:
            failures.append(f"Cargo.toml: [workspace.dependencies] does not name {name}")
        else:
            if spec_value(entry, "path") != member:
                failures.append(f"Cargo.toml: [workspace.dependencies].{name} must point at {member}")
            requirement = spec_value(entry, "version")
            if name not in ring_of and requirement is not None:
                failures.append(f"Cargo.toml: [workspace.dependencies].{name} carries a requirement but {name} has no ring line")
        for section in ("dependencies", "dev-dependencies", "build-dependencies"):
            for match in DEPENDENCY_LINE.finditer(table(manifest, section)):
                if spec_value(match.group("spec"), "path") is not None and match.group("name") in entries:
                    failures.append(
                        f"{member}/Cargo.toml: {match.group('name')} is spelled by path; inherit it from the table"
                    )

    # ---- The emitter's declared stdlib line ----
    emitter = root / "loaves/compiler/incan_emit/src/lib.rs"
    declared = re.search(r'^pub const GENERATED_FOR_STDLIB_VERSION: &str = "([^"]+)";$', emitter.read_text(encoding="utf-8"), re.MULTILINE)
    stdlib_line = lines["stdlib"].get("incan_stdlib")
    if declared is None:
        failures.append(f"{emitter.relative_to(root)}: no GENERATED_FOR_STDLIB_VERSION declaration")
    elif stdlib_line is not None and declared.group(1) != stdlib_line:
        failures.append(
            f"{emitter.relative_to(root)}: GENERATED_FOR_STDLIB_VERSION is {declared.group(1)!r} but the stdlib ring line is "
            f"{stdlib_line!r}; the emitter must generate for the line the ring ships"
        )

    # ---- Each ring: one line, and the table requires it ----
    for ring, crates in RING_LINES.items():
        declared = lines[ring]
        distinct = sorted(set(declared.values()))
        if len(distinct) > 1:
            failures.append(f"the {ring} ring declares more than one version line: {declared}")
        for crate in crates:
            if crate not in names:
                failures.append(f"the {ring} ring names {crate}, which is not a workspace member")
                continue
            line = declared.get(crate)
            requirement = spec_value(entries.get(crate, ""), "version")
            if line is not None and requirement != line:
                failures.append(
                    f"Cargo.toml: [workspace.dependencies].{crate} must require the {ring} ring's line {line!r}, "
                    f"not {requirement!r}"
                )
    return failures


def main() -> int:
    failures = check(REPO_ROOT)
    if failures:
        print("check_ring_versions: the ring version lines and the workspace dependency table disagree:")
        for failure in failures:
            print(f"  {failure}")
        return 1
    print(
        "check_ring_versions: "
        + ", ".join(f"{ring} ring on its own line" for ring in RING_LINES)
        + "; every other crate inherits the workspace version"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
