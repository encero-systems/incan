# RFC 167: Debugging and diagnostics for Incan programs and tools

- **Status:** Planned
- **Created:** 2026-10-31
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 005 (Rust interop and its unsafe-call boundary)
    - RFC 018 and RFC 019 (assertions and testing)
    - RFC 093 and RFC 095 (application telemetry and span vocabulary)
    - RFC 103 (secret values and redaction)
    - RFC 106 and RFC 111 (semantic inspection and contract feedback)
    - RFC 116 (checked C bindings)
    - RFC 118 (Incan and Oven command-line surfaces)
    - RFC 120 and RFC 124 (source-symbol and compiled-unit identity)
    - RFC 017 and RFC 085 (constrained types and field constraints)
    - RFC 126 (evidence-backed investigations)
    - RFC 131 (error origins, cause chains and opt-in stack capture)
    - RFC 165 (std.db typed embedded storage)
    - RFC 169 (typed record sets and record streams)
- **Issue:** [#1640](https://github.com/encero-systems/incan/issues/1640)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

An Incan developer should be able to find most failures before running, understand a compiler refusal, read a failed assertion, stop at an authored statement, explore and evaluate values in a stopped program, ask where a value came from, change code and continue, identify an allocation, profile an operation, and diagnose a failing test through one supported suite. This RFC defines the observable contracts: compile-time exploration through the checker and language server, readable failures, readable symbols, source-matched native debugging, interactive exploration of a stopped program, recorded executions, live code changes, consistent diagnostic records, correlated program and toolchain observations, memory and leak inspection, profiling, and checked C-boundary evidence. Oven builds Rust as well as Incan, so the suite gives Incan, Rust and mixed programs one debugging experience on Linux, macOS and Windows. People, editors and agents use the same session contract, which answers in bounded, structured form. It applies to ordinary safe programs as well as foreign calls and the toolchain itself. It supplies observations to RFC 126's durable investigations without requiring an investigation to inspect one failure.

## Core model

1. **A diagnostic explains an outcome.** A compiler refusal, an assertion failure, a native crash and an observer failure are different outcomes with different producers.
2. **An observation describes identified work.** It names the execution or build request, the observed interval, the exact artifact when known, and its coverage. Missing evidence is never a successful observation of nothing.
3. **Identity and presentation are distinct.** A readable function name is a presentation of a symbol, not its identity. Source coordinates belong to a particular source revision and executable.
4. **One fact has one authority.** Terminal, JSON, editor, debugger, memory and profile views consume the same underlying diagnostic, identity or observation. Combining them does not merge their meanings.
5. **Capabilities are explicit.** Support depends on target, toolchain, artifact, optimization, observer and permissions. A partial report describes its limits before making a conclusion.
6. **Most failures are found before running.** The checker and the language server know types, constraints, contracts and where a value can come from without executing anything. Runtime evidence is for what only a run can show: outside data, timing and foreign code.
7. **Runtime failure evidence comes in three levels.** An ordinary failure says which condition failed, where, and which values it compared. A stack capture says how execution got there. A debugger, a recorded execution or an explicit value capture says what the program's values were. Each level is a separate choice, and a lower level never claims to supply a higher one.
8. **Incan and Rust share one experience.** Oven builds both languages. A mixed execution is reported, debugged and captured as one execution: frames of either language appear in one stack, stepping crosses the boundary, and each frame keeps its own language's names and value presentation.
9. **Agents are first-class users.** Every operation answers in bounded, structured form with stable handles, and can be asked by meaning rather than step by step. A person at a terminal, an editor and an agent use the same session contract.

## Motivation

A program that reports only an assertion message leaves the compared values, the caller, the source line and the surrounding state unknown. A native allocation stack can name a module through an encoded identifier while concealing the authored operation. A debugger may recognize an executable without having the matching line tables or names of its locals. These are obstacles in ordinary programming, independently of whether any source contains `unsafe:`.

Much of what a Python developer debugs at runtime, such as a `None` where a value was expected, a missing attribute or a wrong type, an Incan checker reports before the program runs. Incan can go further. The same compiler facts that drive the debugger can answer, in the editor, which constructions can reach a use, whether a constant can ever satisfy its constraint, and what a pure expression evaluates to. Debugging should start there, with the runtime tools reserved for what only a run can show.

Seeing one variable at a stop is only the start. Developers who work in interactive environments expect to explore a stopped program: browse its namespaces, view models and collections as tables, evaluate an expression against the current frame, and find where a value came from. That experience should not depend on a particular editor.

Oven builds Rust as well as Incan, and a program often mixes them. Rust already has native debuggers, panic backtraces and profilers. A mixed program should not need two debugging experiences that disagree about the same execution.

Agents debug too, and their costs differ from a person's. Stepping is a human loop: for an agent, every step is a full request that carries its whole context. Screens meant for people must be scraped, and an unbounded value dump consumes the context it was meant to inform. An agent needs one request per question, a bounded and structured answer, stable handles for follow-up questions, and questions asked by meaning, such as "stop when this condition holds" or "where did this value come from".

The toolchain has the same needs. A slow command may spend time preparing a build, waiting for shared work, compiling a dependency, launching an executable or running the program. A large resident set may contain shared mappings, live allocations, allocator reserves or unreachable allocations. Treating these as one duration or one memory number does not explain them.

Existing compiler diagnostics, native debugging, engine observations and application telemetry supply parts of the answer. The missing contract is their coherent presentation, with precise limits. An understandable failure should not require manually combining host-specific utilities or reconstructing source identity from a native symbol.

## Goals

- Find failures before running: constants that can never satisfy their constraints, where a value can come from, and pure-expression evaluation in the editor, from the same compiler facts the debugger uses.
- Report a failed comparison's condition, location and compared values by default, without capturing a stack or evaluating an operand again.
- Display authored names in supported stack, debugger, allocation, trace and profile views while preserving exact native identity.
- Support breakpoints, stepping, call stacks and bounded value inspection against matching Incan source.
- Let a developer explore a stopped program independently of the editor: namespaces, tabular views of models and collections, explicit evaluation in the selected frame, and where a value came from.
- Record an execution on request and replay it exactly, so that where any value came from can be answered after the fact and a fix can be tried against the recorded run, on every supported platform.
- Change code in a stopped program and continue, within declared limits.
- Give Incan, Rust and mixed programs built by Oven one debugging experience on Linux, macOS and Windows.
- Serve agents through the same session contract: bounded, structured answers, stable handles and questions asked by meaning.
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
- Identical debugger and memory capabilities at every optimization level.
- Recording every run by default, instruction-level recording, automatic repair or autonomous hypothesis management; RFC 126 owns investigation workflows.
- Implicit evaluation of application code to display values, automatic attachment to unrelated processes, or automatic upload of captures.
- A stable native ABI or unchanged linker spelling across compiler versions merely because names are readable.
- A second trace store, code graph or source-identity registry, or total-size ceilings on the existing artifact store.

## Guide-level explanation

The following commands and transcripts describe the proposed surface. They are not claims of shipped commands. Counts and identifiers are illustrative; real reports obtain them from the selected execution. Existing `incan check --format json` keeps its established contract.

### Find it before running

The examples in this section use one invoice. When the invariant is part of the type, the checker can see a bad order line before anything runs:

```incan
model OrderLine:
    product: str
    quantity: int[gt=0]
    unit_price: int


def main() -> None:
    lines = [
        OrderLine(product="Keyboard", quantity=2, unit_price=80),
        OrderLine(product="Monitor", quantity=-1, unit_price=300),
    ]
```

A constant that can never satisfy its constraint is a diagnostic in the editor and at check time:

```text
main.incn:10: OrderLine.quantity requires int[gt=0]; the constant -1 can never satisfy it
```

That rule belongs to the checker's constraint and contract rules (RFC 017, RFC 085 and RFC 111). This suite presents it like any other diagnostic. When the quantity comes from outside, such as a file, it is checked at construction instead, and the failure names the field, the value and the construction site, before the bad line reaches any arithmetic.

The language server answers exploration questions from the same facts, without a run. In the full invoice program below, asked where `line.quantity` in `line_total` can come from, it lists the constructions that can reach that use:

```text
can come from (static):
  OrderLine(...) quantity=2      main.incn:21
  OrderLine(...) quantity=-1     main.incn:22
```

It can also evaluate a pure expression with constant inputs, including calls to pure functions, using the same interpreter that evaluates read-only expressions at a stop:

```text
evaluate> line_total(OrderLine(product="Keyboard", quantity=2, unit_price=80))
160   (int; evaluated before running; interpreted line_total)
```

An expression with effects, such as reading a file or the clock, is refused with an explanation that it needs a run.

### Read a failure

Not every invariant fits a type, and data from outside is known only at runtime. The rest of this section keeps a plain `quantity: int`:

```incan
model OrderLine:
    product: str
    quantity: int
    unit_price: int


def line_total(line: OrderLine) -> int:
    assert line.quantity > 0
    return line.quantity * line.unit_price


def invoice_total(lines: list[OrderLine]) -> int:
    mut total = 0
    for line in lines:
        total += line_total(line)
    return total


def main() -> None:
    lines = [
        OrderLine(product="Keyboard", quantity=2, unit_price=80),
        OrderLine(product="Monitor", quantity=-1, unit_price=300),
    ]
    println(invoice_total(lines))
```

The second line fails its assertion. An ordinary run reports which condition failed, where, and the values it compared:

```text
Assertion failed: line.quantity > 0
  left:  -1
  right: 0
  main.incn:8 in line_total
```

The program evaluated both operands to compare them, so the report shows those values; it does not evaluate either operand again. No call stack or frame values are captured. A secret value is redacted, and a value whose type cannot be displayed is reported as not displayable.

This report does not say how execution reached `line_total`. With stack capture enabled through the runtime configuration that RFC 131 defines for errors, the same failure adds its stack:

```text
Assertion failed: line.quantity > 0
  left:  -1
  right: 0
  main.incn:8 in line_total

Stack trace:
  line_total      main.incn:8
  invoice_total   main.incn:15
  main            main.incn:24
```

Neither report says which product had the bad quantity. Frame values come from a debugger, from a recorded execution, or from an explicit capture made before the process exits; a saved stack trace cannot reconstruct them afterwards.

### Make a native stack readable

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

The session stops at a failed assertion as well as at breakpoints on source lines and functions. Here it stops at the assertion and presents the live values:

```text
Source match: verified; source debugging: supported
Stopped: assertion failed in line_total at main.incn:8

line:
    product       "Monitor"
    quantity      -1
    unit_price    300
```

Selecting the caller's frame shows its values:

```text
Frame: invoice_total at main.incn:15
    total         160
    lines         list[OrderLine], 2 items
```

A breakpoint is not reported as resolved until it has an executable location. After the source changes, the client offers the matching retained source or a new build. It does not label a location in a newer file as the old executable's location. An optimized value may be shown as `optimized out`; a missing debug artifact is a different failure.

### Explore a stopped program

A stop is a place to explore, not only to read locals. The same session serves the terminal client and editors, so the experience does not depend on the editor.

The namespace view lists the selected frame's parameters and locals, the values captured by an enclosing closure, and the module's values. A collection of models can be viewed as a table:

```text
lines (list[OrderLine], 2 items)

  #   product    quantity   unit_price
  0   Keyboard          2           80
  1   Monitor          -1          300
```

An explicit evaluation runs an expression in the selected frame. It is checked against that frame's names and types, like source written at that line:

```text
evaluate> [line.product for line in lines if line.quantity <= 0]
["Monitor"]   (list[str]; read-only: no program code ran)

evaluate> line_total(lines[0])
160   (int; called line_total)
```

The first evaluation only reads values. The second calls program code, and the session says so, because a call can have effects.

The session can also say where a value came from. Without a recording, it combines the static answer with what the stop itself shows:

```text
where> line.quantity
  line       bound by `for line in lines`                main.incn:14   (this stop: iteration 2)
  quantity   can come from OrderLine(...) quantity=2     main.incn:21   (static)
             can come from OrderLine(...) quantity=-1    main.incn:22   (static)
```

### Record a run and ask where a value came from

A recorded run answers that question exactly. A recording keeps only what the program cannot recompute: what entered it from outside, such as file and network reads, the clock, randomness, the environment, its arguments and the results of foreign calls, and the order in which its threads synchronized. After the run, Oven replays the same artifact against the recording once and keeps an index of where each value came from. Recording relies on the compiler and the runtime, not on processor or operating-system tracing, so it works on every supported platform.

```sh
incan debug --record main.incn
```

The invoice program reads nothing from outside, so its recording holds only its arguments and environment. Every value of the run is still answerable:

```text
where> line.quantity
  -1    OrderLine(product="Monitor", quantity=-1, unit_price=300)   main.incn:22
        element 1 of lines                                           main.incn:20
        bound to line, iteration 2                                   main.incn:14

where> total
  0     mut total = 0                 main.incn:13
  160   total += line_total(line)     main.incn:15   iteration 1, line.product = "Keyboard"
```

A provenance answer stays in the project's own code. A value produced by a dependency is reported as returned by that call, with the arguments it received, and the session goes inside the dependency only when asked. A session over a recording can also stop at any recorded point, earlier as well as later than the current one.

The recording outlives the process. After it exits, the same questions get the same answers from the index, without running the program again. A recording can also be replayed against a changed build, for example after a fix to `line_total`, to see what the fix does to the exact recorded run. The result is reported as a replay against changed code, and the replay stops with a report at the first point where the changed code asks for an input the recording does not contain. Recording costs time and storage in proportion to what enters the program, not to how much it computes, and it is opt-in.

### Change code and continue

A developer can change a value and continue, for example setting `line.quantity` to `1`. A developer can also change code. Fixing `line_total` in the editor and applying the change in a stopped session recompiles only the changed function and redirects later calls to it:

```text
apply> main.incn
  patched: line_total
  later calls use the new code; the call already running keeps the old code
```

A change to a model's fields, an enum's variants or a function's signature cannot apply to a live process, and the session says the run must restart. Every value change and code patch is recorded, and the execution's outcome is reported as the outcome of a modified run.

### Debug Rust and Incan together

Suppose the invoice drops its assertion and takes a bulk discount from a Rust crate, `pricing`, that the same Oven project builds:

```incan
from rust::pricing import bulk_discount


model OrderLine:
    product: str
    quantity: int
    unit_price: int


def line_total(line: OrderLine) -> int:
    return line.quantity * line.unit_price - bulk_discount(line.quantity)
```

The rest of the program is unchanged. `bulk_discount` converts the quantity to an unsigned integer and panics on the negative one. The failure, the stack and the debugger cover both languages:

```text
Panic: called `Result::unwrap()` on an `Err` value: TryFromIntError(())
  src/lib.rs:14 in pricing::bulk_discount

Stack trace:
  rust    pricing::bulk_discount   src/lib.rs:14
  incan   line_total               main.incn:11
  incan   invoice_total            main.incn:17
  incan   main                     main.incn:26
```

Stepping out of the Rust frame returns to the Incan line that called it. Selecting a Rust frame shows its values with Rust names and types; selecting an Incan frame shows Incan's. A Rust-only project gets the same commands and reports.

### Debug as an agent

An agent uses the same session through structured requests, for example over MCP. One request asks a question by meaning and gets one bounded answer with handles for follow-up questions:

```json
{"op": "run_until", "session": "s1", "in": "line_total", "when": "line.quantity < 0"}
```

```json
{
  "stopped": {"frame": "f1", "function": "line_total", "location": "main.incn:8"},
  "values": {
    "line": {
      "handle": "v3",
      "type": "OrderLine",
      "fields": {"product": "Monitor", "quantity": -1, "unit_price": 300}
    }
  },
  "omitted": []
}
```

Follow-up questions refer to handles instead of repeating earlier answers:

```text
{"op": "where", "session": "s1", "value": "v3.quantity"}
{"op": "rows", "session": "s1", "value": "v7", "from": 100, "count": 50}
{"op": "compare", "captures": ["run-41", "run-42"], "value": "total"}
{"op": "replay", "capture": "run-41", "against": "current"}
```

Each answer names its source locations and identities, states what it left out and why, and fits a declared size. A session or capture handle stays valid for its lifetime. A live value handle answers for the stop where it was made, and reports that the stop has ended once the process moves on. The terminal client and editors use the same operations; nothing here is agent-only.

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
| `incan debug PATH [--record] [-- ARGS...]` | Select a program and exact debug-capable artifact, then start a source-debugging session. `--record` selects a recording artifact. Report unsupported capabilities before promising a session. |
| `incan diagnose run PATH --capture KINDS --output CAPTURE [-- ARGS...]` | Execute a program under explicitly selected observations. |
| `incan diagnose test PATH --capture KINDS --output CAPTURE` | Use ordinary test discovery and execution, retaining selected failure or resource observations. |
| `incan diagnose command --capture KINDS --scope SCOPE --output CAPTURE -- COMMAND [ARGS...]` | Observe the explicitly launched command and identified work within scope. It must not require shell interpretation of operands. |
| `incan diagnose attach PID --capture KINDS --output CAPTURE` | Observe an explicitly selected existing process, subject to process identity, permission and observer capability. |
| `incan diagnose inspect CAPTURE --view VIEW --format text\|json` | Read a capture without running or attaching to its subject. |
| `incan diagnose capabilities --format text\|json` | Report supported kinds, targets, providers, launch/attach requirements and known permission constraints. |

`KINDS` is a comma-separated selection from `failures`, `values`, `trace`, `memory`, `leaks`, `cpu` and `ffi`; unknown kinds must be refused. `values` is a recorded execution and selects a recording artifact. `SCOPE` is `program`, `toolchain` or `all`. Run and test default to `program`; command requires an explicit scope; attach observes the selected process and explicitly included descendants. An observation must not silently broaden its scope. The views used above are `memory`, `timeline`, `foreign-calls` and `failures`; a view with no captured data must say `not captured`.

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

A Loaf's debug information, source maps and the layout facts used to present its values must be stored with the Loaf in the local store, beside the artifacts they describe, whether those artifacts were built locally or obtained with the Loaf. The session must obtain them by asking the Loaf, matched by artifact identity, without a network request at debug time. A Loaf published without its source must still carry its debug information, and the session must report that source as unavailable.

Each requested value must distinguish available, optimized out, not captured, omitted for budget, out of scope, uninitialized, unsupported, redacted, unreadable and lost to a damaged capture, as applicable, and must never render one of these as an ordinary null or empty value. A read that may have effects, including device memory, must not count as passive inspection. Raw foreign layout is not an Incan value merely because bytes are readable. Value display must not call user formatters, properties or other effectful code without an explicit evaluation request. A library may present its own values through inspectors declared in checked metadata; an inspector that would run target code is an evaluation. Async task reconstruction, foreign frames and optimized stepping may have narrower support, which must be declared.

Runtime diagnostics must distinguish an assertion, panic, error returned from an entry point, process signal, requested cancellation and toolchain failure when evidence permits. They must preserve the original message and termination semantics. An opt-in backtrace must use the same symbol and source policy. Unwinding and fatal-signal constraints may prevent complete capture; a missing stack must not be presented as an empty, complete stack. This RFC does not authorize catching a foreign panic and converting it into an ordinary Incan result.

An ordinary failed assertion must report its condition and source location. When the condition is a comparison, the report must include both operands' values as the comparison evaluated them; it must not evaluate an operand again to obtain a value. A value of a secret type (RFC 103) must be redacted. A value whose type has no display must be reported as not displayable; reporting must not add a display requirement to a previously valid assertion. An ordinary failure must not capture a call stack or frame values.

Stack capture for assertions and panics must follow RFC 131's policy for errors: off by default, and enabled through the same process runtime configuration. Frame values require a live debugging session or an explicit value capture.

### Exploring a stopped program

A debugging session must offer, for a stopped execution, a namespace view of the selected frame's parameters, locals and captured closure values, and of the values of the modules it uses. Each entry must carry its declared type and an availability state from the set above.

Collections and models must support bounded tabular views: rows for list and map entries, columns for model fields, with paging rather than complete materialization. Record batches (RFC 169) should use the same view; a record set or stream shows its declared kinds, schema and source capabilities without being read. A table must show its total size when known and must say when it shows only part of a collection.

An evaluation must be an explicit request. Its expression language is the selected frame's language. It must be checked against the selected frame's scope and types, as source written at that location would be, and must be refused with an ordinary diagnostic when it does not check. An evaluation that only reads values must run no code in the stopped process. An evaluation that calls program code must be requested as such; it runs in the stopped process, using code produced by the same compiler identity that built the artifact, and its result must say that code ran. A result must identify the frame and execution it was evaluated in. Evaluation must never rerun the program or reconstruct a value from current source.

The session must be able to report a value's provenance and must label each answer as static or observed. A static answer lists the declarations, assignments and constructions that can reach the use, from checked bodies. An observed answer reports what produced the value in this execution: the binding visible at the stop, and, in a recorded execution, the chain of assignments, constructions and calls that produced it, from the recording's provenance index. Provenance that is neither statically known nor observed must be reported as unavailable, not inferred from current source.

The terminal client, editor clients and agents must use the same session contract. Editor integration should use the Debug Adapter Protocol, so that an editor needs no Incan-specific debugger.

### Before running

The checker and the language server must present the facts this suite uses at runtime wherever they are known statically. Static provenance must be available in the editor for any checked use. A constant that can never satisfy its type's constraint should be diagnosed before running; the rule belongs to the checker's constraint and contract rules, and this suite presents it as an ordinary diagnostic.

The language server must offer evaluation of pure expressions with constant inputs, using the same interpreter as read-only frame evaluation. It may interpret calls to pure functions, and its result must name the functions it interpreted. It must refuse an expression that reads files, the clock, randomness, the environment or foreign code, and must report that the expression needs a run. An edit-time result is not evidence about any execution.

### Recorded executions

A recorded execution must use an artifact built for recording. The recording must keep every input the execution cannot recompute: the results of operations that reach the host, including those receipted under RFC 104, the clock, randomness, the environment, arguments, the results of calls into foreign code, and the order of synchronization between threads. Recording must not change the program's semantics. It must be available on every supported platform and must not depend on processor or operating-system tracing facilities. Recorded content must be stored by content identity, so that content already in the store is not stored again.

Values must be recomputed by replaying the same artifact against the recording. Replay must perform no operation on the host; every recorded operation's result must come from the recording. Replay must reproduce the recorded execution exactly. Where it cannot, for example because memory shared with another process changed or unsafe or foreign code raced, it must report the first point of divergence and must not present anything after it as observed.

A recording must state its scope, what it omitted and the overhead measured while recording. Values of secret types (RFC 103) must be redacted when recorded, never stored in plaintext. Replay past a redacted input requires the secret to be supplied again; otherwise replay stops at that input and later values are reported as unavailable. A recording must not manufacture RFC 104 receipts for a run that has none, or imply that a raw foreign call was governed.

The provenance index must be built once, by a replay after the run, and kept with the recording. Queries against a completed recording must read that index, run nothing and return the same answer to the same question. A provenance answer must stay in the project's own code: a value produced by a dependency is reported as returned by that call, with its arguments, and the answer goes inside the dependency only on request. A session over a recording may stop at any recorded point, earlier or later than the current one.

A recording may be replayed against a different build of the same program. The result must be reported as a replay against changed code, and replay must stop with a divergence report at the first input the changed code requests that the recording does not contain.

### Changing values and code

A session may change a value at an explicit request. It may also apply a code change to a stopped process: only the changed functions are recompiled, by the same compiler identity that built the artifact, and later calls to them use the new code. A frame already executing a changed function must keep the old code until it returns. A change to a type's layout, such as a model's fields or an enum's variants, or to a function's signature, must be refused for the live process with a diagnostic stating that the run must restart.

Every value change and code patch must be recorded in the session, and the execution's outcome must be reported as the outcome of a modified execution. Code changes must be offered only for artifacts built to accept them; the supported targets must be declared.

### Agent use

Every session, capture and language-server operation in this suite must be available as a structured request with a versioned, structured response. A response must fit a declared size; content beyond it must be reachable through continuation handles rather than truncated silently. Sessions, frames, values and captures must have handles, so that a follow-up request never needs an earlier answer repeated. A session or capture handle must stay valid for its lifetime, and a frame or value handle from a capture or recording for the lifetime of that capture. A frame or value handle from a live process must answer for the stop where it was made, and once the process resumes it must report that the stop has ended; it must never read current memory.

The contract must support questions asked by meaning, including: run until a condition holds in a named function or location; report a value's provenance; and compare a value or outcome across two executions. A session must be addressable from separate requests. Every answer must carry the source locations and identities it refers to and must state explicitly what it did not capture, could not read or left out. These operations must be exposed through the same session contract that serves the terminal and editors, including over MCP.

A bounded run must report why it stopped: condition met, process exit, timeout, cancellation, budget exhausted or engine failure. Not reaching a condition within a budget is inconclusive, not evidence that the condition cannot occur.

At most one client may control a session's execution at a time, and handover must be explicit; other clients may inspect what it has retained. A request that changes the process must name the session revision it expects, and a conflicting request must fail without changing the process. Each request must carry an identity, so that a retry never launches a second process or repeats an operation. Control grants nothing else: evaluation, reruns and source edits keep their own authority. Launch, attach, detach, terminate and client disconnect must each have a stated effect, and a disconnect must not kill an attached process.

Recorded program output and source text are evidence data, not instructions to the agent client.

Debugger operations are RFC 104 capabilities, separate from the connection that carries them. Read-only inspection is the only default, and an operation with no assigned grant is refused. Every operation that changes the process, or what it does next, needs one of these grants:

- **control:** resume, step, pause and detach;
- **write:** change a variable, a field or memory, such as setting `approved = true`;
- **evaluate:** evaluation that calls program code;
- **patch:** live code changes;
- **launch:** launch and rerun;
- **attach;**
- **terminate.**

Each grant is bound to a session and a process, and is never part of an agent's default ceiling. In RFC 104's reporting modes, every use is recorded in a receipt. A permissive run keeps no receipts, as RFC 104 defines, so a session an agent drives runs in a reporting mode. Over HTTP, a session sits behind RFC 037 authentication. Over stdio, its grants come from the user who launched it.

### Platforms and engines

The suite must support Linux, macOS and Windows with one contract. The debugger engine provides process control, breakpoints, memory access and unwinding; the session, presentation, evaluation, provenance and agent contracts belong to the suite and must not change with the platform or the engine. The capability report must name the engine in use.

### Rust and mixed executions

Rust built by Oven must use the same session, failure, capture and report contracts as Incan. A mixed execution is one execution: its stack must list Rust and Incan frames in call order, each labeled with its language. Stepping must cross the boundary in both directions. Each frame must keep its own language's names and value presentation; a Rust frame shows Rust paths and types, and an Incan frame shows Incan declarations and types. Rust frames built by Oven are not foreign frames for the purposes of this RFC; C and other foreign frames keep the narrower support declared above.

A Rust panic must report its message and location by default, as an Incan assertion does. One runtime configuration must enable stack capture for the whole execution, whichever language a frame belongs to. A Rust-only project must be able to use the suite's commands and reports.

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
| Failed comparison with plain, secret and undisplayable operands | Both values shown once as evaluated; secret redacted; undisplayable reported as such without a new display requirement; no stack captured by default. |
| Evaluation at a stop: read-only, calling program code, not checking | Read-only result with no program code run; calling result marked as having run code; an unchecked expression refused with a diagnostic. |
| Changed value, then continue | Change recorded; outcome reported as that of a modified execution. |
| Collection larger than one page | Bounded table with total size and a partial-view marker. |
| Mixed Rust and Incan stack; Rust panic | One stack in call order with each frame's language and names; stepping across the boundary; panic message and location by default. |
| Constant that can never satisfy its constraint; outside value that fails one | Diagnostic before running; construction-time failure naming field, value and construction site. |
| Edit-time evaluation: pure call, effectful expression | Result naming the interpreted functions; effectful expression refused as needing a run. |
| Provenance without and with a recording | Static candidates labeled static; recorded chain labeled observed; same answer after the process exits. |
| Recording with a secret value | Secret redacted at record time; replay without the secret stops at that input; recording states scope, omissions and overhead. |
| Recorded run that reads a file, the clock and randomness; replay; stop at an earlier point | Replay performs no host operation and reproduces every value; the file's content stored once by content; values at the earlier point as recorded. |
| Replay after shared memory changed, or after a race in unsafe code | Divergence reported at its first point; nothing after it presented as observed. |
| Replay against a fixed build | Reported as a replay against changed code; stops at the first input the recording does not contain. |
| Code change: function body, then a model field | Later calls use the new body while the running call keeps the old one; the layout change refused with a restart diagnostic. |
| Agent request with an oversized answer | Bounded response with continuation handles; follow-up through a handle without repeating the answer. |
| Linux, macOS and Windows | The same contract and reports on each; capability report names the engine. |
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

RFC 126 owns durable investigations, hypotheses, experiments, reduction and verification. This RFC owns sessions, control between clients, values, evaluation, recordings, captures and the agent contract, and an investigation refers to them by identity. A standalone capture is sufficient for a failure report; it need not create a second investigation model. RFC 126 investigates Rust, Incan and mixed executions; this RFC gives them one debugging experience, so neither narrows the other.

RFC 131 owns error origins, cause chains, propagation trails and stack capture for errors, with full stack capture off by default. This RFC applies the same policy to assertions and panics: their ordinary report gives the condition, the location and the compared values; stack capture uses the same runtime configuration; frame values need a debugger or an explicit capture. An Incan developer therefore meets one rule for every kind of failure: location by default, stack on request, values in the debugger.

RFC 093 and RFC 095 own application telemetry and span authoring. Engine tracing owns build operations. This suite joins their identities and presents their evidence without defining another telemetry language or provider. Compiler semantic inspection remains the authority for declaration meaning, while native debug information describes a particular executable.

### Artifact and compatibility boundaries

Debug information is an artifact requirement, not a consequence of naming a build profile `debug`. Selecting instrumentation or debug metadata that changes compilation must select an appropriately identified artifact. Merely inspecting an existing capture must not rebuild its subject. Separate debug artifacts must be matched by content or native artifact identity, not only by a filename.

Enriched captures are opt-in, so ordinary program streams are unchanged by this suite. The one change to default output is that a failed comparison now reports its compared values and its location. Existing check JSON and foreign ABI spellings remain compatible. The suite must work with honestly reduced information for older artifacts instead of claiming they satisfy a newer support profile.

### Capability and retention boundaries

Capability reporting distinguishes provider absence, permission denial, unsupported target, missing instrumentation, missing debug information and source mismatch. The suite may explain how to obtain a capability but must not silently install tools, elevate privileges or alter system tracing policy.

Explicit capture retains selected observations and references to the artifacts needed to interpret them. Retention and collection use the established artifact lifecycle; this RFC creates no second store or automatic global collection policy. Captures may be bounded by duration, event count, inspected bytes or selected processes. The store's total size is not such a bound. Export must declare omitted dependencies and whether later source or symbol resolution is still possible.

### Prior art

- **Rust.** `assert_eq!` prints both operands; plain `assert!` prints only its text, and the work to capture its operands has stayed unstable. `std::backtrace` is off by default and enabled through the environment, the same policy RFC 131 adopts. Rust has no debugger of its own: rustc emits standard debug information, and LLDB and GDB present it with Rust formatting scripts. GDB parses Rust expressions; LLDB does not, so editors built their own evaluators on top of it.
- **evcxr**, Rust's interactive shell and Jupyter kernel, compiles each snippet into a small library, loads it into a running process and keeps variables between snippets. It shows that compile-and-load evaluation works, at the cost of a compilation per snippet.
- **rust-analyzer** interprets Rust's intermediate representation to evaluate constants in the editor, and **Miri** interprets it to detect undefined behavior: interpretation over checked code is a working alternative to compiling each expression.
- **Dioxus's subsecond** replaces functions in a running Rust process: calls go through a jump table, and only changed code is compiled and linked against the running program. It does not migrate state when a type's layout changes.
- **rr and Pernosco.** rr records only what enters a process and the order of its threads, and replays the rest exactly; it relies on processor facilities on one operating system. Pernosco replays an rr recording once to build a database, so that a developer can click a value and see where it came from.
- **Replay.io** records a browser's inputs and replays them, so that a print statement added after the run shows its output for every iteration. **Elm's and Redux's time-travel debuggers** record only the messages a program received and recompute its state from them.
- **PyCharm's scientific mode** combines a variables view, tabular data viewers and evaluation in the stopped frame. It sets the experience bar for exploring a stopped program.

## Alternatives considered

- **Document host commands only.** Useful as provider documentation, but leaves process attribution, symbol presentation, coverage and capture interchange to each user.
- **Make names prettier only at emission.** Helps new binaries but cannot decode old stacks, preserve unknown symbols or unify source/value inspection. Emission policy and presentation are both needed.
- **Require a full RFC 126 investigation for every failure.** Adds workflow overhead to a single assertion or memory check; ordinary captures can feed investigations when needed.
- **Use one universal event type for everything.** Erases distinctions between compiler facts, samples, leak classifications and user logs. A shared envelope with typed payloads preserves them.
- **Enable all instrumentation by default.** Changes costs, timing and data exposure without a user decision, and makes incomplete providers harder to distinguish.
- **Guarantee every unsafe violation is caught.** Cannot follow from checked declarations or partial runtime observation; the suite must report the evidence it actually has.
- **Show no values in ordinary failures.** Keeps default output smallest, but hides the single most useful fact about a failed comparison, which the program has already computed. Showing them needs no stack capture and no second evaluation.
- **Show the stack and frame values by default.** Contradicts RFC 131's off-by-default stack policy, costs every failing run, and exposes values that only a debugger session or an explicit capture should reveal.
- **Keep Rust and Incan debugging separate.** Simpler to build, but a mixed program would get two stacks, two failure formats and two sessions for one execution, with no stepping across the boundary.
- **Record through processor and operating-system facilities, as rr does.** Records any code completely, but needs specific processors and one operating system. Recording at the boundaries the compiler and runtime own works everywhere.
- **Record every value at every step, as Chronon for Java and GDB's record mode do.** Answers everything, but slows the program many times over, and storage grows with the computation rather than with its inputs.
- **Record only events and call arguments chosen in advance, as .NET IntelliTrace does.** Cheap, but a question nobody anticipated needs a second run, which may not reproduce the failure.
- **Keep only a snapshot of each frame at the failure, as post-mortem debuggers and error reporters do.** Shows the final state, but cannot say where a value came from.
- **Write a debugger engine.** Would remove a dependency, but means separate process-control, debug-information and unwinding layers for each operating system. The suite's value is above the engine.
- **Give agents a separate interface.** Would let the human interface stay unstructured, but splits one debugging fact into two presentations that can drift. Bounded, structured answers serve people's tools as well.

## Drawbacks

Source debugging and observer integration require a maintained platform and toolchain support matrix. Retaining matching source and debug artifacts consumes storage and can expose implementation details. Memory instrumentation and sampling change execution costs, and some faults prevent complete capture. Recording and patchable artifacts are further artifact kinds to build, identify and qualify. Building a recording's provenance index costs one replay of the run, and effects outside the recorded boundaries, such as memory shared with another process or races in unsafe code, can only be reported as divergence. Versioned symbol formats and typed observation outcomes impose compatibility work. The suite also adds command concepts; the benefit depends on keeping ordinary workflows small and unsupported cases explicit.

## Implementation architecture (non-normative)

- **Engine and session.** LLDB, the LLVM project's debugger, ships with the toolchain at the LLVM version the pinned rustc uses and provides process control, breakpoints, memory access and unwinding on Linux, macOS and Windows. Oven owns the session above it: presentation, evaluation, provenance, recording, patching and the agent contract. The terminal client, a Debug Adapter Protocol adapter and MCP tools are clients of that session.
- **Debug information.** Incan code compiles through the pinned rustc, so Incan and Rust frames share one debug-information format, one unwinder and one panic mechanism. Incan source lines and scopes must be carried into that debug information.
- **Values.** Oven reads values from the process through the engine and presents Incan and Rust values from the layout and type facts the compiler writes, not from the engine's own type formatting or per-debugger scripts, so values look the same on every platform.
- **Debug artifacts.** A build stores debug information, source maps and layout facts in the Loaf's entry of the local store; the session reads them from there by artifact identity.
- **Evaluation.** Read-only expressions are interpreted over checked bodies against values read from the stopped process; the same interpreter evaluates pure expressions in the language server. Expressions that call program code are compiled as a small function against the frame's types, by the compiler identity that built the artifact, then loaded into the process and called.
- **Recording and replay.** A recording artifact records at the boundaries where inputs enter: the runtime's host operations, which already carry RFC 104 receipts, calls into foreign code, and thread synchronization. Safe Incan and Rust code has no data races, so the synchronization order is enough to replay threads exactly. After the run, Oven replays the artifact with the recorded results supplied and writes the provenance index to std.db. Static provenance comes from checked bodies and the code graph.
- **Patching.** A patchable artifact routes calls to patchable functions through an indirection table. Applying a change compiles only the changed functions, links them against the running program's symbols and updates the table.
- **Storage.** Captures and recordings are typed records stored through std.db (RFC 165) within the established artifact lifecycle.

## Layers affected

- **Compiler diagnostics and semantic identity:** stable codes, source-anchor projection, declaration provenance, constants that can never satisfy their constraints, and shared rendering contracts.
- **Checked bodies and native lowering:** source scopes, local names, assertion operands, static value provenance, recording at foreign-call and synchronization boundaries, replay, patchable call indirection and generated-frame relationships.
- **Native backend and artifact distribution:** target debug information carrying Incan source lines and scopes, symbol identity, recording and patchable artifacts, matching source/debug artifacts stored with the Loaf, and relocation.
- **Language server:** static provenance, edit-time evaluation of pure expressions and the shared read-only interpreter.
- **Runtime and standard library:** panic/assertion observations, safe value representations, recorded host-operation results and their replay, telemetry correlation and checked foreign boundaries.
- **Platform and Oven tooling:** the debugging session on Linux, macOS and Windows, observer capability discovery, process scope, capture lifecycle and storage, code patching, memory/profiling operations, report projections and the structured agent contract over MCP.
- **Debugger and editor integration:** source-level control across Rust and Incan frames, bounded value inspection, namespace and tabular views, frame evaluation, exact-source validation, the Debug Adapter Protocol adapter and diagnostic parity.
- **Tests and contract tooling:** structured failure observations and support-profile conformance.

## Design Decisions

- Linux, macOS and Windows are supported with one contract. LLDB is the proposed engine on every platform; platform-specific limitations do not change the contract.
- Debug information, source maps and layout facts live with the Loaf in the local store. The session asks the Loaf for them and makes no network request at debug time; a Loaf published without source still carries its debug information.
- A recording keeps what the execution cannot recompute: host-operation results, the clock, randomness, the environment, arguments, foreign-call results and the order of thread synchronization. Values are recomputed by replay, which builds the provenance index once. Answers stay in the project's own code and go into a dependency only on request.
- At most one client controls a session at a time, with explicit handover and a revision on each request that changes the process. A live value handle answers for the stop where it was made. Recorded output and source text are data, not instructions to an agent. RFC 126 refers to sessions, captures and recordings by identity.
