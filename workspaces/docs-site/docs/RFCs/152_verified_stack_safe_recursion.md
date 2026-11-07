# RFC 152: Recursion that never overflows the stack

- **Status:** Planned
- **Created:** 2026-10-17
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 033 (`ctx` and YAML configuration sources)
    - RFC 094 (scoped cleanup obligations)
    - RFC 100 (`std.re`)
    - RFC 103 (secret non-disclosure)
    - RFC 117 (`loaf.toml`)
    - RFC 126 (investigation evidence)
    - RFC 131 (diagnostic provenance)
    - RFC 142 (task groups and task panics)
    - RFC 146 (typed JSON decoding)
    - RFC 158 (incremental XML records)
    - RFC 165 (Crumb encoding)
- **Issue:** [#1290](https://github.com/encero-systems/incan/issues/1290)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Recursive Incan code gets the stack it needs. The compiler knows which calls can recurse, checks the remaining stack at those calls, and continues on a fresh heap-allocated stack segment when the current one runs low, so recursion depth is limited by memory rather than by a fixed thread stack. When a task exceeds its stack budget, it panics cleanly with the recursion cycle and depth, runs its cleanup, and never segfaults. Every standard parser of external data has the same typed depth limit, lifted only on explicit request, so hostile nesting produces an ordinary error instead of unbounded memory use. No annotation is needed for any of this; a library that wants its own limit declares it with `@max_recursion_depth`.

## Core model

1. **Recursion is ordinary:** any function may call itself or take part in mutual recursion, with no annotation.
2. **The stack grows on demand:** at every call that can recurse, the compiler checks the remaining stack and, when it is low, continues on a new heap segment that is released on return.
3. **Each task has a stack budget:** exhausting it is a panic with a report of the recursion cycle, never a crash of the whole process.
4. **Untrusted input has typed limits:** every standard parser of external data, from JSON and XML to regular expressions and query inputs, has a `max_depth` limit and reports excess nesting as a typed error. The default is 128 unless the format specifies its own limit. Only an explicit request lifts it.
5. **Libraries declare their own limits:** `@max_recursion_depth` gives any recursive function a depth limit that returns a typed error or panics, the same mechanism the standard parsers use.

## Motivation

Deep recursion is how real programs walk real data: compilers walk syntax trees, query engines walk plans, decoders walk nested documents, and user-interface frameworks walk component trees. None of these walks is a tail call, because each node is revisited after its children. On a fixed thread stack, a deep enough input crashes the process. In Rust that is an abort with "has overflowed its stack"; in Python it is a `RecursionError` at depth 1000 by default.

The problem shows up in production rather than in tests, because test fixtures are shallow. It is also an attack: a client that sends `[[[[...]]]]` nested a million deep can take down a service that decodes it recursively.

Mature systems already work around it by hand. rustc wraps its recursive passes in `ensure_sufficient_stack`, which uses the `stacker` crate to move to a new heap-allocated stack segment when fewer than 100 KiB remain. serde_json refuses input nested more than 128 levels by default. Go grows goroutine stacks automatically, up to 1 GB on 64-bit systems. Incan can make the first of these automatic and the last two standard.

Tail-call-only schemes, such as Scala's `@tailrec` or Rust's unstable `become`, do not help these cases: tree walks are not tail calls, and a self-call that is a tail call is already a plain loop.

## Goals

- Let recursive Incan code, including mutual recursion and recursion through function values, run to the depth its memory allows.
- Make running out of stack a clean, diagnosable panic instead of a process crash.
- Keep the cost to code that does not recurse at zero, and the cost to recursive calls to one comparison in the common case.
- Give every standard parser of untrusted input the same typed depth limit, lifted only on explicit request.
- Let library authors declare a depth limit on their own recursive functions, returning their own typed error.

## Non-Goals

- Turning recursion into loops, or guaranteeing tail calls between functions.
- Bounding the memory of code that legitimately needs deep recursion, beyond the task's budget.
- Making foreign code safe: a C or Rust library that recurses on its own stack is outside this guarantee.
- Proving termination.

## Guide-level explanation

### Test drive: the Incan compiler checks a deeply nested expression

The type checker walks expressions recursively, as any compiler does:

```incan
# compiler/check.incn
def check_expr(expr: Expr, scope: Scope) -> Result[Type, TypeError]:
    match expr:
        Expr.Binary(left, op, right) =>
            left_type = check_expr(left, scope)?
            right_type = check_expr(right, scope)?
            return binary_result(op, left_type, right_type)
        Expr.Call(callee, args) =>
            callee_type = check_expr(callee, scope)?
            return check_call(callee_type, args, scope)
        Expr.Name(name) => return scope.lookup(name)
```

Generated source, such as a code generator's sum `x0 + x1 + ... + x199999`, parses into a left-nested tree 200,000 levels deep, so `check_expr` recurses 200,000 times. At a few hundred bytes a frame, that is tens of megabytes of stack. On an 8 MiB thread stack the compiler crashes today, and the user sees an abort with no hint of which input caused it.

With this RFC the same code compiles the file. Each time the stack runs low, execution continues on a fresh 1 MiB segment taken from the heap, and the segments are released as the recursion unwinds. The source of `check_expr` does not change.

### Test drive: a runaway recursion stops cleanly

A bug can make recursion endless, for example a symbol that resolves to itself:

```incan
def resolve(name: str, scope: Scope) -> Type:
    match scope.lookup(name):
        Binding.Alias(target) => return resolve(target, scope)   # an alias cycle never ends
        Binding.Type(found) => return found
```

Instead of eating all memory, the task stops at its stack budget, 1 GiB by default, and reports what was repeating:

```text
panic: task main exceeded its 1 GiB stack budget
  repeating: resolve (compiler/resolve.incn:3), 4,194,304 calls deep
  entered from: check_module (compiler/check.incn:40)
  first argument at the deepest call: name = "Config"
```

The panic follows RFC 094, so open `with` blocks still exit. Inside an RFC 142 task group, it is that task's own `Panicked` outcome. A compiler server or an HTTP service therefore loses one request, not the process.

### Test drive: hostile JSON gets a typed error

A service decoding client input never needs a million levels of nesting:

```incan
def create_order(body: str) -> Response:
    match Order.from_json(body):
        Ok(order) => return Response.created(save(order))
        Err(JsonError.TooDeep(path, max_depth)) =>
            return Response.bad_request(f"nesting deeper than {max_depth} at {path}")
        Err(error) => return Response.bad_request(error.message())
```

`from_json` stops at 128 levels by default, the same default as serde_json, and returns `JsonError.TooDeep` before allocating anything for the deeper levels. The stack growth above would also survive the input, but the typed limit stops an attacker from turning deep nesting into memory use.

The same rule holds for every standard parser of external data, each through its own error family: a TOML document, an XML record, a YAML configuration source, a Crumb payload, a regular expression pattern supplied by a user, a form posted to `std.web`, or an IncQL query received by a server.

### Test drive: parsing deep input on purpose

A program that really does need extreme nesting says so, and never gets it by default:

```incan
value = JsonValue.parse(text, max_depth=2_000_000)?   # a higher limit
value = JsonValue.parse(text, max_depth=None)?        # no limit, only ever on purpose
```

The dynamic parsers, such as `JsonValue.parse`, never recurse: they keep their own work stack on the heap, so a million levels costs a few megabytes and never touches the call stack. Typed decoding into a recursive model, such as `Node(next: Option[Node])`, does recurse, so it relies on the stack growth above, and a task that needs more than its budget raises it:

```toml
# loaf.toml: the default for every task of the program
[runtime]
stack-budget = "4 GiB"
```

```incan
handle = spawn(import_archive(text), stack_budget="4 GiB")   # this task only
```

### Test drive: a library sets its own depth limit

A library author whose parser recurses declares the limit on the recursive function itself:

```incan
error ConfigError:
    TooDeep(line: int, max_depth: int) = f"Sections nested deeper than {max_depth} at line {line}."

pub def parse(text: str, max_depth: Option[int] = 64) -> Result[Config, ConfigError]:
    return parse_section(Lexer(text), max_depth)

@max_recursion_depth(max_depth, error=(limit) => ConfigError.TooDeep(line=lexer.line, max_depth=limit))
def parse_section(lexer: Lexer, max_depth: Option[int]) -> Result[Section, ConfigError]:
    section = Section(name=lexer.header()?)
    while lexer.opens_child():
        section.add(parse_section(lexer, max_depth)?)     # level 65 returns Err(ConfigError.TooDeep(...))
    return Ok(section)
```

The call that would be the 65th level does not run its body; it returns the `error=` value, so the library's caller gets an ordinary typed error with the line where nesting went too deep. The limit is read once, when the recursion is entered from outside, so `parse(text, max_depth=None)` lifts it for that call and every level below. When several functions recurse through each other, such as `parse_value`, `parse_array`, and `parse_object`, they form one cycle and share one depth, so decorating one of them is enough.

Without `error=`, reaching the limit is a panic with the same cycle report as the stack budget. That suits an internal algorithm where depth beyond the limit can only mean a bug:

```incan
@max_recursion_depth(10_000)
def resolve_imports(module: Module, graph: ModuleGraph) -> None:
    for child in graph.imports(module):
        resolve_imports(child, graph)
```

The standard parsers meet the `max_depth` convention the same way.

## Reference-level explanation

### Which calls are checked

The compiler must determine, from the whole program's call graph, every call that can lead back to a function already on the stack: calls within a strongly connected component of the call graph, including direct self-calls and mutual recursion. It must also treat every call through a function value, closure, or dynamically dispatched method as a call that can recurse. Every such call must be preceded by a stack check. Calls the call graph proves cannot recurse must not be checked.

### Growing the stack

A stack check must compare the remaining stack of the current thread with a red zone of at least 100 KiB. When the remaining stack is smaller, the call must run on a new stack segment of at least 1 MiB allocated from the heap, and must return to the previous segment when it returns, normally or by panic. A released segment may be kept for reuse by the same thread, so a call that repeatedly crosses one segment boundary does not repeatedly allocate. Stack growth must preserve the semantics of the call: evaluation order, ownership, cleanup, and the values returned.

Async functions follow the same rule for the stack used while they run or are polled; their suspended states live on the heap and are unaffected.

### Stack budget

Each task, including the main task and each spawned task, must have a stack budget covering its original stack and every segment it has added. The default must be 1 GiB. A program must be able to set its default with `[runtime] stack-budget` in `loaf.toml`, and spawning a task must accept `stack_budget=` for that task. Both take a byte size such as `"512 MiB"` or `"4 GiB"`; an unbounded budget is not offered, so a runaway recursion always ends in a report rather than in exhausting the machine's memory.

When a check would exceed the budget, the task must panic with a `StackBudgetExceeded` report. The report must include the budget, the depth reached, the repeating cycle of functions with source locations, and the function that entered the cycle. Values in the report must follow RFC 103 redaction. The panic must follow RFC 094 cleanup and RFC 142 task-panic rules; it must not abort the process unless the panic itself is not caught anywhere in the process, as for any other panic.

Stack exhaustion in Incan code must never be reported as a segmentation fault or an abort without a report.

### Foreign code

Before calling foreign code, the compiler must ensure at least the red zone of remaining stack. Recursion inside the foreign code is outside this guarantee.

### Depth limits for untrusted input

Every standard parser or decoder that builds nested structure from external data must follow one convention:

- It must accept `max_depth: Option[int]`. The default must be 128 levels unless the format's own RFC sets another; `None` must lift the limit, and only an explicit `None` may do so.
- Exceeding the limit must produce a `TooDeep` case in that format's own error family, carrying the format's structured location of the point where the limit was reached and the limit itself, and must be reported before allocating for the deeper levels.
- A parser that produces a dynamic value, such as `JsonValue`, must not recurse on the call stack; it must keep its nesting on a heap work stack, so its depth costs memory proportional to the input and nothing else. Typed decoding into recursive models may recurse and relies on stack growth and the task budget.

The convention covers at least `std.json` (RFC 146), `std.toml`, XML (RFC 158), YAML configuration sources (RFC 033), Crumb decoding (RFC 165), `std.re` pattern parsing (RFC 100), where the default is 250 levels of group nesting as in Rust's `regex` crate, `std.web` request bodies such as forms and multipart payloads, and IncQL query text. A parser of source code the program's own author wrote, such as the Incan compiler parsing a project, is not external data in this sense: it relies on stack growth and the budget instead of a depth limit. The same parser fed text from outside, such as an IncQL query from a network client, applies the limit.

### Declared recursion limits

`@max_recursion_depth(limit, error=...)` must be a built-in decorator applicable to any function or method. Its depth must count the activations, on the current task, of the decorated function together with every function in the same strongly connected component of the call graph, so mutual recursion shares one depth.

`limit` must be an expression of type `int` or `Option[int]` that may refer to the function's parameters. It must be evaluated when the recursion is entered from outside its cycle, and that value must govern every nested activation until the outermost one returns. A positive `int` sets the limit; `None` must remove it, and only an explicit `None` may do so.

When a call would exceed the limit, its body must not run. With `error=`, it must instead return `Err` of the value the `error=` closure builds from the limit; the closure may refer to the arguments of the refused call, and the compiler must check that the function returns `Result[T, E]` and that the closure produces an `E`. Without `error=`, it must panic with a report of the limit, the repeating cycle, and the entry point, following the same rules as `StackBudgetExceeded`. A declared limit must never be overridden by stack growth: the call is refused even when stack remains.

The standard parsers covered by the depth convention must implement their typed `max_depth` limits either through this decorator or, for parsers that keep their nesting on a heap work stack, through an equivalent check with the same observable behavior.

### Diagnostics and tooling

Investigation tooling under RFC 126 must show segment switches as ordinary call frames, so a stack trace lists source calls rather than segment boundaries. Profiles should report the number of segments a task allocated.

## Design details

### Why this is not the segmented stacks Rust removed

Early Rust and early Go used segmented stacks for every function, and both abandoned them because of the "hot split": a loop calling a function right at a segment boundary allocated and freed a segment on every iteration. This design checks only calls that can recurse, uses large segments, and keeps a released segment for reuse, so the common path is one comparison and a boundary crossing does not allocate repeatedly. It is the approach rustc applies by hand with `stacker`, made automatic.

### Why a panic and not a `Result`

Exhausting a 1 GiB stack budget means a runaway recursion or a missing input limit, both of which are bugs. A `Result` would force every recursive function to carry an error it cannot meaningfully handle. A panic with a precise report, isolated to its task, is the right signal for a bug; typed input limits are the right signal for bad input.

### Prior art

- rustc's `ensure_sufficient_stack`, built on the `stacker` crate, with a 100 KiB red zone and 1 MiB segments.
- Go's growable goroutine stacks and their 1 GB default maximum.
- serde_json's default recursion limit of 128.
- Python's `RecursionError`, a fixed depth limit that refuses legitimate deep inputs.

## Alternatives considered

- **`@stack_safe` for tail-recursive self-calls:** covers only calls that are already loops and misses the tree walks that overflow in practice.
- **Guaranteed tail calls between functions, like Rust's `become`:** helps interpreters and state machines, which Incan writes idiomatically as a loop over a `match`, and would rest on an unstable Rust feature or a whole-program rewrite.
- **A fixed recursion depth limit, like Python:** refuses legitimate deep inputs and still needs each program to tune it.
- **Grow every function's stack:** reintroduces the hot-split cost for code that never recurses.
- **Report stack exhaustion as a `Result`:** adds an unhandleable error to every recursive signature.
- **Leave it to the platform:** keeps the crash.
- **Thread a depth counter through every recursive call by hand:** works, but every library repeats it and a forgotten increment silently removes the limit; `@max_recursion_depth` declares it once.

## Drawbacks

- Recursive and indirect calls pay one comparison each, and a deep recursion pays for heap segments.
- A program with a genuine need beyond 1 GiB of stack must raise its budget.
- The call graph must be whole-program to find mutual recursion, which constrains separate compilation to publishing each unit's call-graph facts.
- Foreign code keeps its own stack limits.

## Layers affected

- **Typechecker:** `@max_recursion_depth` limit and `error=` closure checking against the function's `Result` type.
- **Call-graph analysis:** whole-program recursion detection, including indirect calls, with call-graph facts published per unit.
- **Lowering / native runtime:** stack checks at recursive call sites, segment switching, segment reuse, per-task budgets, and the `StackBudgetExceeded` panic report.
- **Async runtime and task spawning:** per-task budgets and the poll path.
- **Stdlib:** the `max_depth` convention and iterative dynamic parsers across `std.json`, `std.toml`, XML, YAML sources, Crumb, `std.re`, `std.web` request bodies, and IncQL query text; `spawn(..., stack_budget=)`.
- **Manifest schema:** `[runtime] stack-budget` in `loaf.toml`.
- **Diagnostics / investigation tooling:** cycle-compressed panic reports and stack traces without segment noise.

## Design Decisions

- Recursion needs no annotation; `@stack_safe` and guaranteed cross-function tail calls are not part of this design.
- `@max_recursion_depth(limit, error=...)` is a built-in decorator that limits the shared depth of a function's recursion cycle. The limit, which may come from a parameter and is lifted only by an explicit `None`, is read when the cycle is entered from outside. A refused call returns the `error=` value as a typed `Err`, or panics with the cycle report without `error=`, and stack growth never overrides it. Standard parsers implement their `max_depth` limits with it or an equivalent check.
- Every call that can recurse, determined from the whole-program call graph and including indirect calls, checks the remaining stack against a red zone of at least 100 KiB and continues on a heap segment of at least 1 MiB when it is low, with segment reuse to avoid hot splits.
- Each task has a stack budget, 1 GiB by default, set per program and per spawned task. Exceeding it is a `StackBudgetExceeded` panic that reports the repeating cycle, depth, and entry point, follows RFC 094 cleanup and RFC 142 task-panic isolation, and never segfaults.
- Every standard parser of external data accepts `max_depth: Option[int]`, 128 by default (250 for regular-expression group nesting), lifted only by an explicit `None`, and reports a `TooDeep` case in its own error family with a structured location. Dynamic parsers keep nesting on a heap work stack and never recurse. Source code a program's own author wrote relies on stack growth instead.
- The stack budget is set with `[runtime] stack-budget` in `loaf.toml` and `spawn(..., stack_budget=)` per task; there is no unbounded budget.
- Foreign code gets at least the red zone and is otherwise outside the guarantee.
