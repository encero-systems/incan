# Scripts

Utility scripts for development and CI.

## Contents

|      Script       |                                                                                              Purpose                                                                                              |   Invoked by    |
| ----------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------- |
| `run_examples.sh` | Smoke-tests all examples: typechecks every `.incn` file under `examples/`, then runs files that define `def main(...)` with a configurable timeout. Skips web examples and long-running programs. | `make examples` |

## Configuration

`run_examples.sh` respects these environment variables:

|         Variable         |                       Default                       |                  Description                   |
| ------------------------ | --------------------------------------------------- | ---------------------------------------------- |
| `INCAN_BIN`              | `./target/release/incan` (if present), else `incan` | Path to the Incan binary                       |
| `INCAN_EXAMPLES_TIMEOUT` | `5`                                                 | Per-example timeout in seconds for `incan run` |
