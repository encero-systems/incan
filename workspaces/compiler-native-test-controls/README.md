# Native test feature control

This Incan workspace exercises the public `incan oven compiler-native-tests` route against a disposable authored root. It requires two real libtest cases with the default `marker` feature, one case after disabling it, and reuse of the original native bytes on the first exact restoration. A guarded changed selection must refuse compilation and retain the previous binary.

The sibling test declaration's `[project.features]` supplies local unit cfg names and their includes through the existing package feature resolver. Rust dependency features belong in its `[rust.dependencies]` records; the test route does not reinterpret Incan dependency or SDK-component feature edges as Rust dependency selection. Unknown feature includes are rejected before compilation.

From this workspace, with a prepared SDK and a compatible source compiler:

```sh
incan run src/main.incn -- /absolute/path/to/incan /absolute/path/to/rustc /absolute/path/to/new-empty-probe
```

The three paths are explicit control inputs. The probe must not exist. Child output, the real test inventory, native test binary and store remain under it. This control verifies feature coverage and native-output invalidation; it does not establish full-suite coverage, installed-stage compatibility or the ordinary fresh-project no-bake experience.
