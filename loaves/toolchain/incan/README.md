# `incan`

Ring: **toolchain**

The `incan` command: clap surface, terminal rendering, exit codes.

## Moves here from

- `src/main.rs`
- `src/cli/ (the command-surface half; logic goes to compiler/incan_driver)`

## May depend on

`compiler`, `oven`

Should be small. If a command body grows, it belongs in the driver.

This directory is a layout skeleton. It holds no code yet; `src/` is a placeholder for the conventional crate root.
