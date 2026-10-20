# Contracts

The contract families say what `incan` must do, each case as a program that carries its own expectations. A table runner, written in Incan, runs every case against an `incan` and reports each one. Nothing in a case names a route, a backend or a build step: a case states what a user observes.

## Families

| Family | Runs | Compares |
| --- | --- | --- |
| `behavior` | `incan run src/main.incn`, or `incan check src/main.incn --format json` when the header declares diagnostics | stdout, stderr, exit code; or the refusal's diagnostic codes |
| `check` | `incan check src/main.incn --format json` | acceptance (`expect-accepted:`, warnings allowed) or the refusal's codes |
| `library` | as `behavior`, for a project with path dependencies; the Oven bakes them | as `behavior` |
| `interop` | as `behavior`, for programs that import through `rust::` | as `behavior` |
| `testing` | `incan test tests/<name>.incn` | the per-test outcome lines, stderr, exit code |
| `project` | `incan describe-unit src/main.incn` | the unit description's dependencies, standard packages and active features, or the refusal's stderr and exit code |

## Layout

```text
workspaces/contracts/
  src/                       the table runner
  fixtures/<family>/<area>/
    <name>.incn              single-file case: the whole program, header first
    <name>/main.incn         module case: main.incn carries the header, sibling .incn files are importable modules
    <name>/loaf.toml         project case: a complete project; the header is in src/main.incn
```

Every entry in an area is one of those shapes, and an empty area is refused. A family directory may hold a `README.md`; nothing else but areas. Hidden entries are skipped. Names are lowercase letters, digits and `_`, starting with a letter or `_`. A testing case is one file named `test_<subject>.incn`; it runs from the project's `tests/` directory. A library case is a project.

The runner materializes each case as its own project in a scratch directory: a single-file or module case gets a minimal `loaf.toml` (`[project.scripts] main = "src/main.incn"`), a project case is copied as it is. The project is removed after the case, whatever its result.

## Header

The header is the unbroken run of `#` comment lines at the top of the program.

| Directive | Meaning |
| --- | --- |
| `# behavior: <one line>` | What the case proves. Required, once. |
| `# retires: <path>.rs::<fn>` | The retired test the case replaces. Repeatable. |
| `# expect-stdout:` | The whole stdout, one indented line per line. Exact; an empty block means nothing is printed. |
| `# expect-stdout-contains:` | Lines that must each appear as a whole stdout line. Not with `expect-stdout`. |
| `# expect-stderr-contains:` | Substrings that must each appear in stderr. |
| `# expect-exit: <n>` | The exit code, `0` to `255`. Default `0`. |
| `# expect-diagnostic: <CODE>` | The program is refused at check time with this code. Repeatable. Never with a run expectation. |
| `# expect-accepted:` | The program checks without errors and is never run. |
| `# expect-dependencies:` | Exactly these dependencies, one `<key> <package>` per line; an empty block means none. |
| `# expect-std-packages:` | Standard packages the unit must reach. |
| `# expect-active-features:` | Exactly these active features; an empty block means none. |
| `# pending: <reason>` | The case does not hold yet, and why. A pending case that holds fails: its line is stale. |

A directive is `#`, one space, the key, a colon. Block lines follow their directive, each `#` and at least two spaces; the first line sets the indentation. A bare `#` inside a block is an empty line. A blank line or any other non-comment line ends the header, and a directive after it is refused. Every case declares at least one expectation its family compares; a family refuses the expectations it does not compare.

Output is compared as lines: `\n` ends a line, a trailing `\r` is dropped, a final newline adds no empty line. An expected line that ends with whitespace is refused, because a report could not show the difference.

## Running

```bash
incan run workspaces/contracts/src/main.incn -- INCAN workspaces/contracts/fixtures [--scratch DIR] [SELECTOR...]
```

A selector is `<family>`, `<family>/<area>` or `<family>/<area>/<name>`. The runner prints `PASS`, `FAIL` with each broken expectation, or `PEND` with its reason for every case, a summary per area and a total, and exits 1 when a case fails or a family cannot be read.
