# Retained source locations

Build with `incan build workspaces/benchmarks/plan_spans --profile release`. Measure the executable separately from compilation.

The program retains 100,000 source-span copies with an 8-KiB file name, asserts shared file allocation identity, and prints the exact diagnostic after its creating scope returns. The last output line is `100000`. Compare standard output byte for byte across representation changes.

The memory target is at most 64 MiB peak physical footprint for execution. Compilation has its own data and lowering budgets; it is excluded from this runtime probe.
