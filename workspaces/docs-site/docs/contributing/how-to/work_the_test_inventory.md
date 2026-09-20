# Work the test inventory

The [test corpus inventory](../reference/test_corpus_inventory.md) is generated from two inputs: the tests in the tree, which `scripts/test_inventory/collect.py` enumerates, and `scripts/test_inventory/dispositions.json`, which records by hand what each test file (or test) is for the slice-7 cutover. `make test-inventory` regenerates the page; `make test-inventory-check` runs in `make pre-commit-fast` and in the CI fast gate and refuses a tree whose inventory has drifted. This page says what to do when it refuses, and how to record the three kinds of decision it tracks.

## Classify a new test

A new test in a file that already has a row inherits that file's disposition; only the page changes. Run `make test-inventory` and commit the regenerated page with the test.

A test in a new file has no disposition, and the gate says so:

```text
- unclassified: `loaves/compiler/incan_emit/tests/new_tests.rs` carries 1 test(s) with no row in dispositions.json (inventory_probe)
```

Add a row under `files` in `dispositions.json`:

```json
"loaves/compiler/incan_emit/tests/new_tests.rs": {
  "disposition": "retire",
  "owner": 1561,
  "twin": "",
  "split_required": false,
  "split_target": "",
  "notes": "asserts generated Rust text"
}
```

Pick the disposition by what the test proves and how, not by where it lives:

- `keep`: the assertion goes through the parser, typechecker, Body IR, formatter, LSP or semantics core and never touches generated Rust.
- `re-point`: the assertion is about program behaviour (output, exit code, diagnostics of a run) and is proved by building or running generated Rust, for example through `run_incan`.
- `retire`: the assertion is about the generated Rust itself: an `insta` snapshot, `contains("fn ...")` on emitted source, an emitter unit test.
- `unaffected`: the cutover does not touch it (Oven, store, installer, stdlib runtime, layering guards).
- `unreviewed`: you have not read it. Say so rather than guess; write the mechanical proposal in `notes` if you have one.

To see what the collector thinks, run `python3 scripts/test_inventory/collect.py --propose`. It writes `scripts/test_inventory/proposals.json` (git-ignored) with a disposition per file and per test derived from the lane signals. Fold what you agree with into `dispositions.json`; the sidecar is never the record.

When one file mixes dispositions, give the file the majority and override the rest under `tests`, keyed by the function name (module-qualified, such as `tests::inner::plain`, only when the bare name repeats in the file):

```json
"tests": {
  "generated_rust_names_the_element_type": { "disposition": "retire", "twin": "", "notes": "asserts generated Rust text" }
}
```

The gate refuses an override whose key is not a test in that file, and a row whose file no longer carries tests.

## Record a twin

A `retire` test may be deleted only once it names a twin: a `keep` or `re-point` test that covers the same behaviour. Put the twin in `twin` as `path::fn`, on the file row when one test covers the whole file, or on the test's own override:

```json
"twin": "loaves/toolchain/incan-cli/tests/cli_language_regression_tests.rs::named_constructor_arguments_evaluate_in_written_order_issue1462"
```

A declared fixture root is also a valid twin, spelled as its bare path, when the behaviour is covered by running that root's programs. The gate refuses a twin whose file, test or root does not exist and a twin whose own disposition is not `keep` or `re-point`. The page's `Twins` column counts named twins against retire-class tests per file. This field is the record the emitter freeze rule in #1561 refers to: a deletion under `loaves/compiler/incan_emit/src/emit/` is admissible only for rows that name one.

## Plan a split

The collector measures each file's test region: the whole file for a test file, the `#[cfg(test)]` modules for a source file. When that region exceeds `split_threshold_lines` (1500), the row must carry `split_required: true`, and the gate says which value to record when the flag and the measurement disagree:

```text
- `loaves/kernel/incan_syntax/src/parser/tests.rs`: split_required is false but the test region is 6673 lines against a threshold of 1500; record true
```

Write the planned module layout in `split_target` (for example `parser/tests/{decls,exprs,stmts}.rs`) when you plan it, and leave it `""` until then; the page shows `required` or `planned: ...`. When the split lands, add a row for each new file, remove the old row, and set the flag back to `false` on any file that shrank below the threshold. A `retire` file over the threshold is deleted rather than split; leave its `split_target` empty.

## Add a fixture root

`.incn` fixtures are inventoried by root, not per file. Add the directory under `fixture_roots` with the glob that selects its cases and a disposition:

```json
"loaves/compiler/incan_test_support/fixtures/valid": {
  "pattern": "**/*.incn",
  "disposition": "re-point",
  "owner": 1561,
  "notes": "typechecked in bulk and run as programs by CLI tests"
}
```

The gate refuses a root that does not exist or matches no case.

## Regenerate and check

```bash
make test-inventory          # rewrite the reference page
make test-inventory-check    # what pre-commit-fast and CI run
python3 scripts/test_inventory/collect.py            # summary: totals, split candidates, anomalies
python3 scripts/test_inventory/collect.py --json     # every test with its lanes, for tooling
```

The check fails, with one line per finding, when:

- a test file carries no row in `dispositions.json`;
- a row or override names a file or test that no longer exists;
- a `twin` does not resolve, or resolves to a test that is not `keep` or `re-point`;
- `split_required` disagrees with the measured test region;
- a fixture root is missing or empty;
- the rendered page differs from what the tree and `dispositions.json` produce.

Commit `dispositions.json` and the regenerated page together with the change that moved them.
