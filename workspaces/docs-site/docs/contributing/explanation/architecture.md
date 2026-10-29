# Incan compiler architecture

Dev.8 compiles Incan through one native route. Oven is the command line, written in Incan; the compiler combines a Rust frontend with Incan lowering and runtime libraries.

## Pipeline

```mermaid
flowchart LR
    project["Source and loaf.toml"] --> oven["Oven"]
    oven --> unit["Unit description"]
    unit --> check["Lexer, parser and checker"]
    check --> body["Checked Body IR"]
    body --> lowering["Incan lowering"]
    lowering --> plan["Native plan"]
    plan --> adapter["Rust MIR adapter"]
    adapter --> rustc["Pinned rustc"]
    rustc --> native["Native artifacts"]
```

The checker and Body IR own semantic facts. Lowering consumes those facts; it does not resolve them again. Oven owns project intake and describes each unit before invoking the compiler.

## Source boundaries

| Area | Source |
| --- | --- |
| Command line, project intake, features, reuse, archives and graph queries | `workspaces/oven/src/` |
| Lexer, parser, AST and diagnostics | `loaves/kernel/incan_syntax/src/` |
| Language policy and registries | `loaves/kernel/incan_lang/src/` |
| Checker and Body IR | `loaves/compiler/incan_frontend/src/` |
| Compiler session and unit boundary | `loaves/compiler/incan_driver/src/` |
| Incan lowering | `loaves/compiler/incan_mir_lowering/src/` |
| Native plan data | `loaves/compiler/incan_mir_plan/src/` |
| Rust compiler adapter and metadata extraction | `loaves/toolchain/incan-rustc-driver/src/` |
| Native runtime authored in Incan | `loaves/compiler/incan_native_runtime/src/` |
| Standard packages | `loaves/stdlib/` |
| Contract runner and fixtures | `workspaces/contracts/` |

## Libraries and Rust facts

Standard packages compile as ordinary Loaves. Native implementations travel with checked library sidecars, so consumers need neither a copy of private library source nor a generated Rust project.

Rust items are checked against ABI caches supplied by the installed toolchain. Paths outside those caches can be extracted by the pinned Rust compiler during checking. The checker records the result once for downstream compilation.

## Build and inspection

`incan build PATH` describes and compiles a unit or Rust project. `incan run PATH` builds and executes a source program. Unchanged executables and resident project preparation can be reused. Tests still execute afresh.

`incan inspect codegraph` exposes checked declaration and call relationships across Incan and Rust. It can return parseable facts for units that fail checking, with explicit warnings and degraded state. Strict inspection refuses those units.

See [Layering](layering.md), [CLI reference](../../tooling/reference/cli_reference.md) and [contract families](https://github.com/encero-systems/incan/blob/main/workspaces/contracts/README.md).
