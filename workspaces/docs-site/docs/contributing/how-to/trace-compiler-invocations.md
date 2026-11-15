# Trace Oven reuse decisions

From the repository root, run the hello example once, then observe another invocation:

```sh
incan run examples/simple/hello.incn
mkdir -p trace && INCAN_ENGINE_TRACE=trace incan run examples/simple/hello.incn
```

`INCAN_ENGINE_TRACE` names the [tracing](../../tooling/reference/cli_reference.md#tracing) directory; each process writes its own file there, and Oven's reasoning is its `reason` records. Leave the variable unset for normal timing measurements.

A `reused` reason reports a retained executable. A `compiled (...)` reason gives the reason for compilation. Library preparation has its own reasons. A retained project owner can also avoid repeated preparation; `OVEN_OWNER=off` disables owner use without disabling executable reuse.

Keep program output, trace output and the command's status together. A warm build does not establish that first execution of a new binary is equally fast, and reuse of a test executable never implies reuse of test results.
