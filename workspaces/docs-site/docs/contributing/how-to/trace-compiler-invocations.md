# Trace Oven reuse decisions

Run a source program once, then observe another invocation:

```sh
incan run src/main.incn
OVEN_TRACE=- incan run src/main.incn
```

`OVEN_TRACE=-` writes trace lines to stderr. A nonempty path writes them to that file. Leave the variable unset for normal timing measurements.

A `reused` line reports a retained executable. A `compiled (...)` line gives the reason for compilation. Library preparation has its own trace lines. A retained project owner can also avoid repeated preparation; `OVEN_OWNER=off` disables owner use without disabling executable reuse.

Keep program output, trace output and the command's status together. A warm build does not establish that first execution of a new binary is equally fast, and reuse of a test executable never implies reuse of test results.
