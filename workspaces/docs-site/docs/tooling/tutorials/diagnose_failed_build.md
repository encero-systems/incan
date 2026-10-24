# Diagnose a failed build

Introduce a type mismatch, capture its diagnostic and verify the repair.

<aside class="inc-tutorial-meta" aria-label="Tutorial details"><dl>
<div><dt>Reader</dt><dd>Project author</dd></div>
<div><dt>Prerequisites</dt><dd>Getting started and jq</dd></div>
<div><dt>Time</dt><dd>10 minutes</dd></div>
<div><dt>Verified</dt><dd>Incan <code>0.6.0-dev.8</code></dd></div>
<div><dt>Status</dt><dd>Development-toolchain executable</dd></div>
<div><dt>Outcome</dt><dd>A diagnostic-to-repair workflow</dd></div>
<div><dt>Artifacts</dt><dd>Diagnostic JSON and repaired executable</dd></div>
</dl></aside>

## Create a deliberate mismatch

Replace the starter's `src/main.incn` with:

```incan
"""Demonstrate a type mismatch."""

def double(value: int) -> int:
    """Return twice the input."""
    return value + "2"

def main() -> None:
    """Display the calculation."""
    println(double(21))
```

## Capture the diagnostic

```bash
mkdir -p target
set +e
incan check src/main.incn --format json > target/diagnostics.json
check_status=$?
set -e
test "$check_status" -ne 0
jq '.diagnostics[0] | {code, phase, message, primary_span, hints}' target/diagnostics.json
```

Read the span and compared types: integer addition received a string. Dev.8 does not expose an `explain` command; read the diagnostic fields and the relevant [language reference](../../language/reference/index.md).

## Repair and verify

Change the return expression to `value * 2`, then run:

```bash
incan check src/main.incn
incan run src/main.incn
incan build src/main.incn
incan inspect codegraph . --query defs double
```

The program prints `42`. Inspection reports the source declaration; native compilation does not provide a generated-Rust inspection command.

<section class="inc-learning-panel inc-learning-panel--complete" data-label="Complete" markdown="1">

You captured a failing check, identified the bad operand, repaired it and built the program.

</section>

## Continue

- [Troubleshooting](../how-to/troubleshooting.md)
- [CLI reference](../reference/cli_reference.md)
