"""Exercise the replay cleanup boundary with real shell/archive subprocesses."""

import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("retain_oven_suite_output.sh")


class TranscriptRetentionTests(unittest.TestCase):
    def run_cleanup(self, *, report_exists=True, succeeded=True, broken_tar=False):
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
        destination = root / "retained" / "report.json"
        environment = os.environ.copy()
        if broken_tar:
            shim = root / "shim"
            shim.mkdir()
            tar = shim / "tar"
            tar.write_text("#!/bin/sh\nexit 37\n")
            tar.chmod(0o755)
            environment["PATH"] = str(shim) + os.pathsep + environment["PATH"]
        result = subprocess.run(
            ["bash", str(SCRIPT), str(output), str(scratch), str(succeeded).lower(), str(destination), "0" if succeeded else "9"],
            env=environment,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertFalse(scratch.exists())
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

    def test_failure_without_report_still_archives_diagnostics(self):
        result, output, transcript, destination = self.run_cleanup(report_exists=False, succeeded=False)
        self.assertEqual(result.returncode, 9, result.stderr)
        self.assertTrue(output.exists())
        self.assertTrue(transcript.is_file())
        self.assertFalse(destination.exists())
        self.assert_archive(destination)

    def test_archive_failure_preserves_output_and_fails_a_green_replay(self):
        result, output, transcript, destination = self.run_cleanup(broken_tar=True)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertTrue(output.exists())
        self.assertEqual(transcript.read_bytes(), b"complete captured diagnostic\n")
        self.assertFalse(Path(str(destination) + ".transcripts.tar.gz").exists())
        self.assertIn("archive failed", result.stderr)

    def test_green_replay_without_requested_report_fails_but_keeps_diagnostics(self):
        result, output, _, destination = self.run_cleanup(report_exists=False)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertTrue(output.exists())
        self.assert_archive(destination)


if __name__ == "__main__":
    unittest.main()
