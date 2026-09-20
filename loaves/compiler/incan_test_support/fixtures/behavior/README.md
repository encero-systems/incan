# Behaviour fixtures

A behaviour fixture is an Incan program that carries its own expected observables. It is the route-agnostic twin of a retire-class test in the test corpus inventory (#1561): where the retired test asserted the shape of generated Rust, the fixture asserts what the program does when it runs, so it stays valid when slice 7 changes how programs run. Nothing in a fixture names a route, a backend, a build step or Rust.

This directory is transitional infrastructure for the v0.6 cutover; the procedure for twinning a test lives on #1561, not on the docs site.

## Layout

```text
loaves/compiler/incan_test_support/fixtures/behavior/
  README.md
  <area>/                     one directory per area; each has a root in loaves/toolchain/incan-cli/tests/behavior_<area>_tests.rs
    <name>.incn               single-file fixture: the whole program, header first
    <name>/main.incn          module fixture: main.incn carries the header, sibling .incn files are importable modules
    <name>/loaf.toml          project fixture: a complete project copied as it is; the header is in <name>/src/main.incn
```

Every entry in an area is one of those three shapes. A stray file is refused, and so is an empty area: something in the tree that proves nothing is exactly what the family exists to prevent. Name a fixture like an identifier (`enum_variants`, not `enum-variants.v2`): the name becomes the scratch project's name.

Two areas ship with the harness: `smoke/`, the first twins (each retires `codegen.rs` unit tests), and `harness/`, the harness proving itself with one fixture per shape of the format (a refused program, a non-zero exit code, contained lines, a module directory, a project directory). The `harness/` fixtures twin nothing; a change to the runner or to the route underneath it fails there before it fails in a twin.

A module fixture becomes the `src/` of a minimal project (`loaf.toml` with `[project.scripts] main = "src/main.incn"`), so `from helper import f` resolves `helper.incn` beside `main.incn`. A project fixture supplies its own `loaf.toml` and anything else it needs (a dependency package in a subdirectory, a facade), and its `[project.scripts] main` must be `src/main.incn`.

## Header

The header is the run of `#` comment lines at the top of the program; the first line that is not a comment ends it. Comments are Incan comments, so the header travels with the program unchanged.

```text
# behavior: a public int function taking two parameters returns their sum
# retires: loaves/compiler/incan_emit/src/codegen.rs::test_simple_function
# expect-stdout:
#   5

pub def add(a: int, b: int) -> int:
    return a + b


def main() -> None:
    println(add(2, 3))
```

| Directive | Meaning |
|---|---|
| `# behavior: <one line>` | What the program proves. Required, exactly once. This is the only prose the header carries. |
| `# retires: <path>.rs::<fn>` | A retire-class test this fixture is the twin of, spelled exactly as the inventory keys it (`scripts/test_inventory/dispositions.json`: the file path and the test's key, module-qualified only when the bare name repeats in the file). Repeatable; each test once. |
| `# expect-stdout:` | The program's whole stdout, one line per indented line below. Exact: every line, in order, nothing else. |
| `# expect-stdout-contains:` | Lines that must each appear as a whole line of stdout, in any order. Cannot be combined with `expect-stdout`. |
| `# expect-exit: <n>` | The exit code the run must end with. Default `0`. |
| `# expect-diagnostic: <CODE>` | The program must be refused at check time with this diagnostic code (`INCAN-T0001`); it is never run. Repeatable. Cannot be combined with any run expectation. |

Lines of a block directive follow it, each `#` plus at least two spaces; the first line sets the indentation and the remainder is the expected text, so deeper indentation is preserved. A bare `#` inside a block is an empty expected line; between directives it is a separator and means nothing. Everything else in the header is refused with the line number: unknown directive, prose, a stdout line that is not indented.

A header must declare at least one observable (`expect-stdout`, `expect-stdout-contains`, an explicit `expect-exit`, or `expect-diagnostic`). A program with nothing to show is not a twin.

## What the runner does

`incan_test_support::behavior_fixtures` discovers the fixtures of an area, and for each one:

1. materializes it as its own scratch project under `INCAN_TEST_TMP_ROOT` (the process temporary directory when that is unset);
2. for a refused program, runs `incan check src/main.incn --format json` and requires the check to fail with every declared code in its report;
3. for a run, runs the program (`incan run src/main.incn`, preceded outside the compiler suite by the explicit Oven bake that gives a fresh project its Loaf authority) and compares stdout and the exit code with the header;
4. records the outcome and moves on to the next fixture.

A root reports **every** failing fixture at once, each with its path, its `behavior:` line, the expected and actual observables and the run's stderr, followed by the list of fixtures that passed. It is one libtest case per area rather than one per fixture: the compiler suite plans roots from Cargo's unit graph with no build-script unit, so a generated per-fixture module has no place to be generated, and a single case whose message attributes each failure to its fixture gives the same information.

## Running

```bash
make test-one TEST_ROOT=loaves/toolchain/incan-cli/tests/behavior_smoke_tests.rs   # through the compiler suite
cargo test -p incan-cli --test behavior_smoke_tests                                # standalone
cargo test -p incan_test_support --lib behavior_fixtures                           # the format and comparison rules
```

A standalone run outside `make` needs the runtime environment `make test` exports (`INCAN_INTERNAL_SDK_PROVIDER_STORE`, `INCAN_GENERATED_CARGO_TARGET_DIR`, `INCAN_SDK_INVENTORY`, `INCAN_STDLIB`; see `TEST_RUNTIME_ENV` in the `Makefile`), or its first `incan check` compiles a cold SDK provider store under the checkout's `target/`. That is how every CLI root behaves, not something the fixtures add.

## Adding a fixture

1. Write the program under the area it belongs to, header first. Copy the retired test's inline source where that is the whole point, and make the program print what the retired test asserted about.
2. Name every retired test in a `# retires:` line.
3. In `scripts/test_inventory/dispositions.json`, point each of those tests' rows at the fixture: `"twin": "<fixture path>"` (the file for a single-file fixture, the directory otherwise), on the file row or on the test's override under `tests`. `make test-inventory-check` refuses a fixture that names a test whose row does not point back at it, and a row whose fixture does not name the test.
4. `make test-inventory` to regenerate the page; run the area's root.

A new area needs a directory here, a root `loaves/toolchain/incan-cli/tests/behavior_<area>_tests.rs` that calls `assert_area_green("<area>")`, a `fixture_roots` entry for the directory in `dispositions.json`, and a row for the root file.
