"""Retain current replay evidence and measure every potentially slow cleanup phase."""

from contextlib import contextmanager
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import threading
import time


HEARTBEAT_SECONDS = 30


def clock_ns():
    """Use the same host monotonic clock in Make and the retention subprocess."""
    return time.monotonic_ns()


def announce(state, subject, detail):
    print(f"{state:<10} {subject} ({detail})", file=sys.stderr, flush=True)


@contextmanager
def phase(name, timings, interval):
    """Report liveness during blocking filesystem work, then join before completion."""
    started = clock_ns()
    stop = threading.Event()
    announce("START", name, "suite retention")

    def heartbeat():
        while not stop.wait(interval):
            announce("WAIT", name, f"{(clock_ns() - started) / 1e9:.1f}s elapsed")

    worker = threading.Thread(target=heartbeat, daemon=True)
    worker.start()
    succeeded = False
    try:
        yield
        succeeded = True
    finally:
        stop.set()
        worker.join()
        elapsed = (clock_ns() - started) // 1_000_000
        timings[name] = elapsed
        announce("DONE" if succeeded else "FAILED", name, f"{elapsed / 1000:.3f}s")


def run(*command, **kwargs):
    subprocess.run(command, check=True, **kwargs)


def write_json_atomic(path, value):
    """Never expose a partial timing document or reuse an older invocation's record."""
    descriptor, temporary = tempfile.mkstemp(prefix=path.name + ".tmp.", dir=path.parent)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as stream:
            json.dump(value, stream, indent=2)
            stream.write("\n")
        os.replace(temporary, path)
    finally:
        Path(temporary).unlink(missing_ok=True)


def raise_walk_error(error):
    """A partial transcript inventory cannot be published as a complete archive."""
    raise error


def retain(arguments, *, heartbeat_interval=HEARTBEAT_SECONDS):
    """Return the replay or retention status, preserving diagnostics on failed publication."""
    if len(arguments) not in (5, 7):
        print("usage: retain_oven_suite_output.sh OUTPUT TMP SUCCEEDED REPORT EXIT_STATUS [WRAPPER_START COMMAND_START]", file=sys.stderr)
        return 2
    output, scratch = map(Path, arguments[:2])
    succeeded = arguments[2] == "true"
    report = Path(arguments[3]) if arguments[3] else None
    status = int(arguments[4])
    replay_status = status
    wrapper_started, command_started = (map(int, arguments[5:]) if len(arguments) == 7 else (None, None))
    retention_started = clock_ns()
    timings = {}
    transcripts = {"inventory_complete": False, "file_count": 0, "input_bytes": 0, "archive_bytes": None}
    failed = False
    timing_path = Path(str(report) + ".wall-time.json") if report else None

    def snapshot(complete):
        now = clock_ns()
        return {
            "schema_version": 1,
            "complete": complete,
            "exit_status": status,
            "replay_exit_status": replay_status,
            "phases": timings,
            "transcripts": transcripts,
            "wrapper_setup_elapsed_ms": ((command_started or retention_started) - wrapper_started) // 1_000_000 if wrapper_started else None,
            "compiler_command_elapsed_ms": (retention_started - command_started) // 1_000_000 if command_started else None,
            "retention_elapsed_ms": (now - retention_started) // 1_000_000,
            "wrapper_elapsed_ms": (now - wrapper_started) // 1_000_000 if wrapper_started else None,
            "scope": "wrapper clock through retention snapshot; compiler interval includes invocation setup and trap entry; excludes final timing-file publication and interpreter exit",
        }

    try:
        with phase("wrapper_scratch_cleanup", timings, heartbeat_interval):
            run("rm", "-rf", "--", str(scratch))
    except (OSError, subprocess.CalledProcessError) as error:
        print(f"Oven scratch cleanup failed: {error}", file=sys.stderr)
        failed = True

    if report:
        source = output / "compiler-suite-report.json"
        try:
            if source.exists() and report.exists() and source.samefile(report):
                raise ValueError("Oven retained report must differ from the disposable source report")
            # No retained artifact may live under the directory reclaimed at the end of this operation.
            if report.resolve().is_relative_to(output.resolve()):
                raise ValueError("Oven retained report must differ from the disposable source report directory")
            report.parent.mkdir(parents=True, exist_ok=True)
            for previous in (report, Path(str(report) + ".transcripts.tar.gz"), timing_path):
                previous.unlink(missing_ok=True)
        except (OSError, ValueError) as error:
            print(f"{error}; Oven suite output retained at {output}", file=sys.stderr)
            return 1

        try:
            with phase("report_publication", timings, heartbeat_interval):
                if source.is_file() and source.stat().st_size:
                    descriptor, temporary = tempfile.mkstemp(prefix=report.name + ".tmp.", dir=report.parent)
                    os.close(descriptor)
                    try:
                        run("cp", "--", str(source), temporary)
                        os.replace(temporary, report)
                    finally:
                        Path(temporary).unlink(missing_ok=True)
                elif succeeded:
                    raise ValueError("Oven replay succeeded without its requested JSON report")
        except (OSError, ValueError, subprocess.CalledProcessError) as error:
            print(f"Oven report publication failed; retaining complete caller output: {error}", file=sys.stderr)
            failed = True

        paths = []
        try:
            with phase("transcript_inventory", timings, heartbeat_interval):
                # Preserve filename bytes, including whitespace and newlines, without following directory symlinks.
                for directory, _, filenames in os.walk(output, onerror=raise_walk_error):
                    for filename in filenames:
                        if not filename.endswith(".libtest-output.txt"):
                            continue
                        path = Path(directory) / filename
                        metadata = path.stat(follow_symlinks=False)
                        if stat.S_ISREG(metadata.st_mode):
                            paths.append(os.fsencode("./" + str(path.relative_to(output))))
                            transcripts["file_count"] += 1
                            transcripts["input_bytes"] += metadata.st_size
                transcripts["inventory_complete"] = True
        except OSError as error:
            print(f"Oven transcript inventory failed; retaining complete caller output: {error}", file=sys.stderr)
            failed = True

        if transcripts["inventory_complete"]:
            try:
                with phase("transcript_archive", timings, heartbeat_interval):
                    if paths:
                        descriptor, staged = tempfile.mkstemp(prefix=report.name + ".archive.tmp.", dir=report.parent)
                        os.close(descriptor)
                        try:
                            run("tar", "--null", "-T", "-", "-czf", str(Path(staged).resolve()),
                                input=b"\0".join(paths) + b"\0", cwd=output,
                                env={**os.environ, "COPYFILE_DISABLE": "1"})
                            archive_bytes = Path(staged).stat().st_size
                            os.replace(staged, Path(str(report) + ".transcripts.tar.gz"))
                            transcripts["archive_bytes"] = archive_bytes
                        finally:
                            Path(staged).unlink(missing_ok=True)
            except (OSError, subprocess.CalledProcessError) as error:
                print(f"Oven transcript archive failed; retaining complete caller output: {error}", file=sys.stderr)
                failed = True

    if failed:
        status = 1
    # Establish current timing evidence before deleting caller output. A failed final update leaves an explicitly
    # incomplete record, never a previous run's measurements or a false claim that cleanup finished.
    if timing_path:
        try:
            write_json_atomic(timing_path, snapshot(False))
        except OSError as error:
            print(f"Oven timing publication failed; retaining caller output: {error}", file=sys.stderr)
            status = 1
            failed = True
    if succeeded and status == 0:
        try:
            with phase("caller_output_cleanup", timings, heartbeat_interval):
                run("rm", "-rf", "--", str(output))
        except (OSError, subprocess.CalledProcessError) as error:
            print(f"Oven caller output cleanup failed: {error}", file=sys.stderr)
            status = 1
    else:
        print(f"Oven suite output retained at {output}", file=sys.stderr)
    if timing_path:
        try:
            write_json_atomic(timing_path, snapshot(True))
        except OSError as error:
            print(f"Oven final timing publication failed: {error}", file=sys.stderr)
            status = 1
    elapsed = (clock_ns() - (wrapper_started or retention_started)) / 1e9
    announce("DONE" if status == 0 else "FAILED", "suite wrapper" if wrapper_started else "suite retention", f"{elapsed:.3f}s, exit {status}")
    return status


if __name__ == "__main__":
    if sys.argv[1:] == ["--clock"]:
        print(clock_ns())
    else:
        sys.exit(retain(sys.argv[1:]))
