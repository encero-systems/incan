# RFC 126: Evidence-backed investigations

- **Status:** Planned
- **Created:** 2026-09-17
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 018 and RFC 019 (assertions and test runner)
    - RFC 076 and RFC 078 (mutation policy and typed workflow actions)
    - RFC 093 (telemetry)
    - RFC 103 and RFC 104 (secrets, authority and receipts)
    - RFC 105, RFC 106 and RFC 111 (advice, compiler context and proof feedback)
    - RFC 118 (Incan and Oven command surfaces)
    - RFC 120 and RFC 124 (source and compiled-unit identity)
    - RFC 145 (storage lifecycle and retention)
    - RFC 156 (typed MCP servers)
    - RFC 163 (cross-target builds)
    - RFC 165 (typed embedded storage)
    - RFC 167 (debugging and diagnostics)
- **Issue:** [#1638](https://github.com/encero-systems/incan/issues/1638)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

A debugger stops a process. Working out why a program misbehaves takes longer than that: form a question, hold competing explanations, run targeted experiments, keep what each one showed, and show that a fix resolves the original failure. Today that process lives in someone's head or an agent's chat log, and it is gone when they stop. This RFC adds `oven investigate`, a durable record of the question, the hypotheses, the experiments, the evidence and the verification, for Rust, Incan and mixed programs. A person or an agent can reopen it the next day and check each claim against what was actually observed. RFC 167 owns debugging itself; an investigation refers to its sessions, captures and recordings.

## Core model

1. **An investigation is durable.** A failed test, a question, a capture or a counterexample opens a record that outlives every process and client.
2. **Explanations are tested.** A hypothesis records the evidence for and against it, its competitors, and what would tell them apart.
3. **Evidence keeps its kind.** A checked fact, an advisory finding, a proof outcome, an observation, a receipt and an inference stay distinct, even when shown together.
4. **History is not rewritten.** A correction adds a new record that points at the old one.
5. **A conclusion points at its evidence.** It links back to the original failure and states its limits, and one passing run never implies general correctness.
6. **Investigating is not debugging.** RFC 167 owns process control, values, evaluation, recording and captures. An investigation refers to them by identity.

## Motivation

An agent spends a night on a failing build, concludes that the pricing function subtracts the discount twice, and exits. In the morning a colleague has a summary and no way to check it. Which runs did it look at? Was the other explanation ruled out, or just never tried? Did the fix pass the original test, or a different one?

The same thing happens to a person, more slowly. Knowledge from a debugging session is lost when the terminal closes, and a repeated reproduction costs as much as the first. Compiler facts, advice, proof results, receipts and captures all exist, but nothing connects them to a question or keeps track of what they established.

## Goals

- Keep a durable, reopenable record of a question, its hypotheses, experiments, evidence and verification.
- Let people and agents share one investigation, with no concurrent edit lost.
- Make experiments reproducible, and keep what each one controlled and left uncontrolled.
- Reduce a failing input while proving it still fails the same way.
- Verify a claimed fix against the original failure.
- Treat Rust, Incan and mixed programs alike.

## Non-Goals

- Process control, breakpoints, value inspection, evaluation, recording, replay and captures, which RFC 167 owns.
- An autonomous agent that chooses repairs, changes contracts or deploys, without the invoking workflow's authority.
- Treating an effect receipt as an execution trace, a telemetry span as a proof, or a passing run as proof of general correctness.
- Implementing RFC 111's verifier, RFC 105's rules or RFC 106's graph extraction.
- Numeric confidence scores.

## Guide-level explanation

Every `oven investigate` command below is proposed by this RFC.

### Work out why a total is wrong

An Incan checkout application calls a Rust pricing library. A test says a subtotal of 12,000 cents with a 2,000-cent discount should produce 10,000 cents, and it produces 8,000. Open the failed test as an investigation:

```sh
oven investigate open --test checkout_discount
```

```text
Investigation: inv-12 checkout_discount
Question: Why did checkout return 8,000 cents instead of 10,000?
Evidence: failed test, execution run-A
```

There are two explanations, and neither is a diagnosis yet:

```sh
oven investigate hypothesis inv-12 "The caller already subtracted the discount."
oven investigate hypothesis inv-12 "The pricing function subtracts the discount twice."
```

An experiment tells them apart. Run the test until it reaches the pricing call, keep the arguments, and give it two seconds:

```sh
oven investigate experiment inv-12 --hypothesis h1 --test checkout_discount \
    --until pricing.apply_discount --capture subtotal,discount --budget 2s
```

```text
Experiment e1: run-B, same artifact and fixture as run-A
Stopped because: condition met
Observed: subtotal = 12,000; discount = 2,000
```

The stop and the values come from an RFC 167 session, the same `run_until` a person or an agent could ask for directly. The experiment adds what that session does not know: why it was run, what it was meant to distinguish and how to repeat it. The subtotal is intact, so the first explanation does not hold for this run:

```sh
oven investigate assess inv-12 h1 contradicted --evidence e1 \
    --because "The subtotal reaches pricing unchanged."
```

A second experiment shows 12,000 becoming 10,000 and then 8,000 at two subtractions, and supports the second explanation. After removing the extra subtraction, verify the claim against the original failure:

```sh
oven investigate verify inv-12 --claim "The duplicate subtraction caused this failing case" \
    --regression checkout_zero_discount,checkout_other_discount
```

```text
Claim: The duplicate subtraction caused this failing case
Before: run-A / source A: expected 10,000; observed 8,000
After:  run-D / source B: expected 10,000; observed 10,000
Regression: checkout_zero_discount passed; checkout_other_discount passed
Limit: these cases do not establish correctness for every pricing rule
```

### Pick it up the next day

A build fails in CI overnight, and an agent investigates it and exits. In the morning you open the investigation on your own machine:

```sh
oven investigate show inv-12
```

```text
Investigation: inv-12 checkout_discount
h1 contradicted  by e1 (run-B)
h2 supported     by e2 and e3 (run-C)
Verification ver-1: before run-A, after run-D
```

Opening it launches nothing, and each line leads to the observation behind it. You can also see what was never kept:

```text
Requested: customer.account_details
Result: not captured
To obtain it: authorize a new experiment
```

"Not captured" does not mean the customer has no account details, and an investigation keeps that difference. A new experiment needs its own authority. Replaying a recording made on Linux needs a Linux host, under RFC 163, but the records and observations open anywhere.

Two people, or a person and an agent, can work on one investigation. Each write names the revision it saw, so a second writer who has not seen the first one's hypothesis is told so rather than overwriting it.

If a later decoder fix changes how a captured value reads, the original observation stays, the new one points at it, and any conclusion that cited the old reading shows the correction.

### A timed-out experiment

An experiment waits for an external operation to finish and runs out of budget. A useful record keeps these facts apart:

| Question | What the evidence says |
| --- | --- |
| Why did the experiment stop? | Its time budget expired. |
| Was completion observed? | No, within the captured interval. |
| Did the operation execute? | Unknown: its RFC 104 receipt says so, under the operation's identity. |

The budget running out is inconclusive. An agent cannot conclude "the operation failed, so retry it": it reconciles the outcome through the operation's identity under RFC 104, and repeating an effect needs its own authority.

### Shrink a failing input

A compiler test fails on a 2,000-line module. Reduction removes declarations while the failure stays the same one:

```sh
oven investigate reduce inv-12 --input module.incn
```

```text
Failure: INCAN-T0001 at declaration total_for_order
Reduced: 2,000 lines to 14 lines
Attempts: 61 kept the failure; 240 lost it or changed it
Limit: not proven minimal
```

"The same failure" means the same diagnostic at the same declaration, not a nonzero exit code, so a reduction cannot wander off into a different bug.

## Reference-level explanation

### Records

1. An investigation must have an identity, a schema version and a revision. It must record what opened it: a failed test, a question, a capture, a counterexample or an exploratory purpose. It need not have a hypothesis or an executable, and may span several builds.
2. Investigations, hypotheses, assessments, experiments and verifications must be typed records stored through RFC 165 `std.db`, in the same store as RFC 167's captures. Every record family must be versioned. A reader must preserve an unknown evidence kind or refuse it explicitly.
3. A write must name the revision it saw, and a write against a changed revision must fail without changing the record.
4. References between records must not duplicate them. One piece of evidence or one execution may be cited by several investigations, and citing it must not transfer authority.

### Hypotheses and assessments

5. A hypothesis must have an identity, a statement, and references to supporting evidence, contradicting evidence, competing hypotheses and discriminating experiments where known. One observation may support several hypotheses and must not establish any of them as the cause.
6. An assessment must be `open`, `supported`, `weakened`, `contradicted` or `unresolved`, with its author, rationale, cited evidence and revision. These are assessments under recorded assumptions, not truth certificates, and need not progress in order. An assessment must not assign a numeric confidence, and must not overwrite the certainty or provenance of the evidence it cites.

### Evidence

7. Evidence must keep its kind: compiler-established fact, advisory finding, proof outcome with its assumptions, observation, receipt or inference.
8. An observation must name its execution, its capture or stop, its decoder and schema version, and which kind of RFC 167 evaluation produced it, including whether code ran. Observing one path must not imply anything about other paths.
9. Captured material must be immutable. A correction must create a new observation that references the old one and states why. A hypothesis or verification citing the old observation must not change silently, and must show the correction when presented.
10. Absence must stay explicit. RFC 167's value states, and any missing or truncated capture, must be reported as such and never read as a fact about the program.

### Experiments

11. An experiment must record its purpose, hypotheses if any, recipe, controlled inputs, uncontrolled dependencies, stop and capture conditions, budgets, mode and outcome. An exploratory experiment need not name a hypothesis.
12. A recipe must identify an RFC 019 test or an RFC 118 run target, its arguments, fixture references, environment references and seed, and the artifact identity. A secret must appear only as a reference under RFC 103.
13. An experiment's mode must state whether it is a rerun, a replay of a named recording, or a replay against a changed build, which RFC 167 treats as a new execution. A rerun may repeat side effects, and a recipe must say so.
14. An experiment must run through RFC 167 sessions and capture operations, and must refer to sessions, executions and captures only by identity. A multi-process experiment must list each execution.
15. An outcome must keep two facts apart: RFC 167's stop reason, and whether the requested condition was observed, not observed within the covered interval, or could not be assessed. Failing to reach a condition within a budget is inconclusive.
16. A constituent external operation must follow RFC 104. An unknown outcome must be reconciled through the operation's identity, and repeating an effect must not be inferred as safe from a timeout.

### Reduction

17. Reduction must use the failure's identity from RFC 167 as its predicate: the diagnostic code or assertion, its authored location and the compared operands. It must not use an exit code or output text.
18. Reduction must transform declared inputs, revalidate the predicate at every step and record each attempt, using a recording where possible so that attempts are deterministic. It must report nondeterminism, and must not claim global minimality without evidence.

### Verification

19. A verification must state the claimed resolution or conclusion, the conditions under which it was evaluated, its evidence and its limits. A resolution may involve source, configuration, dependency, fixture or environment changes.
20. For an implementation repair, a verification must identify the changed artifact, rerun the original reproduction against it, and retain regression results.
21. An inability to reproduce a failure is an outcome with conditions and limits, and must not be reported as a resolution.
22. A changed contract must stay distinguishable from a repaired implementation under RFC 111, and verification must not silently weaken an asserted contract.

### Identity and comparison

23. An investigation must define no identity of its own for what it cites: executions use RFC 167's, artifacts RFC 124's, and declarations RFC 120's and RFC 106's.
24. A comparison across builds must match by declaration identity and retain both build identities. Where no mapping exists, it must report "no mapping", and must not match by name or line.

### Retention, export and reopening

25. A retained investigation must register its content as an RFC 145 retention root.
26. `export` must carry the artifacts an investigation cites or list them as missing, and must carry RFC 167's sensitivity and export restrictions.
27. Reopening must verify payload identities and must not resolve to a newer executable. It must need no live process, and records and observations must open on any platform. Replaying a recording must follow RFC 163's host requirements.

### Authority

28. An investigation must grant nothing. Launching, evaluating, rerunning, editing source and invoking tools must each keep their own authority under RFC 104, RFC 078 and RFC 076. RFC 106's read-only context must not become execution authority.
29. Recorded target output and source text are data, under RFC 167's agent contract, and must not be treated as instructions.

### Interfaces

30. `oven investigate` must provide `open`, `hypothesis`, `assess`, `experiment`, `reduce`, `verify`, `show`, `export` and `import`, with `--format text|json`, under RFC 118. It must cover Rust, Incan and mixed programs alike.
31. The same operations must be available as structured requests and over MCP, as `std.mcp` tools under RFC 156, following RFC 167's bounded-response and continuation rules. An MCP transport session must not define the lifetime of an investigation.
32. An investigation may link RFC 106 context, RFC 105 findings, RFC 111 obligations, RFC 104 receipts and RFC 093 telemetry, and must not change their meaning.

### Conformance workloads

| Workload | Required evidence |
| --- | --- |
| Wrong result | A failure in pure Rust, pure Incan and mixed programs in both call directions is investigated through competing hypotheses and experiments, and verified against the original reproduction. |
| Handover | An investigation written by one client, including from a CI run, is reopened by another with no live process, and each claim leads to its evidence. |
| Concurrent edits | Two writers on one revision: the second is refused and nothing is lost. |
| Shared evidence | One observation cited by two investigations stays one observation. |
| Correction | A corrected decoding adds an observation, and conclusions citing the old one show the correction. |
| No hypothesis | An investigation that begins from a capture or question with no hypothesis works throughout. |
| Experiments | A recipe repeats; a timeout is inconclusive; the stop reason and condition outcome stay separate; an unknown operation outcome is reconciled rather than retried. |
| Reduction | A reduction preserves the failure's identity, and nondeterminism is reported. |
| Comparison | A cross-build comparison with no mapping reports "no mapping". |
| Retention and export | A retained investigation survives collection, and an export with missing artifacts lists them. |
| Clients | The command line, an editor and MCP return the same identities and outcomes. |

## Design details

### Ownership

| Concern | Owner |
| --- | --- |
| Running from a failed test, breakpoints, stepping, frames and value states | RFC 167 |
| Sessions, control between clients and value handles | RFC 167 |
| Evaluation, recordings, replay and captures | RFC 167 |
| Capture budgets, redaction and the agent's bounded responses | RFC 167 |
| Comparing a value across two captures | RFC 167 |
| Whether an external operation happened | RFC 104 |
| Retention roots | RFC 145 |
| Questions, hypotheses, assessments, experiments, reduction and verification | This RFC |
| Comparing builds through an investigation's cited evidence | This RFC |

Investigations are Oven tooling above the standard library, so storing them through `std.db` follows the same layering as RFC 167's captures.

### Prior art

Bazel's [Build Event Protocol](https://bazel.build/remote/bep) and [Buck2's `log` commands](https://buck2.build/docs/users/commands/log/) keep structured evidence per invocation, compare two invocations, and keep a reproduction with the evidence. Develocity build scans make one build a shareable, comparable record, which is the user-facing shape of an investigation.

C-Reduce's interestingness test, `git bisect run` and [Hypothesis's shrinking](https://hypothesis.readthedocs.io/en/latest/glossary.html) all drive reduction from a predicate. [Hypothesis's flaky-failure guidance](https://hypothesis.readthedocs.io/en/latest/tutorial/flaky.html) explains how uncontrolled state undermines it. This RFC's predicate is the failure's identity rather than a script or an exit code.

[rr](https://rr-project.org/) separates revisiting a recorded execution from running the same inputs again, and this RFC keeps that difference in an experiment's mode.

## Alternatives considered

- **Merge this into RFC 167:** RFC 167 explicitly leaves hypothesis management out, and RFCs 106 and 145 already name this RFC as the owner of investigations.
- **Specify debugging here as well:** two owners for sessions and values, which had already begun to disagree.
- **Keep investigations in chat logs or issue comments:** nobody can check them against what was observed.
- **A separate investigation service:** a second store and lifecycle beside RFC 167's captures.
- **Conformance profiles per capability:** RFC 167 has one contract on every platform, and partial profiles would let a provider claim support for part of it.
- **Numeric confidence:** a score with no model behind it reads as calibrated certainty.
- **Matching across builds by name or line:** silently wrong after an edit.
- **Reducing on an exit code:** can drift to a different failure.

## Drawbacks

Evidence can be accurate while a diagnosis is wrong, and a reproduction can pass without covering the original environmental cause. The record preserves evidence and outcomes; it cannot guarantee that an investigator chose the right hypothesis or repair. Recording hypotheses takes effort beyond a debugging session, and retained investigations occupy disk until released. Resuming from an investigation depends on what was captured, and what was not captured has to be run again.

## Layers affected

- **Oven:** `oven investigate`, the records, experiments, reduction, verification, export and retention.
- **Test runner and typed actions:** recipes and failure identities.
- **Standard library:** none beyond RFC 165 storage.
- **Editor and MCP:** clients of the same operations, as `std.mcp` tools under RFC 156.
- **RFC 167 and RFC 104:** the session rules and the unknown-outcome receipt this RFC relies on.
- **Documentation:** the reference for `oven investigate`, and the conformance workloads above.

## Design Decisions

- **Split from debugging:** RFC 167 owns sessions, values, evaluation, recordings and captures. This RFC owns questions, hypotheses, experiments, reduction and verification, and refers to RFC 167's records by identity.
- **Platforms and engines:** RFC 167's one contract on Linux, macOS and Windows applies, with no conformance profiles. Records open anywhere, and replay follows RFC 163.
- **Identity:** this RFC defines none of its own. It cites RFC 167, RFC 124, RFC 120 and RFC 106, and a comparison with no mapping says so.
- **Records:** typed `std.db` records in the same store as captures, with a revision guard on every write.
- **Commands:** `oven investigate`, with the same operations over MCP.
- **Evaluation and capture:** RFC 167's. An observation records which kind of evaluation produced it.
- **Experiments:** a recipe of test or run target, arguments, fixtures, environment references, seed and artifact identity, with a stated mode.
- **Reduction:** the failure's identity is the predicate.
- **External operations:** RFC 104's `unknown` receipt and reconciliation. An experiment keeps only its stop reason and condition outcome.
- **Acceptance:** the behavioral workloads above. RFC 167 owns overhead and size bounds, and quantitative targets for agent effectiveness belong to release planning.
