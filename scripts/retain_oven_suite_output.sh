#!/usr/bin/env bash
# Preserve replay evidence before reclaiming the invocation's temporary outputs.
set -u

if [ "$#" -ne 5 ]; then
    echo "usage: retain_oven_suite_output.sh OUTPUT TMP SUCCEEDED REPORT EXIT_STATUS" >&2
    exit 2
fi

suite_output=$1
suite_tmp=$2
suite_succeeded=$3
report=$4
suite_status=$5
retention_failed=false

if ! rm -rf -- "$suite_tmp"; then
    retention_failed=true
fi

if [ -n "$report" ]; then
    if [ "$report" -ef "$suite_output/compiler-suite-report.json" ]; then
        echo "Oven retained report must differ from the disposable source report" >&2
        echo "Oven suite output retained at $suite_output" >&2
        exit 1
    fi
    if ! mkdir -p -- "$(dirname -- "$report")"; then
        retention_failed=true
    fi
    # Invalidate both previous outputs before publication; any failure must leave current evidence or no evidence.
    if ! rm -f -- "$report" "$report.transcripts.tar.gz"; then
        echo "Oven previous evidence could not be cleared; retaining caller output at $suite_output" >&2
        exit 1
    fi
    if [ -s "$suite_output/compiler-suite-report.json" ]; then
        # Stage beside the requested destination so its final rename is on the same filesystem.
        report_tmp=''
        if report_tmp="$(mktemp "$report.tmp.XXXXXX")" \
            && cp -- "$suite_output/compiler-suite-report.json" "$report_tmp" \
            && mv -- "$report_tmp" "$report"; then
            :
        else
            if [ -n "$report_tmp" ]; then
                rm -f -- "$report_tmp"
            fi
            echo "Oven report publication failed; retaining complete caller output" >&2
            retention_failed=true
        fi
    else
        if [ "$suite_succeeded" = true ]; then
            echo "Oven replay succeeded without its requested JSON report" >&2
            retention_failed=true
        fi
    fi

    # A report-write failure must not hide the transcripts that were already produced. Stage the archive in caller
    # output so a failed tar leaves every diagnostic available and never publishes a partial archive as complete.
    transcript_list="$suite_output/.native-transcript-list"
    if (cd "$suite_output" && find . -type f -name '*.libtest-output.txt' -print0 > .native-transcript-list); then
        if [ -s "$transcript_list" ]; then
            if (cd "$suite_output" && COPYFILE_DISABLE=1 tar --null -T .native-transcript-list -czf .native-transcripts.tar.gz) \
                && mv -- "$suite_output/.native-transcripts.tar.gz" "$report.transcripts.tar.gz"; then
                :
            else
                echo "Oven transcript archive failed; retaining complete caller output" >&2
                retention_failed=true
            fi
        fi
    else
        echo "Oven transcript inventory failed; retaining caller output" >&2
        retention_failed=true
    fi
fi

if [ "$retention_failed" = true ]; then
    suite_status=1
fi
if [ "$suite_succeeded" = true ] && [ "$suite_status" -eq 0 ]; then
    if ! rm -rf -- "$suite_output"; then
        suite_status=1
    fi
else
    echo "Oven suite output retained at $suite_output" >&2
fi
exit "$suite_status"
