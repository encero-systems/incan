# Diagnose and inspect a failed build

This tutorial introduces a deliberate type error, captures the compiler's structured diagnostic, asks the compiler to explain its stable code, then verifies the repaired backend output.

<aside class="inc-tutorial-meta" aria-label="Tutorial details">
  <dl>
    <div><dt>Reader</dt><dd>Project author debugging a failed check or build</dd></div>
    <div><dt>Prerequisites</dt><dd>Getting Started and <code>jq</code></dd></div>
    <div><dt>Time</dt><dd>15–20 minutes</dd></div>
    <div><dt>Verified</dt><dd>Incan <code>&gt;=0.5.0-0,&lt;0.6.0</code> development docs; diagnostic, inspection, and repaired build exercised with the prepared 0.5 release envelope</dd></div>
    <div><dt>Status</dt><dd>Release-envelope executable</dd></div>
    <div><dt>Outcome</dt><dd>A repeatable diagnostic-to-fix workflow</dd></div>
    <div><dt>Artifacts</dt><dd>Diagnostic JSON and Rust inspection JSON</dd></div>
  </dl>
</aside>

<ol class="inc-step-rail" style="--inc-step-count: 4" aria-label="Failed build diagnosis steps">
  <li><strong>Break</strong>Create one type mismatch</li>
  <li><strong>Capture</strong>Keep the diagnostic JSON</li>
  <li><strong>Explain</strong>Use the stable code</li>
  <li><strong>Verify</strong>Repair and inspect</li>
</ol>

## Step 1: create a deliberate mismatch

In a starter project's `src/main.incn`, put:

```incan
def double(value: int) -> int:
    return value + "2"  # (1)

def main() -> None:
    println(double(21))
```

1. This is the only intentional defect: the integer operation receives a string operand.

The function promises an integer calculation but supplies a string operand.

## Step 2: capture the diagnostic

Create the report directory, run the check, and preserve its non-zero status:

```bash
mkdir -p target
set +e
incan check src/main.incn --format json > target/diagnostics.json  # (1)
check_status=$?
set -e
test "$check_status" -ne 0  # (2)
jq '.diagnostics[0] | {code, phase, message, primary_span, hints}' target/diagnostics.json
```

1. Machine-readable output preserves compiler-owned fields without scraping the human rendering.
2. The failing check is expected here, but the shell still proves that it actually failed.

The JSON record carries the compiler-owned code, phase, message, source span, and hints. CI or an editor can consume those fields without parsing the human terminal rendering.

## Step 3: ask the same compiler to explain the code

```bash
code="$(jq -r '.diagnostics[0].code' target/diagnostics.json)"
incan explain "$code"
```

Diagnostic codes and their explanations are versioned with the compiler. Use the code from the captured report rather than hard-coding one in automation.

## Step 4: repair and inspect

Make the operation type-correct:

```incan
def double(value: int) -> int:
    return value * 2

def main() -> None:
    println(double(21))
```

Now verify the source and inspect what the current backend would emit:

```bash
incan check src/main.incn
incan inspect rust src/main.incn --format json > target/rust-inspection.json
jq '{mode, source_files, rust_files}' target/rust-inspection.json
incan build src/main.incn
```

`incan inspect rust` is a debugging and reporting surface. Generated Rust is not the stable source or ABI contract.

<section class="inc-learning-panel inc-learning-panel--complete inc-incus-slot" data-label="Complete" data-incus-category="success" markdown="1">

You turned one failed check into structured evidence, explained it through the matching compiler catalog, repaired the source, and verified both checking and backend inspection.

</section>

## Continue

- [CLI diagnostics and explain reference](../reference/cli_reference.md#incan-check)
- [Troubleshooting](../how-to/troubleshooting.md)
- [Local project to CI artifact](project_to_ci_artifact.md)
