#!/usr/bin/env bash
# Stop detached Oven test runs, never the calling shell.
self=$$; parent=$PPID
for pid in $(pgrep -f 'run-batch\.sh|run-census\.sh|run-test\.sh|incan oven compiler-libtests|incan oven bake|oven-test-one'); do
  if [ "$pid" != "$self" ] && [ "$pid" != "$parent" ]; then kill "$pid" 2>/dev/null; fi
done
