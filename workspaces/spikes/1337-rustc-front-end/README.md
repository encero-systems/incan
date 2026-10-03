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
| 4 | `step4_model_adt.rs` | An Incan model is a real Rust struct, declared by AST injection. Rust constructs a `Point` and passes it to an Incan function whose MIR reads its fields; Incan MIR constructs a `Point` and returns it, and Rust reads its fields. No conversion happens in either direction — RFC 121's representation identity, literally. |
| 5 | `step5_enum_adt.rs` | An Incan enum is a real Rust enum. Incan MIR matches on it (a `SwitchInt` over its discriminant, then a payload read through a downcast) and constructs its variants, unit and tuple alike; Rust `match`es what Incan built and passes Incan variants it built itself. |
| 6 | `step6_generic_function.rs` | A generic Incan function over a generic Incan model. `choose[T: Copy](pair: Pair[T], take_first: bool) -> T` is declared with its parameter and bound, and its MIR is written once against `T`. Rust instantiates it at `&str` and `f64`; Incan code instantiates the model and the function at `i64`. rustc monomorphizes each instance. |
| 7 | `step7_incn_spans.rs` | Spans point at `.incn` sources. The driver loads `scores.incn` into rustc's source map and gives the MIR for `a + b` that expression's span. An overflow panics at `scores.incn:3:12`, and with `-g` the backtrace frame for `add_scores` names the same line. |

rustc's borrow checker validates the constructed MIR, as it would for any Rust function. Incan's ownership facts therefore still have to describe valid Rust ownership, which is already true of the code the emitter generates today.

Steps 5 to 7 share `common.rs`: AST builders for declarations, and `Cfg`, a small builder that adds and terminates basic blocks. `Cfg` is the shape the Body IR lowering grows from.

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
- **rustc checks constructed MIR, but its failures are not Incan diagnostics.** Removing `choose`'s `Copy` bound, so its MIR copies a `T` that need not be `Copy`, makes rustc fail with an internal compiler error (a delayed bug), not a diagnostic. Moving a value and then reading it gives a real `E0382`, but in Rust's vocabulary and at whatever span the MIR carries. So Incan's checker must reject ill-typed and ill-owned programs before lowering. rustc's checks are a backstop, and a failure there is a front-end bug. This matches RFC 120's rule that diagnostics are Incan's own.
- **Spans are free once the file is in the source map.** `SourceMap::load_file` registers the `.incn` file. Spans built from its `start_pos` flow into panic locations, debuginfo and backtraces with no further work.
- **rustc's internal API moves between releases.** This spike hit five differences against older documentation (`Terminator::attributes`, `Spanned`'s path, `Defaultness::Implicit`, `Rvalue::Use` taking a `WithRetag`, and field types returned as `Unnormalized`). Each rustc upgrade is a front-end upgrade; the toolchain already pins one rustc, which bounds that cost. The `rustc-dev` component installs rustc's sources under `lib/rustlib/rustc-src`, which is the reference for each upgrade.

## Not yet tested

Lowering real Body IR rather than hand-built MIR, #1337's four boundary rows (native, both directions), and Oven driving the front end with JEC unit reuse.
