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

<div class="inc-hero__brand">
<img class="inc-hero__mark" src="shared/incapunk/wordmark_small_001.png" alt="Incan">
</div>

<p class="inc-kicker">Programming language + compiler toolchain</p>

<h1 id="incan-home-title"><span>Readable source.</span><span>Native Rust binaries.</span></h1>

<p class="inc-hero__lead">Write typed, Python-shaped code in Incan, a programming language that plans ownership, emits inspectable Rust, and ships native binaries through Cargo.</p>

<div class="inc-hero__actions" markdown="1">
[Try Incan](tooling/tutorials/getting_started.md){ .md-button .md-button--primary }
[GitHub](https://github.com/encero-systems/incan){ .md-button }
[Docs](start_here/index.md){ .md-button }
[Duckborrowing →](contributing/explanation/duckborrowing.md){ .inc-hero__text-link }
</div>

</div>

<div class="inc-hero__flow" aria-label="Compiler flow">
<div class="inc-flow-step inc-tone-cyan"><img src="shared/incapunk/icons/code.svg" alt=""><strong>Write</strong><span>Python-shaped source</span></div>
<span class="inc-flow-arrow"><img src="shared/incapunk/icons/arrow-right.svg" alt=""></span>
<div class="inc-flow-step inc-tone-gold"><img src="shared/incapunk/icons/workflow.svg" alt=""><strong>Plan</strong><span>Ownership without ceremony</span></div>
<span class="inc-flow-arrow"><img src="shared/incapunk/icons/arrow-right.svg" alt=""></span>
<div class="inc-flow-step inc-tone-cyan"><img src="shared/incapunk/icons/file-code.svg" alt=""><strong>Emit</strong><span>Inspectable Rust</span></div>
<span class="inc-flow-arrow"><img src="shared/incapunk/icons/arrow-right.svg" alt=""></span>
<div class="inc-flow-step inc-tone-cyan"><img src="shared/incapunk/icons/shield-check.svg" alt=""><strong>Verify</strong><span>rustc checks the handoff</span></div>
<span class="inc-flow-arrow"><img src="shared/incapunk/icons/arrow-right.svg" alt=""></span>
<div class="inc-flow-step inc-tone-gold"><img src="shared/incapunk/icons/rocket.svg" alt=""><strong>Ship</strong><span>Native binary</span></div>
</div>

</section>

<section class="inc-section inc-build-strip" aria-label="Build path" markdown="1">

<p class="inc-strip-title">Built for how modern software is built.</p>

<div class="inc-build-strip__grid">
<div class="inc-build-card inc-tone-cyan"><img src="shared/incapunk/icons/terminal.svg" alt=""><strong>Typed by default</strong><span>Catch bugs early with static typing.</span></div>
<div class="inc-build-card inc-tone-gold"><img src="shared/incapunk/icons/workflow.svg" alt=""><strong>Ownership planned</strong><span>Borrowing without ceremony.</span></div>
<div class="inc-build-card inc-tone-cyan"><img src="shared/incapunk/icons/file-code.svg" alt=""><strong>Rust emitted</strong><span>Readable Rust you can inspect.</span></div>
<div class="inc-build-card inc-tone-cyan"><img src="shared/incapunk/icons/shield-check.svg" alt=""><strong>Cargo checked</strong><span>rustc is the final safety check.</span></div>
<div class="inc-build-card inc-tone-gold"><img src="shared/incapunk/icons/rocket.svg" alt=""><strong>Native binary</strong><span>Fast startup, small footprint.</span></div>
</div>

</section>

<section class="inc-section inc-section--code" aria-label="Code example" markdown="1">

<div class="inc-code-heading" markdown="1">

<div class="inc-section-intro" markdown="1">

## Incan vs Rust, side by side.

Same model, same greeting. Incan keeps the code close to the intent; Rust remains the build path when ownership, crates, and native artifacts matter.

</div>

<div class="inc-code-proof__checks">
<span>Typed models</span>
<span>No borrow marker at call</span>
<span>Rust remains inspectable</span>
<span>Cargo/rustc path</span>
</div>

</div>

<div class="inc-code-proof" markdown="1">

<div class="inc-code-compare" markdown="1">

<div class="inc-code-pane" markdown="1">
<p class="inc-code-label">Incan source</p>

```incan
model User:
    name: str
    age: int

def greet_user(user: User) -> str:
    return f"Hello, {user.name}!"

def main() -> None:
    user = User(name="Incan", age=42)
    println(greet_user(user))
```
</div>

<div class="inc-code-pane inc-code-pane--rust" markdown="1">
<p class="inc-code-label">Comparable Rust</p>

```rust
#[derive(Debug, Clone)]
struct User {
    name: String,
    age: i64,
}

fn greet_user(user: &User) -> String {
    format!("Hello, {}!", user.name)
}

fn main() {
    let user = User {
        name: "Incan".to_string(),
        age: 42,
    };
    println!("{}", greet_user(&user));
}
```
</div>

</div>

</div>

</section>

<p class="inc-code-caption">You write intent. Incan handles ownership, lifetimes, and the hard parts of the Rust-shaped handoff.</p>

<section class="inc-section inc-section--why" aria-label="Why Incan" markdown="1">

<div class="inc-section-title" markdown="1">

## Why Incan?

</div>

<div class="inc-why-grid">

<div class="inc-why-card inc-tone-purple">
<img src="shared/incapunk/icons/python.svg" alt="">
<span>Python-like source without the Python runtime.</span>
</div>

<div class="inc-why-card inc-tone-gold">
<img src="shared/incapunk/icons/rust.svg" alt="">
<span>Native Rust artifacts without writing every ownership detail by hand.</span>
</div>

<div class="inc-why-card inc-tone-cyan">
<img src="shared/incapunk/icons/code.svg" alt="">
<span>Use Rust where control matters. Use Incan where intent matters.</span>
</div>

<div class="inc-why-card inc-tone-cyan">
<img src="shared/incapunk/icons/eye.svg" alt="">
<span>Inspect the Rust before you trust the compiler.</span>
</div>

<div class="inc-why-card inc-tone-gold">
<img src="shared/incapunk/icons/package.svg" alt="">
<span>Cargo crates and Rust tooling stay on the path.</span>
</div>

<div class="inc-why-card inc-tone-pink">
<img src="shared/incapunk/icons/workflow.svg" alt="">
<span>Compiler-side ownership planning for clear source code.</span>
<a href="contributing/explanation/duckborrowing/">Learn more</a>
</div>

</div>

</section>

<section class="inc-brand-line" aria-label="Incan brand line">
<p><span>You write intent.</span><span>Incan writes ownership.</span></p>
</section>

<section class="inc-section inc-section--thesis" markdown="1">

<div class="inc-thesis-grid" markdown="1">

<div markdown="1">

## AI makes syntax cheaper. Toolchains matter more.

As coding agents become better at writing source text, the value shifts away from syntax and toward diagnostics, contracts, inspectability, runtime behavior, and deployment.

Incan is designed for that future.

</div>

<div class="inc-matters-card">
<strong>What matters most</strong>
<ul>
<li><span>Maintainability:</span> explicit models and failure paths.</li>
<li><span>Diagnostics:</span> stable errors, explanations, and inspection data.</li>
<li><span>Ecosystem:</span> Cargo crates and Rust tooling, end to end.</li>
<li><span>Runtime:</span> native binaries instead of a Python process model.</li>
</ul>
</div>

</div>

</section>

<section class="inc-section inc-section--duck" markdown="1">

<div class="inc-section-intro" markdown="1">

## Duckborrowing in a nutshell.

Incan decides what Rust needs. You keep writing clear, direct code.

</div>

<div class="inc-duck-diagram" aria-label="Duckborrowing flow">
<div class="inc-duck-step inc-tone-purple"><img src="shared/incapunk/icons/file-code.svg" alt=""><span>Python-shaped source</span></div>
<span class="inc-duck-arrow"><img src="shared/incapunk/icons/arrow-right.svg" alt=""></span>
<div class="inc-duck-step inc-tone-gold"><img src="shared/incapunk/icons/workflow.svg" alt=""><span>Ownership planning</span></div>
<span class="inc-duck-arrow"><img src="shared/incapunk/icons/arrow-right.svg" alt=""></span>
<div class="inc-duck-step inc-tone-cyan"><img src="shared/incapunk/icons/link.svg" alt=""><span>Borrow</span></div>
<span class="inc-duck-arrow"><img src="shared/incapunk/icons/arrow-right.svg" alt=""></span>
<div class="inc-duck-step inc-tone-cyan"><img src="shared/incapunk/icons/arrow-right.svg" alt=""><span>Move</span></div>
<span class="inc-duck-arrow"><img src="shared/incapunk/icons/arrow-right.svg" alt=""></span>
<div class="inc-duck-step inc-tone-cyan"><img src="shared/incapunk/icons/copy.svg" alt=""><span>Clone</span></div>
<span class="inc-duck-arrow"><img src="shared/incapunk/icons/arrow-right.svg" alt=""></span>
<div class="inc-duck-step inc-tone-cyan"><img src="shared/incapunk/icons/workflow.svg" alt=""><span>Convert</span></div>
<span class="inc-duck-arrow"><img src="shared/incapunk/icons/arrow-right.svg" alt=""></span>
<div class="inc-duck-step inc-tone-cyan"><img src="shared/incapunk/icons/database-arrow-right.svg" alt=""><span>Owned storage</span></div>
<span class="inc-duck-arrow"><img src="shared/incapunk/icons/arrow-right.svg" alt=""></span>
<div class="inc-duck-step inc-tone-gold"><img src="shared/incapunk/icons/rust.svg" alt=""><span>Generated Rust</span></div>
</div>

<p class="inc-duck-link"><a href="contributing/explanation/duckborrowing/">Read the Duckborrowing deep dive</a></p>

</section>

<section class="inc-final-cta" markdown="1">

<div class="inc-final-cta__copy" markdown="1">

## Try Incan. Break it. Help shape it.

Incan is beta software. The useful next step is to run it, inspect the artifacts, compare it against Python and Rust for your workload, and report where the toolchain does not yet earn trust.

</div>

<div class="inc-hero__actions" markdown="1">
[Try Incan](tooling/tutorials/getting_started.md){ .md-button .md-button--primary }
[GitHub](https://github.com/encero-systems/incan){ .md-button }
[Docs](start_here/index.md){ .md-button }
[Duckborrowing →](contributing/explanation/duckborrowing.md){ .inc-hero__text-link }
</div>

</section>

</div>
