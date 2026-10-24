# Run project checks in CI

Install the matching dev.8 toolchain archive on a compatible runner before checking a project. Select its Oven executable on `PATH` and set `INCAN_HOME` and `INCAN_STDLIB` for that installation. See [Install and run](install_and_run.md).

## Check a project

From the project root:

```bash
incan --version
incan fmt --check src tests
incan check src/main.incn --format json
incan test tests/test_main.incn
incan build src/main.incn
```

Pass each test source explicitly. Directory discovery and test-report flags are not supported by this CLI. Preserve the command's exit status as the CI result.

## Select a release profile

```bash
incan build src/main.incn --profile release
```

The toolchain must include standard-package artifacts for `release`. A home containing only `debug` artifacts cannot satisfy this build.

## Retain evidence

Capture `incan check src/main.incn --format json` for diagnostics and `incan describe-unit src/main.incn` for the selected unit, features and Loaf paths. Program output is under `target/incan/<profile>/bin/`. There is no JSON build-report flag.

For graph consumers, use `incan inspect codegraph . --format jsonl --strict` to refuse unchecked units. Without `--strict`, inspect warnings and degraded records before interpreting an empty query as absence of references. [Agent context CLI and MCP](../reference/agent_context_cli.md) specifies schemas and freshness.

Repository contributors use the [repository checks](../../contributing/how-to/ci_and_automation.md).
