---
search:
  boost: 4
tags:
  - python
  - performance
  - benchmarks
  - static typing
  - readable source
  - rust
---

# Incan vs Python

Python is the default choice when ecosystem reach, hiring familiarity, notebooks, and package availability matter most. Incan should not be chosen just because a program could be written in a Python-like syntax.

Choose Incan when you want Python-shaped application code but do not want Python's runtime and deployment tradeoffs.

## Where Python wins

- Existing packages, especially for data science, notebooks, AI/ML, and web frameworks.
- Team familiarity and hiring.
- Fast one-file scripts where runtime correctness risk is low.
- Interactive exploration.
- Compatibility with the broader Python packaging ecosystem.

## Where Incan is trying to win

- New application code that benefits from static type checking before runtime.
- Tools, services, and workflows where deployment should produce a native binary.
- CPU-bound application logic where native output materially changes runtime cost.
- Codebases where errors and mutability should be explicit in review.
- Rust ecosystem access without forcing every line of application code to be Rust.
- Agent-generated code where a smaller typed surface is easier to audit than dynamic Python.

## The honest tradeoff

Python has the ecosystem. Incan has to earn every library, tool, and example. That means Incan is a bad fit if the first question is, "Can I use all my Python packages?"

The better question is, "Would this new tool or service be safer and easier to ship if it were typed, native, and still readable to a Python-minded developer?"

## Current benchmark snapshot

The repository benchmark suite was rerun against the current v0.4 release candidate on July 3, 2026. The result is the core performance promise in concrete form: readable Incan source, native execution, and timings that sit much closer to Rust than Python.

| Benchmark | Incan | Rust | Python | Incan vs Python |
| --- | ---: | ---: | ---: | ---: |
| Fibonacci (1M) | 7ms | 7ms | 67ms | 9.5x faster |
| Collatz (1M) | 130ms | 127ms | 4732ms | 36.4x faster |
| GCD (10M) | 115ms | 96ms | 935ms | 8.1x faster |
| Mandelbrot (2K) | 140ms | 139ms | 4723ms | 33.7x faster |
| N-Body (500K) | 19ms | 17ms | 1521ms | 80.0x faster |
| Prime Sieve (50M) | 149ms | 130ms | 3281ms | 22.0x faster |
| Quicksort (1M) | 58ms | 50ms | 890ms | 15.3x faster |
| Mergesort (1M) | 88ms | 121ms | 1266ms | 14.3x faster |

Across this suite, Incan is 8.1x to 80.0x faster than the equivalent Python programs. That is the point of the toolchain: keep the intent readable, then let the compiler lower it into native code you can inspect, build, and ship.

## Compatibility boundary

Incan uses Python-readable syntax where that helps authoring, but it is not a Python runtime and does not try to preserve CPython behavior.

| Python expectation | Incan position |
| --- | --- |
| Existing `pip` packages run directly. | Python package compatibility is not the target. Use Python when that matters. |
| Python syntax and object behavior carry over. | Incan keeps familiar forms where they fit a typed language. It is not Python grammar compatibility. |
| Errors, nullability, and mutation can stay dynamic. | Fallible paths, optionality, and mutability should be visible to the type checker and reviewers. |
| Deployment includes a Python interpreter and environment. | The deployment target is a native artifact with Incan/Rust build metadata. |

## Python compatibility tools

RustPython, Codon, Nuitka, and Cython are valuable projects. They preserve, accelerate, compile, or extend Python. Incan is in a different category: new typed application code with Python-like readability, native artifacts, and Rust ecosystem boundaries.

Use those tools when existing Python code, packages, or semantics must stay intact. Use Incan when the code is new and the goal is static contracts, native deployment, and inspectable compiler output rather than Python compatibility.

## Decision guide

| Use Python when... | Use Incan when... |
| --- | --- |
| You need existing Python libraries. | You are writing new application logic. |
| You are exploring data interactively. | You want a native binary. |
| Runtime flexibility matters more than static guarantees. | Reviewable contracts matter more than dynamic flexibility. |
| A script will stay small. | A script is becoming a product, service, or governed workflow. |

## Source notes

- Stack Overflow's 2025 Developer Survey says Python adoption "accelerated significantly" and ties it to AI, data science, and backend work: [Technology | 2025 Stack Overflow Developer Survey](https://survey.stackoverflow.co/2025/technology/).
- Meta's typed Python survey reports that 88% of respondents often or always use types in Python code, while also naming usability, latency, and library typing gaps as pain points: [Typed Python in 2024](https://engineering.fb.com/2024/12/09/developer-tools/typed-python-2024-survey-meta/).
