# RFC 063: `std.process` — process spawning and command execution

- **Status:** Draft
- **Created:** 2026-04-14
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 027 (`incan-vocab` block registration and desugaring)
    - RFC 040 (scoped DSL glyph surfaces)
    - RFC 022 (namespaced stdlib modules and compiler handoff)
    - RFC 023 (compilable stdlib and Rust module binding)
    - RFC 055 (`std.fs` path-centric filesystem APIs)
    - RFC 056 (`std.io` in-memory byte streams and binary parsing helpers)
    - RFC 058 (`std.datetime` durations and time handling)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/341
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

This RFC proposes `std.process` as Incan's standard library module for explicit process spawning, command composition, pipelines, and process I/O control. The contract is argument-vector-first by default, shell execution is explicit opt-in, and timeout/termination behavior is first-class. The module also exposes a shell-oriented `sh { ... }` vocab DSL lane for DevOps-style authoring without changing the default safety model.

## Motivation

Process execution is foundational for automation and data workflows: invoking tools, streaming output, composing pipelines, and enforcing exit-code contracts. Without a standard process module, users depend on shell string helpers or backend interop that weakens safety and consistency.

The standard contract should match the runtime reality: explicit child-process control with predictable behavior across normal CLI orchestration cases.

## Goals

- Provide a first-class process API in `std.process`.
- Make argument-vector invocation the default model.
- Keep shell execution explicit and opt-in.
- Provide an explicit shell DSL lane (`sh { ... }`) for shell-native workflows.
- Support capture, streaming, pipelines, exit-status handling, and timeout/termination controls.
- Keep environment, working directory, and stdio behavior explicit.

## Non-Goals

- Replacing workflow orchestrators or job schedulers.
- Making PTY/terminal emulation part of this RFC.
- Standardizing remote execution APIs.
- Hiding command semantics behind implicit shell parsing.
- Making shell DSL syntax globally active without explicit vocab activation.

## Guide-level explanation

```incan
from std.process import Command

result = Command("git")
    .arg("status")
    .capture_output()
    .run_checked()?

println(result.status.code)
println(result.stdout_text()?)
```

```incan
from std.process import Command
from std.fs import Path

input = Path("data.ndjson").open("rb")?
output = Path("data.sorted.ndjson").open("wb")?

Command("sort")
    .stdin(input)
    .stdout(output)
    .run_checked()?
```

```incan
from std.process import Pipeline, Command

result = Pipeline([
    Command("cat").arg("data.txt"),
    Command("grep").arg("ERROR"),
    Command("sort"),
]).capture_output().run_checked()?
```

```incan
from std.process import Command
from std.datetime import Duration

Command("long_task")
    .run_checked(timeout=Duration.minutes(5))?
```

```incan
import std.process

sh {
  kubectl get pods -n prod | grep CrashLoopBackOff
}.run_checked(timeout=Duration.minutes(2))?
```

## Reference-level explanation

### Module scope

`std.process` should provide:

- `Command` for process construction and execution;
- `Pipeline` for first-class pipe composition;
- `sh { ... }` as an import-activated vocab DSL surface for explicit shell-mode execution;
- `ProcessResult` for completed outcomes;
- `ChildProcess` for spawned long-running process control;
- process error types for spawn, exit, timeout, and I/O failures.

### Core model

- command invocation is argument-vector-first;
- arguments are passed literally to the program;
- shell interpretation is explicit opt-in;
- shell DSL exists as a separate explicit lane and compiles to shell-mode process execution;
- pipelines are explicit and typed, not shell-string tricks.

### Core API direction

`Command` should expose:

- program + args (`arg`, `args`);
- cwd (`cwd`);
- env controls (`env`, `env_remove`, `env_clear`);
- stdio controls (`stdin`, `stdout`, `stderr`, `inherit_*`, `capture_output`, `pipe_*`);
- execution (`run`, `run_checked`, `spawn`);
- timeout-aware execution (`run(..., timeout=Duration)` and `run_checked(..., timeout=Duration)`).

`Pipeline` should expose:

- ordered command stages;
- pipeline stdio controls at pipeline boundaries;
- `run` / `run_checked` with consistent semantics.

`sh` vocab DSL should expose:

- shell-native command authoring in `sh { ... }` blocks;
- explicit execution controls (`run`, `run_checked`, timeout, capture/inherit behavior);
- explicit interpolation rules with safe escaping by default.
- a POSIX shell contract (`/bin/sh` baseline) in this RFC.

## Design details

### Shell boundary

Shell execution exists in core API but is explicitly marked and not the default path. Argument-vector execution remains the standard path for safety and predictability.

A shell mode helper should be explicit, for example:

- `Command.shell("...")`

The RFC should document shell-mode quoting and injection risks explicitly.

This RFC defines shell behavior as Unix/Linux-focused. The `sh` lane targets a POSIX shell baseline (`/bin/sh` semantics). Cross-platform Windows shell parity is explicitly out of scope for this RFC and may be handled in a follow-on proposal.

### Vocab DSL integration

`sh { ... }` must be modeled as a vocab DSL surface, not as an ad hoc parser exception. Activation should follow RFC 027 style import-activated vocabulary behavior.

That means:

- shell DSL syntax is only active when `std.process` vocab is activated in the current file/module;
- shell DSL remains an explicit lane and does not redefine ordinary process APIs;
- DSL glyph use (for example `|`) is scoped inside the `sh` block surface, consistent with RFC 040-style scoped DSL semantics.

### Interpolation policy for shell DSL

Shell DSL should define interpolation semantics explicitly:

- safe interpolation is the default (shell-escaped);
- raw interpolation is explicit opt-in;
- docs must call out injection risk when raw interpolation is used.

### `run_checked()` contract

`run_checked()` guarantees:

- successful spawn;
- successful completion within timeout if provided;
- zero exit status.

On failure it returns a typed error carrying:

- exit status (for non-zero exits);
- captured stdout/stderr when available;
- timeout metadata when timeout-triggered.

### Pipelines as first-class

Pipelines are in scope for this RFC as first-class API (`Pipeline`), not only as shell convenience. This keeps cross-platform semantics explicit and avoids fragile shell-dependent pipeline behavior for common data workloads.

### Timeout and termination

Timeouts are first-class via `Duration`.

`ChildProcess` should support explicit lifecycle control:

- `wait()`
- `try_wait()`
- `terminate()`
- `kill()`

Contract should state:

- `terminate()` is graceful where supported;
- `kill()` is forceful;
- platform differences are documented but API intent is stable.

### Stdio and composition with stdlib

`std.fs.File` and `std.io.BytesIO` compose directly with process stdio binding. This avoids requiring additional abstractions just to wire common file and in-memory process flows.

### Error model

Process errors should be distinct at the type level:

- spawn failure;
- non-zero exit failure;
- timeout failure;
- stdio I/O failure;
- pipeline stage failure with stage index/identity metadata.

## Alternatives considered

1. **Single shell-string helper only**
   - Too implicit and too fragile by default.

2. **Rust interop only**
   - Too low-level for common Incan automation and pipeline tasks.

3. **No first-class pipelines**
   - Too weak for data and CLI composition workloads.

## Drawbacks

- Cross-platform command and termination behavior requires careful documentation.
- First-class pipelines add extra API surface compared to single-command-only execution.
- Timeout + streaming + capture interactions require precise contract wording.

## Layers affected

- **Stdlib / runtime**: command spawn, stdio wiring, pipeline execution, and timeout or termination semantics.
- **Language surface**: the module types and methods must be available as specified.
- **Execution handoff**: implementations must preserve process behavior without backend leakage.
- **Docs / examples**: explicit shell boundary, checked execution, and pipeline usage.

## Design Decisions

- `std.process` is argument-vector-first by default.
- Shell execution is available but explicit opt-in.
- `sh { ... }` is in scope as an explicit shell DSL lane.
- `sh { ... }` is import-activated vocab DSL syntax, not globally active parser syntax.
- `sh { ... }` is POSIX-focused (`/bin/sh` baseline) in this RFC; Windows shell parity is out of scope.
- `run_checked()` enforces zero exit status and returns typed failures otherwise.
- First-class `Pipeline` support is in scope in this RFC.
- Timeout-aware execution is in scope and uses `Duration`.
- `ChildProcess` lifecycle controls include `wait`, `try_wait`, `terminate`, and `kill`.
- `std.fs.File` and `std.io.BytesIO` are directly supported for stdio binding.
