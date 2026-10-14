# Retained lock host API

The retained-owner lock operation is implemented and callable from Incan. `src/owner_lock.incn` constructs `ExecutableBuildOwner`, calls its public `prepare_lock` operation twice with the same request, and checks that the published lock bytes remain unchanged. Both calls execute through the original owner; the example adds no resolver or cache.

## Positive acceptance

With prepared compiler dependencies, run from this directory:

```bash
incan run src/owner_lock.incn -- /absolute/path/to/prepared-project
```

The selected project must have a `loaf.toml` and a declared `[project.scripts]` entry, because this example supplies no explicit source entrypoint. Its successful output is:

```text
public owner lock first and repeat passed
```

The example was compiled against the host-library source at PR #2211 head `061592e1cdefeda4de9d4529f1962ce93ff8dca0` and executed successfully with both Cargo guards empty. This proves the public host operation from Incan; it is not a fresh installation or complete dependency-lifecycle acceptance test. The existing owner controls and command traces separately verify retained reuse and invalidation.

## Preserved private-access repro

`src/main.incn` deliberately calls the private `collect_and_publish_project_lock_with_session` function. It still fails with E0603, as expected. Its six typed arguments preserve the original visibility failure; it is not the supported entrypoint and does not indicate that `prepare_lock` is missing.

The public operation owns the retained session and delegates to that private collector. Making the collector public would expose the wrong ownership boundary; calling the older stateless collector would discard retained state. Mutable Rust interop is supported: the positive example exercises it directly.

## Remaining migration scope

The host adapter and transport are implemented in Rust. Moving their orchestration to Incan remains tracked under [#1034](https://github.com/encero-systems/incan/issues/1034#issuecomment-6173949508), within the broader [toolchain workflow](https://github.com/encero-systems/incan/issues/1698). That migration is still open; this PR delivers the working retained-lock operation and its Incan-facing API, not the entire control-plane migration.
