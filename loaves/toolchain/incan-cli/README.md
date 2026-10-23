# `incan-cli`

Ring: **toolchain**

The compiler's commands over one unit Oven describes: checking and explaining diagnostics, formatting, and the compiler-backed inspections. Each returns an exit code; `incan-rustc-driver` renders it (`--check`). The `incan` command line is Oven's.

## Depends on

`compiler` and `kernel`. It links no standard-library facet: the runtime is the compiled program's, not the compiler's.

## Tests

The commands are held to the contract families in `workspaces/contracts`, which its table runner runs against an `incan`.
