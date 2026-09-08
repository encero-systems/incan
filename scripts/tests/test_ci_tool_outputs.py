"""Behavioral candidate-identity and output-integrity tests without compiler builds."""

import importlib.util
import json
import io
import tarfile
from unittest import mock
import sys
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


SPEC = importlib.util.spec_from_file_location("ci_tool_outputs", Path(__file__).parents[1] / "ci_tool_outputs.py")
cache = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(cache)


class ToolOutputTests(unittest.TestCase):
    """Keep candidate equality distinct from complete production admission."""

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        subprocess.run(["git", "init", "-q", str(self.root)], check=True)
        for name in ("Cargo.toml", "Cargo.lock", "src/main.rs", "crates/core/lib.rs", "assets/logo.txt",
                     ".github/workflows/ci.yml", "tests/contract.rs", ".cargo/config.toml"):
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(name)
        subprocess.run(["git", "add", "."], cwd=self.root, check=True)
        self.coordinates = dict(recipe=["cargo", "build", "--release"], rustc="rustc identity",
                                cargo="cargo identity", linker="linker identity", runner_image="image revision",
                                environment={"RUSTFLAGS": ""})

    def candidate(self):
        return cache.input_manifest(self.root, self.coordinates)

    def test_mtime_and_administrative_edits_preserve_candidate(self):
        original = self.candidate()
        os.utime(self.root / "src/main.rs", (1, 1))
        for name in ("tests/contract.rs", ".github/workflows/ci.yml"):
            (self.root / name).write_text("administrative change")
        self.assertEqual(original, self.candidate())

    def test_source_config_lock_and_version_changes_invalidate(self):
        for name in ("src/main.rs", "assets/logo.txt", ".cargo/config.toml", "Cargo.lock", "Cargo.toml"):
            with self.subTest(name=name):
                original = self.candidate()
                (self.root / name).write_text("changed")
                self.assertNotEqual(original["identity"], self.candidate()["identity"])

    def test_external_recipe_or_environment_changes_invalidate(self):
        for key in self.coordinates:
            with self.subTest(key=key):
                original = self.candidate()
                self.coordinates[key] = {"changed": "value"} if key == "environment" else "changed"
                self.assertNotEqual(original["identity"], self.candidate()["identity"])

    def test_checkout_location_is_bound(self):
        candidate = self.candidate()
        self.assertEqual(str(self.root), candidate["inputs"]["workspace"])

    def test_local_coverage_refuses_excluded_and_new_inputs(self):
        candidate = self.candidate()
        cache.verify_local_inputs(candidate, ["src/main.rs", self.root / "crates/core/lib.rs"])
        for name in ("tests/contract.rs", "new-untracked-file"):
            (self.root / name).write_text("consumed")
            with self.assertRaises(ValueError):
                cache.verify_local_inputs(candidate, [name])

    def test_changed_consumed_bytes_or_mode_refuse(self):
        candidate = self.candidate()
        path = self.root / "src/main.rs"
        path.chmod(0o755)
        with self.assertRaises(ValueError):
            cache.verify_local_inputs(candidate, [path])
        path.write_text("different bytes")
        with self.assertRaises(ValueError):
            cache.verify_local_inputs(candidate, [path])

    def test_empty_evidence_and_symlink_refuse(self):
        candidate = self.candidate()
        with self.assertRaises(ValueError):
            cache.verify_local_inputs(candidate, [])
        path = self.root / "src/main.rs"
        path.unlink()
        path.symlink_to(self.root / "Cargo.toml")
        with self.assertRaises(ValueError):
            self.candidate()

    def test_three_tools_validate_then_tamper_refuses(self):
        tools = self.root / "tools"
        tools.mkdir()
        for name in cache.TOOLS:
            (tools / name).write_text(name)
            (tools / name).chmod(0o755)
        manifest = cache.output_manifest(tools, "expected")
        cache.verify_outputs(tools, manifest, "expected")
        with self.assertRaises(ValueError):
            cache.verify_outputs(tools, manifest, "wrong")
        (tools / "incan").write_text("tampered")
        with self.assertRaises(ValueError):
            cache.verify_outputs(tools, manifest, "expected")
        (tools / "incan").unlink()
        (tools / "incan").symlink_to(tools / "generate_lang_reference")
        with self.assertRaises(ValueError):
            cache.verify_outputs(tools, manifest, "expected")


    def test_registry_token_is_absent_from_identity_state_and_output(self):
        sentinel = "REGISTRY_TOKEN_SENTINEL_DO_NOT_PUBLISH"
        workflow = self.root / ".github/workflows/ci.yml"
        workflow.write_text("\n".join(command + " | python3 scripts/ci_tool_outputs.py record private.jsonl"
                                      for command in cache.RECIPE))
        state = self.root / "evidence"
        environment = {"PATH": os.environ["PATH"], "HOME": str(self.root / "home"),
                       "CARGO_HOME": str(self.root / "cargo-home"), "ImageOS": "fixture", "ImageVersion": "1",
                       "CARGO_REGISTRIES_EXAMPLE_TOKEN": sentinel}
        argv = ["ci_tool_outputs.py", "identity", "--workspace", str(self.root),
                "--state", str(state), "--bundle", str(self.root / "bundle")]
        output = io.StringIO()
        with mock.patch.dict(os.environ, environment, clear=True), mock.patch.object(sys, "argv", argv), \
                mock.patch.object(cache, "command_text", return_value="fixture tool identity"), \
                mock.patch("sys.stdout", output):
            cache.main()
        public = output.getvalue() + "".join(path.read_text() for path in state.iterdir())
        self.assertNotIn(sentinel, public)
        self.assertNotIn("CARGO_REGISTRIES_EXAMPLE_TOKEN", public)
        self.assertEqual(cache.build_environment({}), cache.build_environment({"CARGO_REGISTRY_TOKEN": sentinel}))

    def test_unknown_config_refuses_without_exposing_its_value(self):
        sentinel = "SECRET_CONFIG_SENTINEL"
        with self.assertRaises(ValueError) as error:
            cache.build_environment({"CARGO_REGISTRIES_PRIVATE_INDEX": sentinel})
        self.assertIn("CARGO_REGISTRIES_PRIVATE_INDEX", str(error.exception))
        self.assertNotIn(sentinel, str(error.exception))

    def registry_fixture(self, extra_members=()):
        """Create a normal locked .crate archive and its extracted source, without a vendor inventory."""
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        cargo_home = Path(temporary.name).resolve()
        package = cargo_home / "registry/src/index/example-1.0.0"
        package.mkdir(parents=True)
        (package / "Cargo.toml").write_text('[package]\nname="example"\nversion="1.0.0"\n')
        files = [package / "one.rs", package / "two.rs"]
        for file in files:
            file.write_text(file.name)
        archive = cargo_home / "registry/cache/index/example-1.0.0.crate"
        archive.parent.mkdir(parents=True)
        with tarfile.open(archive, "w:gz") as target:
            for file in [package / "Cargo.toml", *files]:
                target.add(file, arcname="example-1.0.0/" + file.name)
            for name, kind in extra_members:
                member = tarfile.TarInfo(name)
                member.type = kind
                member.size = 0
                member.linkname = "one.rs" if kind in (tarfile.SYMTYPE, tarfile.LNKTYPE) else ""
                target.addfile(member, io.BytesIO())
        checksum = cache.digest(archive)
        (self.root / "Cargo.lock").write_text('[[package]]\nname="example"\nversion="1.0.0"\n'
                                             'source="registry+https://github.com/rust-lang/crates.io-index"\n'
                                             f'checksum="{checksum}"\n')
        return cargo_home, package, files, archive, checksum

    def test_registry_package_evidence_is_parsed_once_but_each_file_is_checked(self):
        cargo_home, package, files, archive, checksum = self.registry_fixture()
        with mock.patch.dict(os.environ, {"CARGO_HOME": str(cargo_home)}):
            verifier = cache.ConsumedInputs(self.candidate())
            with mock.patch.object(verifier, "toml", wraps=verifier.toml) as parsed, \
                    mock.patch.object(cache, "digest", wraps=cache.digest) as hashed, \
                    mock.patch.object(cache, "authenticated_archive_files", wraps=cache.authenticated_archive_files) as unpacked:
                for file in files:
                    self.assertEqual(checksum, verifier.record(file)["package_checksum"])
                self.assertEqual(2, parsed.call_count)  # one package manifest plus one lock
                self.assertEqual(1, unpacked.call_count)
                self.assertEqual(3, hashed.call_count)  # authenticated manifest, then each file once
            files[1].write_text("tampered")
            with self.assertRaises(ValueError):
                verifier.record(files[1])

    def test_registry_missing_or_changed_archive_refuses_despite_forged_vendor_inventory(self):
        for missing in (True, False):
            with self.subTest(missing=missing):
                cargo_home, package, files, archive, checksum = self.registry_fixture()
                (package / ".cargo-checksum.json").write_text(json.dumps(
                    {"package": checksum, "files": {file.name: cache.digest(file) for file in files}}))
                if missing:
                    archive.unlink()
                else:
                    archive.write_bytes(b"changed archive")
                with mock.patch.dict(os.environ, {"CARGO_HOME": str(cargo_home)}), self.assertRaises(ValueError):
                    cache.ConsumedInputs(self.candidate()).record(files[0])

    def test_locked_malformed_archive_refuses_cleanly(self):
        cargo_home, package, files, archive, checksum = self.registry_fixture()
        archive.write_bytes(b"not a tar archive")
        with self.assertRaises(ValueError):
            cache.authenticated_archive_files(archive, cache.digest(archive), package.name)

    def test_registry_archive_refuses_duplicate_escaping_and_linked_entries(self):
        cases = [("example-1.0.0/one.rs", tarfile.REGTYPE), ("example-1.0.0/../escape", tarfile.REGTYPE),
                 ("/absolute", tarfile.REGTYPE), ("example-1.0.0/link", tarfile.SYMTYPE),
                 ("example-1.0.0/hardlink", tarfile.LNKTYPE)]
        for member in cases:
            with self.subTest(member=member):
                cargo_home, package, files, archive, checksum = self.registry_fixture([member])
                with mock.patch.dict(os.environ, {"CARGO_HOME": str(cargo_home)}), self.assertRaises(ValueError):
                    cache.ConsumedInputs(self.candidate()).record(files[0])
                self.assertFalse((cargo_home / "escape").exists())

    def test_registry_ambiguous_locked_owner_and_changed_package_manifest_refuse(self):
        cargo_home, package, files, archive, checksum = self.registry_fixture()
        with (self.root / "Cargo.lock").open("a") as target:
            target.write((self.root / "Cargo.lock").read_text())
        with mock.patch.dict(os.environ, {"CARGO_HOME": str(cargo_home)}), self.assertRaises(ValueError):
            cache.ConsumedInputs(self.candidate()).record(files[0])
        cargo_home, package, files, archive, checksum = self.registry_fixture()
        with (package / "Cargo.toml").open("a") as target:
            target.write('description="changed"\n')
        with mock.patch.dict(os.environ, {"CARGO_HOME": str(cargo_home)}), self.assertRaises(ValueError):
            cache.ConsumedInputs(self.candidate()).record(files[0])

    def test_extension_env_and_directive_values_are_not_public_evidence(self):
        sentinel = "EXTENSION_SENTINEL_DO_NOT_PUBLISH"
        generated = self.root / "generated/out"
        generated.mkdir(parents=True)
        (generated.parent / "output").write_text("cargo:rustc-env=SECRET=" + sentinel + "\n")
        logs = self.cargo_logs(dict(reason="build-script-executed", package_id="opaque@1", out_dir=str(generated),
                                   env=[["SECRET", sentinel]], linked_paths=[sentinel]))
        depfile = self.root / "target/release/incan.d"
        with depfile.open("a") as output:
            output.write("# env-dep:SECRET=" + sentinel + "\n")
        evidence = cache.collect_evidence(self.root, self.candidate(), logs, {"SECRET": sentinel})
        public = json.dumps(evidence)
        self.assertNotIn(sentinel, public)
        self.assertIn("SECRET", public)
        self.assertFalse(evidence["admitted"])
        diagnostics = io.StringIO()
        private = self.root / "private/raw.jsonl"
        cache.record_cargo_messages(private, io.StringIO(logs[0].read_text()), diagnostics)
        self.assertNotIn(sentinel, diagnostics.getvalue())
        self.assertIn(sentinel, private.read_text())

    def cargo_logs(self, extra=None):
        """Create concrete fake Cargo artifact records; these are not compiler-coverage proof."""
        release = self.root / "target/release"
        release.mkdir(parents=True, exist_ok=True)
        logs = []
        for index, names in enumerate((cache.TOOLS[:2], cache.TOOLS[2:])):
            messages = []
            for name in names:
                path = release / name
                path.write_text("executable-" + name)
                path.chmod(0o755)
                path.with_suffix(".d").write_text(f"{path}: {self.root / 'src/main.rs'}\n")
                messages.append(dict(reason="compiler-artifact", package_id="fixture", manifest_path=str(self.root / "Cargo.toml"),
                                     target={"name": name, "kind": ["bin"]}, executable=str(path), filenames=[str(path)],
                                     features=[], profile={"opt_level": "3"}, fresh=False))
            if extra and index == 0:
                messages.append(extra)
            messages.append(dict(reason="build-finished", success=True))
            log = self.root / f"cargo-{index}.jsonl"
            log.write_text("\n".join(json.dumps(message) for message in messages))
            logs.append(log)
        return logs

    def invoke(self, operation, state, bundle, logs=()):
        """Execute the real cache CLI without invoking a compiler or build tool."""
        command = [sys.executable, str(Path(cache.__file__)), operation,
                   "--workspace", str(self.root), "--state", str(state), "--bundle", str(bundle)]
        for log in logs:
            command += ["--cargo-log", str(log)]
        return subprocess.run(command, capture_output=True, text=True, check=True)

    def test_cold_admission_then_warm_cli_restores_exact_tools(self):
        self.coordinates["environment"] = {}
        candidate = self.candidate()
        state = self.root / "evidence"
        state.mkdir()
        (state / "inputs.json").write_text(json.dumps(candidate))
        bundle = self.root / "bundle"
        logs = self.cargo_logs()
        self.invoke("admit", state, bundle, logs)
        self.assertEqual("admitted", json.loads((state / "admit.json").read_text())["status"])
        original = cache.output_manifest(bundle, candidate["identity"])
        for name in cache.TOOLS:
            (self.root / "target/release" / name).unlink()
        self.invoke("restore", state, bundle)
        self.assertEqual("hit", json.loads((state / "restore.json").read_text())["status"])
        cache.verify_outputs(self.root / "target/release", original, candidate["identity"])

    def test_identity_ineligible_cold_run_retains_safe_facts_without_admission(self):
        state = self.root / "evidence"
        bundle = self.root / "bundle"
        logs = self.cargo_logs(dict(reason="build-script-executed", package_id="opaque@1", out_dir="generated",
                                   env=[["SECRET", "NEVER_PUBLISH_SENTINEL"]]))
        self.invoke("collect", state, bundle, logs)
        evidence = json.loads((state / "coverage.json").read_text())
        self.assertFalse(evidence["admitted"])
        self.assertEqual(3, len(evidence["artifacts"]))
        self.assertEqual(set(cache.TOOLS), set(evidence["observed_tool_outputs"]))
        self.assertTrue(evidence["files"])
        self.assertNotIn("NEVER_PUBLISH_SENTINEL", json.dumps(evidence))
        self.assertFalse(bundle.exists())
        self.assertFalse((state / "inputs.json").exists())

    def test_unknown_build_script_refuses_save_after_successful_build(self):
        logs = self.cargo_logs(dict(reason="build-script-executed", package_id="opaque@1", out_dir="generated"))
        evidence = cache.collect_evidence(self.root, self.candidate(), logs, {})
        self.assertFalse(evidence["admitted"])
        self.assertIn("unreviewed-build-script", {item["kind"] for item in evidence["refusals"]})

    def test_opaque_proc_macro_is_not_admitted_as_ordinary_source(self):
        logs = self.cargo_logs()
        rows = [json.loads(line) for line in logs[0].read_text().splitlines()]
        rows[0]["target"]["kind"] = ["proc-macro"]
        logs[0].write_text("\n".join(json.dumps(row) for row in rows))
        evidence = cache.collect_evidence(self.root, self.candidate(), logs, {})
        self.assertFalse(evidence["admitted"])
        self.assertIn("unreviewed-proc-macro", {item["kind"] for item in evidence["refusals"]})

    def test_failed_or_incomplete_cargo_stream_refuses_admission(self):
        logs = self.cargo_logs()
        logs[1].write_text(json.dumps(dict(reason="build-finished", success=False)))
        with self.assertRaises(ValueError):
            cache.collect_evidence(self.root, self.candidate(), logs, {})

    def test_same_owner_includes_normalize_but_escape_and_links_refuse(self):
        candidate = self.candidate()
        verifier = cache.ConsumedInputs(candidate)
        actual = verifier.record(self.root / "src/../assets/logo.txt")
        self.assertEqual(actual["path"], str(self.root / "assets/logo.txt"))
        for path in (self.root / "../" / self.root.name / "assets/logo.txt",):
            with self.assertRaises(ValueError):
                verifier.record(path)
        (self.root / "src/link").symlink_to(self.root / "assets", target_is_directory=True)
        with self.assertRaises(ValueError):
            verifier.record(self.root / "src/link/../main.rs")
        cargo_home, package, files, _, _ = self.registry_fixture()
        (package / "src").mkdir()
        with mock.patch.dict(os.environ, {"CARGO_HOME": str(cargo_home)}):
            verifier = cache.ConsumedInputs(self.candidate())
            self.assertEqual(verifier.record(package / "src/../one.rs")["sha256"], cache.digest(files[0]))
            with self.assertRaises(ValueError):
                verifier.record(package / "../example-1.0.0/one.rs")

    def test_custom_build_depfile_is_bound_to_exact_owner_source_and_output(self):
        directory = self.root / "target/release/build/example-0123456789abcdef"
        directory.mkdir(parents=True)
        output = directory / "build-script-build"
        source = self.root / "src/main.rs"
        depfile = directory / "build_script_build-0123456789abcdef.d"
        native = depfile.with_suffix("")
        message = {"package_id": "registry+https://example.invalid#index#example@1.0.0",
                   "target": {"kind": ["custom-build"], "src_path": str(source)},
                   "filenames": [str(output)]}
        depfile.write_text(f"{native}: {source}\n")
        self.assertEqual(cache.artifact_depfiles(message, self.root), [depfile])
        for contents in (f"{native}: other.rs\n", f"{output}: {source}\n"):
            depfile.write_text(contents)
            with self.assertRaises(ValueError):
                cache.artifact_depfiles(message, self.root)
        depfile.unlink()
        (directory / "unrelated.d").write_text(f"{native}: {source}\n")
        with self.assertRaises((ValueError, OSError)):
            cache.artifact_depfiles(message, self.root)
        depfile.symlink_to(directory / "unrelated.d")
        with self.assertRaises(ValueError):
            cache.artifact_depfiles(message, self.root)
        message["package_id"] = "registry+https://example.invalid#other@1.0.0"
        with self.assertRaises(ValueError):
            cache.artifact_depfiles(message, self.root)

    def test_depfile_spaces_and_env_are_observed_without_guessing(self):
        depfile = self.root / "sample.d"
        depfile.write_text("out: src/a\\ b.rs src/main.rs\n# env-dep:FLAG=value\n# env-dep:ABSENT\n")
        files, environment = cache.depfile_facts(depfile)
        self.assertEqual(["src/a b.rs", "src/main.rs"], files)
        self.assertEqual({"FLAG": "value", "ABSENT": None}, environment)
        depfile.write_text("out: $(UNSUPPORTED)/input\n")
        with self.assertRaises(ValueError):
            cache.depfile_facts(depfile)

    def test_workflow_keeps_one_staging_path_and_conditional_build(self):
        workflow = (Path(cache.__file__).parents[1] / ".github/workflows/ci.yml").read_text()
        job = workflow.split("  linux-tool-handoff:", 1)[1].split("  verified-docs:", 1)[0]
        self.assertLess(job.index("ci_tool_outputs.py restore"), job.index("cargo build --locked --release"))
        self.assertIn("if: steps.tool-restore.outputs.hit != 'true'", job)
        self.assertIn("if: steps.tool-admission.outputs.admitted == 'true'", job)
        self.assertEqual(2, job.count("path: target/ci-tools-restored"))
        self.assertIn("path: target/ci-tool-evidence", job)
        self.assertNotIn("path: target/ci-tool-private", job)
        self.assertNotIn(" | tee ", job)
        self.assertNotIn("restore-keys:", job[job.index("- name: Restore exact Linux tool outputs"):])
        self.assertLess(job.index("ci_tool_outputs.py admit"), job.index('install -m 755'))


if __name__ == "__main__":
    unittest.main()
