//! What Oven learns about Incan, in one place: the compiler's identity and the provider facts the ring asks for
//! through [`OvenProviderHooks`]. Oven's crates name no compiler crate; the driver hands them these values.
//!
//! The only crate in the compiler ring that depends on the Oven ring's bakers as well as on the provider loaders;
//! the binaries hand its two values to every Oven request.

#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]

use std::fs;
use std::path::Path;
use std::sync::Arc;

use incan_frontend::library_manifest::published_layout::LIBRARY_MANIFEST_EXTENSION;
use incan_frontend::library_manifest::digest_provider_artifact;
use incan_lang::version::{INCAN_VERSION, SDK_PROVIDER_CODEGEN_REVISION};
use oven_model::compiler_identity::CompilerIdentity;
use oven_store::{OvenProviderHookError, OvenProviderHooks};

/// The running compiler's identity for Oven: its version and the generated-provider revision it emits.
pub fn compiler_identity() -> CompilerIdentity {
    CompilerIdentity::new(INCAN_VERSION, SDK_PROVIDER_CODEGEN_REVISION)
}

/// The provider hooks every Oven request from this compiler carries.
pub fn provider_hooks() -> Arc<dyn OvenProviderHooks> {
    Arc::new(IncanProviderHooks)
}

/// Incan's answers to Oven's provider questions: what a packaged provider looks like on disk and the source
/// closure of an authored native Loaf.
#[derive(Debug, Default, Clone, Copy)]
pub struct IncanProviderHooks;

impl OvenProviderHooks for IncanProviderHooks {
    fn packaged_provider_digest(&self, dependency_root: &Path) -> Option<Result<String, OvenProviderHookError>> {
        is_packaged_provider_root(dependency_root).then(|| {
            digest_provider_artifact(dependency_root).map_err(|error| OvenProviderHookError::PackagedDigest {
                dependency_root: dependency_root.to_path_buf(),
                source: Box::new(error),
            })
        })
    }

    fn authored_native_source_digest(
        &self,
        dependency: &oven_model::manifest::DependencySpec,
    ) -> Option<Result<String, oven_store::OvenError>> {
        let oven_model::manifest::DependencySource::Path { path } = &dependency.source else {
            return None;
        };
        match fs::symlink_metadata(path.join("loaf.toml")) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
            Err(error) => {
                return Some(Err(oven_store::OvenError::InvalidProjectSource {
                    path: path.clone(),
                    message: error.to_string(),
                }));
            }
        }
        Some({
            oven_rustc::sdk_closure::local_native_dependency_source_digest(
                path,
                &dependency.features,
                dependency.default_features,
            )
            .map_err(|error| oven_store::OvenError::InvalidProjectSource {
                path: path.clone(),
                message: error.to_string(),
            })
        })
    }
}

/// Whether a path dependency root is a packaged Incan provider: a generated library crate carrying its `.incnlib`
/// manifest. An authored Rust crate has no such manifest.
fn is_packaged_provider_root(root: &Path) -> bool {
    let Ok(entries) = fs::read_dir(root) else {
        return false;
    };
    entries.flatten().any(|entry| {
        entry
            .path()
            .extension()
            .is_some_and(|extension| extension == LIBRARY_MANIFEST_EXTENSION)
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;

    use incan_lang::lang::stdlib::{self, StdlibExtraCrateSource};
    use incan_provider::requirements::{dependency_spec_from_stdlib_dep, dependency_specs_match};
    use oven_model::manifest::{DependencySource, DependencySpec, ProjectManifest};
    use oven_rustc::loaf::{
        OvenLoafEnvelope, OvenLoafMemberRole, loaf_envelope_inspection_packages, loaf_envelope_specifications,
    };
    use oven_store::digest_dependency_specs;

    use crate::provider_hooks;

    #[test]
    fn a_packaged_provider_is_identified_by_its_sealed_artifact_not_its_private_cargo_edges_issue1469()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = tempfile::tempdir()?;
        let provider = fixture.path().join("catalog/target/lib");
        fs::create_dir_all(provider.join("src"))?;
        // The generated manifest still names a private Rust crate that no longer exists.
        fs::write(
            provider.join("Cargo.toml"),
            "[package]\nname = \"immutable_catalog\"\nversion = \"0.1.0\"\n\n[dependencies.package_store_witness]\npath = \"../../../package-store-witness\"\n",
        )?;
        fs::write(provider.join("src/lib.rs"), "pub fn answer() -> i64 { 42 }\n")?;
        let dependency = DependencySpec {
            crate_name: "stock".to_string(),
            version: None,
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Path { path: provider.clone() },
            optional: false,
            package: None,
        };
        let as_authored_crate = digest_dependency_specs(std::slice::from_ref(&dependency), provider_hooks().as_ref());
        assert!(
            as_authored_crate.is_err(),
            "an authored crate's missing path dependency is still a fault: {as_authored_crate:?}"
        );

        fs::write(provider.join("immutable_catalog.incnlib"), "{}")?;
        // With its `.incnlib` beside the manifest the same tree is a packaged provider, and the missing private
        // crate is no longer anyone's business.
        let sealed = digest_dependency_specs(std::slice::from_ref(&dependency), provider_hooks().as_ref())?;
        fs::write(provider.join("src/lib.rs"), "pub fn answer() -> i64 { 43 }\n")?;
        assert_ne!(
            sealed,
            digest_dependency_specs(std::slice::from_ref(&dependency), provider_hooks().as_ref())?,
            "the sealed artifact's own bytes still decide its identity"
        );
        Ok(())
    }

    /// Return the canonical standard-library modules owned by checked SDK component sources.
    ///
    /// Component entrypoints are the source-of-truth provider surface. Normalizing their `*.prelude` implementation
    /// modules to their public facade mirrors provider publication, while `std.interop` is the intentionally
    /// source-less vocabulary-backed provider component.
    fn checked_stdlib_component_modules() -> Result<BTreeSet<String>, Box<dyn std::error::Error>> {
        let stdlib_root = oven_model::toolchain_layout::development_root().join("loaves/stdlib");
        let catalog = incan_frontend::provider::StdlibSources::from_root(&stdlib_root)?;
        let mut modules = BTreeSet::from(["std.interop".to_string()]);
        for component in catalog.catalog().components.values() {
            let source = component.project_root.join("src/lib.incn");
            if !source.is_file() {
                continue;
            }
            for line in fs::read_to_string(&source)?.lines() {
                let Some(import) = line.trim().strip_prefix("import ") else {
                    continue;
                };
                let module = import
                    .split_whitespace()
                    .next()
                    .ok_or("stdlib component import has no module path")?;
                let module = match module.strip_suffix(".prelude") {
                    Some(facade) => facade,
                    None => module,
                };
                modules.insert(format!("std.{module}"));
            }
        }
        Ok(modules)
    }

    /// Return the standard-library imports a checked complete-stdlib fixture declares.
    fn checked_stdlib_fixture_imports(source: &str) -> BTreeSet<String> {
        source
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                let module = line
                    .strip_prefix("import ")
                    .or_else(|| line.strip_prefix("from "))?
                    .split_whitespace()
                    .next()?;
                module.starts_with("std.").then(|| module.to_string())
            })
            .collect()
    }

    #[test]
    fn complete_stdlib_loaf_fixtures_cover_every_checked_component_module() -> Result<(), Box<dyn std::error::Error>> {
        let expected_modules = checked_stdlib_component_modules()?;
        for envelope in [OvenLoafEnvelope::Release, OvenLoafEnvelope::CompilerSuite] {
            for specification in loaf_envelope_specifications(envelope) {
                let fixture_modules = checked_stdlib_fixture_imports(specification.source);
                let missing = expected_modules
                    .difference(&fixture_modules)
                    .cloned()
                    .collect::<Vec<_>>();
                if !missing.is_empty() {
                    return Err(format!(
                        "{envelope:?}/{}/{} complete stdlib fixture omits checked provider modules: {}",
                        specification.label,
                        specification.profile,
                        missing.join(", ")
                    )
                    .into());
                }
            }
        }
        Ok(())
    }

    #[test]
    fn checked_envelope_names_the_complete_declared_repository_test_inspection_surface()
    -> Result<(), Box<dyn std::error::Error>> {
        let specifications = loaf_envelope_specifications(OvenLoafEnvelope::CompilerSuite);
        let packages = loaf_envelope_inspection_packages(OvenLoafEnvelope::CompilerSuite)?;
        assert_eq!(
            loaf_envelope_inspection_packages(OvenLoafEnvelope::Release)?,
            packages,
            "the release and compiler-suite `stdlib` Loafs must declare one identical complete standard-library dependency surface"
        );
        let mut expected_packages = stdlib::extra_crate_deps()
            .filter(|dependency| matches!(dependency.source, StdlibExtraCrateSource::Declared))
            .map(|dependency| {
                stdlib::extra_crate_package_alias(dependency.crate_name)
                    .unwrap_or(dependency.crate_name)
                    .to_string()
            })
            .collect::<BTreeSet<_>>();
        expected_packages.extend([
            "bitflags".to_string(),
            "semver".to_string(),
            "serde".to_string(),
            "serde_json".to_string(),
            // Declared root intent rather than a registry extra: the stdlib facet crates request tokio features,
            // and the Rust policy engine settles the closure's features only from what the Loaf declares.
            "tokio".to_string(),
            "uuid".to_string(),
        ]);
        assert_eq!(
            packages
                .iter()
                .map(|package| package.package.clone())
                .collect::<BTreeSet<_>>(),
            expected_packages
        );
        let provider = specifications
            .iter()
            .find(|specification| specification.label == "stdlib" && specification.profile == "debug")
            .ok_or("missing compiler-suite standard-provider Loaf")?;
        assert_eq!(
            provider
                .inspection_packages()?
                .iter()
                .map(|package| package.package.clone())
                .collect::<BTreeSet<_>>(),
            expected_packages
        );
        for imported_crate in ["rand", "uuid"] {
            assert!(
                provider.source.contains(&format!("from rust::{imported_crate}")),
                "the compiler-suite provider Loaf must retain an actual checked source import for `{imported_crate}` so reachable dependency resolution produces its direct-rustc leaf"
            );
        }
        assert!(
            provider.source.contains("std.serde"),
            "the compiler-suite provider Loaf must retain the checked stdlib serde surface that produces its derive-enabled direct-rustc leaf"
        );
        for reachable_use in ["Uuid.new_v4()", "thread_rng()", ".gen_range("] {
            assert!(
                provider.source.contains(reachable_use),
                "the compiler-suite provider Loaf must exercise `{reachable_use}` so its declared raw Rust dependency is usable rather than merely imported"
            );
        }
        assert!(provider.role.provides_source_authority());
        assert!(specifications.iter().all(|specification| {
            specification.role == OvenLoafMemberRole::CompiledClosureAndSourceAuthority
                && specification.retain_complete_registry_leaves
        }));
        Ok(())
    }

    /// The Loaf publisher declares every checked `[rust-dependencies]` entry on its generated root, where the
    /// standard-library facet requirements for the same crates join it. That merge refuses any identity
    /// disagreement, so the checked manifests must agree with the registry by meaning for every shared crate.
    #[test]
    fn checked_stdlib_manifests_agree_with_the_registry_for_every_shared_crate()
    -> Result<(), Box<dyn std::error::Error>> {
        for envelope in [OvenLoafEnvelope::Release, OvenLoafEnvelope::CompilerSuite] {
            for specification in loaf_envelope_specifications(envelope) {
                let path = std::path::Path::new("loaves/oven/oven_rustc/src/fixtures")
                    .join(format!("{}.toml", specification.project_name));
                let manifest = ProjectManifest::from_str(specification.manifest, &path)?;
                let mut shared = 0;
                for (alias, declared) in manifest.rust_dependencies() {
                    let Some(registry) = stdlib::find_extra_crate_dep(alias) else {
                        continue;
                    };
                    shared += 1;
                    let registry = dependency_spec_from_stdlib_dep(registry)?;
                    let declared = declared.clone().normalized();
                    assert!(
                        dependency_specs_match(&declared, &registry),
                        "{envelope:?}/{}/{} declares `{alias}` as {declared:?} but the standard-library registry requires {registry:?}",
                        specification.label,
                        specification.profile
                    );
                }
                assert!(
                    shared > 0,
                    "{envelope:?}/{}/{} shares no crate with the standard-library registry; this test proves nothing",
                    specification.label,
                    specification.profile
                );
            }
        }
        Ok(())
    }
}
