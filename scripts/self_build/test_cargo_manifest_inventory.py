#!/usr/bin/env python3
"""Unit tests for tracked Cargo-manifest discovery."""

import importlib.util
import unittest
from pathlib import Path
from unittest.mock import patch

MODULE_PATH = Path(__file__).with_name("cargo_manifest_inventory.py")
SPEC = importlib.util.spec_from_file_location("cargo_manifest_inventory", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"could not load {MODULE_PATH}")
inventory = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(inventory)


class CargoManifestInventoryTests(unittest.TestCase):
    """Manifest discovery is based on repository-tracked paths."""

    def test_find_manifests_uses_git_tracked_files_only(self) -> None:
        """Untracked scratch manifests do not enter the reproducible inventory."""
        result = type(
            "Result",
            (),
            {"returncode": 0, "stdout": "Cargo.toml\nloaves/compiler/incan_driver/Cargo.toml\n", "stderr": ""},
        )()
        with patch.object(inventory.subprocess, "run", return_value=result) as run:
            self.assertEqual(
                inventory.find_manifests(),
                [inventory.ROOT / "Cargo.toml", inventory.ROOT / "loaves/compiler/incan_driver/Cargo.toml"],
            )
        run.assert_called_once_with(
            ["git", "ls-files", "--", "Cargo.toml", "**/Cargo.toml"],
            cwd=inventory.ROOT,
            capture_output=True,
            text=True,
            check=False,
        )


if __name__ == "__main__":
    unittest.main()
