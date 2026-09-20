#!/usr/bin/env python3
"""Inventory every Cargo manifest in the repository as the facts a `loaf.toml` Rust facet must carry (#1698).

The compiler is to be built by Oven from `loaf.toml` Rust facets (RFC 119) with no `Cargo.toml` left in the tree.
Before a manifest can be deleted, everything it declares that a compile unit depends on has to be expressible in the
facet. This script parses every `Cargo.toml` with `tomllib` and writes one JSON record per manifest: package
identity (with `workspace = true` inheritance resolved and recorded), edition, features and their defaults, normal /
dev / build dependencies split into workspace-internal and third-party with their version requirements, explicit
`[lib]` / `[[bin]]` / `[[test]]` / `[[example]]` / `[[bench]]` targets, procedural-macro and crate-type facts, build
scripts (inert per #1561), `[lints]` tables, `[package.metadata]`, and the crate-level `#![...]` attributes of every
target root (the in-source lint policy that stays as code).

Usage:

    python3 scripts/self_build/cargo_manifest_inventory.py            # rewrite the JSON inventory
    python3 scripts/self_build/cargo_manifest_inventory.py --check    # fail when the JSON on disk is stale
    python3 scripts/self_build/cargo_manifest_inventory.py --markdown # print the rendered per-manifest table

The JSON is committed next to this script so a reviewer can diff the inventory as the manifests retire; `--check`
keeps it reproducible. No Cargo process is involved: this reads TOML and source text only.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import tomllib
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
OUTPUT = Path(__file__).resolve().parent / "cargo_manifest_inventory.json"
SCHEMA_VERSION = 1

# Directories that hold build output, lane-private state or vendored trees rather than authored manifests.
SKIPPED_DIRECTORIES = {
    ".git",
    ".incan",
    ".lane",
    ".ralph-cache",
    "node_modules",
    "target",
    "incan_generated_shared_target",
}

# `[package]` keys that may be written `key.workspace = true` and inherited from `[workspace.package]`.
INHERITABLE_PACKAGE_KEYS = (
    "authors",
    "categories",
    "description",
    "documentation",
    "edition",
    "exclude",
    "homepage",
    "include",
    "keywords",
    "license",
    "license-file",
    "publish",
    "readme",
    "repository",
    "rust-version",
    "version",
)

DEPENDENCY_TABLES = (
    ("dependencies", "normal"),
    ("dev-dependencies", "dev"),
    ("build-dependencies", "build"),
)

INNER_ATTRIBUTE_RE = re.compile(r"^\s*#!\[(?P<body>.*)\]\s*$")


def find_manifests() -> list[Path]:
    """Every `Cargo.toml` below the repository root, in sorted portable order, skipping output and vendored trees."""
    found: list[Path] = []
    stack = [ROOT]
    while stack:
        directory = stack.pop()
        for entry in sorted(directory.iterdir(), key=lambda item: item.name):
            if entry.is_symlink():
                continue
            if entry.is_dir():
                if entry.name in SKIPPED_DIRECTORIES:
                    continue
                stack.append(entry)
            elif entry.name == "Cargo.toml":
                found.append(entry)
    return sorted(found, key=lambda path: path.relative_to(ROOT).as_posix())


def portable(path: Path) -> str:
    """Repository-relative POSIX path."""
    return path.relative_to(ROOT).as_posix()


def load_toml(path: Path) -> dict[str, Any]:
    """Parse one TOML document."""
    with path.open("rb") as handle:
        return tomllib.load(handle)


def classify(path: Path, document: dict[str, Any], member_directories: set[str]) -> str:
    """Which population the manifest belongs to; the packet plan treats each population differently."""
    relative = portable(path)
    directory = portable(path.parent) if path.parent != ROOT else "."
    if path.parent == ROOT:
        return "workspace-root"
    if directory in member_directories:
        return "workspace-member"
    if relative.startswith("loaves/third_party/"):
        return "third-party-patch"
    if "/fixtures/" in relative:
        return "fixture"
    if relative.startswith("examples/"):
        return "example-companion"
    if "workspace" in document:
        return "standalone"
    return "unclassified"


def resolve_package(
    package: dict[str, Any], workspace_package: dict[str, Any]
) -> tuple[dict[str, Any], list[str]]:
    """Resolve `key.workspace = true` against `[workspace.package]`; return the resolved table and the inherited keys."""
    resolved: dict[str, Any] = {}
    inherited: list[str] = []
    for key, value in package.items():
        if isinstance(value, dict) and value.get("workspace") is True:
            if key not in INHERITABLE_PACKAGE_KEYS or key not in workspace_package:
                raise SystemExit(f"package key `{key}` claims workspace inheritance but the root does not declare it")
            resolved[key] = workspace_package[key]
            inherited.append(key)
        else:
            resolved[key] = value
    return resolved, sorted(inherited)


def normalize_dependency(
    alias: str,
    entry: Any,
    workspace_dependencies: dict[str, Any],
    workspace_packages: set[str],
) -> dict[str, Any]:
    """One dependency as the facet needs it: package, origin, requirement, features and role flags.

    A `workspace = true` entry is merged over the root's `[workspace.dependencies]` declaration the way Cargo
    merges it: the member may add features and set `optional`; every other field comes from the root.
    """
    if isinstance(entry, str):
        table: dict[str, Any] = {"version": entry}
    elif isinstance(entry, dict):
        table = dict(entry)
    else:
        raise SystemExit(f"dependency `{alias}` has an unsupported shape: {entry!r}")
    inherited = table.pop("workspace", False) is True
    if inherited:
        root_entry = workspace_dependencies.get(alias)
        if root_entry is None:
            raise SystemExit(f"dependency `{alias}` inherits from the workspace but the root does not declare it")
        root_table = {"version": root_entry} if isinstance(root_entry, str) else dict(root_entry)
        member_features = list(table.pop("features", []) or [])
        member_optional = table.pop("optional", None)
        unexpected = sorted(table)
        if unexpected:
            raise SystemExit(f"dependency `{alias}` sets {unexpected} alongside workspace = true")
        table = root_table
        table["features"] = sorted(set(table.get("features", []) or []) | set(member_features))
        if member_optional is not None:
            table["optional"] = member_optional
    package = table.get("package", alias)
    if "path" in table:
        origin = "path"
    elif "git" in table:
        origin = "git"
    else:
        origin = "registry"
    kind = "workspace-internal" if package in workspace_packages else "third-party"
    record: dict[str, Any] = {
        "package": package,
        "kind": kind,
        "origin": origin,
        "workspace_inherited": inherited,
        "version_req": table.get("version"),
        "features": sorted(table.get("features", []) or []),
        "default_features": table.get("default-features", True),
        "optional": bool(table.get("optional", False)),
    }
    if package != alias:
        record["renamed_from"] = alias
    if origin == "path":
        record["path"] = table["path"]
    if origin == "git":
        record["git"] = {key: table[key] for key in ("git", "branch", "tag", "rev") if key in table}
    return record


def dependency_tables(
    document: dict[str, Any],
    workspace_dependencies: dict[str, Any],
    workspace_packages: set[str],
) -> dict[str, dict[str, Any]]:
    """Normal, dev and build dependency tables, plus any `[target.'cfg(..)'.*]` tables, normalized."""
    tables: dict[str, dict[str, Any]] = {}
    for key, role in DEPENDENCY_TABLES:
        entries = document.get(key, {}) or {}
        tables[role] = {
            alias: normalize_dependency(alias, entry, workspace_dependencies, workspace_packages)
            for alias, entry in sorted(entries.items())
        }
    target_tables: dict[str, Any] = {}
    for predicate, sections in sorted((document.get("target", {}) or {}).items()):
        for key, role in DEPENDENCY_TABLES:
            entries = sections.get(key, {}) or {}
            if entries:
                target_tables.setdefault(predicate, {})[role] = {
                    alias: normalize_dependency(alias, entry, workspace_dependencies, workspace_packages)
                    for alias, entry in sorted(entries.items())
                }
    tables["target"] = target_tables
    return tables


def crate_level_attributes(source: Path) -> list[str]:
    """The `#![...]` inner attributes at the top of one crate root, in file order.

    Only leading inner attributes are collected: they are the crate's lint and feature policy, which stays in the
    source when the manifest goes. The scan stops at the first line that is neither an inner attribute, a comment,
    nor blank, and it joins multi-line attributes so `#![deny(\n    clippy::unwrap_used,\n)]` is one entry.
    """
    if not source.is_file():
        return []
    attributes: list[str] = []
    pending: list[str] = []
    for raw_line in source.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if pending:
            pending.append(line)
            if line.endswith("]"):
                attributes.append(" ".join(pending))
                pending = []
            continue
        if not line or line.startswith("//"):
            continue
        if line.startswith("#!["):
            if line.endswith("]"):
                attributes.append(line)
            else:
                pending = [line]
            continue
        break
    return [re.sub(r"\s+", " ", attribute) for attribute in attributes]


def target_roots(manifest_dir: Path, document: dict[str, Any]) -> dict[str, str]:
    """Crate root source path per target (lib and every bin), explicit or by Cargo convention."""
    roots: dict[str, str] = {}
    package_name = document.get("package", {}).get("name", "")
    lib = document.get("lib")
    lib_path = None
    if isinstance(lib, dict) and "path" in lib:
        lib_path = manifest_dir / lib["path"]
    elif (manifest_dir / "src" / "lib.rs").is_file():
        lib_path = manifest_dir / "src" / "lib.rs"
    if lib_path is not None:
        roots["lib"] = portable(lib_path)
    for binary in document.get("bin", []) or []:
        name = binary.get("name", package_name)
        path = binary.get("path", f"src/bin/{name}.rs")
        roots[f"bin:{name}"] = portable(manifest_dir / path)
    if not document.get("bin") and (manifest_dir / "src" / "main.rs").is_file():
        roots[f"bin:{package_name}"] = portable(manifest_dir / "src" / "main.rs")
    return roots


def build_script(manifest_dir: Path, package: dict[str, Any]) -> dict[str, Any]:
    """Whether the package has a build script and what Cargo would do with it; Oven treats every one as inert."""
    declared = package.get("build")
    if declared is False:
        return {"present": False, "declared": False, "path": None}
    candidate = manifest_dir / (declared if isinstance(declared, str) else "build.rs")
    return {
        "present": candidate.is_file(),
        "declared": declared if declared is not None else None,
        "path": portable(candidate) if candidate.is_file() else None,
    }


def manifest_record(
    path: Path,
    workspace: dict[str, Any],
    member_directories: set[str],
    workspace_packages: set[str],
) -> dict[str, Any]:
    """The complete per-manifest inventory record."""
    document = load_toml(path)
    manifest_dir = path.parent
    category = classify(path, document, member_directories)
    package_table = document.get("package", {}) or {}
    workspace_package = workspace.get("package", {}) or {}
    workspace_dependencies = workspace.get("dependencies", {}) or {}
    package, inherited = (
        resolve_package(package_table, workspace_package)
        if category in {"workspace-member", "workspace-root"}
        else (dict(package_table), [])
    )
    lib = document.get("lib")
    record: dict[str, Any] = {
        "path": portable(path),
        "category": category,
        "cargo_lock_beside": (manifest_dir / "Cargo.lock").is_file(),
        "is_virtual_workspace": "package" not in document and "workspace" in document,
        "declares_own_workspace": "workspace" in document and path.parent != ROOT,
    }
    if package:
        record["package"] = {
            "name": package.get("name"),
            "version": package.get("version"),
            "edition": package.get("edition"),
            "rust_version": package.get("rust-version"),
            "license": package.get("license"),
            "publish": package.get("publish", True),
            "description": package.get("description"),
            "resolver": package.get("resolver"),
            "auto_discovery_off": sorted(
                key for key in ("autolib", "autobins", "autoexamples", "autotests", "autobenches") if package.get(key) is False
            ),
        }
        record["inherited_package_keys"] = inherited
        record["lib"] = (
            {
                "name": lib.get("name"),
                "path": lib.get("path"),
                "proc_macro": bool(lib.get("proc-macro", False)),
                "crate_type": lib.get("crate-type"),
                "doctest": lib.get("doctest", True),
            }
            if isinstance(lib, dict)
            else None
        )
        record["bins"] = [
            {"name": binary.get("name"), "path": binary.get("path")} for binary in document.get("bin", []) or []
        ]
        record["explicit_tests"] = [target.get("name") for target in document.get("test", []) or []]
        record["explicit_examples"] = [target.get("name") for target in document.get("example", []) or []]
        record["explicit_benches"] = [target.get("name") for target in document.get("bench", []) or []]
        features = document.get("features", {}) or {}
        record["features"] = {name: list(values) for name, values in sorted(features.items())}
        record["default_features"] = list(features.get("default", []))
        record["dependencies"] = dependency_tables(document, workspace_dependencies, workspace_packages)
        record["build_script"] = build_script(manifest_dir, package)
        record["lints"] = document.get("lints")
        record["package_metadata"] = package.get("metadata")
        roots = target_roots(manifest_dir, document)
        record["target_roots"] = roots
        record["crate_level_attributes"] = {
            target: crate_level_attributes(ROOT / source) for target, source in sorted(roots.items())
        }
    if "workspace" in document:
        section = document["workspace"] or {}
        record["workspace"] = {
            "members": list(section.get("members", []) or []),
            "exclude": list(section.get("exclude", []) or []),
            "resolver": section.get("resolver"),
            "package": section.get("package"),
            "dependencies": {
                alias: ({"version": entry} if isinstance(entry, str) else entry)
                for alias, entry in sorted((section.get("dependencies", {}) or {}).items())
            },
        }
    if "patch" in document:
        record["patch"] = document["patch"]
    if "profile" in document:
        record["profile"] = document["profile"]
    return record


def build_inventory() -> dict[str, Any]:
    """The complete inventory document."""
    manifests = find_manifests()
    root_document = load_toml(ROOT / "Cargo.toml")
    workspace = root_document.get("workspace", {}) or {}
    member_directories = set(workspace.get("members", []) or [])
    workspace_packages: set[str] = set()
    for member in sorted(member_directories):
        member_manifest = ROOT / member / "Cargo.toml"
        if not member_manifest.is_file():
            raise SystemExit(f"workspace member `{member}` has no Cargo.toml")
        workspace_packages.add(load_toml(member_manifest)["package"]["name"])
    records = [manifest_record(path, workspace, member_directories, workspace_packages) for path in manifests]

    categories: dict[str, int] = {}
    for record in records:
        categories[record["category"]] = categories.get(record["category"], 0) + 1
    members = [record for record in records if record["category"] == "workspace-member"]
    third_party: dict[str, set[str]] = {}
    for record in members:
        for role in ("normal", "dev", "build"):
            for dependency in record["dependencies"][role].values():
                if dependency["kind"] == "third-party":
                    third_party.setdefault(dependency["package"], set()).add(role)
    summary = {
        "manifest_count": len(records),
        "by_category": dict(sorted(categories.items())),
        "workspace_member_count": len(members),
        "workspace_packages": sorted(workspace_packages),
        "proc_macro_members": sorted(
            record["package"]["name"] for record in members if record["lib"] and record["lib"]["proc_macro"]
        ),
        "members_with_bins": {
            record["package"]["name"]: [binary["name"] for binary in record["bins"]]
            for record in members
            if record["bins"]
        },
        "members_with_features": sorted(record["package"]["name"] for record in members if record["features"]),
        "members_with_dev_dependencies": sorted(
            record["package"]["name"] for record in members if record["dependencies"]["dev"]
        ),
        "members_with_build_scripts": sorted(
            record["package"]["name"] for record in members if record["build_script"]["present"]
        ),
        "manifests_with_lints_table": sorted(record["path"] for record in records if record.get("lints")),
        "manifests_with_build_scripts": sorted(
            record["path"] for record in records if record.get("build_script", {}).get("present")
        ),
        "editions": sorted({record["package"]["edition"] for record in records if record.get("package")}, key=str),
        "third_party_packages_used_by_members": {
            package: sorted(roles) for package, roles in sorted(third_party.items())
        },
        "third_party_package_count_members": len(third_party),
        "crate_level_lint_attributes": {
            record["path"]: {
                target: [attribute for attribute in attributes if re.match(r"#!\[(deny|warn|forbid|allow)\(", attribute)]
                for target, attributes in record["crate_level_attributes"].items()
            }
            for record in records
            if record.get("crate_level_attributes")
            and any(
                re.match(r"#!\[(deny|warn|forbid|allow)\(", attribute)
                for attributes in record["crate_level_attributes"].values()
                for attribute in attributes
            )
        },
    }
    return {
        "schema_version": SCHEMA_VERSION,
        "generated_by": "scripts/self_build/cargo_manifest_inventory.py",
        "summary": summary,
        "manifests": records,
    }


def render_json(inventory: dict[str, Any]) -> str:
    """Stable JSON text."""
    return json.dumps(inventory, indent=1, sort_keys=False, ensure_ascii=False) + "\n"


def render_markdown(inventory: dict[str, Any]) -> str:
    """One Markdown row per manifest with the facts a facet must carry."""

    def deps(record: dict[str, Any], role: str) -> str:
        table = record["dependencies"][role]
        internal = sum(1 for dependency in table.values() if dependency["kind"] == "workspace-internal")
        external = len(table) - internal
        return f"{internal} ws / {external} 3p" if table else "–"

    def lint_attributes(record: dict[str, Any]) -> str:
        attributes = [
            attribute
            for attributes in record["crate_level_attributes"].values()
            for attribute in attributes
            if re.match(r"#!\[(deny|warn|forbid|allow)\(", attribute)
        ]
        if not attributes:
            return "–"
        names = sorted({re.sub(r"^#!\[(\w+)\((.*)\)\]$", r"\1(\2)", attribute) for attribute in attributes})
        return "; ".join(names)

    lines = [
        "| manifest | category | package | version | edition | targets | features (default) | deps | dev-deps | build.rs | `[lints]` | crate-level `#![…]` |",
        "|---|---|---|---|---|---|---|---|---|---|---|---|",
    ]
    for record in inventory["manifests"]:
        package = record.get("package")
        if package is None:
            workspace = record.get("workspace", {})
            lines.append(
                f"| `{record['path']}` | {record['category']} | (virtual workspace, {len(workspace.get('members', []))} members) | "
                f"{(workspace.get('package') or {}).get('version', '–')} | {(workspace.get('package') or {}).get('edition', '–')} | – | – | – | – | – | – | – |"
            )
            continue
        targets = []
        if record["lib"] is not None:
            kind = "proc-macro" if record["lib"]["proc_macro"] else "lib"
            if record["lib"]["crate_type"]:
                kind += f" ({', '.join(record['lib']['crate_type'])})"
            targets.append(kind)
        elif "lib" in record["target_roots"]:
            targets.append("lib (conventional)")
        targets.extend(f"bin `{binary['name']}`" for binary in record["bins"])
        if not record["bins"]:
            targets.extend(
                f"bin `{target.split(':', 1)[1]}` (conventional)"
                for target in record["target_roots"]
                if target.startswith("bin:")
            )
        features = record["features"]
        feature_text = "–"
        if features:
            names = [name for name in features if name != "default"]
            feature_text = f"{len(names)}: {', '.join(names)}" if names else "0"
            feature_text += f" (default = {', '.join(record['default_features']) or '∅'})"
        lints = "yes" if record["lints"] else "–"
        script = record["build_script"]["path"] or ("declared off" if record["build_script"]["declared"] is False else "–")
        lines.append(
            f"| `{record['path']}` | {record['category']} | `{package['name']}` | {package['version']} | {package['edition']} | "
            f"{', '.join(targets) or '–'} | {feature_text} | {deps(record, 'normal')} | {deps(record, 'dev')} | {script} | {lints} | {lint_attributes(record)} |"
        )
    return "\n".join(lines) + "\n"


def main() -> int:
    """Entry point."""
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--check", action="store_true", help="fail when the committed JSON differs from a fresh run")
    parser.add_argument("--markdown", action="store_true", help="print the rendered per-manifest table instead")
    parser.add_argument("--output", type=Path, default=OUTPUT, help="where to write the JSON inventory")
    arguments = parser.parse_args()
    inventory = build_inventory()
    if arguments.markdown:
        sys.stdout.write(render_markdown(inventory))
        return 0
    text = render_json(inventory)
    if arguments.check:
        if not arguments.output.is_file() or arguments.output.read_text(encoding="utf-8") != text:
            sys.stderr.write(f"{portable(arguments.output)} is stale; rerun scripts/self_build/cargo_manifest_inventory.py\n")
            return 1
        print(f"{portable(arguments.output)} is current ({inventory['summary']['manifest_count']} manifests)")
        return 0
    arguments.output.parent.mkdir(parents=True, exist_ok=True)
    arguments.output.write_text(text, encoding="utf-8")
    print(f"wrote {portable(arguments.output)} ({inventory['summary']['manifest_count']} manifests)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
