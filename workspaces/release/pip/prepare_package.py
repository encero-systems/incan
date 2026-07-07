#!/usr/bin/env python3
"""Prepare the Python thin installer package."""

from __future__ import annotations

import argparse
import re
import shutil
import subprocess
import sys
from pathlib import Path


def repo_root() -> Path:
    return Path(__file__).resolve().parents[3]


def pep440_version(version: str) -> str:
    normalized = version.replace("-dev.", ".dev")
    return re.sub(r"-(a|b|rc)(\d+)$", r"\1\2", normalized)


def replace_line(path: Path, pattern: str, replacement: str) -> None:
    path.write_text(re.sub(pattern, replacement, path.read_text(encoding="utf-8"), flags=re.MULTILINE), encoding="utf-8")


def prepare_package(dist_dir: Path, skip_build: bool = False) -> None:
    root = repo_root()
    package_dir = root / "workspaces/release/pip"
    version = (dist_dir / "toolchain-version.txt").read_text(encoding="utf-8").splitlines()[0].strip()
    if not version:
        raise ValueError(f"empty toolchain version in {dist_dir / 'toolchain-version.txt'}")

    stage_dir = dist_dir / "_pip-package"
    shutil.rmtree(stage_dir, ignore_errors=True)
    shutil.copytree(
        package_dir,
        stage_dir,
        ignore=shutil.ignore_patterns("__pycache__", "*.pyc", "prepare_package.py"),
    )

    vendor_dir = stage_dir / "src/incan_toolchain/vendor"
    vendor_dir.mkdir(parents=True, exist_ok=True)
    shutil.copy2(root / "workspaces/release/install-incan.sh", vendor_dir / "install-incan.sh")

    package_version = pep440_version(version)
    replace_line(stage_dir / "pyproject.toml", r'^version = ".*"$', f'version = "{package_version}"')
    replace_line(stage_dir / "src/incan_toolchain/__init__.py", r'^__version__ = ".*"$', f'__version__ = "{package_version}"')

    if not skip_build:
        subprocess.run(
            [sys.executable, "-m", "build", "--no-isolation", str(stage_dir), "--outdir", str(dist_dir)],
            check=True,
        )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("dist_dir", type=Path)
    parser.add_argument("--skip-build", action="store_true", help="Only stage and version the package")
    args = parser.parse_args()
    prepare_package(args.dist_dir, skip_build=args.skip_build)


if __name__ == "__main__":
    main()
