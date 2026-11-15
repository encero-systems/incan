# Your first Incan project

Build a greeting module and test the same public functions your program calls.

<aside class="inc-tutorial-meta" aria-label="Tutorial details"><dl>
<div><dt>Reader</dt><dd>New project author</dd></div>
<div><dt>Prerequisites</dt><dd>Getting started completed</dd></div>
<div><dt>Time</dt><dd>15 minutes</dd></div>
<div><dt>Verified</dt><dd>Incan <code>0.6.0-dev.8</code></dd></div>
<div><dt>Status</dt><dd>Development-toolchain executable</dd></div>
<div><dt>Outcome</dt><dd>A program and tests sharing a module</dd></div>
<div><dt>Artifacts</dt><dd>Manifest, two modules and one test source</dd></div>
</dl></aside>

## Create the project

```bash
mkdir -p greeter/src greeter/tests
cd greeter
```

Write `loaf.toml`:

```toml
[project]
name = "greeter"
version = "0.1.0"
```

## Add a module

Create `src/greet.incn`:

```incan
"""Greeting utilities."""

pub def greet(name: str) -> str:
    """Return a greeting for the supplied name."""
    return f"Hello, {name}!"

pub def farewell(name: str) -> str:
    """Return a farewell for the supplied name."""
    return f"Goodbye, {name}!"
```

Create `src/main.incn`:

```incan
"""Display a greeting and farewell."""

from greet import greet, farewell

def main() -> None:
    """Print both messages for World."""
    println(greet("World"))
    println(farewell("World"))
```

Run it:

```bash
incan run src/main.incn
```

It prints `Hello, World!` and `Goodbye, World!`. Public functions cross the module boundary through imports from the project's `src/` source root.

## Test the public functions

Create `tests/test_main.incn`:

```incan
"""Check the messages the application uses."""

from greet import greet, farewell
from std.testing import assert_eq

def test_messages() -> None:
    """Check ordinary and empty names."""
    assert_eq(greet("Alice"), "Hello, Alice!")
    assert_eq(greet(""), "Hello, !")
    assert_eq(farewell("Alice"), "Goodbye, Alice!")
```

```bash
incan fmt src tests
incan check src/main.incn
incan test tests/test_main.incn
incan build src/main.incn
```

<section class="inc-learning-panel inc-learning-panel--complete" data-label="Complete" markdown="1">

The program and passing tests use the same public module. Oven builds a debug executable under `target/incan/debug/bin/`.

</section>

## Next steps

- [Build and consume a library](build_and_consume_library.md)
- [Testing](../how-to/testing.md)
