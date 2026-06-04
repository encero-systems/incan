# Incan vs Rust

Rust is the right answer when low-level control, performance predictability, and ecosystem maturity matter more than authoring speed. Incan does not replace Rust. It compiles to Rust so that high-level application code can land on the Rust toolchain and ecosystem.

Choose Incan when the code is mostly application logic and the Rust version would be more ceremony than signal.

## Where Rust wins

- Low-level systems programming.
- Precise ownership, borrowing, lifetime, and memory-layout control.
- Mature crate ecosystem and tooling.
- Libraries that need to expose a stable Rust API.
- Performance-sensitive internals where every abstraction boundary matters.

## Where Incan is trying to win

- Python-readable business, workflow, data, CLI, and service logic.
- Strong domain models without Rust-level boilerplate at every call site.
- Explicit `Result` / `Option` handling without making every application module feel like systems code.
- Rust interop for the smaller parts of the program that need real Rust.
- Faster iteration for teams that like Rust's guarantees but not Rust's surface area for everyday code.

## The honest tradeoff

Rust is more mature, more explicit, and more powerful. Incan deliberately gives up some low-level control to make high-level code smaller and easier to scan.

That tradeoff is only worth it when the output is application-shaped. If the code is a runtime, database engine, kernel, allocator, compiler backend, or heavily optimized crate, write Rust.

## Decision guide

| Use Rust when... | Use Incan when... |
| --- | --- |
| You need precise memory and lifetime control. | You need clear application logic with native output. |
| You are publishing a Rust crate API. | You are building a CLI, service, workflow, or app layer. |
| Runtime internals dominate the code. | Domain rules and orchestration dominate the code. |
| Every allocation and abstraction boundary matters. | Readability and reviewability matter most. |

## Source notes

- Stack Overflow's 2025 Developer Survey calls Rust the "most admired programming language (72%)": [Technology | 2025 Stack Overflow Developer Survey](https://survey.stackoverflow.co/2025/technology/).
