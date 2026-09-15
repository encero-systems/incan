"""Check the ring version gate against the real tree and against a tree whose lines drift."""

from pathlib import Path
import re
import shutil
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_ring_versions  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent


def copy_manifests(destination: Path) -> None:
    """Copy the root manifest, every member manifest, and the emitter's crate root, which is all the gate reads."""
    shutil.copy(ROOT / "Cargo.toml", destination / "Cargo.toml")
    emitter = Path("loaves/compiler/incan_emit/src/lib.rs")
    (destination / emitter.parent).mkdir(parents=True, exist_ok=True)
    shutil.copy(ROOT / emitter, destination / emitter)
    for member in check_ring_versions.workspace_members((ROOT / "Cargo.toml").read_text()):
        (destination / member).mkdir(parents=True, exist_ok=True)
        shutil.copy(ROOT / member / "Cargo.toml", destination / member / "Cargo.toml")


def edit(path: Path, pattern: str, replacement: str) -> None:
    text = path.read_text()
    edited, count = re.subn(pattern, replacement, text, count=1, flags=re.MULTILINE)
    if count != 1:
        raise AssertionError(f"{path}: {pattern!r} did not match")
    path.write_text(edited)


class RingVersionTests(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory(prefix="incan-ring-versions.")
        self.root = Path(self.scratch.name)
        copy_manifests(self.root)

    def tearDown(self):
        self.scratch.cleanup()

    def test_the_checkout_is_consistent(self):
        self.assertEqual(check_ring_versions.check(ROOT), [])

    def test_a_ring_crate_that_inherits_the_workspace_version_is_reported(self):
        edit(self.root / "loaves/oven/oven_store/Cargo.toml", r'^version = "[^"]+"$', "version.workspace = true")
        failures = check_ring_versions.check(self.root)
        self.assertTrue(any("oven_store is in the oven ring" in failure for failure in failures), failures)

    def test_a_ring_whose_crates_disagree_is_reported(self):
        edit(self.root / "loaves/oven/oven_store/Cargo.toml", r'^version = "[^"]+"$', 'version = "9.9.9"')
        failures = check_ring_versions.check(self.root)
        self.assertTrue(any("more than one version line" in failure for failure in failures), failures)
        self.assertTrue(any("must require the oven ring" in failure for failure in failures), failures)

    def test_a_table_requirement_that_lags_the_ring_line_is_reported(self):
        edit(self.root / "Cargo.toml", r'^(incan_std_core = \{ path = "[^"]+", version = )"[^"]+"', r'\1"0.0.1"')
        failures = check_ring_versions.check(self.root)
        self.assertTrue(any("[workspace.dependencies].incan_std_core must require" in failure for failure in failures), failures)

    def test_a_crate_outside_the_rings_must_inherit(self):
        edit(self.root / "loaves/compiler/incan_format/Cargo.toml", r"^version\.workspace = true$", 'version = "0.1.0"')
        failures = check_ring_versions.check(self.root)
        self.assertTrue(any("incan_format has no ring line" in failure for failure in failures), failures)

    def test_an_emitter_that_generates_for_another_stdlib_line_is_reported(self):
        edit(
            self.root / "loaves/compiler/incan_emit/src/lib.rs",
            r'^pub const GENERATED_FOR_STDLIB_VERSION: &str = "[^"]+";$',
            'pub const GENERATED_FOR_STDLIB_VERSION: &str = "0.0.1";',
        )
        failures = check_ring_versions.check(self.root)
        self.assertTrue(any("GENERATED_FOR_STDLIB_VERSION is '0.0.1'" in failure for failure in failures), failures)

    def test_a_member_spelled_by_path_is_reported(self):
        edit(
            self.root / "loaves/compiler/incan_format/Cargo.toml",
            r"^incan_syntax = \{ workspace = true \}$",
            'incan_syntax = { path = "../../kernel/incan_syntax" }',
        )
        failures = check_ring_versions.check(self.root)
        self.assertTrue(any("incan_syntax is spelled by path" in failure for failure in failures), failures)


if __name__ == "__main__":
    unittest.main()
