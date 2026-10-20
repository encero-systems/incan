# `incan-cli`

Ring: **toolchain**

The `incan` binary: clap surface, terminal rendering, exit codes, the test runner, and the `generate_feature_inventory` tool that renders the CLI's documented feature surface. The library target `incan_cli` exists for those two binaries; nothing else depends on it. The `oven`, `lock` and `tools` families are `oven-cli`'s and mounted here under `incan oven …`, `incan lock` and `incan tools`.

## Depends on

`compiler` (every crate, including `incan_oven_facet`), `oven`, `kernel`, and `oven-cli` for the commands it mounts. It links no standard-library facet: the runtime is the generated program's, not the command line's.

## Tests

The command line is held to the contract families in `workspaces/contracts`, which its table runner runs against an `incan`.

Should be small. If a command body grows, it belongs in the driver.
