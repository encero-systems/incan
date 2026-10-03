# Coming from Rust (evaluator)

This route is for Rust-first evaluators deciding whether Incan offers a useful higher-level boundary for application code.

<aside class="inc-bridge-note inc-bridge-note--rust inc-incus-slot" data-incus-category="rust" aria-label="Rust to Incan mental model">
  <span class="inc-eyebrow">Rust → Incan</span>
  <strong>Keep explicit fallibility, traits, native compilation, and crate reach. Spend less surface syntax on application structure.</strong>
</aside>

## Choose your route

<div class="inc-route-grid">
  <a class="inc-route-card" href="../tooling/tutorials/getting_started/"><span class="inc-eyebrow">Start</span><strong>Run the first project</strong><span>Install the current toolchain and complete the native run, test, and release-build loop.</span></a>
  <a class="inc-route-card" href="../tooling/tutorials/build_and_consume_library/"><span class="inc-eyebrow">Package</span><strong>Build an Incan library boundary</strong><span>Choose a small public API, build the producer, and run its locked <code>pub::</code> consumer.</span></a>
  <a class="inc-route-card" href="../language/explanation/rust_shaped_confidence/"><span class="inc-eyebrow">Semantics</span><strong>Inspect the Rust-shaped guarantees</strong><span>See what Incan keeps and what it deliberately makes smaller at the authoring surface.</span></a>
  <a class="inc-route-card" href="../language/how-to/rust_interop/"><span class="inc-eyebrow">Interop</span><strong>Cross the Rust boundary</strong><span>Import crates and author explicit interop where native ecosystem reach matters.</span></a>
</div>

## Add a scoped crate boundary

[Add a Rust crate](../language/tutorials/add_a_rust_crate.md) locks `regex`, explicitly bakes a project extension, and runs the program through the normal Incan path. That proves the supported boundary without promising that every Rust crate API maps naturally into Incan.

## Install once

Follow [Getting Started](../tooling/tutorials/getting_started.md) for the current stable release channel, installer choices, and canonical first-project commands. Development tutorials declare their required compiler range separately.

## What transfers

- Native compilation, explicit `Result` and `Option` values, traits, enums, pattern matching, and crate access
- Reproducible manifests and lockfiles
- A preference for compiler-visible contracts over runtime convention

## What changes

- Ownership and borrowing remain implementation concerns at Rust boundaries, but ordinary Incan application code does not expose Rust lifetime syntax.
- Models, derives, named arguments, and Python-shaped control flow reduce authoring ceremony.
- Representation details Rust asks you to spell out are the compiler's job. A generic model whose type parameter appears only in method signatures — `model Column[T]` with `def __mul__(self, other: T) -> Column[T]` and no field of type `T` — is written exactly like that; the compiler records that the parameter is stored nowhere and supplies whatever the target representation needs (in Rust, the zero-sized `PhantomData<T>` field you would otherwise add by hand). Only a newtype or an enum must store its parameters, and the checker says so at the parameter.
- Generated Rust is inspectable backend output, not the public source or ABI compatibility contract.

## What not to expect

- A low-level Rust replacement for kernels, unsafe systems work, or exact control over allocation and representation
- Every crate API to feel natural without a small wrapper
- Stable compatibility based on generated Rust internals

## Continue

- [Fallible and infallible paths](../language/tutorials/fallible_and_infallible_paths.md)
- [Projects today](../tooling/explanation/projects_today.md)
- [Stability policy](../stability.md) and [release notes](../release_notes/index.md)

## Contributing to the compiler

The contributor journey is separate from learning Incan as a user. Use the [Contributor Book](../contributing/tutorials/book/index.md), [compiler architecture](../contributing/explanation/architecture.md), and [RFC index](../RFCs/index.md) when you want to work on the toolchain itself.
