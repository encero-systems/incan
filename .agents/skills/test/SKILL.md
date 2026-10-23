---
name: test
description: Run and write tests for the Incan compiler. Use when the user asks to test changes, add tests, run the test suite, verify a feature, or says /test. Guides test selection, provides correct patterns, and runs the right commands.
---

# Test — Incan Compiler

## Step 1: Determine what changed

Identify which compiler stages are affected by the current changes:

| Change area | Stages affected |
| --- | --- |
| New syntax or keyword | Parser, Typechecker, Lowering, Emission |
| New semantic rule or validation | Typechecker |
| New or changed codegen output | Lowering, Emission |
| Stdlib addition | Typechecker (loader), Emission, possibly Runtime |
| CLI behavior | Project, testing, or behavior contracts |
| Formatter change | Format contracts (exact text, check immutability, idempotency) |

For non-trivial changes, do a quick pattern intake before writing or changing tests:

- Identify the active area and downstream stages the behavior must survive.
- Read 2-3 nearby tests or fixtures that already cover a similar behavior shape.
- Name the source-of-truth boundary, such as the RFC, diagnostics catalog, stdlib registry, ownership policy, CLI reference, or docs contract.
- Use `incan inspect codegraph PATH --query defs TERM` or `--query callers TERM` for declaration and call relationships before grep.
- Pick the narrowest failing test first, then the downstream verification that proves the behavior reaches the right stage.

## Step 2: Pick the right tests to write

### Decision tree

```text
Did you change syntax, semantic validation, or a diagnostic?
  → Extend a check contract for acceptance or stable diagnostic codes

Did you change lowering or runtime behavior?
  → Extend a behavior contract that runs the program and checks output

Did you change imports across Loaves or Rust interop?
  → Extend a library or interop contract at that boundary

Did you change unit description, test execution, or formatting?
  → Extend a project, testing, or format contract respectively

Did you change codegraph inspection?
  → Extend a graph contract
```

### What to always do

Follow `AGENTS.md`'s test budget: the whole suite stays under about 3,000 tests, including contract cases, Rust unit tests, and Incan test functions. Extend an existing case in the same area before adding one; a new case is justified only when none can carry the behavior. Never add a case per bug: name the guarded bug in its `# behavior:` line. A library case has one producer Loaf carrying every declaration it needs.

For pipeline features, prove semantic acceptance/refusal and observable execution where relevant. Do not demand a second unit test when an existing contract already exercises the behavior without a concrete coverage gap.

## Step 3: Write the tests

### Acceptance contract pattern

Cases live under `workspaces/contracts/fixtures/<family>/<area>/`. Read `workspaces/contracts/README.md` for the permitted layouts and headers. Extend the nearest case; a case states what a user observes and never names a route or build step.

```incan
# behavior: A function may return its declared string type
# expect-accepted:

def example() -> str:
    """Return a string with the declared type."""
    return "hello"
```

For refusals, use `# expect-diagnostic: <CODE>` with the actual stable code from the diagnostics catalog. Do not mix diagnostic expectations with run expectations. Cover accepted and refused forms where the semantic boundary requires both.

**Important**: test functions that do fallible work must return `Result` and use `?`. Never use `.unwrap()` or `.expect()`.

### Execution contract pattern

```incan
# behavior: A function result is printed by its caller
# expect-stdout:
#   hello

def example() -> str:
    """Return the caller's greeting."""
    return "hello"

def main() -> None:
    """Print the function result."""
    println(example())
```

### Boundary contract pattern

Use a library project case for path dependencies and imported declarations, with one producer Loaf carrying all required declarations. Use an interop case for `rust::` imports. Put assertions at the consumer boundary rather than relying on checker acceptance alone.

### Tooling contract pattern

The `project` family compares unit descriptions; `testing` checks per-test outcomes; `format` checks exact text, check immutability, and repeated-format stability; `graph` checks inspection results and invalidation. Reuse their existing cases and supported header directives from the contracts README.

## Step 4: Run the tests

### During development (targeted)

```bash
# Replace INCAN with the compiler under test and SELECTOR with family/area/name or family/area
incan run workspaces/contracts/src/main.incn -- INCAN workspaces/contracts/fixtures SELECTOR
```

### At commit points (affected whole families)

```bash
# Replace FAMILY with the affected whole-family selector; pass several when needed
incan run workspaces/contracts/src/main.incn -- INCAN workspaces/contracts/fixtures FAMILY

# Rust formatting and fast local gate; these do not run contracts
make fmt
make pre-commit-fast
```

`make fmt-check`, `make us-english-check`, and `make rustdoc-gate` are also available separately. Run `make us-english-check` before every commit. Oven builds and runs units; never run Cargo. Verify commands against the Makefile and `workspaces/oven/src/main.incn`.

## Step 5: Verify

After tests pass, check:

- [ ] No `.unwrap()` or `.expect()` in test code (clippy will reject them)
- [ ] Test functions that do fallible work return `Result`
- [ ] Observable expectations match the language contract; baseline passing cases still pass
- [ ] Both valid and invalid cases are covered for new diagnostics
- [ ] Existing cases were extended before adding any, and the suite remains under about 3,000 tests
- [ ] Guarded bugs appear in `# behavior:` and each library case has one producer Loaf

## Rust compiler error workflow

When the task is to debug a Rust compiler error, collect the full context before changing code:

- exact command that failed;
- full diagnostic text, including error code, notes, help text, and secondary spans;
- active features or build mode, especially default build vs. `rust-metadata`;
- relevant function signature and nearby type definitions;
- local precedent files or tests that show the intended pattern.

Classify the root cause before proposing a fix: lifetime/borrow across an await or compiler boundary, missing trait bound, feature-gate/build-mode mismatch, orphan/coherence rule, missing import, or incomplete parser/typechecker/lowering/emission wiring. Prefer fixing the owning boundary over adding local conversion or clone workarounds.
