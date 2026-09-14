#!/usr/bin/env python3
"""Prove the Oven ring depends on no Incan ring: check the `oven_*` crates in a workspace that has nothing else.

Copies every crate under `loaves/oven/` into a scratch workspace whose manifest carries only the root's
`[workspace.package]` and `[workspace.dependencies]` tables and the root `Cargo.lock`, then runs `cargo check` there.
A crate that names `incan_*` (or the root crate) fails to resolve because nothing under that name exists in the scratch
workspace. This is the property test `loaves/LAYOUT.md` asks for from step 3 of the layout rewrite on.
"""

from __future__ import annotations

import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OVEN_RING = ROOT / "loaves" / "oven"


def table(manifest: str, name: str) -> str:
    """Return the `[name]` table of a manifest, header included, up to the next table header."""
    match = re.search(rf"^\[{re.escape(name)}\]\n.*?(?=^\[|\Z)", manifest, re.M | re.S)
    if match is None:
        raise SystemExit(f"Cargo.toml has no [{name}] table")
    return match.group(0).rstrip("\n") + "\n"


def main() -> int:
    crates = sorted(path.parent for path in OVEN_RING.glob("*/Cargo.toml"))
    if not crates:
        raise SystemExit("no crates under loaves/oven yet")
    root_manifest = (ROOT / "Cargo.toml").read_text()
    with tempfile.TemporaryDirectory(prefix="incan-oven-ring.") as scratch_name:
        scratch = Path(scratch_name)
        members = []
        for crate in crates:
            shutil.copytree(crate, scratch / crate.name, ignore=shutil.ignore_patterns("target"))
            members.append(crate.name)
        workspace = "[workspace]\nmembers = [" + ", ".join(f'"{member}"' for member in members) + "]\nresolver = \"2\"\n\n"
        workspace += table(root_manifest, "workspace.package") + "\n" + table(root_manifest, "workspace.dependencies")
        (scratch / "Cargo.toml").write_text(workspace)
        shutil.copy(ROOT / "Cargo.lock", scratch / "Cargo.lock")
        env = dict(os.environ)
        env.setdefault("CARGO_TARGET_DIR", str(scratch / "target"))
        command = ["cargo", "check", "--all-targets"]
        print("oven ring:", ", ".join(members), "->", " ".join(command), flush=True)
        completed = subprocess.run(command, cwd=scratch, env=env)
        if completed.returncode != 0:
            print("the Oven ring does not build without the compiler crates", file=sys.stderr)
            return completed.returncode
    print("oven ring builds without the compiler crates")
    return 0


if __name__ == "__main__":
    sys.exit(main())
