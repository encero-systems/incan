# Library artifact sets

A library build publishes `lib<crate>.rlib` (the package name with `-` replaced by `_`), `<package>.incnlib`, `<package>.incnbody`, `<package>.native.json` and `<package>.incngraph`. When produced, `<package>.rustabi` and, for a library with a vocabulary desugarer, `vocab-worker` join the set. The artifacts of one set come from one library build.

A source file named `lib.incn` selects a library build. Active path dependency artifacts reside under `<consumer>/target/incan/loaves/<package>/<profile>/`. Standard-package artifacts reside under `<home>/toolchains/0.6.0-dev.8/loaves/<package>/<profile>/`.

Public imports resolve through the dependency's checked namespace. Library declarations retain their identities through consumer aliases and re-exports. A missing or incompatible required artifact is refused; a source declaration does not replace an unavailable compiled library.

A unit's dependency closure is the set of packages whose declarations it requires. A declaration is required when the unit's checked source names it (directly or through a re-export, trait adoption or coercion), when one of its comparisons or primitive operations selects it as a default or adapter, when its plan performs it as a runtime operation, or when another required declaration requires it. Only declarations in the closure are read from `.incnbody`. A standard package outside the closure contributes no declarations and its native closure is not linked; a declared dependency outside the closure contributes no declarations. When a required declaration's package is not in the unit's dependency graph, the build is refused with that declaration and the chain of declarations that requested it. `incan inspect dependencies` prints the closure, each package's entering declarations and why each was requested.

Command signatures are in [CLI reference](cli_reference.md); dependency declarations are in [Project configuration](project_configuration.md).
