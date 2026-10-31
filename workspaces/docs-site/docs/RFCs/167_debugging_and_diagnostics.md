# RFC 167: Debugging and diagnostics for Incan programs and tools

- **Status:** Draft
- **Created:** 2026-10-31
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 005 (Rust interop and its unsafe-call boundary)
    - RFC 018 and RFC 019 (assertions and testing)
    - RFC 093 and RFC 095 (application telemetry and span vocabulary)
    - RFC 106 and RFC 111 (semantic inspection and contract feedback)
    - RFC 116 (checked C bindings)
    - RFC 118 (Incan and Oven command-line surfaces)
    - RFC 120 and RFC 124 (source-symbol and compiled-unit identity)
    - RFC 126 (Oven debugging and evidence-backed investigations)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

An Incan developer should be able to understand a compiler refusal, stop at an authored statement, inspect a value, identify an allocation, profile an operation, and diagnose a failing test through one supported suite. This RFC defines the observable contracts: readable symbols, source-matched native debugging, consistent diagnostic records, correlated program and toolchain observations, memory and leak inspection, profiling, and checked C-boundary evidence. It applies to ordinary safe programs as well as foreign calls and the toolchain itself. It supplies observations to RFC 126's durable investigations without requiring an investigation to inspect one failure.

## Core model

1. **A diagnostic explains an outcome.** A compiler refusal, an assertion failure, a native crash and an observer failure are different outcomes with different producers.
2. **An observation describes identified work.** It names the execution or build request, the observed interval, the exact artifact when known, and its coverage. Missing evidence is never a successful observation of nothing.
3. **Identity and presentation are distinct.** A readable function name is a presentation of a symbol, not its identity. Source coordinates belong to a particular source revision and executable.
4. **One fact has one authority.** Terminal, JSON, editor, debugger, memory and profile views consume the same underlying diagnostic, identity or observation. Combining them does not merge their meanings.
5. **Capabilities are explicit.** Support depends on target, toolchain, artifact, optimization, observer and permissions. A partial report describes its limits before making a conclusion.

## Motivation

A program that reports only an assertion message leaves its caller, source line and surrounding state unknown. A native allocation stack can name a module through an encoded identifier while concealing the authored operation. A debugger may recognize an executable without having the matching line tables or names of its locals. These are obstacles in ordinary programming, independently of whether any source contains `unsafe:`.

The toolchain has the same needs. A slow command may spend time preparing a build, waiting for shared work, compiling a dependency, launching an executable or running the program. A large resident set may contain shared mappings, live allocations, allocator reserves or unreachable allocations. Treating these as one duration or one memory number does not explain them.

Existing compiler diagnostics, native debugging, engine observations and application telemetry supply parts of the answer. The missing contract is their coherent presentation, with precise limits. An understandable failure should not require manually combining host-specific utilities or reconstructing source identity from a native symbol.

## Goals

- Display authored names in supported stack, debugger, allocation, trace and profile views while preserving exact native identity.
- Support breakpoints, stepping, call stacks and bounded value inspection against matching Incan source.
- Preserve stable compiler codes and extend consistent diagnostics to runtime, toolchain, foreign-boundary and test outcomes.
- Offer supported commands for program and toolchain tracing, memory and leak inspection, and CPU profiling.
- Correlate observations across participating processes and language boundaries without contaminating program streams.
- Explain checked C-binding refusals and observable call boundaries without weakening their safety rules.
- Make test failures actionable through source locations, captured assertion evidence, streams and execution outcomes.
- Publish capability, completeness, overhead and artifact requirements that can be tested.

## Non-Goals

- A new language-level exception model, changed panic/unwind behavior, or permission to call unsafe Rust APIs.
- A replacement native debugger, allocator, operating-system profiler, application telemetry model, test runner or build engine.
- Guaranteed recovery after memory corruption, complete detection of undefined behavior, or a proof of memory safety from a clean leak report.
- Identical debugger and memory capabilities on every operating system, target or optimization level.
- Mandatory instruction recording, deterministic replay, time-travel debugging, automatic repair or autonomous hypothesis management; RFC 126 owns investigation workflows.
- Implicit evaluation of application code to display values, automatic attachment to unrelated processes, or automatic upload of captures.
- A stable native ABI or unchanged linker spelling across compiler versions merely because names are readable.
- A second trace store, code graph or source-identity registry, or total-size ceilings on the existing artifact store.

## Guide-level explanation

The following commands and transcripts describe the proposed surface. They are not claims of shipped commands. Counts and identifiers are illustrative; real reports obtain them from the selected execution. Existing `incan check --format json` keeps its established contract.

### Find the authored operation

Given this program:

```incan
def subtotal(value: int) -> int:
    adjusted = value + 1
    assert adjusted == 100
    return adjusted


def main() -> None:
    println(subtotal(12))
```

A failure view connects the assertion to its source and caller:

```text
Assertion failed: adjusted == 100
  main.incn:3  subtotal
  observed left: 13
  expected right: 100
  called from main.incn:8  main
Execution: run-A; source match: verified
```

These values are captured from the original evaluation. Reporting the failure does not call either operand again. If values were unavailable, the report would say so instead of reconstructing them from current source.

For a native stack obtained elsewhere, an offline filter makes supported Incan symbols readable:

```sh
incan demangle --format text < native-stack.txt
incan demangle --format json < native-symbols.txt
```

The readable view names modules, declarations and concrete instances when the symbol or matching metadata contains that information. If only a module can be recovered, the result identifies the module and labels the declaration unavailable. Foreign names and unrecognized text remain intact.

### Stop in Incan source

```sh
incan debug main.incn
```

An interactive client can then present:

```text
Source match: verified; source debugging: supported
Breakpoint: main.incn:3 → resolved
Stopped: subtotal at main.incn:3
Locals: value = 12; adjusted = 13
Next: main.incn:4
```

A breakpoint is not reported as resolved until it has an executable location. After the source changes, the client offers the matching retained source or a new build. It does not label a location in a newer file as the old executable's location. An optimized value may be shown as `optimized out`; a missing debug artifact is a different failure.

### Inspect memory through a supported command

```sh
incan diagnose run main.incn --capture memory,leaks --output memory.capture
incan diagnose inspect memory.capture --view memory --format text
```

A report separates the measurements:

```text
Observed: program process and identified children
Coverage: allocator allocations; foreign custom allocator unavailable
Current resident bytes: <measured>
Peak physical footprint: <measured or unavailable>
Live allocated bytes: <measured>
Unreachable allocations: <count and bytes at the named checkpoint>
Retained reachable allocations: <measured or unavailable>
Allocation stacks: readable names; unresolved frames reported separately
Conclusion: no leaks detected within the stated coverage
```

For a long-lived process, attach explicitly and compare named checkpoints:

```sh
incan diagnose attach 4242 --capture memory,leaks --output idle.capture
```

The tool verifies process identity before observing it. If allocation history required instrumentation at process launch, attaching later reports that history as unavailable and explains the required rerun. Growing RSS alone does not become a leak diagnosis.

### Separate program work from toolchain work

```sh
incan diagnose run main.incn --capture trace,cpu --scope program --output program.capture
incan diagnose command --capture trace,cpu,memory --scope toolchain --output build.capture -- incan build main.incn
incan diagnose inspect build.capture --view timeline --format json
```

The command capture names the selected build request and participating compiler or resident-engine work. A resident process shared with another request is not charged entirely to this invocation. A view distinguishes waiting, compilation, launch and program execution, with unknown attribution left visible.

Application-authored spans retain the opt-in telemetry semantics of RFC 093 and RFC 095. The suite can correlate them with toolchain and process observations when context is available; it does not turn ordinary program logs into inferred timing spans or configure a network exporter.

### Understand a checked C boundary

```incan
from std.interop import c


binding LibC:
    header = "stdlib.h"
    link = c.system_library("c")

    symbol absolute(value: c.i32) -> c.i32:
        native = "abs"


def absolute(value: i32) -> i32:
    unsafe:
        return LibC.absolute(value)


def main() -> None:
    println(absolute(-5))
```

```sh
incan diagnose run main.incn --capture ffi --output calls.capture
incan diagnose inspect calls.capture --view foreign-calls --format text
```

An observation can identify `LibC.absolute`, native symbol `abs`, the selected library, the Incan call site, entry and normal return. Arguments and return values appear only under an explicit value-capture policy. Removing `unsafe:` produces a compiler refusal linked to the binding and call site; it does not produce a runtime call event.

If execution ends after an entry event but before a return event, the report says `completion not observed`. It must not claim that the C function failed, returned, or caused a later crash solely from event adjacency.

### Keep the failed test's evidence

```incan
def test_total() -> None:
    actual = 12 + 1
    assert actual == 100


def test_next() -> None:
    assert true
```

```sh
incan diagnose test test_totals.incn --capture failures --output tests.capture
incan diagnose inspect tests.capture --view failures --format json
```

The first failure retains its test identity, source assertion, available original operand values, frames and separately identified stdout and stderr. The second test still runs according to the existing isolation contract. Rerunning the failure creates a new execution; reading this capture runs nothing.

## Reference-level explanation

### Commands and compatibility

The suite must expose the following operations through the Incan/Oven command ownership established by RFC 118. Equivalent front ends must call the same operation contracts.

| Proposed operation | Contract |
| --- | --- |
| `incan demangle [SYMBOL] --format text\|json` | Decode one symbol or stream stdin when the operand is absent. Text mode preserves non-symbol content; JSON mode reports original input, readable presentation, recognized format and decoding status. No program is executed. |
| `incan debug PATH [-- ARGS...]` | Select a program and exact debug-capable artifact, then start a source-debugging session. Report unsupported capabilities before promising a session. |
| `incan diagnose run PATH --capture KINDS --output CAPTURE [-- ARGS...]` | Execute a program under explicitly selected observations. |
| `incan diagnose test PATH --capture KINDS --output CAPTURE` | Use ordinary test discovery and execution, retaining selected failure or resource observations. |
| `incan diagnose command --capture KINDS --scope SCOPE --output CAPTURE -- COMMAND [ARGS...]` | Observe the explicitly launched command and identified work within scope. It must not require shell interpretation of operands. |
| `incan diagnose attach PID --capture KINDS --output CAPTURE` | Observe an explicitly selected existing process, subject to process identity, permission and observer capability. |
| `incan diagnose inspect CAPTURE --view VIEW --format text\|json` | Read a capture without running or attaching to its subject. |
| `incan diagnose capabilities --format text\|json` | Report supported kinds, targets, providers, launch/attach requirements and known permission constraints. |

`KINDS` is a comma-separated selection from `failures`, `trace`, `memory`, `leaks`, `cpu` and `ffi`; unknown kinds must be refused. `SCOPE` is `program`, `toolchain` or `all`. Run and test default to `program`; command requires an explicit scope; attach observes the selected process and explicitly included descendants. An observation must not silently broaden its scope. The views used above are `memory`, `timeline`, `foreign-calls` and `failures`; a view with no captured data must say `not captured`.

A missing required observer, unsupported kind or invalid output destination must fail preflight before execution. A capture destination must not silently overwrite an existing capture. All selected kinds are required unless a caller explicitly accepts partial capture after seeing the capability report.

For run, test and command, successful observation must preserve the subject's exit status and stream bytes. Usage errors before execution use status 2; observer setup failure uses status 1. If observation fails after execution starts, a nonzero subject status remains the command status; a successful subject with incomplete required observation yields status 1. A completed capture must record the subject outcome separately from observer status, including a signal when available. If storage failure prevents that completion record, readers must classify the capture as incomplete and the caller must receive the observer failure on a separate diagnostic channel. Interactive termination and attach detachment must distinguish a detached observer from a terminated subject. Ordinary unobserved commands keep their existing exit and stream contracts.

### Diagnostic facts and rendering

A diagnostic must have a versioned representation carrying its stable catalog code, severity, producer domain, outcome category, message, related diagnostics and any available source locations. Expected and actual values, hints and related declarations must remain structured when their producer supplies them. A diagnostic without a source location must represent absence rather than invent a line or attribute it to its caller.

Compiler codes must retain their meaning and must not be recycled. The existing check JSON schema must remain available; adding runtime observations must not silently change it. New envelope versions must identify compatibility explicitly. Consumers may ignore additive optional fields but must not reinterpret an unknown version or outcome as success. Every advertised explanation command must work for the code it names.

Terminal, JSON and editor diagnostics must be projections of producer-owned facts. Clients must not derive stable codes, severity, types or identities by parsing rendered prose. Equivalent compiler refusals reached by check, build, run or test must preserve those facts. Rendering differences must be deliberate, such as terminal color or editor coordinate encoding.

Reusable semantic diagnostics must bind their anchors to the selected source revision before display. A comment or line-layout edit must not cause a stale location to be presented as current. Failed anchor binding must be reported or trigger a valid fresh evaluation; it must not quietly reuse old coordinates. Historical execution frames remain bound to their original executable and source, even if current compiler diagnostics refer to a newer revision.

### Symbols and source identity

Supported native Incan symbol formats must have versioned decoding rules. The decoder must distinguish decoded, partially decoded, non-Incan, unsupported-version and malformed input. It must validate lengths, UTF-8 and nesting limits before allocating or interpreting payloads, and must not guess from arbitrary underscore-delimited numbers. Work per request must be bounded; an over-limit input must be reported without losing its original identity.

A display must retain the raw symbol or an exact reference to it, artifact identity and any discriminator needed to distinguish equal displayed names. It should show module, authored declaration, receiver and concrete generic arguments when available. Generated closures, adapters and async frames must be labeled as generated and related to an authored origin only when that relationship is established.

All suite-owned symbol views must use the same decoding and naming policy. Integrations should make this presentation available to native debuggers and external profilers. Unmodified external tools may still show their native spelling; this RFC does not promise control over every host utility. Display names must never replace compiler identities or C ABI symbol names.

An encoded module path alone is insufficient to invent a function name. Older artifacts may require matching metadata or may remain partially decoded. Formatting fixes must not silently change emitted symbols; any encoding change must be versioned and preserve the native linkage contract for artifacts built together.

### Source debugging and runtime failures

The required source-debugging capability includes source-line and function breakpoints, step into/over/out, stack inspection, and named parameters and locals for supported unoptimized Incan code. The minimum value set comprises scalar values, strings, nominal fields, enums, `Option`, `Result` and bounded collection previews. Scope and shadowing must reflect authored declarations. Displays must respect validity and initialization state.

A supported artifact must retain or identify matching debug information and source revisions for every frame it claims to resolve. DWARF or another target-native format may provide this information. Stripping, relocation, downloaded artifacts and separated debug files must not silently sever the association. A source map must preserve its declared byte and character coordinate conventions; editor UTF-16 conversion must not change the underlying location.

Each requested value must distinguish available, optimized out, not captured, out of scope, uninitialized, unsupported, redacted and unreadable states as applicable. Raw foreign layout is not an Incan value merely because bytes are readable. Value display must not call user formatters, properties or other effectful code without an explicit evaluation request. Async task reconstruction, foreign frames and optimized stepping may have narrower support, which must be declared.

Runtime diagnostics must distinguish an assertion, panic, error returned from an entry point, process signal, requested cancellation and toolchain failure when evidence permits. They must preserve the original message and termination semantics. An opt-in backtrace must use the same symbol and source policy. Unwinding and fatal-signal constraints may prevent complete capture; a missing stack must not be presented as an empty, complete stack. This RFC does not authorize catching a foreign panic and converting it into an ordinary Incan result.

### Trace correlation and observation integrity

Program and toolchain observations must share a correlation contract while retaining their producer-specific payloads. A capture must identify its schema, subject, toolchain and target, execution/build request, participating processes, observation configuration, timestamps with clock origin, and completeness. Artifact, source and dependency identities must be included when known; unknown external-command provenance must remain unknown.

Engine spans must reuse the existing engine trace authority, including explicit compatibility with `incan.engine.trace/1`; this RFC does not replace that schema silently. Existing trace switches may remain compatibility adapters, but must not create independent facts or duplicate spans. Application telemetry must retain RFC 093's provider, sampling and export ownership. Linking an application trace to a build request must be explicit; temporal proximity is not a causal edge.

Durations must use a declared monotonic clock. Timestamps from unrelated process clock origins must not be subtracted without a measured alignment and its uncertainty. Parent-child span identity and cross-process links must be scoped so process-id reuse and separate requests cannot collide.

Observers must use bounded buffers and requests. Drops, truncation, sampling, back-pressure, unavailable counters and write failures must be visible in completion metadata. A truncated final record or missing completion marker must yield an incomplete capture. An observer failure must not change the subject's language semantics or be reported as a compiler rejection of valid source.

Program stdout and stderr must remain program streams. Capture data must use the requested destination or a separate client channel. A legacy option that explicitly requests stderr output may keep that compatibility behavior, but its mixed stream must not be represented as an uncontaminated program stream. Source contents, environment values, argument payloads and memory contents must not be collected implicitly. Explicit value capture must state its exposure; redacted display is not deletion of raw captured data.

### Memory and leak inspection

A memory observation must identify its provider, allocator coverage, process identity, observation interval or checkpoint, units and measurement method. It must distinguish live allocation bytes, unreachable allocation bytes, retained reachable allocations, resident memory, private memory, shared mappings, allocator reserves and physical footprint where supported. Unsupported quantities must be unavailable, never zero.

A leak finding must name the provider's criterion and checkpoint, with allocation or ownership evidence where available. A reachable object retained by a program is not an unreachable leak merely because it remains after a request. Conversely, a flat resident set does not prove correct destruction. Reports should expose resource retention such as file descriptors separately from heap leaks when the provider supports it.

Launch and attach must state which allocation history they can observe. Custom allocators, shared libraries, child processes, mapped memory and foreign libraries must appear in the coverage assessment. A report must not sum per-process RSS and label it unique physical memory. Shared resident-engine work must be attributed by request where supported, with unattributed memory shown separately.

Comparisons must identify their checkpoints, matching workload, artifact and observer configuration. Repeated-work growth must be reported as measured deltas with sample counts, not inferred from one peak. A negative leak result means no leak detected within the named coverage and checkpoint, not a universal correctness claim. Checkpoints are explicit; the suite must not invent application quiescence or force cleanup to improve a report.

### Profiling

Profiling must distinguish sampled CPU activity, measured span durations, wall time, blocked/waiting time and allocation activity. Sample counts, sample interval, lost samples, observation window, observer version and unresolved-stack proportion must be retained when applicable. Inclusive time and self time must be labeled; overlapping spans must not be added as if they were disjoint wall time.

A profile must bind symbols to matching artifacts and preserve unresolved frames. Export adapters may provide formats accepted by external viewers, but must disclose information lost in conversion. A profile captured with instrumentation must identify its overhead measurement or state that overhead was not measured. Comparison evidence must state workload, cold/warm condition, sample count and shared-host interference; this RFC establishes no unmeasured speedup claim.

### Unsafe and foreign boundaries

RFC 116 remains the authority for checked C bindings and `unsafe:` acknowledgment. RFC 005's exclusion of unsafe Rust calls remains unchanged. Diagnostics must distinguish a source-binding violation, target/header verification failure, missing native capability, observed runtime check failure and foreign crash. They must retain the binding, call-site and declared obligation when available.

An enabled boundary capture must identify the checked declaration, selected native symbol/library, target ABI, call site and correlation identity. It must distinguish entry, observed normal return, observed failure and completion not observed. It may record declared outcome or initialization obligations only when the corresponding evidence exists. A successful header check does not prove a native function obeyed its pointer, lifetime or initialization promises at runtime.

The observer must not dereference arbitrary pointers, scan unbounded foreign memory or invoke a foreign function merely to explain it. Bounded value inspection must respect available validity evidence and explicit capture policy. Sanitizer or allocator findings may be attached as provider observations with their coverage; they must not be presented as complete enforcement of the binding contract. Instrumentation may perturb timing and must not promise race reproduction or survival after undefined behavior.

### Tests and acceptance

Failure observations must carry test identity, selected parameters when capturable, exact artifact/source identities, execution identity, outcome, original assertion or returned error, and separately labeled streams. Assertion operands must be evaluated exactly once according to ordinary evaluation and short-circuit rules. Reporting must not introduce a new display-trait requirement for a previously valid assertion; unsupported operand display must be explicit.

Compilation refusal, assertion failure, returned error, panic, timeout, signal, cancellation, skip, expected failure and unexpected pass must remain distinct when the runner observes them. Existing discovery, isolation and expected-failure policy remain authoritative. Capture must not turn historical execution into a fresh pass or rerun external effects without a new execution request.

A claimed support profile must provide conformance evidence for the following cases. An unsupported target or optional capability must be declared; it cannot count as passing a required row.

| Case | Required observation |
| --- | --- |
| Equal names in different modules; generated and generic symbols | Readable distinctions and retained raw identity; malformed and unknown formats remain explicit. |
| Assertion in a helper; failing error-returning entry point | Correct authored failure and caller where supported; unchanged subject outcome. |
| Source moves, Unicode columns, stale source, relocated binary | Correct matching source or explicit mismatch/unavailability; no convincing stale location. |
| Debug locals, shadowed names, optimized values | Correct values and scopes, or precise availability states. |
| Safe program with allocating dependencies | Coverage-qualified allocation/leak evidence with readable and unresolved stacks separated. |
| Repeated requests and shared mappings | Checkpoint deltas, process scope and sharing limitations; no RSS-based leak inference. |
| C refusal, normal return, interrupted call | Static failure and observed runtime outcomes remain distinct. |
| One failed test followed by a passing test | Both outcomes, original assertion evidence and separate streams retained. |
| Observer disabled, enabled, denied, full destination or interrupted | Console/subject behavior preserved where observation permits execution; incomplete evidence never presented as complete. |
| Program plus toolchain and a concurrent unrelated request | Correlated selected work, no invented attribution of unrelated work. |

Performance qualification must compare disabled and enabled observation under the same workload, reporting wall time, CPU, allocation/footprint and capture growth with sample counts. Existing engine overhead requirements continue to apply. Heavy leak analysis and interactive debugging require separately disclosed costs, not an assertion that the lightweight trace budget covers them.

## Design details

### Relationship to existing proposals

RFC 126 owns durable investigations, hypotheses, experiments and verification. This RFC owns the diagnostic and observation capabilities that an investigation can reference. A standalone capture is sufficient for a failure report; it need not create a second investigation model. Live debugger control and editor adapters must preserve RFC 126's session and control ownership. Its support requirements for Rust and mixed executions are not narrowed by this RFC's Incan source-debugging contract.

RFC 093 and RFC 095 own application telemetry and span authoring. Engine tracing owns build operations. This suite joins their identities and presents their evidence without defining another telemetry language or provider. Compiler semantic inspection remains the authority for declaration meaning, while native debug information describes a particular executable.

### Artifact and compatibility boundaries

Debug information is an artifact requirement, not a consequence of naming a build profile `debug`. Selecting instrumentation or debug metadata that changes compilation must select an appropriately identified artifact. Merely inspecting an existing capture must not rebuild its subject. Separate debug artifacts must be matched by content or native artifact identity, not only by a filename.

The initial proposal preserves ordinary program streams by making enriched captures opt-in. Whether ordinary runtime failures should additionally acquire richer default text is a maintainer policy question below. Existing check JSON and foreign ABI spellings remain compatible independently of that choice. The suite must work with honestly reduced information for older artifacts instead of claiming they satisfy a newer support profile.

### Capability and retention boundaries

Capability reporting distinguishes provider absence, permission denial, unsupported target, missing instrumentation, missing debug information and source mismatch. The suite may explain how to obtain a capability but must not silently install tools, elevate privileges or alter system tracing policy.

Explicit capture retains selected observations and references to the artifacts needed to interpret them. Retention and collection use the established artifact lifecycle; this RFC creates no second store or automatic global collection policy. Captures may be bounded by duration, event count, inspected bytes or selected processes. The store's total size is not such a bound. Export must declare omitted dependencies and whether later source or symbol resolution is still possible.

## Alternatives considered

- **Document host commands only.** Useful as provider documentation, but leaves process attribution, symbol presentation, coverage and capture interchange to each user.
- **Make names prettier only at emission.** Helps new binaries but cannot decode old stacks, preserve unknown symbols or unify source/value inspection. Emission policy and presentation are both needed.
- **Require a full RFC 126 investigation for every failure.** Adds workflow overhead to a single assertion or memory check; ordinary captures can feed investigations when needed.
- **Use one universal event type for everything.** Erases distinctions between compiler facts, samples, leak classifications and user logs. A shared envelope with typed payloads preserves them.
- **Enable all instrumentation by default.** Changes costs, timing and data exposure without a user decision, and makes incomplete providers harder to distinguish.
- **Guarantee every unsafe violation is caught.** Cannot follow from checked declarations or partial runtime observation; the suite must report the evidence it actually has.

## Drawbacks

Source debugging and observer integration require a maintained platform and toolchain support matrix. Retaining matching source and debug artifacts consumes storage and can expose implementation details. Memory instrumentation and sampling change execution costs, and some faults prevent complete capture. Versioned symbol formats and typed observation outcomes impose compatibility work. The suite also adds command concepts; the benefit depends on keeping ordinary workflows small and unsupported cases explicit.

## Layers affected

- **Compiler diagnostics and semantic identity:** stable codes, source-anchor projection, declaration provenance and shared rendering contracts.
- **Checked bodies and native lowering:** source scopes, local names, assertion evidence and generated-frame relationships.
- **Native backend and artifact distribution:** target debug information, symbol identity, matching source/debug artifacts and relocation.
- **Runtime and standard library:** panic/assertion observations, safe value representations, telemetry correlation and checked foreign boundaries.
- **Platform and Oven tooling:** observer capability discovery, process scope, capture lifecycle, memory/profiling operations and report projections.
- **Debugger and editor integration:** source-level control, bounded value inspection, exact-source validation and diagnostic parity.
- **Tests and contract tooling:** structured failure observations and support-profile conformance.

## Unresolved questions

- Which operating systems, architectures and native debugger/provider combinations must qualify for the first supported release, and which may initially be explicitly unsupported? The proposed minimum source-debugging and value contracts apply to every target claimed as supported.
- Should ordinary runtime failures adopt a source location and concise stack by default, accepting the text-compatibility change, or should enriched runtime output remain opt-in while existing stderr stays unchanged?
- What debug-artifact distribution policy should official releases and published dependencies guarantee: bundled source and symbols, separately obtainable matching artifacts, or source-free symbols by default? This determines download/storage cost and source-disclosure expectations, not the requirement to report missing artifacts honestly.

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
