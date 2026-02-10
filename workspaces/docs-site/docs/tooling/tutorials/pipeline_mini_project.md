# Pipeline mini-project (tutorial)

This tutorial is a lightweight, CI-friendly walkthrough for “step-based” automation in Incan.

Prerequisite: [Install, build, and run](../how-to/install_and_run.md).

## Goal

Write a small program that:

- defines a step-like function with typed inputs/outputs
- returns typed failures (`Result`)
- can be tested and run deterministically

## Step 1: Create a file

Create this small project layout:

```text
my_project/
├── pipeline_step.incn
└── tests/
    └── test_pipeline_step.incn
```

Run the commands below from `my_project/` (this matters for module resolution).

Create `pipeline_step.incn`:

```incan
"""
A tiny step-style function that validates input and returns a typed error.
"""

def normalize_name(name: str) -> Result[str, str]:
    if len(name.strip()) == 0:
        return Err("name must not be empty")
    return Ok(name.strip().lower())


def main() -> None:
    result = normalize_name("  Alice  ")
    match result:
        case Ok(value): println(f"ok: {value}")
        case Err(e): println(f"err: {e}")
```

## Step 2: Run it

```bash
incan run pipeline_step.incn
```

--8<-- "_snippets/callouts/no_install_fallback.md"

## Step 3: Test it

Create `tests/test_pipeline_step.incn`:

```incan
from ..pipeline_step import normalize_name
from std.testing import assert_eq

def test_normalize_name_ok() -> None:
    assert_eq(normalize_name("  Alice  "), Ok("alice"))

def test_normalize_name_err() -> None:
    assert_eq(normalize_name("   "), Err("name must not be empty"))
```

Run:

```bash
incan test tests/
```

## Next

- Typed errors: [Error Handling](../../language/explanation/error_handling.md)
- Multi-file layouts: [Imports and modules (how-to)](../../language/how-to/imports_and_modules.md)
- CI entrypoint: [CI & automation](../how-to/ci_and_automation.md)
- Contributing CI entrypoints (repo): [CI & automation (repository)](../../contributing/how-to/ci_and_automation.md)
