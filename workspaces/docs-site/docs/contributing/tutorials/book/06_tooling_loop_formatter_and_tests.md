# 6. Tooling loop: formatter + tests

This chapter explains the contributor “inner loop” for making changes safely:

- format code consistently
- run tests quickly
- prevent drift between syntax, compiler, and tooling

## End-to-end checklist

Just like in the previous chapter, use this checklist to keep the pipeline aligned while you work:

--8<-- "_snippets/contributing/compiler_pipeline_checklist.md"

## Formatter (`incan fmt`)

The formatter is part of the toolchain and should remain aligned with the parser/AST:

- it should be idempotent (formatting twice yields the same output)
- it should never print invalid syntax

See:

- [Code formatting](../../../tooling/how-to/formatting.md)

Where it lives:

- `src/format/`

## Testing (Rust tests + integration checks)

Depending on your change, you will usually run:

- `make test` for the Rust unit/integration test suite (fast feedback)
- `make pre-commit` for a fast local gate (fmt-check + cargo check)
- `make pre-commit-full` before pushing (fmt-check + tests + clippy)
- `make smoke-test` when you want extra confidence (build + tests + examples + benchmarks-incan)

See:

- [Testing](../../../tooling/how-to/testing.md)

## A practical workflow

When you add a feature:

1. add/adjust a small Rust regression test (parse/typecheck/codegen)
2. run `make test`
3. run `make pre-commit` as a quick local sanity pass
4. run `make pre-commit-full` before opening the PR
5. run `make smoke-test` if the change touches the pipeline end-to-end (parser/typechecker/codegen/tooling)

## Next

Next chapter: [07. Tooling loop: LSP](07_tooling_loop_lsp.md).
