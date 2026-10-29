#!/bin/zsh
# Render retained samples without repeating measured commands.
set -eu
setopt pipefail
output=${1:?select a sample directory}
report=${2:?select an output Markdown file}
oven=${3:?select the measured Oven executable}
home_dir=${4:?select the measured installed home}
table=${5:-$output/times.tsv}
script_dir=${0:A:h}
driver=$(jq -er .driver "$output/toolchain.json")
[[ $driver == /* ]] || driver=$home_dir/toolchains/0.6.0-dev.8/$driver
mkdir -p "${report:h}"
{
    print -rl -- '# Development-loop measurements' ''
    print -rl -- "Report generated: $(date -u +%FT%TZ). Harness checkout: $(git -C "$script_dir" rev-parse HEAD). Host: $(uname -sm)." ''
    print -rl -- "Driver SHA-256: $(shasum -a 256 "$driver" | cut -d ' ' -f1). Oven SHA-256: $(shasum -a 256 "$oven" | cut -d ' ' -f1)." ''
    print -rl -- "Raw samples, load readings and command output: ${output:t} under BENCH_SCRATCH. Table: ${table:t}." ''
    print -rl -- 'Run metrics measure time to the first verified output line. Other metrics measure completion. Each command starts a new CLI process; project owners may be reused. Subjects rotate within each round to interleave load. Spread is minimum to maximum. Failed commands have no passing timing.' ''
    print -rl -- 'Direct first/repeat launches isolate a newly compiled executable from its repeated execution. Their difference includes system assessment and cache effects; no fixed OS-cost subtraction is applied.' ''
    print -rl -- 'The dev.7 handoff measured small unchanged commands at 11–15 ms and one check at 12.76 ms. Other rows have no matched historical baseline.' ''
    print '| Subject | Metric | Target ms | dev.7 ms | dev.8 median ms | Spread ms | Met target | Successful |'
    print '| --- | --- | ---: | --- | ---: | --- | --- | --- |'
    awk -F '\t' 'NR > 1 {print $1 "\t" $2 "\t" $7}' "$table" | sort -u | while IFS=$'\t' read -r subject metric target; do
        awk -F '\t' -v subject="$subject" -v metric="$metric" '$1 == subject && $2 == metric {print ($4 == "-" ? $5 : $4), $6}' "$table" | sort -n | \
            awk -v subject="$subject" -v metric="$metric" -v target="$target" '
                {total++; if ($2 == 0) {values[++n]=$1; if (target != "-" && $1 < target) met++}}
                END {
                    baseline = metric == "run_unchanged" ? "11-15 (small workloads)" : (metric == "check_unchanged" ? "12.76 (one workload)" : "unmeasured")
                    median = n ? (n % 2 ? values[(n+1)/2] : (values[n/2] + values[n/2+1])/2) : 0
                    spread = n ? sprintf("%.3f-%.3f", values[1], values[n]) : "refused"
                    measured = n ? sprintf("%.3f", median) : "refused"
                    met_target = target == "-" ? "n/a" : sprintf("%d/%d", met, total)
                    printf "| %s | %s | %s | %s | %s | %s | %s | %d/%d |\n", subject, metric, target, baseline, measured, spread, met_target, n, total
                }'
    done
} > "$report"
print -r -- "$report"
awk -F '\t' 'NR > 1 && $6 != 0 {failed = 1} END {exit failed}' "$table"
