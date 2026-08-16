# Pipeline mini-project (tutorial)

This tutorial is a lightweight, CI-friendly walkthrough for “step-based” automation in Incan.

<aside class="inc-tutorial-meta" aria-label="Tutorial details">
  <dl>
    <div><dt>Reader</dt><dd>Automation or pipeline developer</dd></div>
    <div><dt>Prerequisites</dt><dd>Getting Started and basic Result handling</dd></div>
    <div><dt>Time</dt><dd>15–20 minutes</dd></div>
    <div><dt>Verified</dt><dd>Incan <code>&gt;=0.5.0-0,&lt;0.6.0</code> development docs; run and tests exercised with the prepared 0.5 release envelope</dd></div>
    <div><dt>Status</dt><dd>Release-envelope executable</dd></div>
    <div><dt>Outcome</dt><dd>A deterministic typed workflow step</dd></div>
    <div><dt>Artifacts</dt><dd>Runnable step and focused tests</dd></div>
  </dl>
</aside>

## Goal

Write a small program that:

- defines a step-like function with typed inputs/outputs
- returns typed failures (`Result`)
- can be tested and run deterministically

## Step 1: Create a file

Create this small project layout:

```text
my_project/
├── incan.toml
├── src/
│   └── pipeline_step.incn
└── tests/
    └── test_pipeline_step.incn
```

Run the commands below from `my_project/` (this matters for module resolution).

Create `incan.toml`:

```toml
[project]
name = "pipeline_step"
version = "0.1.0"
requires-incan = ">=0.5.0-0,<0.6.0"

[project.scripts]
main = "src/pipeline_step.incn"
```

Create `src/pipeline_step.incn`:

```incan
"""
A tiny step-style function that validates input and returns a typed error.
"""

pub def normalize_name(name: str) -> Result[str, str]:  # (1)
    if len(name.strip()) == 0:
        return Err("name must not be empty")  # (2)
    return Ok(name.strip().lower())  # (3)


def main() -> None:
    result = normalize_name("  Alice  ")
    match result:
        case Ok(value): println(f"ok: {value}")
        case Err(e): println(f"err: {e}")
```

1. `pub` exposes the step to the test module; the `Result[str, str]` signature keeps both outcomes explicit.
2. Invalid input returns a typed failure instead of logging and continuing with ambient state.
3. The success branch contains the normalized value, so downstream stages cannot confuse it with the original input.

## Step 2: Run it

```bash
incan run
```

--8<-- "_snippets/callouts/no_install_fallback.md"

## Step 3: Test it

Create `tests/test_pipeline_step.incn`:

```incan
from pipeline_step import normalize_name
from std.testing import assert_eq

def test_normalize_name_ok() -> None:
    assert_eq(normalize_name("  Alice  "), Ok("alice"))

def test_normalize_name_err() -> None:
    assert_eq(normalize_name("   "), Err("name must not be empty"))
```

Run:

```bash
incan test
```

<section class="inc-learning-panel inc-learning-panel--complete inc-incus-slot" data-label="Complete" data-incus-category="success" markdown="1">

You built a deterministic workflow step whose input, successful output, failure, and tests are explicit.

</section>

## Next

- Typed errors: [Error Handling](../../language/explanation/error_handling.md)
- Multi-file layouts: [Imports and modules (how-to)](../../language/how-to/imports_and_modules.md)
- CI entrypoint: [CI & automation](../how-to/ci_and_automation.md)
- Contributing CI entrypoints (repo): [CI & automation (repository)](../../contributing/how-to/ci_and_automation.md)
