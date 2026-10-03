# RFC 130: Incan compiles natively through the pinned rustc

- **Status:** Draft
- **Created:** 2026-10-03
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 097 (Rust-hosted Incan caller)
    - RFC 117 (`loaf.toml` and Oven's language-neutral project model)
    - RFC 119 (Oven-native Rust build facets and Cargo interoperation)
    - RFC 120 (canonical source symbol identity)
    - RFC 121 (unified Incan/Rust type substrate)
    - RFC 123 (package executable representation)
    - RFC 124 (Oven store unit identity and cross-plan artifact sharing)
- **Issue:** [#1337](https://github.com/encero-systems/incan/issues/1337)
- **RFC PR:** —
- **Written against:** v0.6 (in development)
- **Shipped in:** —

## Summary

Incan becomes a second front end to the rustc the toolchain already pins. Incan declarations enter rustc's compilation as ordinary items, and their bodies enter as MIR lowered from checked Body IR; rustc then borrow-checks, monomorphizes, optimizes and generates code exactly as it does for Rust. The front end is a rustc driver that is its own Loaf, built against that one rustc. Oven invokes it once per compilation unit, for Incan units and Rust units alike, the way Cargo invokes rustc. No Rust source is produced at any point, and Cargo is not involved. Rust crates and Incan modules share one crate graph, so a call from Incan to Rust, or from Rust to Incan, is an ordinary call rather than a boundary crossing.

## Core model

1. **One compiler, two front ends.** rustc is the back end for both languages. Rust source enters through rustc's parser; Incan source enters through Incan's checker. Everything after MIR is shared.
2. **Declarations are items, bodies are MIR.** Each Incan declaration becomes a rustc item with a real signature, so Rust can name it and rustc can check calls against it. Each body is MIR the front end supplies; rustc never compiles an Incan body from any other form.
3. **The driver is its own Loaf.** It is the only code that uses rustc's internal interface, and it is built against exactly the toolchain's pinned rustc.
4. **Oven is what runs.** Oven plans the units and invokes the driver per unit, as it invokes rustc today. A Rust unit given to the driver compiles exactly as rustc would compile it.
5. **Incan's checker is the authority.** A program the checker accepts lowers to MIR that rustc accepts. rustc's own checks are a backstop: a rustc failure on front-end-supplied MIR is a compiler defect, never a diagnostic for the user.

## Motivation

By the end of 0.6 there is no Cargo, no generated Rust, and Oven is fast and usable. Today an Incan program becomes native code one way: the compiler emits Rust source and Oven compiles it with direct rustc. Removing that route needs another one that produces native code without Rust source, and nothing else owns one. The Body IR interpreter proved that Body IR carries the complete semantics of a program, but it produces no binary and cannot call compiled Rust.

The route also has to keep Incan and Rust one language with two syntaxes. RFC 121 makes values representation-identical across the two, and RFC 097 lets Rust call Incan through a stable namespace. Both are easiest to honour, and only fully honoured, when the two languages are compiled by one compiler into one crate graph: generic Rust functions, trait resolution and async lowering are things only rustc can instantiate, so any route that does not end in rustc needs rustc-compiled glue at every Rust call.

Speed is the other constraint. The 0.6 bar is an unchanged `incan run` under 50 ms above the program's own run time, a one-line edit to first output under 250 ms, and a test case that builds a project under 1 s. Generating, formatting and re-parsing Rust source on every edit costs time that a direct route does not spend.

## Goals

- Compile Incan programs to native binaries and libraries with no Rust source produced and no Cargo invocation.
- Put Incan and Rust units in one crate graph, with calls in both directions and representation-identical values (RFC 121).
- Make every Incan declaration a real rustc item: functions, models, enums, generic declarations with their bounds, and closure expressions.
- Report panics, debuginfo and backtraces against `.incn` source lines.
- Keep user-facing diagnostics Incan's own.
- Bound the cost of rustc's unstable internal interface to one Loaf.
- Let Oven drive both languages through one compiler identity, so unit reuse (RFC 124) applies to Incan and Rust units the same way.

## Non-Goals

- A native code generator of Incan's own, such as Cranelift.
- Interpreting Body IR, or calling compiled Rust from an interpreter.
- Producing Rust source as an inspection artifact. Inspection of the native route is a separate design.
- Supporting more than one rustc per toolchain release.
- Showing rustc's diagnostics or lints to users for Incan code.

## Guide-level explanation

### Building is unchanged; Rust source is gone

A developer runs `incan build`, `incan run` or `incan test` as before. Oven plans the project's units and compiles each one through the driver. Nothing under the project, the store or a temporary directory holds Rust source generated from Incan; the artifacts are the same libraries and binaries Rust would produce.

### Incan calls Rust as Rust does

```incan
from rust::host import apply, increment, text_len

def bump(value: int) -> int:
    return increment(value)

def measure(text: str) -> int:
    return text_len(text)

def shift(value: int, offset: int) -> int:
    return apply(value, (x) => x + offset)
```

`increment` is a plain Rust function, `text_len` takes `&str`, and `apply` takes `impl FnOnce(i64) -> i64`. Each call is an ordinary call in the same crate graph. Passing `text`, an owned `str`, where Rust expects `&str` borrows it; the closure expression `(x) => x + offset` is a real Rust closure that captures `offset`, and `apply` is instantiated at its type by rustc.

### Rust calls Incan as Rust does

A Rust unit reaches an Incan library through RFC 097's caller namespace:

```rust
use policy::caller::incan::{Pair, choose, shift};

fn main() {
    let picked = choose(Pair { first: "first", second: "second" }, false);
    println!("{} {}", shift(10, 5), picked);
}
```

`Pair` is the Incan model itself, not a copy of it, and `choose` is a generic Incan function that this Rust unit instantiates at `&str`.

### Failures point at Incan

A panic in an Incan body names the `.incn` file, line and column, and a backtrace frame for an Incan function names the same place. A program the checker rejects is reported by Incan's checker, in Incan's terms; rustc never reports on Incan code.

## Reference-level explanation

### The driver

- The driver must be built against exactly the rustc the toolchain pins, and must refuse to run, before any effect, when the rustc library it loads is not that one.
- The driver must be a Loaf of its own. No other Loaf may use rustc's internal interface.
- Given a unit with no Incan sources, the driver must produce the same artifacts rustc would for the same invocation.
- The driver's build identity must be part of the identity of every unit it compiles (RFC 124), for Rust units and Incan units alike.

### Units and invocation

- Oven must invoke the driver for every unit it compiles natively; it must not invoke Cargo.
- An Incan unit must compile from Incan sources and checked facts only. Its crate root must contain no Rust source, and no Rust source may be produced for it, whether written to disk or held in memory as text.
- Dependencies of an Incan unit must be the units its checked program names, and only those.

### Declarations

- Every Incan declaration a unit defines must enter rustc as an item before rustc resolves names: functions, models, enums and their variants, generic parameters with their bounds, modules, and re-exports.
- A function item's source body must be a placeholder that is never compiled; its compiled body must be the MIR the front end supplies.
- Each closure expression must enter as a closure inside its enclosing function's declaration, with the variables it captures and how it captures them, because rustc determines a closure's kind and captured state from the declaration.
- Models and enums must be rustc ADTs whose layout is the representation RFC 121 defines. No conversion may occur when values cross between Incan and Rust code.
- Every linker-visible symbol for an Incan declaration must be recoverable to that declaration's canonical identity (RFC 120).

### Bodies

- Each body must be MIR lowered from checked Body IR.
- That MIR must be well-typed and well-owned for every program the checker accepts. When rustc rejects front-end-supplied MIR, that is a compiler defect and must be reported as one, never as a user diagnostic.
- Owned values must be dropped on every path out of their scope, including unwinding.
- A call into Rust must name its callee and generic arguments from the checked call plan, resolved by canonical path. It must never be recovered from a generated name.
- A conversion the checker selects at a call, such as borrowing an owned `str` as `&str`, must lower to the same operations rustc would generate for the equivalent Rust.

### Spans and diagnostics

- The `.incn` sources of a unit must be part of rustc's source map for that compilation, and MIR must carry spans in them, so that panic locations, debuginfo and backtraces name Incan source lines.
- User-facing diagnostics for Incan code must come from Incan's checker. rustc's lints must not be reported for Incan items.

### Rust-hosted callers

- An Incan library's RFC 097 caller namespace must be a real module of its unit, so a Rust unit compiled against it calls Incan items directly.
- A generic Incan function exposed there must be instantiable by the Rust caller.

## Design details

### Why the driver is its own Loaf

rustc's internal interface changes every release. Building the spike against Rust 1.98.0 hit five differences from older documentation in about six hundred lines. Keeping that interface inside one Loaf bounds each rustc upgrade to that Loaf, which the toolchain already re-provisions with every rustc it pins. This mirrors how Rust ships its own tools: a tool that needs rustc's internals is a separate driver built against one exact rustc, and Cargo invokes it in rustc's place.

The run-time cost is nothing new. The `rustc` executable is itself a thin binary over rustc's shared library, and the driver loads the same library. The developer-only `rustc-dev` component is needed to build the driver, not to run it.

### Why the front end runs inside the driver

The Incan checker and Body IR do not depend on rustc's internals, so the driver can depend on them as ordinary libraries and run them in-process. That removes any serialized handoff between the checker and the lowering: there is no format to keep in step with Body IR, and nothing the lowering needs can be missing from it. The commands that do not compile, such as `incan check` and the language server, still never load rustc.

### Relationship to RFC 119 and RFC 124

RFC 119's direct-rustc plan is unchanged in shape: the driver takes rustc's place in it. Because every unit, in either language, goes through one compiler identity, RFC 124's unit identity and reuse apply to Incan units exactly as to Rust units.

### Relationship to RFC 123

RFC 123's executable representation is not the handoff to the driver, which runs the front end itself. How a published Loaf's generic Incan bodies reach a native consumer is left open below.

### Evidence

A spike on Rust 1.98.0, built and driven by plain rustc with no Cargo, established the following.

- Bodies supplied as MIR replace placeholder bodies.
- Items injected after parsing are real items: Rust code names them and rustc checks calls against them.
- Models and enums are Rust ADTs that Rust constructs and matches and Incan code reads.
- A generic Incan function and model are written once and instantiated from Rust and from Incan.
- `.incn` spans reach panic locations and backtraces.
- The four boundary rows execute across a three-unit graph whose Incan unit has an empty crate root:
    - a free call;
    - an owned `str` borrowed as `&str`;
    - a capturing closure expression passed as `impl FnOnce`;
    - a Rust caller using `<library>::caller::incan`, instantiating a generic Incan function from metadata.

It also found the rules this RFC adopts:

- A missing trait bound in supplied MIR makes rustc fail with an internal compiler error, while a use-after-move yields a Rust-worded `E0382`. Hence the checker's authority.
- rustc determines closure kinds and captures from declarations. Hence captures are declared.
- HIR lints see only placeholder bodies. Hence lints are not reported for Incan items.

Measured on macOS arm64 for a minimal unit:

| | time |
|---|---|
| Compile and link in a fresh process | ~73 ms |
| Process start and loading rustc's library | ~21 ms |
| Each compilation in an already-running process | ~41 ms |
| Analysis alone | ~2 ms |

The remainder is code generation and linking.

## Alternatives considered

- **Hand rustc Rust source generated in memory.** Rejected: the 0.6 end state is no generated Rust, and an in-memory source handoff is still one. It also keeps generating and re-parsing source on every edit.
- **A native back end of Incan's own, such as Cranelift.** Rejected: generic, trait and async Rust can only be instantiated by rustc, so every Rust call would need rustc-compiled glue, and the two languages become two again.
- **An interpreter host that calls compiled Rust.** Rejected: the interpreter does not ship.
- **A thin driver fed serialized checked facts.** Rejected in favour of running the front end in the driver: the serialized form would have to carry everything lowering needs, including closure captures and complete trait obligations, and would be one more format to keep in step with Body IR, with no isolation gained, since the front end does not touch rustc's internals.
- **Linking rustc into the compiler.** Rejected: every compiler binary, including the language server, would carry rustc's library and build against its unstable interface, and every rustc upgrade would ripple through the whole compiler.

## Drawbacks

- The driver depends on rustc's unstable internal interface, so each rustc upgrade includes a driver upgrade.
- Building the driver needs rustc's unstable-features escape hatch and the `rustc-dev` component on the machines that build the toolchain.
- MIR is rustc's internal representation, not a stable contract. The lowering follows rustc's MIR rules for the pinned release.
- When the front end supplies invalid MIR, the failure is an internal compiler error rather than a readable diagnostic, so lowering defects are harder to diagnose than emitted-Rust defects were.
- Each unit still pays rustc's code generation and linking, which dominate a small unit's compile time.

## Implementation architecture

*Non-normative.*

**Ring placement.** The lowering from Body IR to MIR belongs with the compiler, beside the checker it depends on. The driver executable belongs with the toolchain. Oven learns the driver's location through the compiler's provider facet, so the Oven ring continues to know Incan by name only.

**Bilingual driver.** rustc's interface is built around lifetimes and interned types, so a narrow Rust layer should own every rustc type. It can present the lowering with a plain builder of blocks, places, calls and drops addressed by index. The lowering itself, including drop and unwind paths, can then be written in Incan against that builder. The first build of that Incan code goes through the previous compiler, as for the rest of the self-hosted toolchain.

**Resident process.** A driver kept running across units saves process start and library loading, about a quarter of a small unit's compile time. rustc does not reuse analysis across compilations in memory, so a resident driver saves that fixed cost and not more.

## Layers affected

- **Typechecker / Symbol resolution**: the checker must guarantee that accepted programs lower to valid MIR, including complete trait obligations and closure capture facts.
- **IR Lowering**: a new lowering from Body IR to MIR, including drop and unwind paths, alongside the existing lowering.
- **Emission**: unchanged while the emitted-Rust route remains the comparison route. It is retired by the backend removal that follows this route.
- **Stdlib / Runtime (the `incan_std_<component>` facets)**: compiled through the driver like any other unit.
- **Oven / Tooling**: Oven invokes the driver per unit for both languages. The driver's identity enters unit identity. Toolchain mismatch is refused before any effect.
- **LSP / Tooling**: no rustc dependency, unless the language server hosts the resident driver (see below).

## Unresolved questions

- Should the resident driver be owned by Oven, as a long-running build service that the language server keeps warm, or hosted inside the language server itself?
- Can Incan's Rust interop call the driver's Rust layer when that layer is built against rustc's internal interface and linked into the driver? This decides whether the lowering is written in Incan from the start.
- Should bodies enter rustc as MIR, as the spike does, or one stage earlier as THIR? THIR would let rustc build drop and unwind paths and lower patterns itself, at the cost of constructing typed expression trees that must agree with rustc's type-check results. The spike tested MIR only.
- How do a published Loaf's generic Incan bodies reach a native consumer: through the rustc metadata of the Loaf's compiled unit, through RFC 123's executable representation, or both?
- How do Incan `async` bodies lower: as MIR coroutines the front end builds, or by another route?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
