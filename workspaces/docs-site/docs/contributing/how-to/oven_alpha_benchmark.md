# Measure an Oven command

Use the same installed dev.8 compiler, source, package features and artifact profile for repeated measurements.

## Establish and measure reuse

From a small project:

```bash
incan build src/main.incn
time incan build src/main.incn
OVEN_TRACE=- incan build src/main.incn
```

Measure with tracing disabled; use the separate traced invocation to explain the decision. `reused` means the executable was retained. `compiled (...)` explains an invalidation. Record the compiler version and whether the project owner was already running.

## Separate execution from compilation

```bash
time incan run src/main.incn
```

A run includes program execution. Tests always execute even when their compilation is reused. Changing the source or replacing the executable establishes a different measurement condition.

On macOS, first execution of a new binary can include a system assessment. Record that condition separately from warm compilation reuse. Retain exact commands, trace output and program results rather than attributing all elapsed time to the compiler.
