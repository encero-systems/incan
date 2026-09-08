#!/usr/bin/env bash
# Retain the stable shell entrypoint; one process owns monotonic timings and cleanup heartbeats.
exec python3 "$(dirname -- "$0")/retain_oven_suite_output.py" "$@"
