# Getting started with Incan

Create a small project, run its program, execute a test, and build a native executable with dev.8.

<aside class="inc-tutorial-meta" aria-label="Tutorial details">
  <dl>
    <div><dt>Reader</dt><dd>First-time Incan evaluator</dd></div>
    <div><dt>Prerequisites</dt><dd>A matching dev.8 Oven executable, toolchain archive and standard-package sources</dd></div>
    <div><dt>Time</dt><dd>10–15 minutes after installation</dd></div>
    <div><dt>Verified</dt><dd>Incan <code>0.6.0-dev.8</code></dd></div>
    <div><dt>Status</dt><dd>Development-toolchain executable</dd></div>
    <div><dt>Outcome</dt><dd>A runnable, tested native project</dd></div>
    <div><dt>Artifacts</dt><dd>Manifest, source, test and debug executable</dd></div>
  </dl>
</aside>

<ol class="inc-step-rail" style="--inc-step-count: 4" aria-label="Getting started steps">
  <li><strong>Install</strong>Verify the toolchain</li>
  <li><strong>Create</strong>Write a small project</li>
  <li><strong>Test</strong>Check its behavior</li>
  <li><strong>Build</strong>Produce an executable</li>
</ol>

## Install and verify

--8<-- "_snippets/learning/install_toolchain.md"

## Create your first project

Make a directory and its source and test directories:

```bash
mkdir -p hello/src hello/tests
cd hello
```

Create `loaf.toml`:

```toml
[project]
name = "hello"
version = "0.1.0"
```

Create `src/main.incn`:

```incan
"""Print a greeting."""

def main() -> None:
    """Print one greeting."""
    println("Hello, Incan!")
```

Create `tests/test_main.incn`:

```incan
"""Check the project's arithmetic."""

from std.testing import assert_eq

def test_addition() -> None:
    """Two plus two is four."""
    assert_eq(2 + 2, 4)
```

## Run, test and build

--8<-- "_snippets/learning/first_project_loop.md"

<section class="inc-learning-panel inc-learning-panel--result" data-label="Result" markdown="1">

The program prints `Hello, Incan!`, and the test reports `test_addition PASSED`. The executable is under `target/incan/debug/bin/`. There is no scaffolding command in dev.8: the files above are the project.

</section>

<section class="inc-learning-panel inc-learning-panel--complete inc-incus-slot" data-label="Complete" data-incus-category="success" markdown="1">

You have a manifest, a native executable and a passing test. Run the commands again to observe executable reuse; tests still run on every invocation.

</section>

## Next steps

- [Your first project](your_first_project.md): share behavior between modules and tests.
- [CLI reference](../reference/cli_reference.md): accepted commands and options.
- [Install and run](../how-to/install_and_run.md): toolchain archive requirements.
