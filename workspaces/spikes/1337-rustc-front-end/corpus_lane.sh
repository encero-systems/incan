#!/bin/sh
# The corpus native lane, first cut: compile every single-file behavior fixture that records `expect-stdout` through the
# step 11/12 driver, run it, and compare stdout and exit code with the fixture's header (the contract in the behavior
# fixtures' README). Each fixture lands in exactly one outcome:
#   pass       compiled natively, exact stdout, expected exit code
#   wrong      compiled natively, but stdout or exit code differ
#   refused    the native lowering refused a construct it does not cover yet (the reason is tallied)
#   failed     rustc rejected the front end's output, or the compiler crashed: a lowering defect
#   unchecked  the in-process front end could not check the file on its own (it lacks the project or stdlib context)
# Module, project, `expect-stdout-contains` and `expect-diagnostic` fixtures are counted as out of scope, never dropped.
#
# Usage: corpus_lane.sh <driver> <incan_std_core rlib> <incan_native_rt rlib> <deps dir> <fixtures dir> <scratch dir>
set -u

driver=$1 std_core=$2 native_rt=$3 deps=$4 fixtures=$5 scratch=$6
sysroot=$(rustc +1.98.0 --print sysroot)
export DYLD_LIBRARY_PATH="$sysroot/lib" LD_LIBRARY_PATH="$sysroot/lib"
mkdir -p "$scratch"
: > "$scratch/outcomes.tsv"

# The `expect-stdout` block: the indented `#   ` lines after the directive, up to the end of the header.
expected_stdout() {
  awk '
    /^[^#]/ { exit }
    in_block && /^#   / { sub(/^#   /, ""); print; next }
    in_block && /^#$/ { print ""; next }
    in_block { exit }
    /^# expect-stdout:/ { in_block = 1 }
  ' "$1"
}

expected_exit() {
  awk '/^[^#]/ { exit } /^# expect-exit:/ { print $3; found = 1 } END { if (!found) print 0 }' "$1"
}

for fixture in $(find "$fixtures" -name '*.incn' | sort); do
  relative=${fixture#"$fixtures"/}
  case $relative in
    */*/*) printf 'scope\t%s\tmodule or project fixture\n' "$relative" >> "$scratch/outcomes.tsv"; continue ;;
  esac
  if ! grep -q '^# expect-stdout:' "$fixture"; then
    printf 'scope\t%s\tno exact expect-stdout\n' "$relative" >> "$scratch/outcomes.tsv"
    continue
  fi
  name=$(basename "$fixture" .incn)
  binary="$scratch/bin_$name"
  if ! "$driver" --incan-source "$fixture" --sysroot "$sysroot" --edition 2024 --cap-lints allow --crate-type bin \
      --crate-name "$name" --extern incan_std_core="$std_core" --extern incan_native_rt="$native_rt" \
      -L dependency="$deps" -L "$(dirname "$native_rt")" -o "$binary" - </dev/null >"$scratch/compile.log" 2>&1; then
    reason=$(grep -m1 -o 'native lowering.*' "$scratch/compile.log" | sed -e 's/native lowering of `[^`]*` //' -e 's/^native lowering //')
    if grep -q 'checking failed' "$scratch/compile.log"; then
      printf 'unchecked\t%s\tthe standalone front end lacks the project or stdlib context\n' "$relative" >> "$scratch/outcomes.tsv"
    elif [ -n "$reason" ]; then
      printf 'refused\t%s\t%s\n' "$relative" "$reason" >> "$scratch/outcomes.tsv"
    else
      first=$(grep -m1 -E '^error|internal compiler error|panicked' "$scratch/compile.log" | cut -c1-160)
      printf 'failed\t%s\t%s\n' "$relative" "$first" >> "$scratch/outcomes.tsv"
    fi
    continue
  fi
  expected_stdout "$fixture" > "$scratch/expected.txt"
  actual_exit=0
  timeout 20 "$binary" > "$scratch/actual.txt" 2>/dev/null || actual_exit=$?
  if cmp -s "$scratch/expected.txt" "$scratch/actual.txt" && [ "$actual_exit" = "$(expected_exit "$fixture")" ]; then
    printf 'pass\t%s\t\n' "$relative" >> "$scratch/outcomes.tsv"
  else
    printf 'wrong\t%s\texit %s\n' "$relative" "$actual_exit" >> "$scratch/outcomes.tsv"
  fi
  rm -f "$binary"
done

printf 'outcomes:\n'
cut -f1 "$scratch/outcomes.tsv" | sort | uniq -c | sort -rn
printf '\nrefusal reasons:\n'
awk -F'\t' '$1 == "refused" { print $3 }' "$scratch/outcomes.tsv" | sort | uniq -c | sort -rn | head -30
printf '\nfailures:\n'
awk -F'\t' '$1 == "failed" || $1 == "wrong" { print $1 ": " $2 " -- " $3 }' "$scratch/outcomes.tsv" | head -30
