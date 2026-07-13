# Add a Rust crate to an Incan program

This tutorial uses Rust's `regex` crate from ordinary Incan code. The goal is not to write Rust-shaped application code; it is to put one established Rust library behind a small Incan boundary.

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

def contains_number(input: str) -> bool:
    pattern = Regex.new("\\d+").unwrap()  # (1)
    return pattern.is_match(input)
```

1. The constructor is fallible. `unwrap()` is acceptable here only because `"\\d+"` is a fixed, reviewed source literal; the warning below describes the input-driven case.

Only this helper needs to know the Rust crate's constructor and methods. Callers get an ordinary Incan function.

!!! warning "About `unwrap()`"
    The pattern is a fixed source literal in this tutorial. If the expression comes from a user, configuration file, or network input, keep `Regex.new(...)` fallible and return or match its error instead of unwrapping it.

## Step 3: Use the wrapper

```incan
def main() -> None:
    for sample in ["invoice-42", "invoice-pending"]:
        println(f"{sample}: {contains_number(sample)}")
```

Run the checked repository example:

```bash
incan run examples/rust_interop_regex/main.incn
```

The first build may fetch and compile the crate. Subsequent builds can reuse the generated Cargo cache when the dependency graph is unchanged.

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
incan run --locked  # (2)
```

1. `incan lock` resolves the manifest dependency and writes the reproducible lockfile.
2. `--locked` rejects dependency drift instead of silently refreshing the lockfile during the run.

The manifest is the single source of truth for a project dependency, so do not retain the inline `@ "1"` annotation after adding the manifest entry. Commit `incan.lock` so CI and collaborators resolve the same dependency graph.

<section class="inc-learning-panel inc-learning-panel--complete inc-incus-slot" data-label="Complete" data-incus-category="success" markdown="1">

You added a Rust crate, contained the Rust-facing calls in one helper, and made the dependency reproducible at the project boundary.

</section>

## Continue

- [Rust interop](../how-to/rust_interop.md)
- [Managing dependencies](../../tooling/how-to/dependencies.md)
- [Rust types for Python developers](../how-to/rust_types_for_python_devs.md)
