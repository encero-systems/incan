# Experimental COMIC conformance runner

The runner reads the committed corpus directly and verifies every data and text file against its manifest digest. Case identities must be globally unique, declared counts must match, and an empty selection is a failure. Exact integer expectations stay strings; float expectations stay explicit bit labels.

```text
incan run workspaces/comic-conformance/src/main.incn -- specs/comic/corpus/manifest.json --inventory
```

`--inventory` exits successfully only after those input checks. Its JSON receipt still says `status: incomplete`, with every unimplemented operation marked `unsupported`. It does **not** claim that those cases passed. Omitting `--inventory` runs every dispatch and exits 3 while any required operation is unsupported; an executed failure takes precedence and exits 1. A receipt distinguishes passed, failed and unsupported cases and records failure details. Invalid input, stale pins and duplicate identities exit 1; invalid command arguments exit 2.

The runner executes member identities, nominal identity preimages and hashes, closed type selectors, written-shape refusals and adaptation comparisons from the corpus. Accepted adaptation cases also assert logical projections: until those projections are implemented, their value comparisons run but the cases remain explicitly unsupported. Other hash and codec operations remain unsupported. The bounded primitive has independent tests for nested records, enum payloads, union identity, recursive plans and shared limits. It must compare the implementation's output against the separately authored expectations; hashing a supplied expected preimage alone is not a hash-conformance test.

JSON and filesystem convenience APIs are dependencies of this test project only. The production Crumb foundation belongs to `std.primitives.crumb` and imports no higher-level standard package.
