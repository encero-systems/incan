#!/bin/zsh
# Measure first-output and completion latency across three reproducible project shapes.
set -eu
setopt pipefail
zmodload zsh/datetime

script_dir=${0:A:h}
oven=${INCAN:?set INCAN to the installed Oven executable}
home_dir=${INCAN_HOME:?set INCAN_HOME to a freshly installed packed toolchain}
stdlib=${INCAN_STDLIB:?set INCAN_STDLIB to the matching standard source}
samples=${SAMPLES:-15}
(( samples >= 15 )) || { print -u2 'SAMPLES must be at least 15'; exit 2; }
first_round=${START_ROUND:-1}
(( first_round >= 1 && first_round <= samples )) || { print -u2 'START_ROUND must be within SAMPLES'; exit 2; }
scratch=${BENCH_SCRATCH:-$script_dir/target}
mkdir -p "$scratch"
output=$(mktemp -d "$scratch/dev-loop.XXXXXX")
report=${DEV_LOOP_REPORT:-$script_dir/../results/dev_loop.md}
export INCAN_HOME=$home_dir INCAN_STDLIB=$stdlib
manifest=$home_dir/toolchains/0.6.0-dev.8/toolchain.json
cp "$manifest" "$output/toolchain.json"
cp "$script_dir/run.sh" "$output/measure-run.sh"
cp "$script_dir/report.sh" "$output/report.sh"
printf 'subject\tmetric\tsample\tfirst_output_ms\tcompletion_ms\tstatus\ttarget_ms\n' > "$output/times.tsv"
uptime > "$output/start-load.txt"

write_value() {
    local project=$1 subject=$2 generation=$3 destination
    case $subject in
        single) destination=$project/src/app.incn ;;
        modules) destination=$project/src/values.incn ;;
        library) destination=$project/deps/values/src/lib.incn ;;
    esac
    print -rl -- '"""Expose the current positive generation for freshness checks."""' '' '' \
        'pub def value() -> int:' '    """Return the generation stored in this source file."""' \
        "    return $generation" > "$destination"
    if [[ $subject == single ]]; then
        print -rl -- '' '' 'def main() -> None:' '    """Print the current generation immediately."""' \
            '    println(value())' >> "$destination"
    fi
}

create_project() {
    local project=$1 subject=$2 generation=$3 entry=main imported=values
    mkdir -p "$project/src" "$project/tests"
    [[ $subject != single ]] || entry=app
    printf '[project]\nname = "dev_loop"\nversion = "0.1.0"\n\n[project.scripts]\nmain = "src/%s.incn"\n' "$entry" > "$project/loaf.toml"
    if [[ $subject == library ]]; then
        imported=pub::values
        mkdir -p "$project/deps/values/src"
        printf '\n[dependencies]\nvalues = { path = "deps/values" }\n' >> "$project/loaf.toml"
        printf '[project]\nname = "values"\nversion = "0.1.0"\n' > "$project/deps/values/loaf.toml"
    fi
    if [[ $subject != single ]]; then
        print -rl -- '"""Print a value supplied by another module."""' '' "from $imported import value" '' '' \
            'def main() -> None:' '    """Print the current generation immediately."""' \
            '    println(value())' > "$project/src/main.incn"
    fi
    [[ $subject != single ]] || imported=app
    print -rl -- '"""Execute the tested function after every source generation."""' '' \
        "from $imported import value" '' '' 'def test_value() -> None:' \
        '    """A changed implementation must still return a positive value."""' \
        '    assert value() > 0' > "$project/tests/test_value.incn"
    write_value "$project" "$subject" "$generation"
}

measure() {
    local subject=$1 metric=$2 round=$3 target=$4 expected=$5
    shift 5
    local prefix=$output/$subject-$metric-$round started code=0 first_ms=-
    uptime > "$prefix.load"
    started=$EPOCHREALTIME
    if [[ $expected != - ]]; then
        "$@" 2> "$prefix.stderr" | {
            if IFS= read -r first; then
                first_ms=$(( (EPOCHREALTIME - started) * 1000 ))
                print -r -- "$first_ms" > "$prefix.first"
                print -r -- "$first" > "$prefix.stdout"
                cat >> "$prefix.stdout"
                [[ $first == "$expected" ]]
            else
                false
            fi
        } || code=$?
        [[ ! -f $prefix.first ]] || first_ms=$(<"$prefix.first")
    else
        "$@" > "$prefix.stdout" 2> "$prefix.stderr" || code=$?
        if [[ $metric == test_* && $code == 0 ]]; then
            grep -q 'test_value.incn::test_value PASSED' "$prefix.stdout" || code=1
        fi
        if [[ $metric == graph_unchanged && $code == 0 ]]; then
            grep -q value "$prefix.stdout" || code=1
        fi
    fi
    local elapsed=$(( (EPOCHREALTIME - started) * 1000 ))
    printf '%s\t%s\t%s\t%s\t%.3f\t%s\t%s\n' "$subject" "$metric" "$round" "$first_ms" "$elapsed" "$code" "$target" >> "$output/times.tsv"
}

for subject in single modules library; do
    project=$output/$subject
    create_project "$project" "$subject" 1000
done

# Rotate subjects in each round so changing machine load is distributed across shapes.
for round in $(seq "$first_round" "$samples"); do
    for subject in single modules library; do
        project=$output/$subject
        entry=$project/src/main.incn
        [[ $subject != single ]] || entry=$project/src/app.incn
        generation=$(( 1000 + round * 3 ))
        write_value "$project" "$subject" "$generation"
        "$oven" run "$entry" > "$output/$subject-condition-run.log" 2>&1 || true
        measure "$subject" run_unchanged "$round" 50 "$generation" "$oven" run "$entry"
        (( generation += 1 ))
        write_value "$project" "$subject" "$generation"
        measure "$subject" run_edit "$round" 250 "$generation" "$oven" run "$entry"
        measure "$subject" check_unchanged "$round" - - "$oven" check "$entry"
        (( generation += 1 ))
        write_value "$project" "$subject" "$generation"
        measure "$subject" check_edit "$round" - - "$oven" check "$entry"
        "$oven" test "$project/tests/test_value.incn" > "$output/$subject-condition-test.log" 2>&1 || true
        measure "$subject" test_unchanged "$round" 100 - "$oven" test "$project/tests/test_value.incn"
        (( generation += 1 ))
        write_value "$project" "$subject" "$generation"
        measure "$subject" test_edit "$round" 400 - "$oven" test "$project/tests/test_value.incn"
        measure "$subject" fmt_check "$round" - - "$oven" fmt --check "$entry"
        "$oven" inspect codegraph "$entry" --query defs value > "$output/$subject-condition-graph.log" 2>&1 || true
        measure "$subject" graph_unchanged "$round" - - "$oven" inspect codegraph "$entry" --query defs value
        for profile in debug release; do
            "$oven" build "$entry" --profile "$profile" > "$output/$subject-condition-$profile.log" 2>&1 || true
            measure "$subject" build_${profile}_warm "$round" - - "$oven" build "$entry" --profile "$profile"
            cold=$output/$subject-$profile-cold-$round
            create_project "$cold" "$subject" "$generation"
            cold_entry=$cold/src/main.incn
            [[ $subject != single ]] || cold_entry=$cold/src/app.incn
            measure "$subject" build_${profile}_cold "$round" - - "$oven" build "$cold_entry" --profile "$profile"
            if [[ $profile == debug && -x $cold/target/incan/debug/bin/dev_loop ]]; then
                measure "$subject" direct_first_launch "$round" - "$generation" "$cold/target/incan/debug/bin/dev_loop"
                measure "$subject" direct_repeat_launch "$round" - "$generation" "$cold/target/incan/debug/bin/dev_loop"
            fi
        done
        fresh=$output/$subject-first-$round
        create_project "$fresh" "$subject" "$generation"
        fresh_entry=$fresh/src/main.incn
        [[ $subject != single ]] || fresh_entry=$fresh/src/app.incn
        measure "$subject" run_first "$round" 1000 "$generation" "$oven" run "$fresh_entry"
    done
done

"$script_dir/report.sh" "$output" "$report" "$oven" "$home_dir"
