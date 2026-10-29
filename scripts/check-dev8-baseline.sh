#!/usr/bin/env bash
# Compare a complete eight-family runner log with committed PASS keys, including missing and pending keys.
# Usage: check-dev8-baseline.sh BASELINE LOG
set -euo pipefail
[[ $# == 2 ]] || { echo 'usage: check-dev8-baseline.sh BASELINE LOG' >&2; exit 2; }
export LC_ALL=C
awk '
    function invalid(message) { print "dev.8 baseline: " message > "/dev/stderr"; bad = 1 }
    function key_valid(key) {
        return key ~ /^(behavior|check|library|interop|testing|project|format|graph)\/[a-z_][a-z0-9_]*\/[a-z_][a-z0-9_]*$/
    }
    NR == FNR {
        if ($0 ~ /^#/ || $0 == "") next
        if (NF != 1 || !key_valid($0)) invalid("invalid baseline key: " $0)
        if ($0 in baseline) invalid("duplicate baseline key: " $0)
        baseline[$0] = 1
        baseline_count++
        next
    }
    /^(PASS|FAIL|PEND) / {
        if (!key_valid($2)) invalid("invalid result key: " $2)
        if ($2 in results) invalid("duplicate result key: " $2)
        results[$2] = $1
        split($2, parts, "/")
        families[parts[1]]++
        counts[$1]++
    }
    /^total: [0-9]+ passed, [0-9]+ failed, [0-9]+ pending$/ {
        totals++
        passed = $2; failed = $4; pending = $6
    }
    END {
        if (baseline_count == 0) invalid("baseline is empty")
        if (totals != 1) invalid("expected one final total; runner log is incomplete")
        if (passed != counts["PASS"] || failed != counts["FAIL"] || pending != counts["PEND"])
            invalid("final total disagrees with case results")
        split("behavior check library interop testing project format graph", required, " ")
        for (i = 1; i <= 8; i++) {
            family = required[i]
            if (!families[family]) invalid("family absent: " family)
        }
        for (key in baseline) {
            status = key in results ? results[key] : "MISSING"
            if (status != "PASS") {
                invalid("lost PASS " key " -> " status)
                lost++
            }
        }
        printf "dev.8: %d PASS, %d FAIL, %d PEND; %d baseline keys, %d lost PASS\n", \
            counts["PASS"], counts["FAIL"], counts["PEND"], baseline_count, lost
        exit bad ? 1 : 0
    }
' "$1" "$2"
