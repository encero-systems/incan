# Probe: where the Incan lowering meets rustc (stage 3a)

DD-0004 left one question open: can the real checker type the Rust builder layer's API through Rust inspection while that layer names rustc's internal crates?

Two probes answer it.

- **`../rustc_seam_check/`**: the `[rust-dependencies]` crate is step 9's seam: `#![feature(rustc_private)]`, `extern crate rustc_middle` and the rest. Its public API holds no rustc type. `incan oven bake --project .` fails while compiling the crate (`E0463: can't find crate for rustc_middle`). A normal bake builds every Rust dependency, and a crate built on rustc's internals cannot build there without the declared `rustc_private` permission, which only the driver unit gets. `incan check` needs the inspection record that bake writes, so the checker never sees the seam.
- **`mir_plan_check/`** (this probe): the `[rust-dependencies]` crate holds only the plan: `BodyPlan` and its builder methods, plain data with no rustc crate at all. It bakes, the Incan program typechecks against it, and `incan run` prints:

```text
1
42
```

So the seam between the Incan lowering and rustc is a plain-data plan crate.

- The Incan lowering builds the plan: Body IR in, plan out. It never calls into rustc code.
- The driver, the one unit allowed rustc's internals, takes the finished plan and turns it into MIR.
- Neither side names the other's types.
- Incan code does not run rustc, so step 9's in-process `compile` call is not part of the product design.

Run it with an Incan toolchain from the dev line: `incan oven bake --project .`, then `incan run`.
