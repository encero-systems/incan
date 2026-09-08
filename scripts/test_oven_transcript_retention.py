"""Exercise the replay cleanup boundary with real shell/archive subprocesses."""

import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import time
import tarfile
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("retain_oven_suite_output.sh")


class TranscriptRetentionTests(unittest.TestCase):
    def run_cleanup(
        self, *, report_exists=True, succeeded=True, broken_tar=False, broken_copy=False,
        previous_evidence=False, report_is_source=False, delayed=False, broken_cleanup=False, wrapper_clock=False, broken_timing=False, broken_final_timing=False
    ):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        output = root / "oven-compiler-suite-output.probe"
        scratch = root / "incan-oven-suite.probe"
        scratch.mkdir()
        transcripts = output / "shards" / "path with spaces"
        transcripts.mkdir(parents=True)
        transcript = transcripts / "probe.libtest-output.txt"
        transcript.write_bytes(b"complete captured diagnostic\n")
        (output / "caller-binary").write_bytes(b"not an artifact")
        if report_exists:
            (output / "compiler-suite-report.json").write_text('{"probe": true}\n')
        destination = output / "compiler-suite-report.json" if report_is_source else root / "retained" / "report.json"
        if previous_evidence:
            destination.parent.mkdir()
            destination.write_text('{"previous_run": true}\n')
            Path(str(destination) + ".transcripts.tar.gz").write_bytes(b"previous run archive")
            Path(str(destination) + ".wall-time.json").write_text('{"previous_run": true}')
        environment = os.environ.copy()
        if broken_timing:
            destination.parent.mkdir(exist_ok=True)
            Path(str(destination) + ".wall-time.json").mkdir()
        if broken_tar or broken_copy or delayed or broken_cleanup:
            shim = root / "shim"
            shim.mkdir()
            for command, broken in [("tar", broken_tar), ("cp", broken_copy)]:
                if broken:
                    executable = shim / command
                    executable.write_text("#!/bin/sh\nexit 37\n")
                    executable.chmod(0o755)
            if delayed:
                for command in ("tar", "rm"):
                    executable = shim / command
                    executable.write_text(f"#!/bin/sh\nsleep 0.18\nexec {shlex.quote(shutil.which(command))} \"$@\"\n")
                    executable.chmod(0o755)
            if broken_cleanup:
                executable = shim / "rm"
                executable.write_text(f'#!/bin/sh\ncase "$*" in *oven-compiler-suite-output*) exit 38;; esac\nexec {shlex.quote(shutil.which("rm"))} "$@"\n')
                executable.chmod(0o755)
            environment["PATH"] = str(shim) + os.pathsep + environment["PATH"]
        arguments = [str(output), str(scratch), str(succeeded).lower(), str(destination), "0" if succeeded else "9"]
        if wrapper_clock:
            now = time.monotonic_ns()
            arguments += [str(now - 700_000_000), str(now - 500_000_000)]
        if delayed:
            # Drive the production retention function in a real child with a short test-only heartbeat cadence.
            command = [sys.executable, "-c", "import sys; from retain_oven_suite_output import retain; sys.exit(retain(sys.argv[1:], heartbeat_interval=0.025))", *arguments]
            process = subprocess.Popen(command, cwd=SCRIPT.parent, env=environment, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            live = []
            while True:
                line = process.stderr.readline()
                self.assertTrue(line, "retention exited without live progress")
                live.append(line)
                if line.startswith("WAIT"):
                    self.assertIsNone(process.poll(), "heartbeat arrived only after cleanup exited")
                    break
            stdout, stderr = process.communicate(timeout=10)
            result = subprocess.CompletedProcess(command, process.returncode, stdout, "".join(live) + stderr)
        elif broken_final_timing:
            code = """import sys
import retain_oven_suite_output as retention
original = retention.write_json_atomic
def fail_final(path, value):
    if value["complete"]:
        raise OSError("injected final publication failure")
    original(path, value)
retention.write_json_atomic = fail_final
sys.exit(retention.retain(sys.argv[1:]))
"""
            result = subprocess.run([sys.executable, "-c", code, *arguments], cwd=SCRIPT.parent, env=environment,
                                    capture_output=True, text=True, check=False)
        else:
            result = subprocess.run(["bash", str(SCRIPT), *arguments], env=environment, capture_output=True, text=True, check=False)
        self.assertFalse(scratch.exists())
        self.assertEqual(list(destination.parent.glob(destination.name + ".tmp.*")), [])
        return result, output, transcript, destination

    def assert_archive(self, destination):
        with tarfile.open(str(destination) + ".transcripts.tar.gz") as archive:
            self.assertEqual(archive.getnames(), ["./shards/path with spaces/probe.libtest-output.txt"])
            stream = archive.extractfile(archive.getnames()[0])
            self.assertIsNotNone(stream)
            self.assertEqual(stream.read(), b"complete captured diagnostic\n")

    def test_success_retains_report_and_transcripts_before_reclaiming_output(self):
        result, output, _, destination = self.run_cleanup()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(output.exists())
        self.assertTrue(destination.is_file())
        self.assert_archive(destination)
        timing = json.loads(Path(str(destination) + ".wall-time.json").read_text())
        self.assertEqual(timing["exit_status"], 0)
        self.assertEqual(set(timing["phases"]), {
            "wrapper_scratch_cleanup", "report_publication", "transcript_inventory", "transcript_archive", "caller_output_cleanup"
        })
        self.assertGreaterEqual(timing["retention_elapsed_ms"], sum(timing["phases"].values()))
        self.assertIsNone(timing["wrapper_elapsed_ms"], "a direct helper invocation has no wrapper clock")
        self.assertEqual(timing["transcripts"], {
            "inventory_complete": True,
            "file_count": 1,
            "input_bytes": len(b"complete captured diagnostic\n"),
            "archive_bytes": Path(str(destination) + ".transcripts.tar.gz").stat().st_size,
        })

    def test_failure_without_report_still_archives_diagnostics(self):
        result, output, transcript, destination = self.run_cleanup(report_exists=False, succeeded=False)
        self.assertEqual(result.returncode, 9, result.stderr)
        self.assertTrue(output.exists())
        self.assertTrue(transcript.is_file())
        self.assertFalse(destination.exists())
        self.assert_archive(destination)
        timing = json.loads(Path(str(destination) + ".wall-time.json").read_text())
        self.assertEqual(timing["exit_status"], 9)
        self.assertNotIn("caller_output_cleanup", timing["phases"])

    def test_archive_failure_preserves_output_and_fails_a_green_replay(self):
        result, output, transcript, destination = self.run_cleanup(broken_tar=True)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertTrue(output.exists())
        self.assertEqual(transcript.read_bytes(), b"complete captured diagnostic\n")
        self.assertFalse(Path(str(destination) + ".transcripts.tar.gz").exists())
        self.assertIn("archive failed", result.stderr)
        timing = json.loads(Path(str(destination) + ".wall-time.json").read_text())
        self.assertEqual(timing["transcripts"]["file_count"], 1)
        self.assertTrue(timing["transcripts"]["inventory_complete"])
        self.assertIsNone(timing["transcripts"]["archive_bytes"])

    def test_green_replay_without_requested_report_fails_but_keeps_diagnostics(self):
        result, output, _, destination = self.run_cleanup(report_exists=False)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertTrue(output.exists())
        self.assert_archive(destination)

    def test_failed_replay_without_report_cannot_reuse_an_older_report(self):
        result, output, _, destination = self.run_cleanup(
            report_exists=False, succeeded=False, previous_evidence=True
        )
        self.assertEqual(result.returncode, 9, result.stderr)
        self.assertTrue(output.exists())
        self.assertFalse(destination.exists(), "an earlier run's report was left as current evidence")
        self.assert_archive(destination)

    def test_archive_failure_cannot_pair_a_new_report_with_an_older_archive(self):
        result, output, _, destination = self.run_cleanup(broken_tar=True, previous_evidence=True)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertTrue(output.exists())
        self.assertEqual(destination.read_text(), '{"probe": true}\n')
        self.assertFalse(Path(str(destination) + ".transcripts.tar.gz").exists())

    def test_report_copy_failure_cannot_pair_an_older_report_with_a_new_archive(self):
        result, output, _, destination = self.run_cleanup(broken_copy=True, previous_evidence=True)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertTrue(output.exists())
        self.assertFalse(destination.exists(), "copy failure left the previous report beside current diagnostics")
        self.assertEqual((output / "compiler-suite-report.json").read_text(), '{"probe": true}\n')
        self.assert_archive(destination)

    def test_delayed_archive_and_deletion_report_live_progress_and_separate_durations(self):
        result, output, _, destination = self.run_cleanup(delayed=True, wrapper_clock=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(output.exists())
        timing = json.loads(Path(str(destination) + ".wall-time.json").read_text())
        self.assertTrue(timing["complete"])
        self.assertEqual(timing["wrapper_setup_elapsed_ms"], 200)
        self.assertGreaterEqual(timing["compiler_command_elapsed_ms"], 500)
        self.assertGreaterEqual(timing["wrapper_elapsed_ms"], 700 + timing["retention_elapsed_ms"])
        for name in ("wrapper_scratch_cleanup", "transcript_archive", "caller_output_cleanup"):
            lines = [line for line in result.stderr.splitlines() if name in line]
            self.assertTrue(lines[0].startswith("START"), lines)
            self.assertTrue(any(line.startswith("WAIT") for line in lines), lines)
            self.assertTrue(lines[-1].startswith("DONE"), lines)
            self.assertGreaterEqual(timing["phases"][name], 180)

    def test_failed_output_deletion_is_timed_and_cannot_claim_wrapper_success(self):
        result, output, _, destination = self.run_cleanup(broken_cleanup=True)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertTrue(output.exists())
        timing = json.loads(Path(str(destination) + ".wall-time.json").read_text())
        self.assertEqual(timing["exit_status"], 1)
        self.assertIn("caller_output_cleanup", timing["phases"])
        self.assertIn("FAILED", result.stderr)

    def test_failed_retention_preserves_the_replay_exit_status(self):
        result, output, _, destination = self.run_cleanup(report_exists=False, succeeded=False, broken_tar=True)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertTrue(output.exists())
        timing = json.loads(Path(str(destination) + ".wall-time.json").read_text())
        self.assertEqual(timing["exit_status"], 1)
        self.assertEqual(timing["replay_exit_status"], 9)

    def test_unpublishable_timing_artifact_preserves_caller_output(self):
        result, output, transcript, _ = self.run_cleanup(broken_timing=True)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertTrue(output.exists())
        self.assertTrue(transcript.exists())

    def test_final_timing_update_failure_leaves_current_incomplete_evidence_and_fails(self):
        result, output, _, destination = self.run_cleanup(broken_final_timing=True, previous_evidence=True)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertFalse(output.exists(), "this injected failure occurs after deletion")
        self.assert_archive(destination)
        timing = json.loads(Path(str(destination) + ".wall-time.json").read_text())
        self.assertFalse(timing["complete"])
        self.assertNotIn("previous_run", timing)
        self.assertIn("final timing publication failed", result.stderr)

    def test_make_replay_trap_retains_outer_clock_on_success_and_early_compiler_failure(self):
        for compiler_status in (0, 9):
            with self.subTest(compiler_status=compiler_status), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                (root / "scripts").mkdir()
                for filename in ("retain_oven_suite_output.py", "retain_oven_suite_output.sh"):
                    shutil.copyfile(SCRIPT.parent / filename, root / "scripts" / filename)
                binary = root / "target" / "debug" / "incan"
                binary.parent.mkdir(parents=True)
                binary.write_text(
                    '#!/bin/sh\nwhile [ "$#" -gt 0 ]; do if [ "$1" = --output ]; then shift; output=$1; fi; shift; done\n'
                    'printf "diagnostic\\n" > "$output/root.libtest-output.txt"\n'
                    + ("printf '%s\\n' '{\"success\":true}' > \"$output/compiler-suite-report.json\"\n" if compiler_status == 0 else '')
                    + f'exit {compiler_status}\n'
                )
                binary.chmod(0o755)
                shim = root / "shim"
                shim.mkdir()
                (shim / "rustup").write_text('#!/bin/sh\nprintf "/unused/fixture/tool\\n"\n')
                (shim / "rustup").chmod(0o755)
                destination = root / "report.json"
                result = subprocess.run(
                    ["make", "-s", "-f", str(SCRIPT.parent.parent / "Makefile"), "test-oven-replay", "TEST_RUNTIME_ENV=",
                     f"INCAN_TEST_OVEN_COMPILER_SUITE_REPORT={destination}"],
                    cwd=root, env={**os.environ, "PATH": str(shim) + os.pathsep + os.environ["PATH"]},
                    capture_output=True, text=True, check=False,
                )
                self.assertEqual(result.returncode == 0, compiler_status == 0, result.stderr)
                timing = json.loads(Path(str(destination) + ".wall-time.json").read_text())
                self.assertEqual(timing["exit_status"], compiler_status)
                self.assertTrue(timing["complete"])
                self.assertIsNotNone(timing["compiler_command_elapsed_ms"])
                self.assertIsNotNone(timing["wrapper_setup_elapsed_ms"])
                self.assertGreaterEqual(timing["wrapper_elapsed_ms"], timing["retention_elapsed_ms"])
                outputs = list((root / "target").glob("oven-compiler-suite-output.*"))
                self.assertEqual(len(outputs), 0 if compiler_status == 0 else 1)
                self.assertEqual(destination.exists(), compiler_status == 0)
                self.assertTrue(Path(str(destination) + ".transcripts.tar.gz").exists())

    def test_retained_report_cannot_remove_its_own_disposable_source(self):
        result, output, _, destination = self.run_cleanup(report_is_source=True)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertTrue(output.exists())
        self.assertEqual(destination.read_text(), '{"probe": true}\n')
        self.assertIn("must differ", result.stderr)


if __name__ == "__main__":
    unittest.main()
