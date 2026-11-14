Development uses the same installed toolchain as project builds. Put its `INCAN_HOME/bin` on `PATH`, and set `INCAN_STDLIB` to the standard-package source directory.

```bash
incan --version
incan check examples/simple/hello.incn
incan run examples/simple/hello.incn
```

Compiler bootstrap is a separate maintainer operation. A source checkout does not supply a compiler executable or compiled standard packages by itself. See [the compiler architecture](../../contributing/explanation/architecture.md) for the source boundaries.
