# RFC 178: Oven time reports

- **Status:** Draft
- **Created:** 2026-11-11
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 118 (Incan and Oven command-line surfaces)
    - RFC 124 (the bakery: one path for cold and warm builds)
    - RFC 145 (Oven storage lifecycle, retention and compact evidence)
    - RFC 167 (debugging)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Oven answers "where did the time go?" for any build, run or test. With `--timings`, an invocation writes a time report: one self-contained HTML file and one `incan.time-report/1` JSON document carrying the same facts. The report places every process and thread on one clock. It derives the critical path from recorded dependency, scheduling and wait relationships, and says plainly where those records are missing rather than guessing from timestamps. It keeps work apart from waiting and traced activity apart from CPU time, says why each unit was rebuilt, and ranks hypotheses with their measured evidence. `oven timings` builds reports from recorded traces, compares two reports, checks a report against elapsed-time and work budgets for CI, and exports the latest report's measurements as OpenMetrics gauges, so a dashboard can follow a project's build times.

## Motivation

A slow build leaves people guessing. Oven and the compiler already record spans for their phases and requests, but each process writes its own file on its own clock, and nothing joins them into an answer. The questions are ordinary:

- What took the time, and which part of it decided the wall time?
- Was the build doing several things at once, or waiting on one lane?
- Was something done twice, or rebuilt when nothing it depends on changed?
- Did this change make the build faster or slower, and where?
- Is the edit loop inside its budget?

Making a toolchain fast is a loop of measuring, changing and measuring again. Each turn of it today needs bespoke scripts to join traces, align clocks and diff two runs. A report that answers these questions directly makes that loop cheap for the people who build Incan and for everyone who builds with it. The same facts in a stable JSON schema let agents and CI compare runs, gate regressions and attach findings to a change without scraping anything.

## Goals

- One flag on the commands that build, run and test, writing one report per invocation.
- A self-contained HTML report for people and a versioned JSON document for tools, carrying the same facts.
- One clock across every process and thread, with phases, waiting and untraced time told apart.
- The critical path across processes, established from recorded dependency, scheduling and wait relationships, never from timestamps alone, with incomplete attribution reported as such.
- Traced activity told apart from CPU time.
- Rebuild reasons: for each unit that ran instead of being reused, the input that changed.
- Ranked hypotheses from a named, versioned rule set, each with measured evidence and a place to look.
- Comparison of two reports, phase by phase and span by span.
- Budgets per command and per phase on a named measure, elapsed time or accumulated work, checked locally or in CI.
- An OpenMetrics export of the latest report's measurements, with a defined metric set and labels.

## Non-Goals

- A general-purpose profiler for the program being run. A run's program execution is one span. Sampled stacks inside the compiler's own processes are in scope as an optional source; profiling a user's program is not.
- A hosted service. Reports are files, and metrics are an export a user's own collector reads.
- Changing build behavior. Timing never alters what a build produces.

## Guide-level explanation

### See where the time went

```text
oven build --timings
```

When the build ends, successfully or not, Oven prints where the report is:

```text
time report: target/incan/timings/2026-11-11T09-14-37Z-build.html (and .json)
```

The HTML report shows, in order:

1. **Summary:** wall time; time on the critical path; processes and lanes; peak and average traced-busy lanes; CPU time per process; the shares of the wall time spent waiting and untraced.
2. **Hypotheses:** for example "lowering is the only work running for 50% of the wall time", "only one lane was traced-busy for 38 s", or "`lower_program` ran three times for one edit". Each one states its evidence and where to look first, and can jump to its stretch of the timeline.
3. **Critical path:** the spans that decided the wall time, in order, with their share, following the recorded relationships between them. Where a span's wait or dependency wasn't recorded, the report shows the path only up to that point and names the missing link.
4. **Timeline:** every process and thread on one clock, colored by phase, with a strip of busy lanes beneath it. Drag to zoom; hover or tap a span for its name, duration, self time, input and rebuild reason.
5. **Where self time went:** totals per phase and per span name, sortable and filterable.
6. **Why it rebuilt:** each unit that ran instead of being reused, with the changed input that caused it.
7. **Repeated work:** the same operation on the same input more than once.

### Did my change help?

```text
oven timings compare before.json after.json
```

writes a comparison report. It shows the wall time and the critical path before and after, the change per phase and per span name with the biggest movers first, spans that appear only on one side, and changes in rebuild reasons. The command also prints a short summary:

```text
wall 68.2 s -> 41.0 s (-40%); lowering 34.3 s -> 9.1 s; checking 17.7 s -> 16.9 s
```

### Keep the edit loop fast

Budgets live in the project manifest:

```toml
[timings.budgets]
"build:edit" = "500ms"
"build:edit:checking" = "100ms"
"build:cold:checking" = { elapsed = "20s", work = "60s" }
```

A plain duration budgets **elapsed** time: how long the phase took on the clock. A table can also budget **work**: the phase's time summed across every lane that ran it. Four checking workers running side by side for 100 ms take 100 ms elapsed and 400 ms of work, so they meet a 150 ms elapsed budget and miss a 300 ms work budget.

`oven build --timings` marks every budget the report exceeds. `oven timings check report.json` exits non-zero when a budget is exceeded, so CI can gate on it.

### Follow build times over time

```text
oven timings export --openmetrics target/incan/timings/
```

prints OpenMetrics text with one current value per metric: the latest report for each project, command and invocation kind in the directory. A collector scraping it sees the most recent build; earlier builds stay in their report files, where `oven timings compare` reads them.

### Read traces recorded earlier

```text
oven timings path/to/traces
```

builds a report from trace files recorded by any supported Oven or compiler version, for example traces attached to a bug report.

## Reference-level explanation

### Invocation

- `oven build`, `oven run` and `oven test` must accept `--timings[=FORMAT|PATH]`. `FORMAT` is `html` (the default, writing both files), `json` (the JSON document only) or `json:summary` (the JSON document without the span list). Any other value names the output path without an extension, and Oven appends `.html` and `.json`.
- `--timings` must enable the trace channels a report needs for that invocation only. It must not change build output: two builds of the same inputs, with and without `--timings`, must produce byte-identical artifacts.
- Oven must write the report when the invocation ends, whether it succeeded, failed or was interrupted. An interrupted report must say so in its summary.
- `oven timings DIR` must read every trace file under `DIR` that a supported version wrote, and must refuse with a diagnostic that names a file it cannot read and why.
- `oven timings compare A B` must accept two reports, or two trace directories, and write a comparison report next to `B`.
- `oven timings check REPORT` must exit zero when every budget holds and non-zero otherwise, printing each exceeded budget with its measured value.
- `oven timings export --openmetrics PATH` must print OpenMetrics text for the report or reports at `PATH`, as specified under Formats.
- Without an explicit path, reports go under the project's build directory, named by the invocation's start time in UTC and its command. Oven keeps the ten most recent reports per command and removes older ones under RFC 145. A report written to an explicit path is never removed.

### Spans, phases and clocks

- The report must place every span of the invocation on one clock. Spans from processes that record different clocks must be aligned through a recorded reference point, and the report must name the processes it aligned.
- When one invocation contains several runs, such as a resident engine serving several requests, the report must present them as separate runs, the invocation's own first.
- Each span must carry its name, phase, process, thread, start, end, self time, and its input when one was recorded.
- Self time is a span's duration minus the time its children in the same thread cover.
- Phases are a closed set: checking, Body IR, lowering, native (back end, codegen and link), reads and I/O, engine and scheduling, waiting, program, and other. A span that only waits on another process is in the waiting phase. Time that no span covers is untraced, never idle.
- Each span must record its relationships at the points where they happen:
    - its parent;
    - a **fork** link where it started another span or sent a request;
    - for each wait, a **wait interval** with its start and its end, and a link to what it waited for. That is a span it **depended on** (it consumed that span's output), a request it sent, or a span it was **scheduled after** (a scheduler held it until that span finished).
- A wait ends, and the span **resumes**, when the awaited span ends or the awaited request answers. A link names the other span, in any process, and its kind.
- A span's **execution intervals** are the times it ran its own work: not inside one of its wait intervals, and not covered by a child span on its own thread.
- A lane is **traced-busy** while a span that is not waiting is open on it. The traced-busy count is the number of traced-busy lanes at each moment. Traced-busy time is not CPU time: an open span doesn't prove its thread was running.
- Where the platform reports it, the report must record each process's CPU time, user and system, and per-thread CPU time where available, and show the traced-busy time that CPU time doesn't account for. Where it isn't available, the report must say so.

### Critical path

- The critical path is a sequence of non-overlapping intervals that covers the invocation's wall time exactly once. Each interval is either one span's **execution** or **incomplete**: time no recorded relationship explains.
- It is built backward from the invocation's end, starting in the span that ended the invocation.
    - That span's execution intervals are on the path.
    - At one of its wait intervals, the time goes to what it waited for. The walk continues inside the awaited span, from the wait's end back to the wait's start, and attributes that span's own execution and waits the same way, recursively.
    - When the walk reaches the wait's start, it resumes in the waiting span, before the wait.
    - A fork link connects a span's start to the span that started it.
- Only recorded relationships connect spans. A span that merely started after another ended is not on its path.
- Time inside a wait interval that the awaited span's execution doesn't cover is **incomplete**: for example, the awaited span started late, or the wait has no link. The report must name the span and the unexplained time, and must not fill that time from timestamps. A path with any incomplete interval is reported as incomplete.
- For example, take a nine-second invocation:

    ```text
    coordinator: work 0–1 s, wait for A, work 4–5 s, wait for B, work 8–9 s
    worker A:    1–4 s
    worker B:    5–8 s
    ```

    The critical path is the coordinator 0–1 s, A 1–4 s, the coordinator 4–5 s, B 5–8 s and the coordinator 8–9 s: exactly nine seconds, each second counted once, with both workers and the coordinator's own work.
- The report must give each span's share of the critical path, and the incomplete share, so that shortening a span off the path is distinguishable from shortening one on it.

### Rebuild reasons

- For each unit that ran instead of being reused, the report must name the reason Oven recorded: the changed input and its kind (source, dependency interface, toolchain, configuration or a missing record). A unit with no recorded reason must be reported as unexplained.
- The report must count, per phase, the work items redone, so that an edit that redoes more than it changed is visible.

### Hypotheses

- Hypotheses come from a named, versioned rule set. The report must name the rule set and its version. The first rule set, `incan.time-rules/1`, contains these rules, with these thresholds as fractions of the wall time:
    - **Solo phase:** a non-waiting span name whose self time with no other lane busy is at least 0.2.
    - **Serial stretch:** stretches with one traced-busy lane, each at least 0.5 s and 0.05, totalling at least 0.1.
    - **Untraced time:** stretches with nothing traced, totalling at least 0.05.
    - **Repeated work:** one span name on one input at least three times, totalling at least 0.02, or at least twenty times totalling at least 0.005.
    - **Many small calls:** a span name with at least 500 calls totalling at least 0.05.
    - **Redo beyond the change:** a phase that redid more than ten items in an invocation whose recorded change touched one definition.
    - **Unexplained rebuild:** any unit rebuilt without a recorded reason.
    - **Native step share:** one native step at least 0.4 of all native time.
- Each hypothesis must carry a severity (`high`, `medium` or `low`), a title, its evidence as measured numbers, a place to look, and its stretch of time when it concerns one. Hypotheses must be ordered by severity, then by the share of wall time they concern.

### Comparison

- A comparison must report, for wall time, critical-path time, each phase's elapsed and work time, and each span name: the value before, the value after, and the change, ordered by the size of the change. It must say when either side's critical path is incomplete.
- It must list span names and rebuild reasons present on one side only.
- Comparing reports whose rule sets differ must state that the hypotheses are not comparable.

### Budgets

- A budget names a command, optionally an invocation kind (`cold`, `warm`, `edit`) and optionally a phase, and one or both measures. Oven classifies an invocation as `cold` when nothing was reused, `warm` when nothing was rebuilt, and `edit` otherwise.
- **Elapsed** is the clock time the measured scope covers: for the whole invocation, its wall time; for a phase, the length of the union of the intervals in which any span of that phase was open on any lane. A plain duration budgets elapsed time.
- **Work** is the sum, across every lane, of the self time of the scope's non-waiting spans. A budget table `{ elapsed = ..., work = ... }` may set either measure or both.
- For the whole invocation, elapsed is wall time and includes untraced gaps. For a phase, elapsed is traced time, the union of that phase's spans. Work is always traced time. None of them is CPU time; the report shows CPU time beside them where the platform provides it.
- A report must mark every budget it checked as held or exceeded, naming the measure and giving the measured value.

### Formats

- The HTML report must be self-contained: no network access and no external scripts, styles or fonts. It must open from the local file system in a current browser.
- The JSON document must use the versioned schema `incan.time-report/1`. It must carry the summary, runs, spans with their relationships (unless written as `json:summary`), the critical path with its completeness, per-phase elapsed and work totals, per-name totals, traced-busy steps, CPU time where recorded, rebuild reasons, redo counts, repeated work and hypotheses. A version may add fields; removing or changing one requires a new version.
- The OpenMetrics export must hold one value per metric and label set: the values of the **latest** report for each project, command and invocation kind found at `PATH`. Values are gauges without timestamps, and earlier reports are not exported. The metric families are:
    - `oven_report_start_timestamp_seconds`: when the reported invocation started, so a collector can tell a stale value;
    - `oven_wall_seconds` and `oven_critical_path_seconds`;
    - `oven_critical_path_complete`: 1 when the critical path is complete, else 0;
    - `oven_phase_elapsed_seconds` and `oven_phase_work_seconds`, labeled by `phase`;
    - `oven_cpu_seconds`: CPU time summed over every process of the invocation, labeled by `mode` (`user` or `system`), when CPU time was recorded. The per-process breakdown stays in the report;
    - `oven_units`, labeled by `state` (`built` or `reused`);
    - `oven_hypotheses`, labeled by `severity`.
- Every metric must carry the labels `project`, `command`, `kind` and `toolchain`. The only other labels are `phase`, `mode`, `state` and `severity`, as listed above.
- Optionally, Oven may attach sampled stacks of its own and the compiler's processes to the slowest spans when the platform permits sampling. A report must say whether samples were attached.

### Paths and sharing

- A report must show paths relative to the project root, the Incan home or the toolchain where one applies, and absolute paths only for files outside all three. The JSON document must record which prefixes it shortened.
- A report must never contain environment variables, credentials or file contents. Inputs appear as the identifiers the traces already carry: paths, unit names and keys.

## Design details

### Interaction with existing features

- **Traces.** Reports read the trace channels Oven and the compiler already write. A channel `--timings` enables for one invocation stays off for every other.
- **The bakery (RFC 124).** A unit read back from the bakery appears as a short reuse span, and its reuse counts toward the `warm` classification. A report therefore shows what was reused as well as what ran.
- **Storage (RFC 145).** Reports in the build directory are retained build output with a bounded count per command.
- **Debugging (RFC 167).** A time report and a debugging session can name the same spans; neither depends on the other.

### Compatibility

All flags and commands are new; existing invocations don't change. Trace files from older versions stay readable while Oven supports their schema; `oven timings` refuses older ones with a diagnostic that names the schema.

## Alternatives considered

- **Document the trace formats and leave viewing to users.** Oven stays smaller, but every user then joins several files on several clocks by hand, which is the part that matters.
- **Emit only the Chrome trace format.** General viewers open it, but they don't know which spans only wait, which phases exist, what counts as repeated work or why a unit rebuilt, so they can't find the critical path or rank hypotheses. A Chrome trace export may be added beside the report; it doesn't replace it.
- **Compute the analysis in the HTML page.** Oven would stay simpler, but tools reading the JSON document would have to repeat the analysis. Computing the facts in Oven gives the report, the comparison, budget checks and the metrics export one source.
- **A live metrics endpoint on the resident engine.** It would follow builds as they happen, but it ties metrics to a long-lived process and to network configuration. An export from reports serves the same dashboards from files that already exist.
- **Infer the critical path from timestamps.** It needs no extra recording, but a span that started after another ended need not have depended on it, so the path would name work that didn't decide the wall time. Recorded relationships are what make the path trustworthy, and missing ones are reported rather than guessed.
- **Export every report as timestamped samples.** OpenMetrics allows several samples with timestamps, but collectors treat them inconsistently, and history already lives in the report files. Latest-value gauges are what scrapers handle well.

## Drawbacks

- Reports of large builds can be large; the span list dominates, and `json:summary` exists for that case.
- Rules can mislead where a phase is not instrumented. Untraced time is reported as untraced, not idle, and the solo-phase rule ignores waiting spans for this reason.
- Shortened paths still reveal a project's structure; people who share a report should know that.
- Budgets measured on different machines differ; a budget is only meaningful against the machine class it was set for.
- The critical path is only as complete as the recorded relationships. Until every wait in Oven and the compiler records its link, some reports will show an incomplete path.

## Layers affected

- **Oven (CLI and engine):** the `--timings` flag on `build`, `run` and `test`; the `timings` command with `compare`, `check` and `export`; enabling trace channels per invocation; recording fork links and wait intervals, with their links, for its requests, units and scheduler; recording process CPU time; recording rebuild reasons; writing, comparing and retaining reports; budgets in the project manifest.
- **Compiler driver:** stable span names and phases for its own work, a recorded clock reference point, fork links and wait intervals for the work it starts and waits on, CPU time, and redo counts per phase.
- **Docs:** reference pages for the commands, the flag, the `[timings.budgets]` manifest table, the `incan.time-report/1` schema and the `incan.time-rules/1` rule set.

## Design Decisions

- **`oven test --timings`** writes one report per invocation. Each test target is a run in that report, so a slow target stands out on the shared timeline.
- **A run's program execution** appears as one span in the program phase by default. The build phases are reported apart from it, so a slow program and a slow build remain distinguishable.
- **The first hypothesis rule set** is `incan.time-rules/1`, with the rules and thresholds under Hypotheses.
- **The critical path comes from recorded relationships**, never from timestamp order. Missing links make the path incomplete, and the report says where.
- **Budgets name their measure.** A plain duration budgets elapsed time; a table can budget work as well. CPU time is reported beside them, never in place of them.
- **The critical path attributes intervals, not whole spans.** Execution and wait intervals, linked at fork, wait and resume points, cover the wall time exactly once.
- **The metrics export holds the latest report** per project, command and invocation kind, as gauges without timestamps. History stays in report files.
- **Retention:** the ten most recent reports per command stay in the build directory; reports written to explicit paths are never removed.
