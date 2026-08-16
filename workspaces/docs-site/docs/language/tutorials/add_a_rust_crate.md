# Add a Rust crate to an Incan program

This tutorial checks how Rust's `regex` crate fits behind a small Incan boundary. It is a source and lock-contract preview, not a promise that the bounded 0.5 release envelope ships an arbitrary crate closure.

<aside class="inc-tutorial-meta" aria-label="Tutorial details">
  <dl>
    <div><dt>Reader</dt><dd>Rust evaluator or native application developer</dd></div>
    <div><dt>Prerequisites</dt><dd>Getting Started and basic functions</dd></div>
    <div><dt>Time</dt><dd>20–30 minutes to review the source, manifest, and lock contract</dd></div>
    <div><dt>Verified</dt><dd>Import syntax, manifest, and lock contract verified with Incan <code>&gt;=0.5.0-0,&lt;0.6.0</code>; Rust metadata and execution need a compatible Loaf beyond the 0.5 release envelope</dd></div>
    <div><dt>Status</dt><dd>Source-verified preview</dd></div>
    <div><dt>Outcome</dt><dd>A declared Rust-backed helper and reproducible dependency boundary</dd></div>
    <div><dt>Artifacts</dt><dd>Manifest dependency, lockfile, and wrapper function</dd></div>
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

## Step 3: Use the wrapper

`main()` now owns the one-time Rust-facing construction, while `contains_number(...)` remains a small ordinary predicate over a prepared value.

The complete repository example is kept as the preview source:

```bash
cd examples/rust_interop_regex
incan lock
```

This verifies the manifest and canonical lock boundary. The finite 0.5 release envelope does not contain the `regex` dependency closure, so `check`, `build`, and `run` stop at the missing compatible Loaf instead of invoking Cargo behind your back.

## Step 4: Move into a project manifest

For a real project, move the version into `incan.toml` and generate `incan.lock`:

```toml
[rust-dependencies]
regex = "1"
```

Then make the source import versionless:

```incan
from rust::regex import Regex
```

```bash
incan lock          # (1)
# incan run --locked becomes available when the active toolchain ships the required Loaf.  # (2)
```

1. `incan lock` resolves the manifest dependency and writes the reproducible lockfile.
2. Once a compatible dependency closure is installed, `--locked` rejects dependency drift instead of silently refreshing the lockfile during the run.

The manifest is the single source of truth for a project dependency, so do not retain the inline `@ "1"` annotation after adding the manifest entry. Commit `incan.lock` so CI and collaborators resolve the same dependency graph.

<section class="inc-learning-panel inc-learning-panel--complete inc-incus-slot" data-label="Complete" data-incus-category="success" markdown="1">

You contained the proposed Rust-facing calls in one helper and made the dependency reproducible at the project boundary. Semantic inspection and native execution remain gated on a compatible published Loaf.

</section>

## Continue

- [Rust interop](../how-to/rust_interop.md)
- [Managing dependencies](../../tooling/how-to/dependencies.md)
- [Rust types for Python developers](../how-to/rust_types_for_python_devs.md)
