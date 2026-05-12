# Comparisons

Incan is easiest to understand by saying what it is not.

It is not a Python compatibility runtime, not a faster Python interpreter, and not a replacement for Rust when you need
low-level control. Incan is for new application code where teams want Python-like readability, static contracts, explicit
failure handling, and Rust-native output.

## Start here

| If you are asking... | Read |
| --- | --- |
| Why not keep using Python? | [Incan vs Python](python.md) |
| Why not just write Rust? | [Incan vs Rust](rust.md) |
| What about Codon, Nuitka, Cython, or RustPython? | [Incan vs Python compatibility tools](python_compatibility_tools.md) |

## The short version

Python won because it made application code readable and fast to write. Rust wins when correctness, deployment shape,
and performance matter enough to pay for more explicit code. Incan tries to keep the readable authoring model while
moving the foundation toward static checking, explicit errors, explicit mutability, and native Rust compilation.

Use Incan when the code is new, application-shaped, and expected to grow beyond a script. Do not use Incan when the
main requirement is running existing Python packages, preserving Python semantics, or controlling every systems-level
detail by hand.

