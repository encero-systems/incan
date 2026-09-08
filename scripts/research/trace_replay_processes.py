#!/usr/bin/env python3
"""Measure intercepted compiler subprocess lifetimes without changing the test runner.

Install explicit compiler wrappers, then launch a focused root with ``run``. The
wrappers preserve stdout/stderr and the process group. Absolute compiler paths
that bypass the wrappers are outside the measurement; this is not a full process
census. Records use a shared host monotonic clock and contain no source arguments.
"""

import argparse
import json
import math
import os
from pathlib import Path
import resource
import shlex
import signal
import subprocess
import sys
import time
import uuid


def save(path, value):
    """Atomically publish one invocation's record without concurrent append races."""
    pending = path.with_suffix('.pending')
    pending.write_text(json.dumps(value, sort_keys=True) + '\n')
    pending.replace(path)


def wrap(args):
    """Forward one child while recording its launch bracket and observed exit."""
    directory = Path(args.output).resolve()
    invocation = uuid.uuid4().hex
    path = directory / 'events' / f'{invocation}.json'
    command = args.command
    if command and command[0] == '--':
        command = command[1:]
    if not command:
        raise ValueError('wrapper requires an executable')
    record = {
        'schema': 1, 'invocation': invocation,
        'parent_invocation': os.environ.get('INCAN_TRACE_PARENT'),
        'wrapper_pid': os.getpid(), 'parent_pid': os.getppid(),
        'label': args.label, 'operation': command[1] if len(command) > 1 and command[1] in
        {'build', 'run', 'check', 'test', 'oven', 'lock', 'env', '--version', '-vV'} else 'other',
        'root': os.environ.get('INCAN_TRACE_ROOT'),
        'case': os.environ.get('INCAN_TRACE_CASE'),
        'start_before_ns': time.monotonic_ns(), 'status': 'starting',
    }
    save(path, record)
    environment = os.environ.copy()
    environment['INCAN_TRACE_PARENT'] = invocation
    try:
        child = subprocess.Popen(command, env=environment)
    except OSError as error:
        record.update(status='spawn_failed', error=str(error), end_observed_ns=time.monotonic_ns())
        save(path, record)
        return 127
    record.update(child_pid=child.pid, start_after_ns=time.monotonic_ns(), status='running')
    save(path, record)
    # Reap the child and preserve evidence when the wrapper is signalled. A
    # group signal may already have reached it; a repeated TERM/INT is harmless
    # for ordinary compiler processes but custom signal handlers are outside
    # this research wrapper's transparency contract.
    def forward(signum, _frame):
        try:
            child.send_signal(signum)
        except ProcessLookupError:
            pass

    previous = {}
    for signum in (signal.SIGINT, signal.SIGTERM):
        previous[signum] = signal.signal(signum, forward)
    code = child.wait()
    usage = resource.getrusage(resource.RUSAGE_CHILDREN)
    record.update(status='exited', returncode=code, end_observed_ns=time.monotonic_ns(),
                  user_cpu_seconds=usage.ru_utime, system_cpu_seconds=usage.ru_stime,
                  maxrss=usage.ru_maxrss, maxrss_unit='bytes' if sys.platform == 'darwin' else 'KiB')
    save(path, record)
    for signum, handler in previous.items():
        signal.signal(signum, handler)
    if code < 0:
        signal.signal(-code, signal.SIG_DFL)
        os.kill(os.getpid(), -code)
    return code


def install(args):
    """Write task-owned wrapper executables with literal-safe interpreter arguments."""
    directory = Path(args.output).resolve()
    directory.mkdir(parents=True, exist_ok=True)
    (directory / 'events').mkdir(exist_ok=True)
    wrappers = directory / 'bin'
    wrappers.mkdir(exist_ok=True)
    for label, executable in (('incan', args.incan), ('rustc', args.rustc)):
        if executable:
            target = Path(executable).resolve(strict=True)
            wrapper = wrappers / label
            if target == wrapper:
                raise ValueError('a wrapper cannot wrap itself')
            argv = [sys.executable, str(Path(__file__).resolve()), 'wrap', '--output', str(directory),
                    '--label', label, '--', str(target)]
            wrapper.write_text('#!/bin/sh\nexec ' + shlex.join(argv) + ' "$@"\n')
            wrapper.chmod(0o755)
    return 0


def summarize(directory):
    """Summarize intercepted intervals; unfinished records never become success."""
    records = [json.loads(path.read_text()) for path in sorted((directory / 'events').glob('*.json'))]
    intervals = []
    durations = {}
    by_label = {}
    for record in records:
        if record['status'] != 'exited':
            continue
        start = record['start_after_ns']
        end = record['end_observed_ns']
        intervals.extend(((start, 1), (end, -1)))
        by_label.setdefault(record['label'], []).extend(((start, 1), (end, -1)))
        durations.setdefault(record['label'], []).append((end - start) / 1_000_000)
    def peak_overlap(events):
        active = peak = 0
        for _, change in sorted(events):
            active += change
            peak = max(peak, active)
        return peak
    return {
        'schema': 1, 'coverage': 'intercepted compiler commands only; absolute paths may bypass wrappers',
        'clock': 'host monotonic nanoseconds; start bracket and exit observed after wait',
        'record_count': len(records), 'completed_count': sum(r['status'] == 'exited' for r in records),
        'incomplete_count': sum(r['status'] in {'starting', 'running'} for r in records),
        'spawn_failed_count': sum(r['status'] == 'spawn_failed' for r in records),
        'failed_count': sum(r.get('returncode', 0) != 0 for r in records),
        'peak_completed_interval_overlap': peak_overlap(intervals),
        'peak_completed_overlap_by_label': {label: peak_overlap(events) for label, events in by_label.items()},
        'duration_ms_by_label': durations,
    }


def stop_group(child):
    """Reap the leader and kill its isolated group even when descendants outlive it."""
    try:
        os.killpg(child.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        child.wait(timeout=2)
    except subprocess.TimeoutExpired:
        pass
    try:
        os.killpg(child.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    child.wait(timeout=2)


def run(args):
    """Launch one bounded root with opt-in wrappers and save the measured result."""
    directory = Path(args.output).resolve()
    if list((directory / 'events').glob('*.json')):
        raise ValueError('use a fresh output directory for every measured run')
    if not math.isfinite(args.timeout) or args.timeout <= 0:
        raise ValueError('timeout must be positive')
    environment = os.environ.copy()
    environment.pop('INCAN_TRACE_PARENT', None)
    environment['INCAN_TRACE_ROOT'] = args.root
    if args.case:
        environment['INCAN_TRACE_CASE'] = args.case
    else:
        environment.pop('INCAN_TRACE_CASE', None)
    wrappers = directory / 'bin'
    environment['PATH'] = str(wrappers) + os.pathsep + environment.get('PATH', '')
    if (wrappers / 'incan').exists():
        environment['CARGO_BIN_EXE_incan'] = str(wrappers / 'incan')
    if (wrappers / 'rustc').exists():
        environment['RUSTC'] = str(wrappers / 'rustc')
    command = args.command[1:] if args.command and args.command[0] == '--' else args.command
    if not command:
        raise ValueError('run requires a command')
    started = time.monotonic_ns()
    child = subprocess.Popen(command, env=environment, start_new_session=True)
    timed_out = False
    try:
        try:
            code = child.wait(timeout=args.timeout)
        except subprocess.TimeoutExpired:
            timed_out = True
            stop_group(child)
            code = 124
        report = summarize(directory)
        report.update(root=args.root, case=args.case, root_returncode=code, timed_out=timed_out,
                      root_elapsed_ms=(time.monotonic_ns() - started) / 1_000_000)
        save(directory / 'summary.json', report)
        print(json.dumps(report, indent=2))
        return code if code >= 0 else 128 - code
    except BaseException:
        # Interrupts and report failures must not strand an isolated process group.
        stop_group(child)
        raise


def main():
    """Parse the research-only installation, execution and report commands."""
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='mode', required=True)
    setup = commands.add_parser('install')
    setup.add_argument('--output', required=True)
    setup.add_argument('--incan')
    setup.add_argument('--rustc')
    launch = commands.add_parser('run')
    launch.add_argument('--output', required=True)
    launch.add_argument('--root', required=True)
    launch.add_argument('--case')
    launch.add_argument('--timeout', type=float, default=120)
    launch.add_argument('command', nargs=argparse.REMAINDER)
    wrapper = commands.add_parser('wrap')
    wrapper.add_argument('--output', required=True)
    wrapper.add_argument('--label', required=True)
    wrapper.add_argument('command', nargs=argparse.REMAINDER)
    report = commands.add_parser('summary')
    report.add_argument('--output', required=True)
    args = parser.parse_args()
    if args.mode == 'summary':
        print(json.dumps(summarize(Path(args.output).resolve()), indent=2))
        return 0
    return {'install': install, 'run': run, 'wrap': wrap}[args.mode](args)


if __name__ == '__main__':
    sys.exit(main())
