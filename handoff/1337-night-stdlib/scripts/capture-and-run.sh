#!/usr/bin/env bash
# Build the native driver test root through the Oven, capture the test process environment, stop the Oven root, and
# run the same binary directly without the 60-minute root deadline. usage: capture-and-run.sh <name> <exact-test>...
S=/tmp/claude-0/-home-user-incan/9cb94770-7bd5-506f-aff2-6b17a01dcafd/scratchpad
name="$1"; shift
capture="$S/capture-$name"; mkdir -p "$capture"
rm -rf /root/incan-target/oven-explicit-bake-workspace
setsid nohup "$HOME/run-batch.sh" "capture-$name" "$@" > /dev/null 2>&1 < /dev/null &
until pid=$(pgrep -f "^/root/incan-target/oven-test-one\.[A-Za-z0-9]+/shards/0000/0000-incan_driver-test-native_driver_project_tests" | head -1) && [ -n "$pid" ]; do
  if grep -q "^exit " "$HOME/batch-capture-$name.log" 2>/dev/null; then echo "oven root ended before the test binary started" > "$HOME/direct-$name.log"; echo "exit 99" >> "$HOME/direct-$name.log"; exit 1; fi
  sleep 2
done
cp "/proc/$pid/environ" "$capture/environ"; tr '\0' '\n' < "/proc/$pid/cmdline" > "$capture/cmdline"; readlink "/proc/$pid/cwd" > "$capture/cwd"
bash "$S/stop-runs.sh"
while pgrep -f "oven-test-one\.[A-Za-z0-9]+/shards" > /dev/null; do sleep 2; done
rm -rf /root/incan-target/oven-explicit-bake-workspace
python3 -I "$S/direct-run.py" "$capture" "$HOME/census/$name" "$HOME/direct-$name.log" "$@"
