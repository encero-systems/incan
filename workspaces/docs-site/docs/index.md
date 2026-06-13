---
title: Incan
hide:
  - navigation
  - toc
---

<!-- markdownlint-disable MD013 MD033 -->

<div class="inc-home" markdown="1">

<section class="inc-hero" aria-labelledby="incan-home-title" markdown="1">

<div class="inc-hero__copy" markdown="1">

<h1 id="incan-home-title">Incan writes the Rust-shaped ownership plan.</h1>

Write typed, Python-like source. Incan lowers it to Rust, lets rustc do the final safety check, integrates with the Rust ecosystem, and ships native binaries through Cargo.

<div class="inc-hero__actions" markdown="1">
[Try Incan](tooling/tutorials/getting_started.md){ .md-button .md-button--primary }
[View on GitHub](https://github.com/dannys-code-corner/incan){ .md-button }
[Duckborrowing](contributing/explanation/duckborrowing.md){ .md-button .inc-button--quiet }
</div>

</div>

<div class="inc-code-compare" aria-label="Incan source and compiler handoff" markdown="1">

<div class="inc-code-pane" markdown="1">
<p class="inc-code-label">Incan source</p>

```incan
model Job:
    name: str

def title(job: Job) -> str:
    return job.name
```
</div>

<div class="inc-code-pane inc-code-pane--handoff" markdown="1">
<p class="inc-code-label">Compiler handoff</p>

<div class="inc-handoff-list" markdown="1">

<div class="inc-handoff-step" markdown="1">
<strong>Typed IR</strong>
<span><code>job: Job -> str</code> before Rust emission.</span>
</div>

<div class="inc-handoff-step" markdown="1">
<strong>Duckborrowing</strong>
<span>Compiler policy for moves, borrows, clones, and owned storage.</span>
</div>

<div class="inc-handoff-step" markdown="1">
<strong>Cargo + rustc</strong>
<span>Generated Rust gets built and checked by familiar tools.</span>
</div>

<div class="inc-handoff-step" markdown="1">
<strong>Native binary</strong>
<span>Rust-built executable, not a Python process.</span>
</div>

</div>
</div>

</div>

</section>

<section class="inc-section inc-section--tight" aria-label="Incan at a glance" markdown="1">

<div class="grid cards inc-answer-grid" markdown="1">

-   **What is it?**

    A typed language and compiler toolchain that builds through Rust.

-   **Why care?**

    Native output, explicit contracts, and smaller application code.

-   **Why not Python?**

    Python-like source, but no Python runtime or compatibility promise.

-   **Why not Rust?**

    Rust remains the backend, ecosystem, and safety layer; Incan is the authoring layer.

-   **Why trust it?**

    Generated Rust, rustc validation, benchmarks, and a public beta roadmap.

</div>

</section>

<section class="inc-section inc-section--tight" aria-label="Incan proof points" markdown="1">

<div class="grid cards inc-proof-grid" markdown="1">

-   **Emits Rust**

    Current compiler path.

-   **Uses Cargo**

    Built by Cargo and rustc.

-   **Ships native**

    Native binaries.

-   **Typed source**

    Models, traits, and checked contracts.

-   **Imports crates**

    Rust crate access through `rust::`.

-   **Benchmarked**

    Current compute suite reports 8.7x-99.5x vs CPython.

</div>

</section>

<section class="inc-section" markdown="1">

<div class="grid inc-section-grid" markdown="1">

<div markdown="1">

## No explicit borrow choreography.

Duckborrowing is a compiler-side ownership planner. It decides when generated Rust should move, borrow, mutably borrow, clone, convert with `.into()`, or materialize owned storage.

It is not "clone until Rust accepts it." It is a tested compiler policy for ownership decisions, designed to keep ordinary Incan source value-oriented while emitted Rust remains valid and predictable.

[Read the Duckborrowing deep dive](contributing/explanation/duckborrowing.md){ .inc-text-link }

</div>

!!! tip "Duckborrowing pipeline"
    1. Incan source
    2. Typed IR
    3. Duckborrowing
    4. Generated Rust
    5. rustc
    6. Native binary

</div>

</section>

<section class="inc-section" markdown="1">

<div class="grid inc-section-grid" markdown="1">

<div markdown="1">

## AI makes syntax cheaper. Toolchains matter more.

As coding agents get better at producing source text, language choice shifts toward the parts syntax alone cannot solve: maintainability, diagnostics, ecosystem fit, operational trust, and runtime shape.

Incan's argument is technical, not mystical: keep the authoring surface small and typed, make errors and mutability reviewable, use Rust and Cargo where they are strongest, and produce artifacts that can be inspected when something fails.

</div>

!!! question "Decision lens"
    - **Maintainability:** Python-like source with declared models, traits, and explicit failure paths.
    - **Diagnostics:** Incan errors first, rustc validation underneath when generated Rust is built.
    - **Ecosystem:** `rust::` imports and Cargo dependency resolution connect to Rust crates.
    - **Runtime:** Native binaries instead of a Python process model.

</div>

</section>

<section class="inc-section" markdown="1">

<div class="grid inc-section-grid" markdown="1">

<div markdown="1">

## Use Rust where it is strongest.

Incan is not trying to replace Rust. Rust remains the backend, ecosystem, safety layer, and performance layer. Incan is the higher-level authoring layer for application-shaped code where Rust's full surface can be more ceremony than signal.

The current compiler path emits Rust, builds through Cargo/rustc, can import Rust crates, and produces native binaries. That gives evaluators a familiar trust boundary: inspect the generated Rust, follow the Cargo build, and profile the resulting binary.

</div>

!!! success "Rust trust boundary"
    - Generated Rust for the current compiler path.
    - rustc checks the emitted Rust project.
    - Rust crates are available through explicit `rust::` imports.
    - Native binaries are the deployment target.

</div>

</section>

<section class="inc-section inc-section--compact" markdown="1">

!!! info "InQL, briefly"

    InQL is a typed relational logic layer that works with Incan model shapes. It belongs in the stack story, not the whole homepage: Incan is the language and compiler substrate; InQL is one downstream layer that proves typed data workflows on top of it.

    [See the Encero stack context](start_here/encero_stack.md){ .inc-text-link }

</section>

<section class="inc-final-cta" markdown="1">

## Try Incan. Break it. Help shape it.

Incan is beta software. The useful next step is to run it, inspect the generated artifacts, compare it against Python and Rust for your workload, and report where the toolchain does not yet earn trust.

<div class="inc-hero__actions" markdown="1">
[Try Incan](tooling/tutorials/getting_started.md){ .md-button .md-button--primary }
[GitHub](https://github.com/dannys-code-corner/incan){ .md-button }
[Documentation](start_here/index.md){ .md-button }
[Duckborrowing Deep Dive](contributing/explanation/duckborrowing.md){ .md-button .inc-button--quiet }
</div>

</section>

</div>
