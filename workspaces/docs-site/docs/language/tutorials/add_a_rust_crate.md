# Add a Rust crate to an Incan program

This tutorial puts Rust's `regex` crate behind a small Incan boundary. An explicit Oven bake prepares the locked project extension; normal Incan commands then reuse it without invoking Cargo on the consumer path.

<aside class="inc-tutorial-meta" aria-label="Tutorial details">
  <dl>
    <div><dt>Reader</dt><dd>Rust evaluator or native application developer</dd></div>
    <div><dt>Prerequisites</dt><dd>Getting Started and basic functions</dd></div>
    <div><dt>Time</dt><dd>20–30 minutes</dd></div>
    <div><dt>Verified</dt><dd>Locking, explicit Oven bake, native build, and execution verified with Incan <code>&gt;=0.5.0-0,&lt;0.6.0</code></dd></div>
    <div><dt>Status</dt><dd>Release-envelope executable</dd></div>
    <div><dt>Outcome</dt><dd>A running Rust-backed helper behind a reproducible dependency boundary</dd></div>
    <div><dt>Artifacts</dt><dd>Manifest, lockfile, project-extension Loaf, wrapper, and native executable</dd></div>
  </dl>
</aside>

## Step 1: Import a versioned crate

Create `main.incn`:

```incan
from rust::regex @ "1" import Regex  # (1)
```

1. `rust::` selects Rust interop; `@ "1"` pins the compatible crate major version for this single-file program.

The `rust::` prefix identifies a Rust dependency. The inline version annotation is convenient for a single-file program.

## Step 2: Wrap the Rust-facing API

```incan
from rust::regex @ "1" import Regex

def contains_number(pattern: Regex, input: str) -> bool:
    return pattern.is_match(input)

def main() -> None:
    pattern = Regex.new("\\d+").unwrap()  # (1)
    for sample in ["invoice-42", "invoice-pending"]:
        println(f"{sample}: {contains_number(pattern, sample)}")  # (2)
```

1. The constructor is fallible. `unwrap()` is acceptable here only because `"\\d+"` is a fixed, reviewed source literal; the warning below describes the input-driven case. Compiling it once keeps that cost visible.
2. The prepared regex is reused by the predicate instead of being recompiled for every item.

Only this helper needs to know the Rust crate's constructor and methods. Callers get an ordinary Incan function.

!!! warning "About `unwrap()`"
    The pattern is a fixed source literal in this tutorial. If the expression comes from a user, configuration file, or network input, keep `Regex.new(...)` fallible and return or match its error instead of unwrapping it.

## Step 3: Move the dependency into the manifest

`main()` now owns the one-time Rust-facing construction, while `contains_number(...)` remains a small ordinary predicate over a prepared value.

For a project, move the version into `loaf.toml` and make the source import versionless:

```toml
[rust-dependencies]
regex = "1"
```

```incan
from rust::regex import Regex
```

The manifest is now the single source of truth for the project dependency. Do not retain the inline `@ "1"` annotation after adding it.

## Step 4: Lock, bake, and run

The complete repository example includes that manifest. Prepare its dependency closure explicitly, then use the normal command path:

```bash
cd examples/rust_interop_regex
incan lock
incan oven bake --project . --format json
incan run --locked
```

`incan lock` writes the reproducible dependency graph. The explicit bake may invoke the bounded compatibility publisher once and seals a project-extension Loaf over the exact full-standard-library base. `incan run --locked` then selects that immutable closure and rejects drift; it never invokes Cargo as a hidden fallback. Commit `oven.lock` so CI and collaborators resolve the same graph.

<section class="inc-learning-panel inc-learning-panel--complete inc-incus-slot" data-label="Complete" data-incus-category="success" markdown="1">

You contained the Rust-facing calls in one helper, made the dependency reproducible, baked the project extension explicitly, and ran the resulting native program through the normal Incan command path.

</section>

## Continue

- [Rust interop](../how-to/rust_interop.md)
- [Managing dependencies](../../tooling/how-to/dependencies.md)
- [Rust types for Python developers](../how-to/rust_types_for_python_devs.md)
