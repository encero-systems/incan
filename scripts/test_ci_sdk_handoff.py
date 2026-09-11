"""Exercise SDK transfer and refusal through real helper/compiler subprocesses."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
import zipfile


SCRIPT = Path(__file__).with_name("ci_sdk_handoff.py")
IDENTITY = "a" * 64


class SdkHandoffTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.workspace = self.root / "source checkout"
        self.workspace.mkdir()
        self.compiler = self.workspace / "incan"
        self.compiler.write_text(f"#!{sys.executable}\n" + '''
import json, os, pathlib, sys
with open(os.environ["PROBE_TRACE"], "a") as trace:
    trace.write(json.dumps({"args": sys.argv[1:], "inventory": os.environ.get("INCAN_SDK_INVENTORY"),
        "store": os.environ.get("INCAN_INTERNAL_SDK_PROVIDER_STORE"),
        "path_file": os.environ.get("INCAN_INTERNAL_SDK_PROVIDER_PATH_FILE")}) + "\\n")
if sys.argv[1:3] == ["oven", "sdk-provider-store-identity"]:
    print(os.environ.get("PROBE_IDENTITY", "a" * 64))
elif sys.argv[1] == "check":
    inventory = os.environ.get("INCAN_SDK_INVENTORY")
    if not inventory:
        pathlib.Path(os.environ["PROBE_COLD_PUBLISHER"]).touch()
        sys.exit("unexpected cold publisher")
    value = json.loads(pathlib.Path(inventory).read_text())
    if not value.get("compatible"):
        sys.exit("SDK provider codegen revision mismatch")
else:
    sys.exit("unexpected compiler command")
''')
        self.compiler.chmod(0o755)
        self.rustc = self.workspace / "rustc"
        self.rustc.write_text(f"#!{sys.executable}\nprint('rustc 1.98.0\\nhost: test-target')\n")
        self.rustc.chmod(0o755)
        self.store = self.root / "producer store"
        self.sdk = self.store / IDENTITY
        self.sdk.mkdir(parents=True)
        (self.sdk / "sdk-inventory.json").write_text('{"compatible": true}')
        (self.sdk / "components").mkdir()
        (self.sdk / "components" / "provider.incnlib").write_bytes(b"immutable checked provider")
        old = self.store / ("b" * 64)
        old.mkdir()
        (old / "sdk-inventory.json").write_text("old unrelated provider")
        self.artifact = self.root / "handoff artifact"
        self.env_file = self.root / "github env"
        self.path_file = self.root / "local provider path"
        self.trace = self.root / "compiler trace"
        self.cold_publisher = self.root / "unexpected publisher"
        self.environment = dict(os.environ, PROBE_TRACE=str(self.trace), PROBE_COLD_PUBLISHER=str(self.cold_publisher))

    def command(self, mode, **environment):
        args = [sys.executable, str(SCRIPT), mode, "--workspace", str(self.workspace),
                "--compiler", str(self.compiler), "--rustc", str(self.rustc), "--artifact", str(self.artifact)]
        if mode == "stage":
            args += ["--store", str(self.store)]
        else:
            args += ["--path-file", str(self.path_file), "--env-file", str(self.env_file)]
        return subprocess.run(args, env=dict(self.environment, **environment), text=True, capture_output=True, timeout=10)

    def stage(self):
        result = self.command("stage")
        self.assertEqual(result.returncode, 0, result.stderr)

    def assert_refused(self, result, diagnostic):
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(diagnostic, result.stderr)
        self.assertFalse(self.env_file.exists(), "rejected handoff exposed runtime environment")
        self.assertFalse(self.path_file.exists(), "rejected handoff exposed SDK path")
        self.assertFalse(self.cold_publisher.exists(), "rejection fell back to cold SDK publication")

    def rewrite_envelope(self, **updates):
        path = self.artifact / "handoff.json"
        envelope = json.loads(path.read_text())
        envelope.update(updates)
        path.write_text(json.dumps(envelope))

    def test_relocated_transfer_contains_only_selected_provider_and_checks_explicit_inventory(self):
        self.stage()
        self.assertEqual(sorted(p.name for p in self.artifact.iterdir()), [IDENTITY, "handoff.json"])
        relocated = self.root / "consumer checkout" / "sdk"
        relocated.parent.mkdir()
        shutil.copytree(self.artifact, relocated)
        shutil.rmtree(self.artifact)
        shutil.rmtree(self.store)
        self.artifact = relocated
        result = self.command("consume")
        self.assertEqual(result.returncode, 0, result.stderr)
        inventory = relocated / IDENTITY / "sdk-inventory.json"
        self.assertEqual(self.path_file.read_text(), f"{inventory.parent}\n")
        self.assertIn(f"INCAN_SDK_INVENTORY={inventory}\n", self.env_file.read_text())
        calls = [json.loads(line) for line in self.trace.read_text().splitlines()]
        self.assertEqual(calls[-1]["args"], ["check", "tests/fixtures/test_assert_canary.incn"])
        self.assertEqual(calls[-1]["inventory"], str(inventory))
        self.assertFalse(self.cold_publisher.exists())

    def test_missing_producer_inventory_refuses_before_compiler_check(self):
        (self.sdk / "sdk-inventory.json").unlink()
        self.assert_refused(self.command("stage"), "SDK inventory is missing")
        self.assertFalse(self.artifact.exists())

    def test_missing_handoff_refuses_without_publishing(self):
        self.assert_refused(self.command("consume"), "handoff")

    def test_current_compiler_identity_overrules_tampered_envelope(self):
        self.stage()
        changed = "c" * 64
        (self.artifact / IDENTITY).rename(self.artifact / changed)
        self.rewrite_envelope(provider_identity=changed)
        self.assert_refused(self.command("consume"), "provider identity mismatch")

    def test_source_change_refuses_previously_valid_artifact(self):
        self.stage()
        self.assert_refused(self.command("consume", PROBE_IDENTITY="d" * 64), "provider identity mismatch")

    def test_changed_compiler_bytes_refuse(self):
        self.stage()
        with self.compiler.open("a") as compiler:
            compiler.write("\n# different compiler\n")
        self.assert_refused(self.command("consume"), "compiler digest mismatch")

    def test_wrong_toolchain_refuses(self):
        self.stage()
        self.rustc.write_text(f"#!{sys.executable}\nprint('rustc 1.99.0')\n")
        self.assert_refused(self.command("consume"), "rustc identity mismatch")

    def test_missing_consumer_inventory_refuses(self):
        self.stage()
        (self.artifact / IDENTITY / "sdk-inventory.json").unlink()
        self.assert_refused(self.command("consume"), "SDK inventory is missing")

    def test_modified_payload_refuses(self):
        self.stage()
        (self.artifact / IDENTITY / "components" / "provider.incnlib").write_bytes(b"changed provider")
        self.assert_refused(self.command("consume"), "payload digest mismatch")

    def test_compiler_rejects_inventory_even_when_transport_digest_matches(self):
        (self.sdk / "sdk-inventory.json").write_text('{"compatible": false}')
        self.stage()
        self.assert_refused(self.command("consume"), "SDK provider codegen revision mismatch")

    def test_malformed_inventory_is_rejected_by_compiler(self):
        (self.sdk / "sdk-inventory.json").write_text("not JSON")
        self.stage()
        self.assert_refused(self.command("consume"), "SDK validation failed")

    def test_symlink_payload_is_not_exported(self):
        (self.sdk / "outside").symlink_to(self.compiler)
        self.assert_refused(self.command("stage"), "symlink")
        self.assertFalse(self.artifact.exists())

    def test_unknown_envelope_version_refuses(self):
        self.stage()
        self.rewrite_envelope(schema_version=2)
        self.assert_refused(self.command("consume"), "unsupported handoff schema")

    def test_cache_is_not_needed_after_artifact_publication(self):
        self.stage()
        shutil.rmtree(self.store)
        self.assertEqual(self.command("consume").returncode, 0)
        self.assertFalse(self.store.exists())

    def test_make_prerequisite_preserves_custom_handoff_paths(self):
        self.stage()
        consumed = self.command("consume")
        self.assertEqual(consumed.returncode, 0, consumed.stderr)
        compiler = self.workspace / "target" / "debug" / "incan"
        compiler.parent.mkdir(parents=True)
        shutil.copy2(self.compiler, compiler)
        exports = dict(line.split("=", 1) for line in self.env_file.read_text().splitlines())
        environment = dict(self.environment, **exports, INCAN_TEST_COMPILER_ALREADY_BUILT="1")
        makefile = SCRIPT.parent.parent / "Makefile"
        result = subprocess.run(["make", "-s", "-f", str(makefile), "test-prewarm-sdk"],
                                cwd=self.workspace, env=environment, capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        call = json.loads(self.trace.read_text().splitlines()[-1])
        self.assertEqual(call["inventory"], str(self.artifact / IDENTITY / "sdk-inventory.json"))
        self.assertEqual(call["store"], str(self.artifact))
        self.assertEqual(call["path_file"], str(self.path_file))
        self.assertFalse(self.cold_publisher.exists())

    def test_file_only_zip_transfer_restores_directory_integrity_and_hidden_payload(self):
        (self.sdk / "empty" / "nested").mkdir(parents=True)
        (self.sdk / ".metadata").write_bytes(b"hidden provider input")
        self.stage()
        archive = self.root / "artifact.zip"
        with zipfile.ZipFile(archive, "w") as output:
            for path in self.artifact.rglob("*"):
                if path.is_file():
                    output.write(path, path.relative_to(self.artifact))
        shutil.rmtree(self.artifact)
        with zipfile.ZipFile(archive) as source:
            source.extractall(self.artifact)
        empty = self.artifact / IDENTITY / "empty" / "nested"
        self.assertFalse(empty.exists(), "file-only transfer unexpectedly preserved empty directories")
        result = self.command("consume")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(empty.is_dir(), "consumer did not restore the authoritative provider directory tree")
        self.assertEqual((self.artifact / IDENTITY / ".metadata").read_bytes(), b"hidden provider input")

    def test_empty_directory_manifest_cannot_escape_selected_provider(self):
        self.stage()
        self.rewrite_envelope(empty_directories=["../../escaped"])
        self.assert_refused(self.command("consume"), "must remain inside the selected provider")
        self.assertFalse((self.root / "escaped").exists())


if __name__ == "__main__":
    unittest.main()
