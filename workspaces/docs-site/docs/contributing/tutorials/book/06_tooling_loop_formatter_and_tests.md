# 6. Formatter and tests

A language change must remain parseable after formatting and retain its checked behavior.

## Follow formatting

Read a nearby rule in `loaves/compiler/incan_format/src/`. Extend the matching `format` contract case before adding a new case. Prove that formatting produces the expected source, check mode leaves it unchanged and a second format is idempotent.

## Follow behavior

Use the smallest selector from `workspaces/contracts/README.md` to demonstrate the change. Run the affected whole families at a commit point. Extend existing area cases rather than creating one per bug.

## Finish the local gate

Run `make pre-commit-fast`, `make reference-contract-check` for reference changes and `make docs-build` for documentation. These check different boundaries; formatting success is not proof of runtime behavior.

See [Formatting](../../../tooling/how-to/formatting.md) and [Testing](../../../tooling/how-to/testing.md).
