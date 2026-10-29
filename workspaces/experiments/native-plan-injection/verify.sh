#!/bin/bash
# Run the private native-plan injection controls; retain outputs and refusals as evidence.
set -euo pipefail
driver=${1:?usage: verify.sh DRIVER TOOLCHAIN_JSON NEW_OUTPUT_DIRECTORY}
manifest=${2:?missing toolchain manifest}
out=${3:?missing output directory}
root=$(cd "$(dirname "$0")/../../.." && pwd)
source_file=$root/workspaces/experiments/native-plan-injection/producer/src/lib.incn
mkdir "$out"
out=$(cd "$out" && pwd)
printf 'case\tstatus\tobserved\n' > "$out/results.tsv"

positive() {
    local name=$1 mode=$2 input=$3 expected=$4
    env -u RUSTC_BOOTSTRAP "$driver" --spike-plan "$mode" "$input" "$source_file" "$out/$name" "$manifest" \
        > "$out/$name.compile.stdout" 2> "$out/$name.compile.stderr"
    "$out/$name" > "$out/$name.stdout" 2> "$out/$name.stderr"
    printf '%s\n' "$expected" > "$out/$name.expected"
    cmp "$out/$name.expected" "$out/$name.stdout"
    printf '%s\tPASS\t%s\n' "$name" "$expected" >> "$out/results.tsv"
}

positive constant_42 constant 42 42
positive constant_73 constant 73 73
positive argument_add arithmetic 40 42
positive branch_true branch 42 42
positive branch_false branch -4 0
positive branch_zero branch 0 0
positive record_fields record 40 42
positive record_negative record -3 -1

if env -u RUSTC_BOOTSTRAP "$driver" --spike-plan invalid 42 "$source_file" "$out/invalid" "$manifest" \
    > "$out/invalid.stdout" 2> "$out/invalid.stderr"; then
    echo 'invalid return plan was unexpectedly accepted' >&2
    exit 1
fi
test ! -e "$out/invalid"
grep -F 'return local must match a scalar return type' "$out/invalid.stderr" > /dev/null
printf 'invalid_return\tPASS\trefused before artifact publication\n' >> "$out/results.tsv"

# Report source fidelity independently: a function-only message does not meet the diagnostic acceptance bar.
if grep -F "$source_file" "$out/invalid.stderr" > /dev/null; then
    printf 'diagnostic_source\tPRESENT\tinspect coordinate fidelity\n' >> "$out/results.tsv"
else
    printf 'diagnostic_source\tGAP\tpreflight error loses supplied source span\n' >> "$out/results.tsv"
fi
env -u RUSTC_BOOTSTRAP "$driver" --spike-plan arithmetic 9223372036854775807 "$source_file" "$out/overflow" "$manifest" \
    > "$out/overflow.compile.stdout" 2> "$out/overflow.compile.stderr"
if "$out/overflow" > "$out/overflow.stdout" 2> "$out/overflow.stderr"; then
    echo 'checked arithmetic overflow was unexpectedly accepted at runtime' >&2
    exit 1
fi
grep -F 'attempt to add with overflow' "$out/overflow.stderr" > /dev/null
printf 'checked_overflow\tPASS\truntime overflow refused\n' >> "$out/results.tsv"
if grep -F "$source_file:1:1" "$out/overflow.stderr" > /dev/null; then
    printf 'runtime_source\tPASS\tsupplied file and coordinates retained\n' >> "$out/results.tsv"
else
    printf 'runtime_source\tGAP\truntime failure lacks supplied coordinates\n' >> "$out/results.tsv"
fi

shasum -a 256 "$driver" "$manifest" "$source_file" > "$out/inputs.sha256"
cat "$out/results.tsv"
