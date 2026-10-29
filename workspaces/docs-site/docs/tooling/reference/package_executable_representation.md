# Library artifact sets

A library build publishes `lib<crate>.rlib` (the package name with `-` replaced by `_`), `<package>.incnlib`, `<package>.incnbody`, `<package>.native.json` and `<package>.incngraph`. When produced, `<package>.rustabi` and, for a library with a vocabulary desugarer, `vocab-worker` join the set. The artifacts of one set come from one library build.

A source file named `lib.incn` selects a library build. Active path dependency artifacts reside under `<consumer>/target/incan/loaves/<package>/<profile>/`. Standard-package artifacts reside under `<home>/toolchains/0.6.0-dev.8/loaves/<package>/<profile>/`.

Public imports resolve through the dependency's checked namespace. Library declarations retain their identities through consumer aliases and re-exports. A missing or incompatible required artifact is refused; a source declaration does not replace an unavailable compiled library.

Command signatures are in [CLI reference](cli_reference.md); dependency declarations are in [Project configuration](project_configuration.md).
