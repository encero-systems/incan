# Why Incan?

Incan is a Python-like language that compiles to Rust.

It exists because many developers love Python’s readability and speed of iteration, but repeatedly run into the same problems when projects grow:

- performance becomes a bottleneck in the “hot” parts
- packaging and deployment often become a separate engineering project
- the ecosystem increasingly pushes performance-critical code into Rust/C backends anyway

Incan is the “step-in” language that makes that path explicit: write clear, Python-shaped code, keep strong typing and tooling, and compile to Rust so you can ship fast, predictable programs.

## The core idea

Incan is built around a simple promise:

- **Author in a Python-shaped syntax**
- **Compile to Rust**
- **Use Rust crates when you need them**

That means you can keep a high-level surface for most of your code, while still having a real escape hatch into the Rust ecosystem for performance, libraries, and integration.

## Who it’s for

Incan is useful when you want:

- Python-like ergonomics, but with stronger structure and performance characteristics
- “scripts that grew up” (small programs that become real projects)
- a gradual path for Python-heavy teams to adopt Rust where it matters
- a simpler way (than writing Rust everywhere) to express everyday application code, while still landing on Rust

## What it focuses on (today)

Incan aims for a strong baseline contributor and user experience:

- strong, explicit types (to catch mistakes earlier)
- predictable behavior (fewer “works on my machine” surprises)
- a clear tooling story (formatter, tests, LSP)
- Rust interop with a strict dependency policy (reproducible builds)

## What it is not

- A replacement for Rust when you need low-level control and maximal explicitness.
- A “marketing layer” over Rust: boundaries, tradeoffs, and current limitations should be clear in the docs.

## Next pages

- Why not just Rust: [Why not just Rust?](why_not_just_rust.md)
- How it works: [How Incan works](how_incan_works.md)
- If you’re deciding between Incan and Rust: [Why not just Rust?](why_not_just_rust.md)
- Roadmap and status: [Roadmap](../../roadmap.md)
