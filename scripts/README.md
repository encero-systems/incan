# Scripts

Utility scripts for development and CI.

## Contents

- `run_examples.sh`: Smoke-tests all examples. It pre-builds nested example library projects, typechecks every `.incn` file under `examples/`, and then runs files that define `def main(...)` with a configurable timeout. Invoked by `make examples`.

## Configuration

`run_examples.sh` respects these environment variables:

|         Variable         |                       Default                       |                  Description                   |
| ------------------------ | --------------------------------------------------- | ---------------------------------------------- |
| `INCAN_BIN`              | `./target/release/incan` (if present), else `incan` | Path to the Incan binary                       |
| `INCAN_EXAMPLES_TIMEOUT` | `30`                                                | Per-example timeout in seconds for `incan run` |
