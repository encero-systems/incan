"""Controlled-process acceptance for the opt-in compiler trace wrappers."""

import json
import os
from pathlib import Path
import subprocess
import signal
import time
import sys
import tempfile
import unittest

TOOL = Path(__file__).with_name('trace_replay_processes.py')


class TraceTests(unittest.TestCase):
    """Check actual overlapping children, failure, and bounded timeout evidence."""

    def exercise(self, source, timeout=10, expected=0):
        """Run an executable fixture through the same wrapper used for real roots."""
        with tempfile.TemporaryDirectory(prefix='trace-selftest-') as temporary:
            directory = Path(temporary)
            fixture = directory / 'fixture'
            fixture.write_text(f'#!{sys.executable}\n' + source)
            fixture.chmod(0o755)
            output = directory / 'trace'
            subprocess.run([sys.executable, str(TOOL), 'install', '--output', str(output),
                            '--incan', str(fixture), '--rustc', sys.executable], check=True)
            result = subprocess.run([sys.executable, str(TOOL), 'run', '--output', str(output),
                                     '--root', 'controlled', '--case', 'synthetic',
                                     '--timeout', str(timeout), '--', str(output / 'bin/incan')],
                                    capture_output=True, text=True, timeout=timeout + 10)
            self.assertEqual(result.returncode, expected, result.stderr)
            report = json.loads((output / 'summary.json').read_text())
            records = [json.loads(path.read_text()) for path in (output / 'events').glob('*.json')]
            return report, records, result.stdout

    def test_overlapping_descendants(self):
        """Two compiler children overlap beneath one intercepted parent invocation."""
        report, records, stdout = self.exercise('''import os, subprocess
children = [subprocess.Popen([os.environ['RUSTC'], '-c', 'import time; time.sleep(0.3)']) for _ in range(2)]
for child in children:
    assert child.wait() == 0
print('fixture output preserved')
''')
        self.assertEqual(report['record_count'], 3)
        self.assertEqual(report['peak_completed_interval_overlap'], 3)
        self.assertEqual(report['peak_completed_overlap_by_label'], {'incan': 1, 'rustc': 2})
        self.assertEqual(report['incomplete_count'], 0)
        self.assertIn('fixture output preserved', stdout)
        parent = next(record for record in records if record['label'] == 'incan')
        for record in records:
            self.assertLessEqual(record['start_before_ns'], record['start_after_ns'])
            self.assertLess(record['start_after_ns'], record['end_observed_ns'])
            self.assertEqual(record['case'], 'synthetic')
            if record['label'] == 'rustc':
                self.assertEqual(record['parent_invocation'], parent['invocation'])

    def test_failure_preserved(self):
        """A measured command failure remains a failed command and root."""
        report, records, _ = self.exercise('raise SystemExit(7)\n', expected=7)
        self.assertEqual(report['failed_count'], 1)
        self.assertEqual(records[0]['returncode'], 7)

    def test_signal_preserved(self):
        """A self-signalled child remains a signal failure through the wrapper."""
        report, records, _ = self.exercise('import os, signal\nos.kill(os.getpid(), signal.SIGTERM)\n', expected=143)
        self.assertEqual(report['root_returncode'], -15)
        self.assertEqual(records[0]['returncode'], -15)

    def test_interrupt_reaps_isolated_group(self):
        """Interrupting the tracer terminates its separately grouped workload."""
        with tempfile.TemporaryDirectory(prefix='trace-interrupt-') as temporary:
            directory = Path(temporary)
            ready = directory / 'ready'
            stopped = directory / 'stopped'
            fixture = directory / 'fixture'
            fixture.write_text(f"#!{sys.executable}\n" +
                               "import os, signal, time\nfrom pathlib import Path\n" +
                               f"def stop(signum, frame):\n    Path({str(stopped)!r}).write_text('yes')\n    raise SystemExit(0)\n" +
                               "signal.signal(signal.SIGTERM, stop)\n" +
                               f"Path({str(ready)!r}).write_text(str(os.getpid()))\ntime.sleep(30)\n")
            fixture.chmod(0o755)
            output = directory / 'trace'
            subprocess.run([sys.executable, str(TOOL), 'install', '--output', str(output),
                            '--incan', str(fixture)], check=True)
            process = subprocess.Popen([sys.executable, str(TOOL), 'run', '--output', str(output),
                                        '--root', 'interrupted', '--timeout', '20', '--',
                                        str(output / 'bin/incan')], stdout=subprocess.PIPE,
                                       stderr=subprocess.PIPE, text=True)
            try:
                deadline = time.monotonic() + 5
                while not ready.exists() and time.monotonic() < deadline:
                    time.sleep(0.01)
                self.assertTrue(ready.exists(), 'fixture never started')
                process.send_signal(signal.SIGINT)
                _, stderr = process.communicate(timeout=6)
                self.assertNotEqual(process.returncode, 0, stderr)
                self.assertTrue(stopped.exists(), 'fixture did not receive cleanup signal')
                with self.assertRaises(ProcessLookupError):
                    os.kill(int(ready.read_text()), 0)
            finally:
                if process.poll() is None:
                    process.kill()
                    process.communicate(timeout=5)

    def test_timeout_bounded(self):
        """Group timeout records failure rather than reporting a successful sample."""
        report, _, _ = self.exercise('import time\ntime.sleep(30)\n', timeout=0.3, expected=124)
        self.assertTrue(report['timed_out'])
        self.assertEqual(report['root_returncode'], 124)
        self.assertLess(report['root_elapsed_ms'], 5000)


if __name__ == '__main__':
    unittest.main()
