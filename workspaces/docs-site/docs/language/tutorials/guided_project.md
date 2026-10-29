# Guided project: carry one workflow through the fundamentals

The Incan Book teaches language concepts in small programs. This guided spine gives those concepts one cumulative destination: the [pipeline mini-project](../../tooling/tutorials/pipeline_mini_project.md).

<aside class="inc-tutorial-meta" aria-label="Tutorial details">
  <dl>
    <div><dt>Reader</dt><dd>Learner who wants continuity between Book chapters</dd></div>
    <div><dt>Prerequisites</dt><dd>Getting Started completed</dd></div>
    <div><dt>Time</dt><dd>1–2 focused sessions</dd></div>
    <div><dt>Verified</dt><dd>Incan <code>0.6.0-dev.8</code></dd></div>
    <div><dt>Status</dt><dd>Development-toolchain executable</dd></div>
    <div><dt>Outcome</dt><dd>A deterministic typed workflow step</dd></div>
    <div><dt>Artifacts</dt><dd>Manifest, public step module, tests and native binary</dd></div>
  </dl>
</aside>

<ol class="inc-step-rail" style="--inc-step-count: 5" aria-label="Guided project stages">
  <li><strong>Shape</strong>Typed transformation</li>
  <li><strong>Fail</strong>Explicit Result</li>
  <li><strong>Organize</strong>Project boundary</li>
  <li><strong>Test</strong>Both outcomes</li>
  <li><strong>Deliver</strong>Run and build</li>
</ol>

## Stage 1: shape one transformation

Read [values, variables, and types](book/02_values_variables_and_types.md), [functions](book/03_functions.md), and [control flow](book/04_control_flow.md). Begin with one pure, typed operation:

```incan
def normalize_name(name: str) -> str:  # (1)
    return name.strip().lower()
```

1. The signature makes the step's input and output visible before any execution or orchestration is introduced.

This is deliberately small. The project becomes useful by strengthening its boundary, not by adding framework machinery.

## Stage 2: make rejection explicit

Read [errors with Result, Option, and `?`](book/06_errors.md). A blank name is not a successful normalized value, so move that fact into the return type:

```incan
pub def normalize_name(name: str) -> Result[str, str]:  # (1)
    if len(name.strip()) == 0:
        return Err("name must not be empty")  # (2)
    return Ok(name.strip().lower())
```

1. `Result[str, str]` requires callers to acknowledge both the useful value and the failure.
2. The failure travels as data; the step does not print, exit, or mutate ambient state.

At the terminal boundary, one `match` is clearer than a combinator chain because both branches intentionally produce visible output:

```incan
def main() -> None:
    match normalize_name("  Alice  "):
        case Ok(value): println(f"ok: {value}")
        case Err(error): println(f"err: {error}")
```

## Stage 3: organize the project

Read [modules and imports](book/05_modules_and_imports.md), then follow the mini-project's [manifest-backed layout](../../tooling/tutorials/pipeline_mini_project.md#step-1-create-a-file). Keep the public step and terminal `main` in `src/pipeline_step.incn`; point `[project.scripts] main` at that file.

The source path selects the entry point. Tests import the public function as `from pipeline_step import normalize_name`; they do not climb directories or duplicate the function.

## Stage 4: test both contracts

Read [unit tests](book/13_unit_tests.md), then add the mini-project's [success and failure tests](../../tooling/tutorials/pipeline_mini_project.md#step-3-test-it).

Run the complete test gate:

```bash
incan test tests/test_pipeline_step.incn
```

A useful workflow test proves both the ordinary value and the rejected input. It does not merely assert that the program starts.

## Stage 5: deliver the program

Run and build the completed project:

```bash
incan run src/pipeline_step.incn
incan test tests/test_pipeline_step.incn
incan build src/pipeline_step.incn
```

The same source and manifest drive local execution, tests and the native artifact.

<section class="inc-learning-panel inc-learning-panel--complete inc-incus-slot" data-label="Complete" data-incus-category="success" markdown="1">

You carried one workflow from a small typed function through explicit fallibility, a public module boundary, focused tests and a native build.

</section>

## Next

- [Build and consume an Incan library](../../tooling/tutorials/build_and_consume_library.md)
- [Diagnose a failed build](../../tooling/tutorials/diagnose_failed_build.md)
- [Build a typed data processor](typed_data_processor.md) when you want to carry the same explicit workflow shape into typed JSON and file boundaries
