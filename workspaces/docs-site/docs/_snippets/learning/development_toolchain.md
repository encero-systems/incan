For checkout-source verification, use the installed Incan 0.6.0-dev.9 toolchain. Put the matching `incan` executable on `PATH`, install its matching archive, and set `INCAN_STDLIB` to the checkout's `loaves/stdlib` directory.

```bash
incan --version
incan check examples/simple/hello.incn
incan run examples/simple/hello.incn
```

Compiler bootstrap is a separate maintainer operation. A source checkout does not supply a compiler executable or compiled standard packages by itself. See [the compiler architecture](../../contributing/explanation/architecture.md) for the source boundaries.
