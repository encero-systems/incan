# Troubleshoot a dev.8 command

## Verify the executable

Run `incan --version`. This guide describes `0.6.0-dev.8`; the 0.5 CLI has different commands and flags. Dev.8 has no `--help` command. The [CLI reference](../reference/cli_reference.md) lists accepted signatures.

## Supply a source path

`run`, `check` and `test` require an explicit source file:

```bash
incan check src/main.incn --format json
incan run src/main.incn
incan test tests/test_main.incn
```

`build` also accepts a project directory. Use `incan build src/lib.incn` to build a library.

## Inspect selected dependencies

```bash
incan describe-unit src/main.incn
```

Check the package, source root, profile and Loaf artifact directories. `INCAN_STDLIB` must name the matching standard-package source directory; compiled artifacts must exist in the selected home and profile. A missing `.incnlib` is an installation or profile mismatch, not a request to download another build system.

## Inspect declarations and calls

```bash
incan inspect codegraph . --query defs main
```

An unchecked-unit warning means reference and call information may be incomplete. Use `--strict` when that must fail the command.

## Observe rebuilds

```bash
OVEN_TRACE=- incan run src/main.incn
```

A `reused` line means the executable was retained. A `compiled (...)` line records why it was rebuilt. Tests always execute; their outcomes are not cached.
