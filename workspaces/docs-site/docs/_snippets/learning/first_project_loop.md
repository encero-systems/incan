Create the files in the tutorial, then run the project with explicit paths:

```bash
incan fmt src tests
incan check src/main.incn
incan run src/main.incn
incan test tests/test_main.incn
incan build src/main.incn
```

The default profile is `debug`. A release build uses `incan build src/main.incn --profile release` and requires matching release-profile standard-package artifacts. Oven builds directly with the installed compiler and reuses unchanged executables. Every test invocation executes the tests.
