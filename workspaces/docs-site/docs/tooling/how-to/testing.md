# Run tests

Pass a test source file to dev.8:

```bash
incan test tests/test_main.incn
```

A minimal source is:

```incan
"""Test arithmetic."""

from std.testing import assert_eq

def test_addition() -> None:
    """Two plus two is four."""
    assert_eq(2 + 2, 4)
```

The command reports each test by file and name. A failed assertion or compilation returns nonzero. Running it again executes the tests again, even when Oven reuses their executable.

## Run a Rust facet's unit tests

Pass the Loaf directory to compile and run its Rust library and declared binary unit tests:

```bash
incan test loaves/kernel/incan_syntax
```

Oven adds the root Loaf's `[envs.test.dependencies]` and `[envs.test.rust-dependencies]` to its production dependency closure. A partial dependency override preserves fields it does not specify and adds its requested features. Dependencies retain their production environments. Library and binary unit harnesses and direct `tests/*.rs` integration entries beside the Rust source root run independently. Each Rust test is reported beside tests from `test_*.incn` modules directly under the Loaf's source root and `tests` directory; failure in either language makes the command fail. Pass an Incan source file explicitly to test inline declarations elsewhere. Rust doctests are not selected.

## Run more than one source

Invoke the command for each source and retain a nonzero result if any invocation fails:

```bash
status=0
for file in tests/test_*.incn; do
    incan test "$file" || status=1
done
test "$status" -eq 0
```

The shell loop assumes at least one matching file. Recursive Incan test discovery, `-k`, marker-selection flags, `--jobs`, `--list`, JUnit and JSON-report flags are not dev.8 CLI options.

## Select features

```bash
incan test tests/test_main.incn --no-default-features
```

Use `--features` for declared package features. These select the source and dependency projection; they do not filter collected test names. See [Package features](sdk_components_and_package_features.md).
