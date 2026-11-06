# Generated Rust and the native compiler

Earlier Incan releases exposed generated Rust as backend inspection output. Those releases' documentation and release notes describe that implementation.

Dev.8 compiles checked Body IR through the pinned Rust compiler. Ordinary project builds do not emit a Rust project or use generated Rust as a semantic handoff. Inspect source declarations and checked relationships with [codegraph inspection](../../tooling/reference/agent_context_cli.md), and inspect diagnostics with `incan check PATH --format json`.

Generated Rust used by a maintainer's bootstrap is an intermediate build artifact. It does not define the language contract or a stable public ABI.
