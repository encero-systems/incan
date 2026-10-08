#!/usr/bin/env bash
# Run one exact test of the native driver root, logging to $HOME/test-<name>.log.
name="$1"
cd /home/user/incan || exit 1
export CARGO_TARGET_DIR=$HOME/incan-target INCAN_TEST_TMP_ROOT=$HOME/incan-tmp TMPDIR=$HOME/incan-tmp
make test-one TEST_ROOT=loaves/compiler/incan_driver/tests/native_driver_project_tests.rs TEST_EXACT="$name" > "$HOME/test-$name.log" 2>&1
echo "exit $?" >> "$HOME/test-$name.log"
