# Parallel test compiler stack

Regression for #2213. Two independent test modules force the shared runner to schedule compiler work on parallel workers. Each has a 40-operand Boolean condition, which overflows the default worker stack in the affected development compiler. Both must pass without setting `RUST_MIN_STACK`.

From this directory, run:

```sh
incan test . --jobs 4 --format json --fail-on-empty
```

Repeat unchanged, then run with `--jobs 1`. Each command must report two passed tests. The parallel run must use the existing compiler stack sizing contract; reducing jobs is only a diagnostic comparison, not the fix. This is a compiler-host regression and does not establish MIR backend parity.
