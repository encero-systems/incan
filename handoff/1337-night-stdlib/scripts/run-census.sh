#!/usr/bin/env bash
# Run the direct-route census into the named output directory, logging to $HOME/census-<name>.log.
name="$1"
cd /home/user/incan || exit 1
export CARGO_TARGET_DIR=$HOME/incan-target INCAN_TEST_TMP_ROOT=$HOME/incan-tmp TMPDIR=$HOME/incan-tmp
mkdir -p "$HOME/census/$name"
INCAN_CENSUS_OUT="$HOME/census/$name" make test-one TEST_ROOT=loaves/compiler/incan_driver/tests/native_driver_project_tests.rs TEST_EXACT=direct_route_fixture_census > "$HOME/census-$name.log" 2>&1
echo "exit $?" >> "$HOME/census-$name.log"
