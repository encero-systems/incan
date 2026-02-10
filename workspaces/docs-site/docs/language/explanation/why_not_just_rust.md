# Why not just Rust?

Rust is an excellent systems language. Many projects should use Rust directly.

Incan is a different tradeoff: it lets you write **Python-shaped application code** while still producing a **typed, compiled**
result. Rust is the target and escape hatch, not the primary authoring experience.

A practical observation behind Incan is that many “Python projects” already depend on Rust/C backends for performance
(especially in AI/data tooling). Incan makes that bridge a first-class experience instead of an ad-hoc stack of bindings,
glue code, and deployment workarounds.

## When Rust is the better choice

Choose Rust when you want:

- maximal explicitness and control (ownership, lifetimes, low-level layout)
- direct access to the ecosystem without translation layers or mapping constraints
- mature stability guarantees and broad community conventions
- to design APIs that are trait-heavy or lifetime-heavy (these can be awkward to express across language boundaries)

## When Incan can be the better choice

Choose Incan when you want:

- a simpler authoring experience (Python-like syntax) with strong, explicit types
- strong structure for “application code” without writing Rust everywhere
- Cargo + Rust crates as the implementation target and escape hatch, not the primary authoring language
- less time spent on ownership/lifetime “plumbing” for everyday app code
  (Incan uses safe ownership defaults and emits Rust that satisfies the borrow checker)

## Tradeoffs and boundaries to keep in mind

- Incan is in Beta; expect evolution, especially in tooling and ecosystem interop.
- The compiler pipeline is effectively “two-step” (Incan → Rust → machine code).
  In practice, that usually means **seconds**, not minutes, but it is still a tradeoff.
- Rust interop is powerful but comes with contracts (dependency policy, type mapping).
  Today, interop prefers **owned** types; borrowing/lifetime-heavy Rust APIs may not map cleanly yet.

See also:

- [Why Incan?](why_incan.md)
- [Rust Interop](../how-to/rust_interop.md)
- [RFC 013: Rust crate dependencies][RFC 013]

--8<-- "_snippets/rfcs_refs.md"
