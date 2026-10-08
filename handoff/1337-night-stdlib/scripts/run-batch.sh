#!/usr/bin/env bash
# Run several exact tests of the native driver root in one Oven invocation; census output goes to $HOME/census/<name>.
name="$1"; shift
cd /home/user/incan || exit 1
export CARGO_TARGET_DIR=$HOME/incan-target INCAN_TEST_TMP_ROOT=$HOME/incan-tmp TMPDIR=$HOME/incan-tmp
mkdir -p "$HOME/census/$name"
exact="$1"; shift
for test in "$@"; do exact="$exact\" --exact \"$test"; done
INCAN_CENSUS_OUT="$HOME/census/$name" make test-one TEST_ROOT=loaves/compiler/incan_driver/tests/native_driver_project_tests.rs TEST_EXACT="$exact" > "$HOME/batch-$name.log" 2>&1
echo "exit $?" >> "$HOME/batch-$name.log"
