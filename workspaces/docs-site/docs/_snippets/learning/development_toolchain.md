Development uses the same installed dev.8 toolchain as project builds. Select its Oven executable on `PATH`, install its matching archive, and set `INCAN_STDLIB` to the standard-package source directory.

```bash
incan --version
incan check examples/simple/hello.incn
incan run examples/simple/hello.incn
```

Compiler bootstrap is a separate maintainer operation. A source checkout does not supply a compiler executable or compiled standard packages by itself. See [the compiler architecture](../../contributing/explanation/architecture.md) for the source boundaries.
