# Reduce macOS first-launch delays

On macOS, the first execution of a newly built program can be slower while the system assesses the new executable. Rebuilding produces another executable that can incur the same assessment. This time is separate from Oven's compilation and executable-reuse work.

## Distinguish compilation from first execution

Make a small source change, then build before timing execution:

```bash
incan build src/main.incn
time env OVEN_TRACE=- incan run src/main.incn
time env OVEN_TRACE=- incan run src/main.incn
```

Read the trace alongside the elapsed time. `reused` means Oven retained the executable; `compiled (...)` identifies a rebuild and its reason. A slower first run with `reused` on both runs points outside compilation. Program initialization and I/O can also affect timing, so use a small deterministic program when comparing the launch cost.

## Enable your development launcher

1. Open **System Settings → Privacy & Security → Developer Tools**.
2. Add the application that launches your development commands with the **+** button, if it is absent.
3. Enable that application. Choose Terminal or your terminal application for terminal launches; choose the editor application for its integrated terminal or tasks.
4. Quit and reopen that application, then run the commands from its new session.

Developer Tools permits the selected application to run local development software outside the ordinary system assessment policy. This removes that assessment cost for its launches; it does not remove compilation or the program's own startup work. Apple documents the setting and app-selection steps in [Control the ability of apps to run software that does not meet the system's security policy](https://support.apple.com/en-euro/guide/mac-help/mchlc5fb7f9c/27/mac/27).

## Verify with another new executable

Change the source and repeat the build and two traced runs. Confirm that the commands come from the enabled application. Compare first execution with the unchanged second run and keep both traces. A `compiled (...)` line still indicates compiler work; Developer Tools does not make a stale executable reusable.

For other rebuild causes, see [Troubleshooting](troubleshooting.md). [CLI reference](../reference/cli_reference.md#environment-variables) specifies `OVEN_TRACE` and `OVEN_OWNER`.
