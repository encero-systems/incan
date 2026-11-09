# Experimental COMIC conformance runner

The runner reads the committed corpus directly and verifies every data and text file against its manifest digest. Case identities must be globally unique, declared counts must match, and an empty selection is a failure. Exact integer expectations stay strings; float expectations stay explicit bit labels.

```text
incan run workspaces/comic-conformance/src/main.incn -- specs/comic/corpus/manifest.json --inventory
```

`--inventory` exits successfully only after those input checks. Its JSON receipt still says `status: incomplete`, with every unimplemented operation marked `unsupported`. It does **not** claim that those cases passed. Omitting `--inventory` runs every dispatch and exits 3 while any required operation is unsupported; an executed failure takes precedence and exits 1. A receipt distinguishes passed, failed and unsupported cases and records failure details. Invalid input, stale pins and duplicate identities exit 1; invalid command arguments exit 2.

The runner executes member identities, nominal identity preimages and hashes, closed type selectors, written-shape refusals and adaptation comparisons from the corpus. Hash cases derive object, canonical, model, description and seal preimages and digests from logical input trees, comparing both with independent literals. Value-relation cases compare derived digests. Accepted adaptation cases check reconstructed values and all three declared object/canonical/model projection relations. Binary row cases execute complete decoding, typed refusal checks and original-shape re-encoding. Text operations execute the COMIC parser and printer; column operations execute the finite column reader. The bounded primitive has independent tests for nested records, enum payloads, union identity, recursive plans and shared limits. It must compare the implementation's output against the separately authored expectations; hashing a supplied expected preimage alone is not a hash-conformance test.

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

## Column writer controls

Run the finite column writer independently of decoder conformance:

```bash
incan run workspaces/comic-conformance/src/write_columns_main.incn -- specs/comic/corpus/manifest.json workspaces/comic-conformance/fixtures/column-writer-choices.json
```

The receipt compares 52 accepted block, dictionary, complete-column and envelope vectors. Logical values and complete descriptions come from the original corpus; the choice fixture supplies only group/column/block encoding IDs for the complete cases. Those IDs were transcribed from the independent literal framing, without using the production encoder or decoder. Expected bytes and manifest pins are unchanged.

These are chosen-encoding comparisons, not a claim that different valid column encodings have identical bytes. `decoding_executed=false` is explicit. The independent reader receipt below covers negative decoder cases and actual decode/re-encode behavior.

## Binary column decoding

```text
incan run workspaces/comic-conformance/src/read_columns_main.incn -- specs/comic/corpus/manifest.json
```

The reader receipt requires all 107 column cases, 292 actual decode executions after expanding proper prefixes, and 52 original-choice decode/re-encode checks. Expected bytes and digests remain independent corpus literals. Accepted cases decode actual values, descriptions and choices, then pass those results to the writer; the adapter never echoes expected bytes as implementation output. Refusals compare typed categories, and setup failures remain test failures.

Focused controls exercise independently retained packed-field handles, a small selected field beside a large unselected field, corruption in an unselected block, complete-prefix boundaries, bounded dictionary restart lookup, repeated-run expansion and reconstructed-prefix resource accounting. These are finite format-value operations. Generated application model reconstruction and transport streaming are not claimed.

## Column-reader callback cost

Column admission and record materialization attach a field identity only when a nested operation fails. These paths use direct Result matching so that error-context callbacks do not capture the complete descriptor dictionary on successful reads. The projection test also exercises a complete-record byte refusal and verifies that the failing field and original cause survive; the malformed-unselected-column case checks the same attribution during admission.

This removes a particular avoidable copy, not every copy in binary decoding. Profile a fixed wide recordset at 100, 1,000 and 10,000 records using the same input bytes, separate decode processes and consumed checksums. Compare admission and record-scan stages separately. Retain native stack samples to distinguish callback capture from parameter passing, retained backing and explicit output construction. Functional budget tests do not detect uncharged allocation costs, and no machine-dependent timing threshold belongs in them.

## Text-reader scaling contract

For a fixed declaration and fixed-width records, multiplying the number of records must multiply tokenization and value-reading work proportionally. A successful per-value budget check must not copy the document's token snapshot. This includes work hidden in callback captures: referring to the cursor from an error callback can copy the entire cursor when the callback is constructed, even if the operation succeeds. Inspecting the cursor only in a direct error branch avoids that cost and preserves the refusal's exact position.

`tests/test_text_limits_and_scaling.incn` checks exact depth/element refusal positions, exact successful element allowances, and proportional token/value charging at 10, 100 and 1,000 records. Those tests measure charged work, not allocations: an uncharged copy can still pass them. The allocation regression check is a separate native profiling contract. Profile the same fixed-width recordset at increasing sizes, with encoding and decoding in separate processes, and inspect successful parser stacks for whole `TextTokens` / `Vec<TextToken>` copies per value. Such a copy is a failure regardless of elapsed time. Record 100, 1,000 and 10,000-record decode measurements alongside the profile to show the effect; keep any resource-censored case explicitly censored. Do not turn those measurements into a machine-dependent wall-clock test threshold.

Keep the independent grammar corpus and the full conformance suite green. Compare the printer and both binary codecs on the same field and record counts so an improvement in one reader is not presented as proof of every codec's scaling. Large individual scalars, collection ordering and growing description sets need their own measurements; this contract does not claim linear behavior for every possible document shape.
