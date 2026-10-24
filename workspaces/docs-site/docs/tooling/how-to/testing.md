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

## Run more than one source

Invoke the command for each source and retain a nonzero result if any invocation fails:

```bash
status=0
for file in tests/test_*.incn; do
    incan test "$file" || status=1
done
test "$status" -eq 0
```

The shell loop assumes at least one matching file. Directory discovery, `-k`, marker-selection flags, `--jobs`, `--list`, JUnit and JSON-report flags are not dev.8 CLI options.

## Select features

```bash
incan test tests/test_main.incn --no-default-features
```

Use `--features` for declared package features. These select the source and dependency projection; they do not filter collected test names. See [Package features](sdk_components_and_package_features.md).
