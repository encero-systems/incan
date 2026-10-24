---
name: test-suite
description: Validate an Incan change with focused contract selectors, affected whole families and repository gates.
---

You validate compiler changes through the installed Incan toolchain and the repository's contract runner. The repository has no Cargo project. Read `workspaces/contracts/README.md` before selecting commands.

## Process

1. Inspect the requested diff and identify the checked behavior and owning compiler stage.
2. Select the smallest existing contract case that proves the behavior. Extend it before adding a case, preserving its original expectations.
3. Establish the failing behavior, rebuild only the affected compiler components, and run that selector again.
4. At commit points, run the affected whole families with the same toolchain and worker budget used for the parent comparison.
5. Run `make pre-commit-fast` and `make us-english-check`. Run `make docs-build` from `workspaces/docs-site` for docs changes.
6. Report exact commands, compiler identity, counts and case movements. Distinguish a passing focused case from a completed family replay.

## Path-to-contract mapping

| Changed area | Verification |
| --- | --- |
| `loaves/kernel/incan_syntax/src/parser/` | Matching `check`, `behavior` and `format` cases |
| `loaves/compiler/incan_frontend/src/typechecker/` | Matching accepted/refused `check` cases and executable `behavior` cases |
| `loaves/compiler/incan_frontend/src/body_ir/` | Executable `behavior`, `library` and `interop` cases for the changed boundary |
| `loaves/compiler/incan_mir_lowering/src/` | Focused executable cases followed by the affected families |
| `loaves/toolchain/incan-rustc-driver/src/` | Native `behavior`, `interop`, `library` or `testing` contracts for the changed adapter |
| `workspaces/oven/src/` | Its matching Incan tests and `project`, `library`, `testing` or `graph` contracts |
| `loaves/compiler/incan_format/src/` | Matching `format` cases |
| `loaves/stdlib/` | Matching public standard-library behavior and interop cases |

The former emitter snapshot, CLI integration and compiler-test-support fixture roots were removed. Their names do not identify runnable test targets on dev.8.

## Evidence and failures

Use a separate scratch directory for each frozen compiler comparison. Do not replace a running comparison's driver or standard packages. Preserve per-case output and process status; a refusal, crash, timeout and output mismatch are different observations.

An expected refusal belongs in the case header with its diagnostic codes. Do not turn an unexpected failure into an expected outcome merely to make a run green. Name any unresolved case and its blocking mechanism.

The suite budget is about 3,000 tests. Extend an existing case in the same area and keep one producer per library case. New or changed Rust functions need rustdoc, and new Incan declarations need docstrings. Tests propagate errors with Result rather than `.unwrap()` or `.expect()`.
