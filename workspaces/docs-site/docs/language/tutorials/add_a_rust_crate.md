# Add a Rust crate to an Incan program

Call Rust's regex API from Incan using an installed registry dependency.

<aside class="inc-tutorial-meta" aria-label="Tutorial details"><dl>
<div><dt>Reader</dt><dd>Native application developer</dd></div>
<div><dt>Prerequisites</dt><dd>A dev.8 toolchain containing the regex registry unit</dd></div>
<div><dt>Time</dt><dd>10 minutes</dd></div>
<div><dt>Verified</dt><dd>Incan <code>0.6.0-dev.8</code></dd></div>
<div><dt>Status</dt><dd>Requires the installed regex dependency</dd></div>
<div><dt>Outcome</dt><dd>An Incan program calling a native Rust API</dd></div>
<div><dt>Artifacts</dt><dd>Manifest and native executable</dd></div>
</dl></aside>

## Declare the requirement

Create `loaf.toml`:

```toml
[project]
name = "regex_tutorial"
version = "0.1.0"

[project.scripts]
main = "main.incn"

[rust-dependencies]
regex = "1"
```

The requirement must match a unit in the installed toolchain closure. It does not download a crate.

## Match the constructor result

Create `main.incn`:

```incan
"""Find a number with Rust's regex API."""

from rust::regex import Regex

def main() -> None:
    """Construct a pattern and print whether it matches."""
    match Regex.new("[0-9]+"):
        case Ok(pattern):
            println(pattern.is_match("invoice-42"))
        case Err(error):
            println(f"invalid pattern: {error}")
```

The source import is versionless because the manifest owns the requirement. The constructor's error remains explicit.

## Check and run

```bash
incan check main.incn
incan run main.incn
```

The program prints `true`. Missing or incompatible installed dependency artifacts cause a compilation error.

<section class="inc-learning-panel inc-learning-panel--complete" data-label="Complete" markdown="1">

You declared a Rust dependency, handled its fallible constructor and called its API from Incan.

</section>

## Continue

- [Managing dependencies](../../tooling/how-to/dependencies.md)
- [Rust interop](../how-to/rust_interop.md)
