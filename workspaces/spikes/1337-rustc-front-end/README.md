# #1337 front-end spike: Incan items in rustc without Rust source

This spike tests the mechanism #1337 depends on: Incan as a second front end to the pinned rustc, so that Rust crates and Incan modules share one compilation graph and no Rust source is produced.

It is evidence, not the front end. Nothing here is built by the workspace, and nothing in the compiler depends on it.

## What it proves

Every step runs on the pinned Rust 1.98.0 with the `rustc-dev` component, built and driven by plain `rustc`. Cargo is not involved.

| step | file | proves |
| --- | --- | --- |
| 0 | `step0_driver.rs` | A driver built against rustc's own crates compiles and runs a program on the pinned toolchain. |
| 2 | `step2_mir_body.rs` | A function's body can come from MIR the front end constructs instead of from Rust source. That MIR calls the **generic** `core::cmp::max::<i64>`, and rustc monomorphizes it. The source body is `loop {}`; the program would hang if rustc compiled it. |
| 3 | `step3_injected_declaration.rs` | A function can be declared with no source at all, by injecting an AST item after the crate root is parsed. `step3_program.rs` never declares `answer`. Its MIR body calls the user-written Rust function `double`, and Rust's `main` calls `answer` back: one crate graph, calls in both directions. |

rustc's borrow checker validates the constructed MIR, as it would for any Rust function. Incan's ownership facts therefore still have to describe valid Rust ownership, which is already true of the code the emitter generates today.

## Running it

```sh
rustup component add rustc-dev --toolchain 1.98.0
./run.sh
```

## Measured

The step-3 driver compiles and links `step3_program.rs` to a native binary in about 70 ms, debug or `-C opt-level=3`, against about 80 ms for plain rustc on the equivalent source (macOS arm64, median of five). Today an unchanged `incan run` costs about 440 ms and a one-line edit about 680 ms, so rustc itself, linking included, is not the bottleneck behind the 0.6 inner-loop bar.

## Findings that shape the design

- **Declarations enter as AST items; bodies never do.** Items need a `DefId` and a signature for Rust to name them and for rustc to type-check calls, so they are injected in `after_crate_root_parsing`. The AST body is a diverging placeholder that type-checks against any return type; the real body is always the MIR supplied through the `mir_built` query.
- **rustc's lints see the placeholder, not the real body.** Step 3 warns that `double` is never used, because the dead-code lint runs on HIR before MIR exists. The front end has to cap rustc's lints for Incan items. Source diagnostics stay Incan's own, which is RFC 120's rule anyway.
- **Body IR is structured; MIR is a control-flow graph.** Lowering Body IR into MIR needs a CFG-building pass, the same job rustc's `mir_build` does from THIR. This is the largest engineering piece.
- **A Rust call is a `Call` terminator.** `Operand::function_handle` takes the callee's `DefId` and its generic arguments; the checked call plan supplies both.
- **rustc's internal API moves between releases.** This spike hit three differences against older documentation (`Terminator::attributes`, `Spanned`'s path, `Defaultness::Implicit`). Each rustc upgrade is a front-end upgrade; the toolchain already pins one rustc, which bounds that cost.

## Not yet tested

Generic Incan functions, Incan models and enums as Rust ADTs, mapping spans to `.incn` sources for diagnostics and debuginfo, lowering real Body IR rather than hand-built MIR, and Oven driving the front end with JEC unit reuse.
