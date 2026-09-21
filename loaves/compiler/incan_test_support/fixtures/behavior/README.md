# Behavior fixtures

A behavior fixture is an Incan program that carries its own expected observables. It is the route-agnostic twin of a retire-class test in the test corpus inventory (#1561): where the retired test asserted the shape of generated Rust, the fixture asserts what the program does when it runs, so it stays valid when slice 7 changes how programs run. Nothing in a fixture names a route, a backend, a build step or Rust.

This directory is transitional infrastructure for the v0.6 cutover; the procedure for twinning a test lives on #1561, not on the docs site.

## Layout

```text
loaves/compiler/incan_test_support/fixtures/behavior/
  README.md
  <area>/                     one directory per leaf area; a root in loaves/toolchain/incan-cli/tests/behavior_<family>_tests.rs holds one #[test] per area
    <name>.incn               single-file fixture: the whole program, header first
    <name>/main.incn          module fixture: main.incn carries the header, sibling .incn files are importable modules
    <name>/loaf.toml          project fixture: a complete project copied as it is; the header is in <name>/src/main.incn
```

Every entry in an area is one of those three shapes. A stray file is refused, and so is an empty area: something in the tree that proves nothing is exactly what the family exists to prevent. Name a fixture like an identifier (`enum_variants`, not `enum-variants.v2`): the name becomes the scratch project's name.

Two areas ship with the harness: `smoke/`, the first twins (each retires `codegen.rs` unit tests), and `harness/`, the harness proving itself with one fixture per shape of the format (a refused program with one code and with two, a non-zero exit code, an exit code as the only observable, an empty stdout, contained lines, a module directory, a project directory). The `harness/` fixtures twin nothing; a change to the runner or to the route underneath it fails there before it fails in a twin. The header *refusals* are unit tests of `parse_header` in `incan_test_support`, not fixtures: an area fixture must pass, so a fixture cannot prove that a malformed header is refused.

One area is set apart by what its programs need from the runner: `cli_dependencies/`, project fixtures with in-fixture path dependencies, whose providers the runner bakes before the run (see *Project fixtures with dependencies* and *Provider bakes and Cargo*). No behavior root, that one included, is registered for a compiler-suite Cargo capability: every bake the runner performs under the suite is Cargo-guarded.

### Area size

An area is one libtest case, and the compiler suite runs roots on two threads with no slicing inside a case (#1549 slices by case). At roughly 3.5 s per fixture locally and 6–10 s per fixture on the four-core CI runner, **an area holds at most 60 fixtures**: about 3.5 minutes locally, 6–10 minutes in CI, the most one case should cost. The runner enforces it: `assert_area_green` refuses a larger area with a message naming this rule.

A family that outgrows one area splits into leaf areas of at most 60 (`driver_imports/`, `driver_traits/`, ...), and its root file holds one `#[test]` per leaf area, each calling `assert_area_green("<area>")`, so the suite's two-thread root budget runs two of them concurrently and case slicing applies. Declare only leaf areas in `fixture_roots`: a parent directory of areas would be discovered as module fixtures. Do not add threading inside an area: the two-thread budget is the suite's by design, and the case is its unit of scheduling.

A module fixture becomes the `src/` of a minimal project (`loaf.toml` with `[project.scripts] main = "src/main.incn"`), so `from helper import f` resolves `helper.incn` beside `main.incn`. A project fixture supplies its own `loaf.toml` and anything else it needs (a dependency package in a subdirectory, a facade), and its `[project.scripts] main` must be `src/main.incn`.

### Project fixtures with dependencies

A project fixture may depend on packages that live inside it. It declares them the way any project does, and the runner bakes them before the program runs:

```text
<name>/
  loaf.toml                 [dependencies] querykit = { path = "deps/querykit" }
  src/main.incn             from pub::querykit import count   (header first, as always)
  deps/querykit/
    loaf.toml               [project] name = "querykit"
    src/
      lib.incn              pub from helpers import count
      helpers.incn
```

- **What is read.** The fixture's `loaf.toml` is read the way the compiler reads it, and every `[dependencies]` path entry is followed into the provider's own `loaf.toml`, so a provider's providers are found too and planned first. The dependency's name is what the program imports as `pub::<name>`.
- **What is baked.** Every provider, once, in dependency order (a provider after the providers it depends on; a provider two consumers share once), with `incan oven bake --project .` in the provider's directory and no Cargo authority (see *Provider bakes and Cargo*). Outside the compiler suite the bake publishes into the fixture project's own Oven home, where the consumer's run finds it. A consumer refuses to run until its providers have published a package Loaf, which is the whole reason for the bake. The consumer project itself is baked outside the suite only, as every run fixture is. A refused program is checked, never baked: `incan check` prepares an unbaked provider's metadata itself.
- **What is refused at discovery** (the area fails to run, naming the fixture, like a malformed header): a dependency whose path resolves outside the fixture directory (only the fixture directory is copied into the scratch project, so `path = "../elsewhere"` would depend on nothing), a cycle among the path dependencies (named as the chain that closes it: neither side could be baked first), and a fixture `loaf.toml` the compiler cannot read.
- **What fails the fixture** (reported beside the other failures, with the bake's stdout and stderr): a provider whose bake fails for any reason, a `loaf.toml` the compiler refuses included, and under the suite a provider whose bake reaches for Cargo. A provider's manifest the runner cannot read is not a discovery refusal: the provider is planned without dependencies of its own and its bake says what is wrong, so one broken provider fails one fixture rather than the whole area.
- **Where.** Under `cli_dependencies/`.

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
| `# expect-stdout:` | The program's whole stdout, one line per indented line below. Exact: every line, in order, nothing else. An empty block means the program prints nothing. |
| `# expect-stdout-contains:` | Lines that must each appear as a whole line of stdout, in any order. At least one line; each line once. Cannot be combined with `expect-stdout`. |
| `# expect-exit: <n>` | The exit code the run must end with, `0` to `255`. Default `0`. |
| `# expect-diagnostic: <CODE>` | The program must be refused at check time with this diagnostic code (`INCAN-T0001`); it is never run. Repeatable, each code once. Cannot be combined with any run expectation. |

A directive is `#`, one space, the key, a colon: `#retires:` is accepted, `#  retires:` (two spaces) is a block item, `#\tretires:` and `# retires :` are refused. The `retires:` value is the inventory's key with no whitespace anywhere: `<path>.rs::<fn>`, the function part module-qualified (`tests::inner::name`) only when the bare name repeats in the file. The inventory collector reads the same line with the same rule, so what one reader accepts the other does too.

Lines of a block directive follow it, each `#` plus at least two spaces; the first line sets the indentation and the remainder is the expected text, so deeper indentation is preserved. A bare `#` inside a block is an empty expected line; between directives it is a separator and means nothing. Everything else in the header is refused with the line number: unknown directive, prose, a stdout line that is not indented.

**The header is contiguous.** A blank line ends it, and so does any other line that is not a `#` comment. A directive after that point (`# expect-stdout:` below a blank line, say) is refused with its line number rather than ignored, because a fixture whose expectations were silently dropped would pass on less than it declares.

**Stdout is compared as lines.** Both sides are split with Rust's `str::lines()`: the final newline is not required, CRLF is accepted, and nothing else is normalized. An expected line that ends with whitespace is refused (a report could not show the difference), so trailing whitespace in a program's output cannot be asserted and does not match. `expect-stdout-contains` refuses an empty block (anything would satisfy it) and a line listed twice (one occurrence satisfies both).

A header must declare at least one observable (`expect-stdout`, `expect-stdout-contains`, an explicit `expect-exit`, or `expect-diagnostic`). A program with nothing to show is not a twin. Two refusals name both lines involved: the second stdout block beside the first, and an `expect-diagnostic` beside a run expectation (or the other way round, whichever comes second).

## What the runner does

`incan_test_support::behavior_fixtures` discovers the fixtures of an area, and for each one:

1. materializes it as its own scratch project under `INCAN_TEST_TMP_ROOT` (the process temporary directory when that is unset; an explicit root that does not exist is an error, not created);
2. for a refused program, runs `incan check src/main.incn --format json` and requires the check to fail with every declared code in its report;
3. for a run, bakes each in-fixture provider the project declares, in dependency order and with no Cargo authority; bakes the project itself outside the compiler suite (a fresh project has no Loaf authority until then; under the suite the sealed standard-library Loaf serves); then runs the program (`incan run src/main.incn`) and compares stdout and the exit code with the header;
4. records the outcome, deletes the scratch project, and moves on to the next fixture.

### Provider bakes and Cargo

Under the compiler suite an ordinary program runs on the sealed standard-library Loaf and no bake happens; `make test-one` puts a Cargo guard on `PATH` and fails the run if anything reaches it. A provider bake is the one bake the runner performs under the suite, and it is run **Cargo-guarded**: through the same command environment as any other `incan` call, with no `CARGO` and none of the explicit-bake authority the suite grants registered roots (`explicit_bake_cargo` in `OvenCompilerSuiteTargetCapabilities`, `loaves/oven/oven_model/src/compiler_suite_env.rs`). No behavior root is registered there, and a test in `oven-cli` asserts it.

What that admits, measured on 2026-09-20 under `make test-one` with the guard: an Incan library provider with no dependencies of its own, which the Oven serves from the active standard-library Loaf without invoking Cargo (the three `cli_dependencies/` fixtures: a helper library, a library exporting a `Callable`-taking function, and a feature-gated library behind a refused program). What it refuses, loudly: a provider that itself declares `[dependencies]`, whose bake goes through the compatibility publisher's test-dependency envelope and runs `cargo metadata` and `cargo build`; under the suite that bake meets the guard, the fixture fails with the bake's stderr, and the suite wrapper fails on the guard log. The same holds for a program that imports through `rust::`: it needs source-current project inspection authority, which only an explicit bake of its own project records, and that bake runs Rust inspection through Cargo (`cargo metadata` of the inspection workspace and the toolchain's `std` sources, `rustc --print` probes, `cargo check`; ten launches for one fixture). Such fixtures are not in the tree; they wait for a Cargo-free bake under the suite, and their candidates are parked with the #1561 test-corpus slice records.

The scratch project is deleted after every fixture, pass or fail, so the stderr in the report is all a failure leaves behind: put what a reader needs to see into the program's output, not into files.

A root reports **every** failing fixture at once, each with its path, its `behavior:` line, the expected and actual observables and the run's stderr, followed by the list of fixtures that passed. It is one libtest case per area rather than one per fixture: the compiler suite plans roots from Cargo's unit graph with no build-script unit, so a generated per-fixture module has no place to be generated, and a single case whose message attributes each failure to its fixture gives the same information. That is also why an area is capped (see *Area size*).

## Running

```bash
make test-one TEST_ROOT=loaves/toolchain/incan-cli/tests/behavior_smoke_tests.rs   # through the compiler suite
cargo test -p incan-cli --test behavior_smoke_tests                                # standalone
cargo test -p incan_test_support --lib behavior_fixtures                           # the format, the provider plan and the comparison rules
```

A standalone run outside `make` needs the runtime environment `make test` exports (`INCAN_INTERNAL_SDK_PROVIDER_STORE`, `INCAN_GENERATED_CARGO_TARGET_DIR`, `INCAN_SDK_INVENTORY`, `INCAN_STDLIB`; see `TEST_RUNTIME_ENV` in the `Makefile`), or its first `incan check` compiles a cold SDK provider store under the checkout's `target/`. That is how every CLI root behaves, not something the fixtures add. The `cli_dependencies/` area needs one thing more standalone: `cargo test` hands the test binary the active toolchain's Cargo in `CARGO`, and the standalone bake of a consumer project with dependencies then fails with "Cargo compiler artifact ... has no exact rustc invocation" unless `CARGO` names the pinned publisher toolchain (`INCAN_TEST_PUBLISHER_TOOLCHAIN` in the `Makefile`). Run that root's test binary with `CARGO` pointed at that toolchain's cargo, or run the root through `make test-one`, where no project bake happens.

## Adding a fixture

1. Write the program under the area it belongs to, header first. Copy the retired test's inline source where that is the whole point, and make the program print what the retired test asserted about.
2. Name every retired test in a `# retires:` line.
3. In `scripts/test_inventory/dispositions.json`, point each of those tests' rows at the fixture: `"twin": "<fixture path>"` (the file for a single-file fixture, the directory otherwise), on the file row or on the test's override under `tests`. `make test-inventory-check` refuses a fixture that names a test whose row does not point back at it, and a row whose fixture does not name the test.
4. `make test-inventory` to regenerate the page; run the area's root.

A new area needs a directory here, a `#[test]` in a root `loaves/toolchain/incan-cli/tests/behavior_<family>_tests.rs` that calls `assert_area_green("<area>")` (one root file per family, one test per leaf area), a `fixture_roots` entry for the directory in `dispositions.json`, and a row for the root file. `dispositions.json` stays sorted by key in `files` and `fixture_roots`; `make test-inventory-check` refuses an unsorted record.
