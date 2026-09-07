//! Resolving an imported declaration to the executable representation its package published.
//!
//! This is the consumer half of RFC 123. A route that does not link Rust reaches an imported declaration's
//! executable meaning through the canonical identity it already resolved — never through a spelling, a module path
//! it reconstructed, or a generated Rust name.
//!
//! Every failure here is a refusal rather than a fallback. RFC 123 requires a consumer that needs a representation
//! and cannot obtain a usable one to stop before producing any result, and to say which package, which version, and
//! which requirement was unmet. Silently choosing another route would make a package's execution semantics depend
//! on what the consumer happened to have, which is the coupling this RFC exists to remove.

use std::fs;
use std::path::PathBuf;

use incan_semantics_core::body_ir::Body;
use incan_semantics_core::executable_representation::{
    ExecutableRepresentationError, SurfaceReader, representation_version,
};
use incan_semantics_core::{CanonicalSymbolId, SymbolOrigin};

use crate::frontend::library_manifest_index::{LibraryManifestIndex, LibraryManifestIndexEntry};
use crate::library_manifest::published_layout::executable_surface_path;

/// Why an imported declaration could not be executed on a route that does not link Rust.
///
/// Each variant names the package and, where one is known, the version — RFC 123 requires the refusal to be
/// actionable in those terms rather than to report a generic failure. None of these is an unsupported language
/// construct, and a consumer must not render them as one: the declaration exists and the package exports it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExecutableResolutionError {
    /// The identity does not name a package declaration, so there is no package to ask.
    ///
    /// A local declaration is executed from the compilation that is happening, not from a published surface.
    #[error("`{declaration}` is not a package declaration, so it has no published executable representation")]
    NotAPackageDeclaration {
        /// Declaration the consumer asked for.
        declaration: String,
    },
    /// The package the identity names is not among this compilation's resolved dependencies.
    #[error("package `{library}` is not a resolved dependency of this compilation")]
    UnknownPackage {
        /// Library the identity names.
        library: String,
    },
    /// The package resolved, but published no executable representation for that module.
    ///
    /// A package is permitted to publish none and remain valid for every route that does not need one, so this is a
    /// well-formed package this route cannot use — not a broken one.
    #[error(
        "package `{library}` version {version} publishes no executable representation for module `{module_path}`, which this route requires to execute `{declaration}`"
    )]
    NoPublishedRepresentation {
        /// Library the identity names.
        library: String,
        /// Version of that library, from its manifest.
        version: String,
        /// Module path the identity carries.
        module_path: String,
        /// Declaration the consumer asked for.
        declaration: String,
    },
    /// The representation exists but could not be read or interpreted.
    ///
    /// Carries the package and version so the refusal names what could not be used, and the underlying reason so a
    /// version mismatch stays distinguishable from corruption.
    #[error(
        "package `{library}` version {version} published an executable representation this compiler cannot use: {source}"
    )]
    UnusableRepresentation {
        /// Library the identity names.
        library: String,
        /// Version of that library, from its manifest.
        version: String,
        /// Why the representation could not be used.
        source: ExecutableRepresentationError,
    },
    /// The representation could not be read from disk.
    #[error("could not read the executable representation for package `{library}` at {path}: {reason}")]
    Unreadable {
        /// Library the identity names.
        library: String,
        /// Path that could not be read.
        path: PathBuf,
        /// Platform's account of the failure.
        reason: String,
    },
}

/// Resolve one imported declaration to the body its package published for it.
///
/// The identity is the only input that selects anything. The library and module path both come from the identity's
/// own origin, so a consumer cannot reach a different declaration than the one it resolved — which is what makes a
/// projection (an alias, a re-export, a facade) resolve to its target rather than to its own spelling, without this
/// function knowing that projections exist.
pub fn resolve_executable_declaration(
    index: &LibraryManifestIndex,
    identity: &CanonicalSymbolId,
) -> Result<Body, ExecutableResolutionError> {
    let SymbolOrigin::Package { library, module_path } = &identity.origin else {
        return Err(ExecutableResolutionError::NotAPackageDeclaration {
            declaration: identity.declaration_name.clone(),
        });
    };
    let Some(LibraryManifestIndexEntry::Loaded { manifest, metadata }) = index.get(library) else {
        return Err(ExecutableResolutionError::UnknownPackage {
            library: library.clone(),
        });
    };
    let version = manifest.version.clone();
    let Some(path) = executable_surface_path(&metadata.manifest_path, module_path) else {
        return Err(ExecutableResolutionError::NoPublishedRepresentation {
            library: library.clone(),
            version,
            module_path: module_path.join("."),
            declaration: identity.declaration_name.clone(),
        });
    };
    if !path.is_file() {
        return Err(ExecutableResolutionError::NoPublishedRepresentation {
            library: library.clone(),
            version,
            module_path: module_path.join("."),
            declaration: identity.declaration_name.clone(),
        });
    }
    let bytes = fs::read(&path).map_err(|error| ExecutableResolutionError::Unreadable {
        library: library.clone(),
        path: path.clone(),
        reason: error.to_string(),
    })?;
    // Read the declared version before interpreting anything, so an unsupported representation refuses in the
    // package's terms rather than surfacing as a decode failure from inside it.
    representation_version(&bytes).map_err(|source| ExecutableResolutionError::UnusableRepresentation {
        library: library.clone(),
        version: version.clone(),
        source,
    })?;
    let reader = SurfaceReader::open(&bytes).map_err(|source| ExecutableResolutionError::UnusableRepresentation {
        library: library.clone(),
        version: version.clone(),
        source,
    })?;
    reader
        .declaration(identity)
        .map_err(|source| ExecutableResolutionError::UnusableRepresentation {
            library: library.clone(),
            version,
            source,
        })
}

#[cfg(test)]
mod tests {
    use super::{ExecutableResolutionError, resolve_executable_declaration};
    use crate::frontend::library_manifest_index::{
        LibraryArtifactKind, LibraryArtifactMetadata, LibraryManifestIndex, LibraryManifestIndexEntry,
    };
    use crate::library_manifest::LibraryManifest;
    use crate::library_manifest::published_layout::executable_surface_path;
    use incan_semantics_core::{
        CanonicalSymbolId, HirSourceSpan, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin,
    };
    use std::collections::HashMap;
    use std::path::Path;

    /// An identity naming a declaration in a package's module, as an import resolves to.
    fn package_identity(library: &str, module: &str, declaration: &str) -> CanonicalSymbolId {
        CanonicalSymbolId {
            namespace: SymbolNamespace::OrdinaryLexical,
            origin: SymbolOrigin::Package {
                library: library.to_string(),
                module_path: vec![module.to_string()],
            },
            declaration_name: declaration.to_string(),
            kind: SemanticSourceTargetKind::Function,
            scope_discriminant: None,
            declaration_span: HirSourceSpan::new(0, 1),
        }
    }

    /// An index holding one resolved dependency whose manifest lives at `manifest_path`.
    fn index_with(library: &str, version: &str, manifest_path: &Path) -> LibraryManifestIndex {
        let mut entries = HashMap::new();
        entries.insert(
            library.to_string(),
            LibraryManifestIndexEntry::Loaded {
                manifest: Box::new(LibraryManifest::new(library, version)),
                metadata: LibraryArtifactMetadata {
                    dependency_key: library.to_string(),
                    manifest_name: library.to_string(),
                    manifest_path: manifest_path.to_path_buf(),
                    crate_root: manifest_path.parent().unwrap_or(Path::new(".")).to_path_buf(),
                    cargo_toml_path: manifest_path.with_file_name("Cargo.toml"),
                    crate_lib_path: manifest_path.with_file_name("lib.rs"),
                    kind: LibraryArtifactKind::Materialized,
                },
            },
        );
        LibraryManifestIndex::from_entries(entries)
    }

    #[test]
    fn a_local_declaration_is_not_asked_of_any_package() {
        // A declaration this compilation owns is executed from this compilation. Reaching for a published surface
        // would be looking for a package that was never involved.
        let mut identity = package_identity("pkg", "lib", "thing");
        identity.origin = SymbolOrigin::Module(vec!["lib".to_string()]);

        match resolve_executable_declaration(&LibraryManifestIndex::default(), &identity) {
            Err(ExecutableResolutionError::NotAPackageDeclaration { declaration }) => {
                assert_eq!(declaration, "thing");
            }
            other => panic!("a local declaration must refuse as local, got {other:?}"),
        }
    }

    #[test]
    fn an_unresolved_package_refuses_by_name() {
        match resolve_executable_declaration(
            &LibraryManifestIndex::default(),
            &package_identity("absent", "lib", "thing"),
        ) {
            Err(ExecutableResolutionError::UnknownPackage { library }) => assert_eq!(library, "absent"),
            other => panic!("an unresolved package must refuse by name, got {other:?}"),
        }
    }

    #[test]
    fn a_package_publishing_nothing_refuses_naming_package_version_and_requirement()
    -> Result<(), Box<dyn std::error::Error>> {
        // RFC 123: a package may publish no representation and stay valid for routes that do not need one. The
        // refusal has to say which package, which version, and what was required — not "unsupported".
        let tmp = tempfile::tempdir()?;
        let manifest_path = tmp.path().join("pkg.incnlib");
        let index = index_with("pkg", "2.1.0", &manifest_path);

        match resolve_executable_declaration(&index, &package_identity("pkg", "lib", "thing")) {
            Err(error @ ExecutableResolutionError::NoPublishedRepresentation { .. }) => {
                let message = error.to_string();
                assert!(
                    message.contains("pkg") && message.contains("2.1.0") && message.contains("thing"),
                    "the refusal must name package, version and declaration: {message}"
                );
            }
            other => return Err(format!("a package with no surface must refuse, got {other:?}").into()),
        }
        Ok(())
    }

    #[test]
    fn an_unusable_representation_refuses_in_the_packages_terms() -> Result<(), Box<dyn std::error::Error>> {
        // Corruption inside a dependency must not surface as a decode error from the middle of it. The consumer
        // names the package and version it could not use, which is what someone reading the failure can act on.
        let tmp = tempfile::tempdir()?;
        let manifest_path = tmp.path().join("pkg.incnlib");
        let surface_path = executable_surface_path(&manifest_path, &["lib".to_string()])
            .ok_or("a manifest path with a parent must yield a surface path")?;
        std::fs::create_dir_all(surface_path.parent().ok_or("surface path must have a parent")?)?;
        std::fs::write(&surface_path, [0xff_u8; 12])?;
        let index = index_with("pkg", "0.4.2", &manifest_path);

        match resolve_executable_declaration(&index, &package_identity("pkg", "lib", "thing")) {
            Err(error @ ExecutableResolutionError::UnusableRepresentation { .. }) => {
                let message = error.to_string();
                assert!(
                    message.contains("pkg") && message.contains("0.4.2"),
                    "the refusal must name the package and version it could not use: {message}"
                );
            }
            other => return Err(format!("an unreadable surface must refuse, got {other:?}").into()),
        }
        Ok(())
    }

    #[test]
    fn a_published_declaration_resolves_through_identity_alone() -> Result<(), Box<dyn std::error::Error>> {
        // The whole contract in one test: the surface is built by a producer, and the consumer reaches the
        // declaration using nothing but the identity — no spelling, no path it reconstructed, no listing.
        use incan_semantics_core::body_ir::{Block, Body, BodyIrModule, ScopeId};
        use incan_semantics_core::executable_representation::build_surface;
        use incan_semantics_core::{CompilerNodeId, CompilerNodeKind, IncanType};

        let identity = package_identity("pkg", "lib", "published");
        let module = BodyIrModule {
            module_id: CompilerNodeId::module("lib"),
            nominal_declarations: Vec::new(),
            fieldless_enum_declarations: Vec::new(),
            value_enum_declarations: Vec::new(),
            bodies: vec![Body {
                decl_id: CompilerNodeId::new(CompilerNodeKind::Declaration, "published".to_string()),
                direct_call_id: CompilerNodeId::new(CompilerNodeKind::Declaration, "published".to_string()),
                canonical: Some(identity.clone()),
                name: "published".to_string(),
                span: HirSourceSpan::new(0, 1),
                return_type: IncanType::Unknown,
                locals: Vec::new(),
                params: Vec::new(),
                param_locals: Vec::new(),
                scopes: Vec::new(),
                block: Block {
                    scope: ScopeId(0),
                    stmts: Vec::new(),
                },
                runtime_requirements: Vec::new(),
                panic_facts: Vec::new(),
                is_async: false,
            }],
        };

        let tmp = tempfile::tempdir()?;
        let manifest_path = tmp.path().join("pkg.incnlib");
        let surface_path = executable_surface_path(&manifest_path, &["lib".to_string()])
            .ok_or("a manifest path with a parent must yield a surface path")?;
        std::fs::create_dir_all(surface_path.parent().ok_or("surface path must have a parent")?)?;
        std::fs::write(&surface_path, build_surface(&module)?)?;

        let resolved = resolve_executable_declaration(&index_with("pkg", "1.0.0", &manifest_path), &identity)?;

        assert_eq!(resolved.name, "published");
        Ok(())
    }
}
