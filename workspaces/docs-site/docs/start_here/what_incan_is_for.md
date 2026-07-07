# What Incan is for

Incan is for new application and data code where Python-readable source is useful, but Python's runtime model is not the right deployment or trust boundary.

The short version:

> Write like Python. Ship like Rust.

That does not mean Incan is Python compatibility tooling or a Rust replacement. It means Incan aims for Python-readable authoring, static contracts, native artifacts, tool-readable compiler facts, and explicit Rust ecosystem boundaries.

## Use Incan when

- The code is new rather than inherited Python.
- The code is expected to become a tool, service, workflow, data pipeline, or governed integration.
- The code is a small script today, but native deployment, types, or future maintenance matter.
- Static contracts should be visible in source and checked before runtime.
- Errors, optional values, and mutability should be explicit enough to review.
- Native deployment or small operational footprint matters.
- Rust crates or Rust-hosted integration are part of the intended boundary.
- Editors, CI, docs, or agents need structured compiler facts instead of terminal prose.

## Choose a different tool when

- The main requirement is running existing Python packages.
- The team needs exact Python semantics to stay intact.
- Interactive notebooks and exploratory data science are the primary workflow.
- Low-level control is the main reason to choose the language; write Rust for that.
- The only problem is speeding up an existing Python program without changing its language surface; use Python acceleration or compatibility tooling for that. Incan is a better fit when the code is new and you want native speed, typed contracts, and tool-readable build facts together.

## Current beta versus 1.0 direction

Incan is beta software. The current compiler path builds through Cargo and rustc, can inspect generated Rust, and produces native artifacts. That is a practical trust boundary today, but generated Rust source should not be treated as the permanent public semantic contract.

The 1.0 direction is:

- native artifacts as the deployment target;
- stable diagnostics and explanation hooks;
- build reports and inspection records with schema versions;
- package metadata and semantic facts that tools can consume;
- Rust-facing ABI and package direction where Rust hosts need stable integration;
- clear labels for stable, experimental, and deferred surfaces.

## What Incan says yes to

| Incan is...     | Meaning                                                                                                   |
| --------------- | --------------------------------------------------------------------------------------------------------- |
| Python-readable | The source should be approachable to Python-minded developers.                                            |
| Typed           | Models, options, results, traits, and metadata should make contracts visible before runtime.              |
| Native          | The deployment target is a native artifact, not a Python runtime process.                                 |
| Inspectable     | Diagnostics, build reports, codegraph records, and checked metadata should be useful to humans and tools. |
| Rust-connected  | Rust crates and Rust-hosted integration should be available where that boundary earns its place.          |

## What Incan does not promise

| Boundary                       | Meaning                                                                                                                                    |
| ------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------ |
| Python runtime compatibility   | Incan source is Python-readable, but it does not promise CPython behavior or Python package compatibility.                                 |
| Existing Python acceleration   | Incan is for new source. If the goal is to keep existing Python code and make it faster, use Python acceleration or compatibility tooling. |
| Rust replacement               | Rust remains the language for low-level control, trait-heavy public Rust APIs, runtimes, allocators, kernels, and similar systems code.    |
| Generic systems language first | Incan's public wedge is higher-level application and data code with native artifacts and inspectable contracts.                            |
| Python framework layer         | Incan is a language and toolchain, not a framework running on top of Python.                                                               |

## Where to go next

- If you are comparing against Python, read [Incan vs Python](../comparisons/python.md).
- If you need Python compatibility details, read the compatibility boundary in [Incan vs Python](../comparisons/python.md#compatibility-boundary).
- If you are evaluating the 1.0 category, read [1.0 domain-native demo target](domain_native_demo.md).
- If you need the stabilization boundary, read [1.0 public contracts](public_contracts.md).
