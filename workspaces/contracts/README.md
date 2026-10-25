# Contracts

The contract families say what `incan` must do, each case as a program that carries its own expectations. A table runner, written in Incan, runs every case against an `incan` and reports each one. Nothing in a case names a route, a backend or a build step: a case states what a user observes.

## Budget

The whole test suite stays under about 3,000 tests (#2255).
- **New cases:** add one only when no existing case in the same area can carry the behavior. Otherwise, extend that case's program and expectations.
- **Bugs:** a guarded bug is named in the case's `# behavior:` line. It never gets a case of its own when an area case can hold it.
- **Library cases:** one producer Loaf per case, carrying every declaration the case needs.
- **Folding:** passing cases in one area can be folded together without losing an expectation; failing and pending cases stay separate until they pass.

## Families

| Family | Runs | Compares |
| --- | --- | --- |
| `behavior` | `incan run src/main.incn`, or `incan check src/main.incn --format json` when the header declares diagnostics | stdout, stderr, exit code; or the refusal's diagnostic codes |
| `check` | `incan check src/main.incn --format json`, repeated unchanged and after each `check-edits:` entry | acceptance (`expect-accepted:`, warnings allowed) or the refusal's codes; an unchanged repeat's exact replay; a fresh check after each edit |
| `library` | as `behavior`, for a project with path dependencies; Oven bakes them | as `behavior` |
| `interop` | as `behavior`, for programs that import through `rust::` | as `behavior` |
| `testing` | `incan test tests/<name>.incn` | the per-test outcome lines, stderr, exit code |
| `project` | `incan describe-unit src/main.incn`, or each `cli-commands:` entry | the unit description's dependencies, standard packages and active features, or each CLI invocation's stdout, stderr and exit code |
| `format` | `incan fmt [--check] src/main.incn` | exact formatted text, check exit codes, check immutability and repeated-format stability |
| `graph` | `incan inspect codegraph` for each `graph-commands:` entry | query output, repeated-query equality, JSON/JSONL agreement, edit-stable declaration identities and invalidation after source changes |

## Layout

```text
workspaces/contracts/
  src/                       the table runner
  fixtures/<family>/<area>/
    <name>.incn              single-file case: the whole program, header first
    <name>/main.incn         module case: main.incn carries the header, sibling .incn files are importable modules
    <name>/loaf.toml         project case: a complete project; the header is in src/main.incn
```

Every entry in an area is one of those shapes, and an empty area is refused. A family directory may hold a `README.md`; nothing else but areas. Hidden entries are skipped. Names are lowercase letters, digits and `_`, starting with a letter or `_`. A testing case is one file named `test_<subject>.incn`; it runs from the project's `tests/` directory. A library case is a project.

Each case gets its own working directory in scratch: a single-file or module case gets a minimal `loaf.toml` (`[project.scripts] main = "src/main.incn"`), and a project case is copied as it is. Compatible programs in `behavior` and `interop` share compilation in a generated project, with each program in its own module. The unit explicitly selects those modules' checked main identities. Canonical entry lowering handles synchronous, async, integer and Result entries, and rustc's standard `Termination` implementation reports the selected result. Each selected case still runs in a fresh process in its own working directory. The dispatcher emits nothing, and its selector does not appear in the program's arguments. A refused compilation, or one that emits output needing individual attribution, is split recursively; singleton cases use their original compilation path.

Computed statics, decorators, entry setup effects, executable/source-path observations, diagnostic and acceptance checks, and project, library, testing, format and graph cases retain their original commands. `--isolated` disables shared compilation. A scratch file named `batch-<family>-refusals.tsv` lists each case kept isolated and its reason; `BATCH-SPLIT` lines identify refused combinations without assigning a failure to any individual case. Projects and shared binaries stay under scratch for Oven reuse; only changed inputs are rewritten, and every case executes afresh. Child invocations use one worker budget, defaulting to twelve; `--jobs N` sets it explicitly. Each child has a separate process group. `--timeout SECONDS` sets a deadline; zero, the default, disables it. A timeout kills the group, retains partial output and preserves a signal as distinct from an exit code. The runner retains Oven's per-unit compiled/reused reasons and counts shared builds once.

`batch-<family>-groups.tsv` retains compilation boundaries after splitting. It contains case names, never test outcomes or compiler acceptance. Discovery first tries balanced combined compilations across the available workers. Reported source paths advise which programs to isolate while the remainder is compiled again; refusals without usable paths fall back to bisection. Successful groups execute without a second repacking build. A repeat tries the retained groups with their current sources; Oven validates their current compilation inputs once, and every selected case executes again through Oven's ordinary process-status policy. Changed or newly selected cases are admitted again, and a group that stops compiling is split again. This avoids rediscovering unchanged incompatible combinations on every run.

Compilation attempts, individual commands and selected-entry executions use the same worker pool. Known isolated commands join the first shared-build wave, including during discovery; each executes once through its original command while shared builds occupy the remaining slots. A refinement that reaches a singleton queues its original command in the next wave. A changed group that refuses still splits and falls back normally. The pool owns native child handles in a queue, polls every active child, reaps completed children and immediately refills their slots. A quiet successful group build releases its selected entries into that queue while remaining commands continue compiling. A refused, timed-out or output-producing build releases no selected entry. Outcomes retain request order, and each child's deadline starts when that child launches. Refused groups split in bounded waves.

Speculative combined compilations have a five-second deadline, shortened by `--timeout` when it selects an earlier deadline. An expired attempt kills its process group and splits its cases for another attempt; it assigns no case verdict. Individual case commands retain the `--timeout` policy, with no deadline by default. This bounds expensive incompatible combinations without learning partitions by compiling every case separately.

Previous durations of original case commands and shared builds order longer invocations first within the worker pool. Bounded high-water marks preserve compilation costs across cheaper unchanged repeats. Full-family runs share these optional durations beside their corpus-scoped partition hints, so a fresh scratch directory can schedule long commands immediately. Selected-entry executions do not contribute timing hints. Hints never retain outcomes or authorize reuse; unavailable or malformed hints have zero weight. Selected entries still execute in fresh processes, and outcomes retain request order regardless of launch order.

When multiple families are selected with at least two jobs, the runner supervises up to three family processes. Each receives `floor(jobs / worker limit)` command slots, keeping their combined command budget at or below `--jobs`. Graph's sequential protocol probes start first while other families fill the remaining slots; reports retain family order. Each process enters one pool at a time, so callback state is never shared concurrently or recursively. A family process has no family-wide deadline: it applies `--timeout` separately to each original command. A missing completion marker, abnormal exit or inconsistent report refuses that family. `--serial-families` keeps the sequential family schedule for comparison; single-family selections keep their existing worker budget and commands. Independently launched runner processes each have their own budget.

Full-family runs retain case-name partition hints under an explicit absolute `INCAN_HOME/cache/contract-groups-v2`, scoped to the canonical fixture root. A new scratch directory can use those partitions immediately. Oven validates each group's current compilation inputs, and every selected case executes. Partial-family selections retain only their local scratch plan. Missing or unusable hints trigger partition discovery; optional hint-storage errors preserve ordinary planning. `--no-group-hints` disables shared hints; combine it with fresh scratch to measure discovery. First discovery and fresh compilation with an existing hint are separate timing conditions.

Oven retains compiler-certified deterministic refusals under the same complete input identity as successful builds. An unchanged refusal replays the exact stdout, stderr and normal exit status; editing a source or changing the compiler, features or linked artifacts requires another compilation. Crashes, signals, uncertified failures and missing linked artifacts always retry. The runner reports replayed refusals separately from compiled and reused units; every successful case still executes afresh. A check is retained the same way: an unchanged `incan check` replays the report and status of its last compiler-certified check, for each report format.

## Header

The header is the unbroken run of `#` comment lines at the top of the program.

| Directive | Meaning |
| --- | --- |
| `# behavior: <one line>` | What the case proves. Required, once. |
| `# retires: <path>.rs::<fn>` | The retired test the case replaces. Repeatable. |
| `# expect-stdout:` | The whole stdout, one indented line per line. Exact; an empty block means nothing is printed. |
| `# expect-stdout-contains:` | Lines that must each appear as a whole stdout line. Not with `expect-stdout`. |
| `# expect-stderr-contains:` | Substrings that must each appear in stderr. |
| `# expect-exit: <n>` | The exit code, `0` to `255`. Default `0`. |
| `# expect-diagnostic: <CODE>` | The program is refused at check time with this code. Repeatable. Never with a run expectation. |
| `# expect-accepted:` | The program checks without errors and is never run. |
| `# expect-dependencies:` | Exactly these dependencies, one `<key> <package>` per line; an empty block means none. |
| `# expect-std-packages:` | Standard packages the unit must reach. |
| `# expect-active-features:` | Exactly these active features; an empty block means none. |
| `# expect-formatted:` | Exact formatted source, indented as a block. Required for a `format` file or project case. The header and its separating blank line are removed from the entry source before formatting; the expected text has one final newline. |
| `# pending: <reason>` | The case does not hold yet, and why. A pending case that holds fails: its line is stale. |
| `# graph-commands:` | Inspection arguments, one invocation per indented line, without `incan inspect codegraph`. Only the `graph` family admits this directive. Paths are relative to the fixture project. |
| `# cli-commands:` | Complete CLI arguments, one whitespace-separated invocation per indented line, without `incan`. Only the `project` family admits this directive. Every invocation independently meets the same stream and exit expectations; unit-description expectations are refused. |
| `# check-edits:` | Files of the case project, one relative path per indented line. Only the `check` family admits this directive. The check runs again unchanged: it must show the same stdout, stderr and exit code, and Oven must trace it as replayed. Then a comment line is appended to each file in turn and the check runs again: it must show the same output, and Oven must trace that it checked the unit again. Each file's original text is restored. |

A directive is `#`, one space, the key, a colon. Block lines follow their directive, each `#` and at least two spaces; the first line sets the indentation. A bare `#` inside a block is an empty line. A blank line or any other non-comment line ends the header, and a directive after it is refused. Every case declares at least one expectation its family compares; a family refuses the expectations it does not compare.

Output is compared as lines: `\n` ends a line, a trailing `\r` is dropped, a final newline adds no empty line. An expected line that ends with whitespace is refused, because a report could not show the difference.

## Running

For `format`, the initial `--check` must exit 1 when the input differs from the expected text, otherwise 0; `expect-exit:` can state that initial code explicitly. Check mode must leave the file unchanged. Formatting must exit 0 and produce the exact expected text. Checking that output must exit 0, and formatting it again must succeed without changing it.

```bash
incan run workspaces/contracts/src/main.incn -- INCAN workspaces/contracts/fixtures [--scratch DIR] [--isolated] [--serial-families] [--jobs N] [--timeout SECONDS] [SELECTOR...]
```

A selector is `<family>`, `<family>/<area>` or `<family>/<area>/<name>`. The runner prints `PASS`, `FAIL` with each broken expectation, or `PEND` with its reason for every case, a summary per area and a total, and exits 1 when a case fails or a family cannot be read.

For the installed dev.8 workflow, pinned release bundles, and the committed PASS-key regression gate, see [Verify an installed dev.8 toolchain](dev8-ci.md).

Set `CONTRACT_TIMINGS=1` to print elapsed milliseconds for each child invocation, each case comparison and each family. Family timings include discovery, execution and comparison, with the units Oven reported compiling or reusing. A batched case's comparison excludes its earlier execution; invocation timings name the case project or shared compilation project. These observations leave commands, expectations and verdicts unchanged.
