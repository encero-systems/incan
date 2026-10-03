---
id: DD-0004
title: Compile Incan natively through the pinned rustc, with a driver Loaf that Oven invokes
status: Draft
type: design-decision
date: 2026-10-03
review_target: v0.6
sources:
  - https://github.com/encero-systems/incan/issues/1337
  - https://github.com/encero-systems/incan/issues/1675
  - https://github.com/encero-systems/incan/issues/654
  - DD-0002
  - RFC 097
  - RFC 119
  - RFC 120
  - RFC 121
  - RFC 123
  - RFC 124
---

# DD-0004: Compile Incan natively through the pinned rustc, with a driver Loaf that Oven invokes

## Context

By the end of 0.6 there is no Cargo, no generated Rust, and Oven is fast and usable. Today an Incan program becomes native code one way: the compiler emits Rust source and Oven compiles it with direct rustc. Removing Rust generation (#654) needs a route that produces native code without Rust source, and #1337 owns it.

#1337 settled the direction: Incan becomes a second front end to the rustc the toolchain pins (DD-0002), so Rust crates and Incan modules share one crate graph and rustc does what only it can do, namely instantiate generic Rust, resolve traits and lower async. What it left open is the shape of the front end. That front end has to use rustc's unstable internal interface, which changes every release, and the shape decides how far that instability reaches into the rest of the compiler and toolchain.

A spike on `feature/1337-rustc-front-end` (`workspaces/spikes/1337-rustc-front-end/run.sh`) settled the mechanism on Rust 1.98.0, built and driven by plain rustc with no Cargo. It showed the following:

- Bodies supplied as MIR replace placeholder bodies.
- Items injected after parsing are real rustc items that Rust names and rustc checks calls against.
- Models and enums are Rust ADTs that Rust constructs and matches and Incan code reads.
- A generic Incan function and model are written once and instantiated from Rust and from Incan.
- `.incn` spans reach panic locations and backtraces.
- #1337's four boundary rows run across a three-unit graph whose Incan unit has an empty crate root:
    - a free call;
    - an owned `str` borrowed as `&str`;
    - a capturing closure expression passed as `impl FnOnce`;
    - a Rust caller using `<library>::caller::incan`, instantiating a generic Incan function from metadata.
- Incan code drives a Rust layer built against rustc's internals. One driver builds that layer, an Incan unit that fills its body plan, and a Rust executable that calls the Incan unit. The executable runs rustc in its own process and compiles a program whose function body the Incan code planned.
- Real Body IR runs natively. The driver runs this repository's Incan front end in-process and lowers the unchanged kernels of the `fib` and `collatz` benchmarks to MIR, calling the same stdlib runtime helpers the emitted route calls. The output is identical to the emitted-Rust route's for the same sources, and optimized runtime is on par.
- Whole programs compile natively. The unchanged `fib`, `collatz` and `mandelbrot` benchmark programs, `main` included, compile from source to native binaries with no Rust source at any point. Their output is identical to the emitted route's, and optimized runtime is at parity.
- A first corpus native lane runs. It compiles every single-file behavior fixture that records its expected output, runs it natively, and compares stdout and exit code. 23 fixtures pass, none produce wrong output, none fail, and 440 are refused with the construct they need named. The lane caught two behavior differences and three lowering defects, all fixed or refused:
    - a `@rust.extern` placeholder run as a body (#2023);
    - an `import this` whose module effect Body IR does not carry.

    The largest refusal is imports, at 113 fixtures: a program that imports from the stdlib needs the stdlib's own units compiled natively, which is where Oven building several units, and the toolchain building its own stdlib, come in.

## Decision

**The driver is its own Loaf, pinned to rustc.**

- The front end is a rustc driver, a separate Loaf built against exactly the toolchain's pinned rustc.
- It is the only code in the project that uses rustc's internal interface.
- It refuses to run, before any effect, when the rustc library it loads is not the pinned one.
- Permission to use rustc's internals is a declared property of a unit's invocation, which the driver grants through rustc's tracked `unstable_features` option, so it is part of the unit's identity. It is never inherited from an ambient `RUSTC_BOOTSTRAP`, which Oven already strips. Only the driver Loaf's own units declare it.
- The driver's executable root depends on rustc's shared library directly, because rustc links `std` from that library, which already contains it, only for a root that names it.
- This is how Rust ships its own tools that need rustc's internals: a separate driver built against one exact rustc, invoked by Cargo in rustc's place.

**Oven invokes it per unit, for both languages.**

- Oven plans the units and invokes the driver once per unit, as Cargo invokes rustc.
- A Rust unit given to the driver compiles exactly as rustc would compile it.
- Every unit, in either language, therefore has one compiler identity. The driver's build identity is part of the identity of every unit it compiles (RFC 124).

**Oven keeps the driver resident.**

- Oven owns a long-running driver process and reuses it across units.
- The language server starts it and keeps it warm while an editor is open.
- Terminal and CI builds use it when it is running, and start it when it is not.

**The Incan front end runs inside the driver.**

- The checker and Body IR do not use rustc's internals, so the driver depends on them as ordinary libraries and runs them in-process.
- There is no serialized handoff between the checker and the lowering. In particular, RFC 123's executable representation is not a carrier for the driver.
- Commands that do not compile, such as `incan check` and the language server's analysis, never load rustc.

**Declarations become rustc items; bodies become MIR.**

- Every Incan declaration enters rustc as an item before rustc resolves names. That covers functions, models, enums, generic parameters with their bounds, modules, re-exports, and RFC 097's caller namespace.
- A function's source body is a placeholder that is never compiled. Its compiled body is MIR lowered from checked Body IR.
- An Incan unit's crate root contains no Rust source, and no Rust source is produced for it, on disk or in memory.
- Models and enums are Rust ADTs with RFC 121's representation.
- Every linker-visible symbol for an Incan declaration is recoverable to its canonical identity (RFC 120).

**Bodies enter as MIR, and rustc elaborates drops.**

- Bodies are supplied as MIR, not THIR. THIR names variables by `HirId`, and rustc schedules their drops through a scope tree computed from the HIR body, which for an Incan function is a placeholder. Supplying THIR would require first mirroring every body into AST, which is generated Rust under another name.
- The lowering emits a drop for every owned value at each scope exit and a cleanup block on every call that can unwind, as rustc's own MIR building does. rustc's drop elaboration removes the drops of values that were moved, so the lowering never tracks which paths moved a value. The spike shows each value dropped exactly once on normal and unwinding paths.

**Closure expressions declare their captures.**

- Each closure expression enters as a closure inside its enclosing function's declaration, naming the variables it captures.
- rustc determines a closure's kind and captured state from the declaration, not from MIR.

**Incan's checker is the authority.**

- A program the checker accepts must lower to MIR that rustc accepts.
- A rustc rejection of front-end-supplied MIR is a compiler defect, never a user diagnostic. The spike showed why this has to be enforced by the checker: a missing trait bound in supplied MIR makes rustc fail with an internal compiler error, and a use-after-move gives a Rust-worded `E0382` at whatever span the MIR carries.
- User-facing diagnostics come from Incan's checker, and rustc's lints are not reported for Incan items. Lints that run before MIR see only placeholder bodies.

**Calls into Rust come from the call plan.**

- A Rust call's callee and generic arguments come from the checked call plan, resolved by canonical path.
- They are never recovered from generated names.

**New code is Incan unless Rust is clearly better.**

- The native route's new code is written in Incan by default. That includes the Body IR → MIR lowering, drop and unwind building, and the driver's own orchestration.
- Rust is used only where it is the better tool. Here that is the narrow layer that holds rustc's internal types, which are bound to rustc's lifetimes and interned values, plus any code where exact lifetimes and borrows matter for performance.
- Every piece that stays Rust names its reason.
- Once both languages share one crate graph, splitting a component across them costs nothing at the boundary, so this extends Incan's standing self-hosting rule to the compiler's own internals.

**Ring placement.**

- The lowering from Body IR to MIR belongs to the compiler ring, beside the checker that owns Body IR.
- The driver executable belongs to the toolchain ring.
- Oven learns the driver's location through the compiler's provider facet, so the Oven ring continues to know Incan by name only.

## Consequences

**Upgrade cost is bounded.** Each rustc upgrade includes a driver upgrade, and nothing else in the compiler moves. In about six hundred lines, the spike hit five differences from older rustc documentation, so the cost is real but contained in one Loaf.

**Nothing new is needed at run time.** The `rustc` executable is itself a thin binary over rustc's shared library, and the driver loads the same library. The developer-only `rustc-dev` component is needed to build the driver, not to run it. Building the driver needs rustc's unstable-features escape hatch.

**The resident driver saves the fixed cost and no more.** Measured on macOS arm64 for a minimal unit:

| | time |
|---|---|
| Compile and link in a fresh process | ~73 ms |
| Process start and loading rustc's library | ~21 ms |
| Each compilation in an already-running process | ~41 ms |
| Analysis alone | ~2 ms |

So code generation and linking dominate a small unit. rustc does not reuse analysis across compilations in memory.

The inner loop on real Body IR, with an optimized driver and the front end in-process: compiling the Incan unit of the two benchmark kernels takes about 43 ms, of which analysis is about 8 ms above process start. Linking the binary takes about 89 ms. A one-line edit therefore reaches a linked binary in about 132 ms, against the 0.6 bar of 250 ms. For a whole program compiled as one unit, unchanged `fib.incn` source becomes a linked native binary in about 70 ms in a fresh driver process, so a one-line edit reaches first output in about 75 ms, against 680 ms today.

**The lowering is the largest piece of work.**

- It turns structured Body IR into a control-flow graph and emits drops and cleanup blocks; rustc's drop elaboration handles which drops actually run.
- When the lowering is wrong, the failure is an internal compiler error rather than a readable diagnostic. So lowering defects are harder to diagnose than emitted-Rust defects were.

**Builtins the emitter expands as macros become runtime functions.** The emitted route turns `println` into Rust's `println!` macro, which no MIR can call. Natively each such builtin is a call to a stdlib runtime function that invokes the macro itself, so behavior such as output capture under a test harness is unchanged.

**The checker carries more facts.** It must hold complete trait obligations and closure capture facts, because the lowering depends on both. Body IR must also record parameter modes. Today it passes an argument to a `mut` parameter as a copy and lets the callee drop it, where RFC 129 and the emitted route write through (#2022). It must also carry the facts the lowering currently compensates for: extern delegation (#2023), the checked type of a literal, which leaves an `int` literal accepted as a `float` as an integer constant, the receiver mode of mutating builtin methods, and the precise type of a `range(..)` value.

**Open, to settle before this record is accepted:**

- **How the Incan lowering reaches rustc.** The lowering is Incan by decision. A narrow Rust layer owns every rustc type and presents a plain builder of blocks, places, calls and drops addressed by index. The lowering drives that builder and is compiled first by the previous compiler. On the native route this works: the spike's Incan unit calls that layer as an ordinary call in one crate graph, and the executable embedding it runs rustc. Incan code also matches Rust enums of Body IR's three variant shapes, struct variants by named field patterns, so the lowering can consume Body IR directly. Still untested is the real checker typing the Rust layer's API through Rust inspection, which reads the layer's source while that source names rustc's internal crates.
- **Generic bodies of published Loaves.** How do they reach a native consumer: through the rustc metadata of the Loaf's compiled unit, through RFC 123's executable representation, or both?
- **Async.** How do Incan `async` bodies lower: as MIR coroutines the front end builds, or by another route?

## Non-goals

- A native code generator of Incan's own, such as Cranelift. Generic, trait and async Rust can only be instantiated by rustc.
- An interpreter that calls compiled Rust. The Body IR interpreter proved Body IR carries complete semantics; it does not ship.
- Rust source generated in memory and handed to rustc. That is still generated Rust.
- A fork of rustc. The driver already gets everything a fork would give: it decides unstable-feature permission per unit, and rustc's driver callbacks and query overrides carry items, bodies, spans and cross-crate metadata. A fork would not remove the churn of rustc's internals; it would turn a driver upgrade into rebasing a patch series every release. It would also mean building and shipping rustc and LLVM for every target, owning security backports, and making Rust built through Incan differ from upstream. Revisit only when a needed hook cannot be had through callbacks or query overrides, such as resolving Incan items without AST injection, injecting THIR, or tying rustc's incremental reuse to unit identity. Even then, propose the hook upstream first, as Clippy and Miri did for theirs.
- Linking rustc into the compiler. Every compiler binary, including the language server, would then carry rustc's library and build against its unstable interface.
- Inspection of the native route. That is a separate design.
- More than one rustc per toolchain release (DD-0002).

## Revisit condition

Revisit this record when any of the following happens:

- A rustc upgrade costs more than a driver upgrade.
- The driver needs a rustc hook that callbacks and query overrides cannot provide, and upstream declines to add it.
- The resident driver fails the 0.6 inner-loop bar: an unchanged `incan run` under 50 ms over the program's own run time, a one-line edit to first output under 250 ms, and a test case that builds a project under 1 s.
- The interop check shows that the lowering cannot reach rustc from Incan.

## Provenance

This record derives from #1337's design and its spike, and from the 0.6 end state recorded on #1675 and #654. It applies DD-0002's single pinned rustc and is consistent with RFC 097, RFC 119, RFC 120, RFC 121 and RFC 124. It supersedes the open driver-shape question in #1337's design comment of 2026-10-03, which preferred a thin driver fed serialized checked facts. It does not change any RFC.

## References

- #1337: Incan compiles natively at HIR through the pinned rustc, with no generated Rust.
- #1675: Slice 7, the 0.6 cutover.
- #654: removal of Rust generation.
- Spike: `workspaces/spikes/1337-rustc-front-end/` on `feature/1337-rustc-front-end`.
- DD-0002, RFC 097, RFC 119, RFC 120, RFC 121, RFC 123, RFC 124.
