#!/usr/bin/env python3
"""Prepare SDK release assets from packaged host archives."""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
from datetime import UTC, datetime
from pathlib import Path


RUST_POLICY = (
    "The SDK ships Incan binaries and expects generated Rust builds to use the user's installed stable Rust toolchain "
    "plus the wasm32-wasip1 target for vocab companions."
)


def repo_root() -> Path:
    return Path(__file__).resolve().parents[3]


def read_first_line(path: Path) -> str:
    return path.read_text(encoding="utf-8").splitlines()[0].strip()


def archive_target(archive: Path, release: str) -> str:
    prefix = f"incan-{release}-"
    suffix = ".tar.gz"
    name = archive.name
    if not name.startswith(prefix) or not name.endswith(suffix):
        raise ValueError(f"archive name does not match release {release}: {archive}")
    return name[len(prefix) : -len(suffix)]


def write_manifest(dist_dir: Path, generated_at: str | None = None) -> Path:
    version = read_first_line(dist_dir / "sdk-version.txt")
    release = read_first_line(dist_dir / "sdk-release.txt")
    generated_at = generated_at or datetime.now(UTC).replace(microsecond=0).isoformat().replace("+00:00", "Z")

    hosts: dict[str, object] = {}
    for archive in sorted(dist_dir.glob(f"incan-{release}-*.tar.gz")):
        target = archive_target(archive, release)
        checksum = read_first_line(archive.with_suffix(archive.suffix + ".sha256"))
        hosts[target] = {
            "archive_url": f"https://github.com/dannys-code-corner/incan/releases/download/{release}/{archive.name}",
            "archive_sha256": checksum,
            "archive_format": "tar.gz",
            "commands": {
                "incan": "bin/incan",
                "incan-lsp": "bin/incan-lsp",
            },
        }

    if not hosts:
        raise ValueError(f"no SDK archives found in {dist_dir}")

    manifest = {
        "schema_version": 1,
        "sdk_version": version,
        "release": release,
        "channel": "stable",
        "generated_at": generated_at,
        "manifest_url": f"https://github.com/dannys-code-corner/incan/releases/download/{release}/manifest.json",
        "rust_toolchain": {
            "channel": "stable",
            "min_rust": "1.92",
            "targets": ["wasm32-wasip1"],
            "policy": RUST_POLICY,
        },
        "commands": ["incan", "incan-lsp"],
        "hosts": hosts,
    }

    manifest_path = dist_dir / "manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    return manifest_path


def prepare_assets(dist_dir: Path, generated_at: str | None = None) -> None:
    root = repo_root()
    dist_dir.mkdir(parents=True, exist_ok=True)
    manifest_path = write_manifest(dist_dir, generated_at=generated_at)

    shutil.copy2(root / "workspaces/release/install-incan-sdk.sh", dist_dir / "install.sh")
    shutil.copy2(root / "workspaces/release/sdk/manifest.schema.v1.json", dist_dir / "sdk-manifest.schema.v1.json")
    subprocess.run(
        [
            sys.executable,
            str(root / "workspaces/release/homebrew/render_formula.py"),
            str(manifest_path),
            str(dist_dir / "incan.rb"),
        ],
        check=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("dist_dir", type=Path)
    parser.add_argument("--generated-at", help="Deterministic timestamp override for tests")
    args = parser.parse_args()
    prepare_assets(args.dist_dir, generated_at=args.generated_at)


if __name__ == "__main__":
    main()
