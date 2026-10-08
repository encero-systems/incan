"""Run an Oven-built native-driver test binary directly with the environment captured from its Oven root.

The Oven's compiler-suite root deadline is fixed at 60 minutes; this runs the same binary, arguments, working directory
and environment with no deadline. usage: direct-run.py <capture-dir> <census-out> <log> <exact-test>...
"""
import os, subprocess, sys

capture, census_out, log, *exact = sys.argv[1:]
with open(os.path.join(capture, "environ"), "rb") as handle:
    pairs = [entry.split(b"=", 1) for entry in handle.read().split(b"\0") if b"=" in entry]
environment = {key.decode(): value.decode() for key, value in pairs}
with open(os.path.join(capture, "cmdline")) as handle:
    binary = handle.readline().strip()
with open(os.path.join(capture, "cwd")) as handle:
    directory = handle.read().strip()
environment["INCAN_CENSUS_OUT"] = census_out
environment.pop("TEST_EXACT", None)
os.makedirs(environment["TMPDIR"], exist_ok=True)
os.makedirs(census_out, exist_ok=True)
arguments = [binary]
for name in exact:
    arguments += ["--exact", name]
arguments += ["--include-ignored", "--show-output", "--test-threads", "1"]
with open(log, "wb") as output:
    result = subprocess.run(arguments, cwd=directory, env=environment, stdout=output, stderr=subprocess.STDOUT)
with open(log, "ab") as output:
    output.write(f"exit {result.returncode}\n".encode())
