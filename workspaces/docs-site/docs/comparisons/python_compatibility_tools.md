# Incan vs Python Compatibility Tools

RustPython, Codon, Nuitka, and Cython are valuable projects. They are also the wrong category for Incan.

Those tools preserve, accelerate, compile, or extend Python. Incan is for new typed application code with Python-like
ergonomics and Rust-native output. If the goal is to keep existing Python code and packages, use the Python tool.

## The comparison

| Tool | Primary promise | Better fit than Incan when... |
| --- | --- | --- |
| RustPython | Python interpreter written in Rust | You want Python semantics in a Rust-implemented runtime. |
| Codon | High-performance Python-like native compilation | You want Python performance and compatibility-oriented acceleration. |
| Nuitka | Compatible Python-to-executable compilation | You want to ship an existing Python app as an executable. |
| Cython | Python/C extension and performance bridge | You need CPython ecosystem integration or C/C++ extension work. |
| Incan | New typed app code that compiles to Rust | You want Python-like authoring but do not need Python compatibility. |

## Why not just build a better Python runtime?

Because that optimizes for the past: existing Python syntax, behavior, libraries, and compatibility constraints.

Incan optimizes for a different future:

- code is new rather than inherited;
- static contracts are part of the language, not a sidecar;
- errors and mutability are explicit;
- the deployment target is Rust-native output;
- AI-generated code has a smaller, more auditable surface.

That makes Incan higher-risk than compatibility tools. It also makes the upside different. The point is not to run old
Python faster. The point is to make new application code easier to trust and ship.

## When these tools are better

Use a Python compatibility tool when:

- you already have Python code;
- existing Python package compatibility is required;
- the team wants Python semantics to stay intact;
- performance or packaging is the main pain;
- rewriting the application surface is not acceptable.

Use Incan when:

- the code is new;
- static correctness matters more than Python compatibility;
- deployment shape matters;
- Rust crates are part of the intended boundary;
- the team wants Python-like readability without Python's runtime model.

## Source notes

- RustPython describes itself as a Python interpreter written in Rust:
  [RustPython](https://rustpython.github.io/).
- Codon describes itself as a Python implementation that "compiles to native machine code":
  [Codon documentation](https://codon.dev/).
- Nuitka describes itself as a Python compiler that is "fully compatible" with Python 2 and Python 3:
  [Nuitka overview](https://nuitka.net/pages/overview.html).
- Cython describes itself as an "optimising static compiler" for Python and the extended Cython language:
  [Cython](https://cython.org/).
- Nim is not a Python compatibility tool, but it is mature prior art for Python-influenced compiled language design:
  [Nim](https://nim-lang.org/).

