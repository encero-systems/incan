Create the files in the tutorial, then format the sources with `incan` and run the project with `oven`, using explicit paths. `incan check`, `incan run`, `incan test` and `incan build` perform the same project operations as their `oven` commands.

```bash
incan fmt src tests
oven check src/main.incn
oven run src/main.incn
oven test tests/test_main.incn
oven build src/main.incn
```

The default profile is `debug`. A release build uses `oven build src/main.incn --profile release` and requires matching release-profile standard-package artifacts. Oven builds directly with the installed compiler and reuses unchanged executables. Every test invocation executes the tests.
