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

<h1 id="incan-home-title">Write like Python. Ship like Rust.</h1>

Incan is a typed, Python-readable language for new application and data code. It checks contracts, builds native artifacts, exposes compiler facts for tools and agents, and keeps Rust ecosystem boundaries available without requiring Rust-shaped application code.

<div class="inc-hero__actions" markdown="1">
[Try Incan](tooling/tutorials/getting_started.md){ .md-button .md-button--primary }
[View on GitHub](https://github.com/encero-systems/incan){ .md-button }
[Start Here](start_here/index.md){ .md-button .inc-button--quiet }
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
<p class="inc-code-label">Toolchain facts</p>

<div class="inc-handoff-list" markdown="1">

<div class="inc-handoff-step" markdown="1">
<strong>Typed IR</strong>
<span><code>job: Job -> str</code> before backend lowering.</span>
</div>

<div class="inc-handoff-step" markdown="1">
<strong>Contracts</strong>
<span>Models, traits, and explicit failure paths are checked before runtime.</span>
</div>

<div class="inc-handoff-step" markdown="1">
<strong>Artifacts</strong>
<span>Build reports, diagnostics, and inspection data are tool-readable.</span>
</div>

<div class="inc-handoff-step" markdown="1">
<strong>Native output</strong>
<span>Native deployment target, not a Python runtime process.</span>
</div>

</div>
</div>

</div>

</section>

<section class="inc-section inc-section--tight" aria-label="Incan at a glance" markdown="1">

<div class="grid cards inc-answer-grid" markdown="1">

-   **What is it?**

    A typed language and compiler toolchain for Python-readable application and data code.

-   **Why care?**

    Static contracts, explicit failure paths, native artifacts, and structured compiler facts.

-   **Why not Python?**

    Python-like source, but no Python runtime or compatibility promise.

-   **Why not Rust?**

    Rust remains the ecosystem and low-level control layer; Incan is the authoring layer for higher-level code.

-   **Why trust it?**

    Inspectable diagnostics, build artifacts, current Rust-backed output, benchmarks, and a public beta roadmap.

</div>

</section>

<section class="inc-section inc-section--tight" aria-label="Incan proof points" markdown="1">

<div class="grid cards inc-proof-grid" markdown="1">

-   **Python-readable**

    Familiar source shape with static checks.

-   **Typed contracts**

    Models, traits, and explicit errors.

-   **Ships native**

    Native artifacts instead of a Python process.

-   **Inspectable**

    Diagnostics, build reports, and compiler facts.

-   **Rust boundary**

    Rust crate access through `rust::` where it earns its place.

-   **Not compatibility**

    Python-like source, not Python package or runtime parity.

</div>

</section>

<section class="inc-section inc-section--tight" aria-label="Incan fit" markdown="1">

<div class="grid cards inc-answer-grid" markdown="1">

-   **A good fit**

    New application or data code, typed models, explicit failure paths, native deployment, Rust ecosystem boundaries, and tool-readable compiler facts.

-   **Not a fit**

    Running existing Python packages, preserving Python semantics, replacing Rust for low-level control, or chasing raw benchmark wins without contract and tooling needs.

-   **Current beta**

    The current compiler path builds through Cargo/rustc and can inspect generated Rust. The 1.0 direction is native artifacts, stable diagnostics, semantic facts, package metadata, and explicit interop contracts.

-   **Performance**

    Current CPU-bound benchmarks show large wins over CPython, but performance is workload-dependent. The stronger promise is native deployment plus static, inspectable contracts.

</div>

</section>

<section class="inc-section" markdown="1">

<div class="grid inc-section-grid" markdown="1">

<div markdown="1">

## AI makes syntax cheaper. Toolchains matter more.

As coding agents get better at producing source text, language choice shifts toward the parts syntax alone cannot solve: contracts, diagnostics, ecosystem fit, operational trust, and runtime shape.

Incan's argument is technical: keep the authoring surface small and typed, make errors and mutability reviewable, use Rust where it is strongest, and produce artifacts that humans and tools can inspect when something fails.

</div>

!!! question "Decision lens"
    - **Maintainability:** Python-like source with declared models, traits, and explicit failure paths.
    - **Diagnostics:** Stable errors, explanations, reports, and inspection data instead of terminal prose only.
    - **Ecosystem:** `rust::` imports and Rust-facing boundaries connect to Rust crates.
    - **Runtime:** Native binaries instead of a Python process model.

</div>

</section>

<section class="inc-section" markdown="1">

<div class="grid inc-section-grid" markdown="1">

<div markdown="1">

## Use Rust where it is strongest.

Incan is not trying to replace Rust. Rust remains the ecosystem, safety, interop, and low-level control layer. Incan is the higher-level authoring layer for application-shaped code where Rust's full surface can be more ceremony than signal.

The current beta compiler path emits Rust, builds through Cargo/rustc, can import Rust crates, and produces native binaries. That gives evaluators a familiar trust boundary today. The longer-term contract should be native artifacts, stable diagnostics, semantic facts, package metadata, and explicit interop boundaries rather than generated Rust as the permanent public surface.

</div>

!!! success "Rust trust boundary"
    - Generated Rust is inspectable for the current compiler path.
    - Cargo/rustc check the emitted Rust project today.
    - Rust crates are available through explicit `rust::` imports.
    - Native artifacts are the deployment target.

</div>

</section>

<section class="inc-section" markdown="1">

<div class="grid inc-section-grid" markdown="1">

<div markdown="1">

## No explicit borrow choreography.

Duckborrowing is a compiler-side ownership planner. It decides when generated Rust should move, borrow, mutably borrow, clone, convert with `.into()`, or materialize owned storage.

It is not "clone until Rust accepts it." It is a tested compiler policy for ownership decisions, designed to keep ordinary Incan source value-oriented while current emitted Rust remains valid and predictable.

[Read the Duckborrowing deep dive](contributing/explanation/duckborrowing.md){ .inc-text-link }

</div>

!!! tip "Duckborrowing pipeline"
    1. Incan source
    2. Typed IR
    3. Ownership plan
    4. Backend lowering
    5. Native artifact
    6. Inspection data

</div>

</section>

<section class="inc-section inc-section--compact" markdown="1">

!!! info "InQL, briefly"

    InQL is a typed relational logic layer that works with Incan model shapes. It belongs in the stack story, not the whole homepage: Incan is the language and compiler substrate; InQL is one downstream layer that proves typed data workflows on top of it.

    [See the Encero stack context](start_here/encero_stack.md){ .inc-text-link }

</section>

<section class="inc-final-cta" markdown="1">

## Try Incan. Break it. Help shape it.

Incan is beta software. The useful next step is to run it, inspect the artifacts, compare it against Python and Rust for your workload, and report where the toolchain does not yet earn trust.

<div class="inc-hero__actions" markdown="1">
[Try Incan](tooling/tutorials/getting_started.md){ .md-button .md-button--primary }
[GitHub](https://github.com/encero-systems/incan){ .md-button }
[Documentation](start_here/index.md){ .md-button }
[Duckborrowing Deep Dive](contributing/explanation/duckborrowing.md){ .md-button .inc-button--quiet }
</div>

</section>

</div>
