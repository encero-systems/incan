# Library artifact sets

Dev.8 libraries publish a native rlib with checked sidecars: `.incnlib`, `.incnbody` and `.native.json`. The sidecars and native implementation belong to the same library build.

A source file named `lib.incn` selects a library build. Active path dependency artifacts reside under `<consumer>/target/incan/loaves/<package>/<profile>/`. Standard-package artifacts reside under `<home>/toolchains/0.6.0-dev.8/loaves/<package>/<profile>/`.

Public imports resolve through the dependency's checked namespace. Library declarations retain their identities through consumer aliases and re-exports. A missing or incompatible required artifact is refused; a source declaration does not replace an unavailable compiled library.

The CLI exposes no `incan inspect representation` command. Command signatures are in [CLI reference](cli_reference.md); dependency declarations are in [Project configuration](project_configuration.md).
