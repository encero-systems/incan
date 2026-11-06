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

## Policy and host ownership

The production lock decision lives in `workspaces/oven/src/lock_preparation.incn`, dispatched by the existing compiler-bound Incan policy engine. It selects canonical fallback, reuse, publication and retention from explicit observations. Both CLI entrypoints reach that policy through the same project owner. The positive example above exercises the public owner API; the example is not the policy implementation.

The Rust host gathers physical observations, retains the original session and leases, validates the engine and response binding, and performs the selected canonical operation. It checks the observations again after the exchange so an answer cannot apply to inputs that changed during evaluation. Missing, malformed or incompatible policy responses fail; there is no Rust policy fallback or second cache.

The broader Oven migration remains [#1034](https://github.com/encero-systems/incan/issues/1034), within the [toolchain workflow](https://github.com/encero-systems/incan/issues/1698). This slice moves lock preparation decisions into Incan; it does not claim that every Oven policy has migrated.
