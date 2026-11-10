# Experimental COMIC conformance runner

The runner reads the committed corpus directly and verifies every data and text file against its manifest digest. Case identities must be globally unique, declared counts must match, and an empty selection is a failure. Exact integer expectations stay strings; float expectations stay explicit bit labels.

```text
incan run workspaces/comic-conformance/src/main.incn -- specs/comic/corpus/manifest.json --inventory
```

`--inventory` exits successfully only after those input checks. Its JSON receipt still says `status: incomplete`, with every unimplemented operation marked `unsupported`. It does **not** claim that those cases passed. Omitting `--inventory` runs every dispatch and exits 3 while any required operation is unsupported; an executed failure takes precedence and exits 1. A receipt distinguishes passed, failed and unsupported cases and records failure details. Invalid input, stale pins and duplicate identities exit 1; invalid command arguments exit 2.

The runner executes member identities, nominal identity preimages and hashes, closed type selectors, written-shape refusals and adaptation comparisons from the corpus. Hash cases derive object, canonical, model, description and seal preimages and digests from logical input trees, comparing both with independent literals. Value-relation cases compare derived digests. Accepted adaptation cases check reconstructed values and all three declared object/canonical/model projection relations. Binary row cases execute complete decoding, typed refusal checks and original-shape re-encoding. Text and column codec operations remain explicitly unsupported. The bounded primitive has independent tests for nested records, enum payloads, union identity, recursive plans and shared limits. It must compare the implementation's output against the separately authored expectations; hashing a supplied expected preimage alone is not a hash-conformance test.

JSON and filesystem convenience APIs are dependencies of this test project only. The production Crumb foundation belongs to `std.primitives.crumb` and imports no higher-level standard package.

## Independent row writer comparisons

The writer has a separate receipt so an encoding test cannot accidentally claim that `binary_decode` ran:

```text
incan run workspaces/comic-conformance/src/write_cases_main.incn -- specs/comic/corpus/manifest.json workspaces/comic-conformance/fixtures/row-writer-inline-descriptions.json
```

The runner admits the same pinned corpus, translates its independently authored logical records into writer inputs, and compares output bytes with each original literal `input_hex`. Four inline cases need complete writer descriptions, supplied explicitly in the fixture file. Those descriptions are not recovered by decoding the expected bytes. Referenced cases use their original supplied descriptions. The receipt requires all 33 accepted row cases to match, records each result, and explicitly states `decoding_executed: false`. Negative input and budget tests run through the ordinary test runner. The primary conformance receipt executes decoder cases independently through the row reader.

## Binary row decoding

```text
incan run workspaces/comic-conformance/src/read_cases_main.incn -- specs/comic/corpus/manifest.json
```

This separate receipt requires all 81 original binary row cases, 163 actual decode executions after expanding every proper prefix, and 33 original-shape decode/re-encode comparisons. Each prefix receives a fresh operation budget. A failed setup or notation translation is a failed test, never a successful format refusal. Refusals are compared through the typed `BinaryFailure` category, not parsed diagnostic messages.

Accepted cases check record count, sealing status, exact nominal selectors and logical object contributions against independently authored expected values. The test translator uses decoded description facts only for field flags omitted from the expected-value notation. It then re-encodes the actual decoded records and complete written descriptions, preserving inline/reference mode, and compares with the original literal input. It never returns the original bytes as its encoded result. The independently authored writer fixtures remain a separate check of those descriptions and byte layouts.

Focused tests additionally cover retained backing after owner/set scope ends, selected-field materialization under a budget too small for a neighboring field, explicit concatenated-envelope boundaries, corruption in an unselected field, finite recursive values and nominal union alternatives with equal shapes. These are format-value tests: no application model reconstruction or generated-hook coverage is claimed.
