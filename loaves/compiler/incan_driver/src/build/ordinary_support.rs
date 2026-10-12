//! Compiler-owned declarations for the mandatory native dependencies of ordinary Incan programs.
//!
//! These are physical source selections, not namespace, macro or complete semantic authority. Native preparation
//! must still resolve the actual declared forward closures and verify their source archives and profile bindings.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use incan_lang::lang::generated_support::{DERIVE_CRATE, SUPPORT_CRATES_EVERY_PROGRAM_LINKS};
use incan_lang::lang::standard_packages::standard_package_namespace_policy;
use incan_lang::lang::stdlib::facets::CORE;
use oven_model::manifest::{DependencySource, DependencySpec, ProjectManifest};
use oven_model::toolchain_layout::{CompilerOwnedSourceLayout, CompilerOwnedSourceMember};

use crate::error::{CliError, CliResult};

/// Original executable-relative core/derive declarations and their actual implicit dependency requests.
///
/// Discovery has no environment or caller-selected path fallback. Installed source-free package selection must
/// supply its own authenticated ordinary dependencies; this source capability cannot stand in for that issuer.
pub(crate) struct CompilerSupportSources {
    layout: Arc<CompilerOwnedSourceLayout>,
    core: CompilerOwnedSourceMember,
    derive: CompilerOwnedSourceMember,
    owner: PathBuf,
    dependencies: Vec<DependencySpec>,
}

impl CompilerSupportSources {
    /// Select the pinned core declaration and its actual derive dependency beside the canonical executable.
    pub(crate) fn discover() -> CliResult<Option<Self>> {
        let Some(layout) = CompilerOwnedSourceLayout::discover().map_err(failure)? else {
            return Ok(None);
        };
        let layout = Arc::new(layout);
        let policy = standard_package_namespace_policy("incan_stdlib_core")
            .ok_or_else(|| failure("mandatory core package policy is unavailable"))?;
        let core = layout
            .open_member(&Path::new(policy.source_directory).join("loaf.toml"))
            .map_err(failure)?;
        let derive = layout
            .open_member(Path::new("derive/incan_derive/loaf.toml"))
            .map_err(failure)?;
        if core.verified_bytes().map_err(failure)? != policy.declaration.as_bytes() {
            return Err(failure(
                "mandatory native source declarations differ from compiler-pinned bytes",
            ));
        }
        let core_manifest = ProjectManifest::from_str(policy.declaration, core.path()).map_err(failure)?;
        let derive_manifest = ProjectManifest::from_str(
            std::str::from_utf8(derive.verified_bytes().map_err(failure)?).map_err(failure)?,
            derive.path(),
        )
        .map_err(failure)?;
        let core_root = parent(core.path())?;
        let derive_root = parent(derive.path())?;
        let core_project = core_manifest
            .project
            .as_ref()
            .ok_or_else(|| failure("core identity missing"))?;
        let derive_project = derive_manifest
            .project
            .as_ref()
            .ok_or_else(|| failure("derive identity missing"))?;
        if core_project.name.as_deref() != Some(policy.package_name)
            || core_project.version.as_deref() != Some(policy.version)
            || core_manifest
                .rust_facet
                .as_ref()
                .and_then(|facet| facet.name.as_deref())
                != Some(CORE)
            || core_manifest
                .rust_facet
                .as_ref()
                .and_then(|facet| facet.kind.as_deref())
                != Some("lib")
            || derive_project.name.as_deref() != Some(DERIVE_CRATE)
            || derive_manifest
                .rust_facet
                .as_ref()
                .and_then(|facet| facet.name.as_deref())
                != Some(DERIVE_CRATE)
            || derive_manifest
                .rust_facet
                .as_ref()
                .and_then(|facet| facet.kind.as_deref())
                != Some("proc-macro")
        {
            return Err(failure(
                "mandatory native declaration disagrees with the generated support registry",
            ));
        }
        let mut derive_dependency = core_manifest
            .rust_dependency_values()
            .into_iter()
            .find(|dependency| dependency.crate_name == DERIVE_CRATE)
            .ok_or_else(|| failure("core declaration has no actual native derive dependency"))?;
        let DependencySource::Path { path } = &derive_dependency.source else {
            return Err(failure("core derive dependency is not its compiler-owned source"));
        };
        let version = derive_project
            .version
            .as_deref()
            .ok_or_else(|| failure("derive version missing"))?;
        let requirement = derive_dependency
            .version
            .as_deref()
            .ok_or_else(|| failure("core derive version missing"))?;
        if !semver::VersionReq::parse(requirement)
            .map_err(failure)?
            .matches(&semver::Version::parse(version).map_err(failure)?)
            || derive_dependency
                .package
                .as_deref()
                .unwrap_or(&derive_dependency.crate_name)
                != DERIVE_CRATE
        {
            return Err(failure(
                "derive source identity does not satisfy the pinned core dependency",
            ));
        }
        if std::fs::canonicalize(core_root.join(path)).map_err(failure)? != derive_root || derive_dependency.optional {
            return Err(failure(
                "core derive dependency differs from its original mandatory source",
            ));
        }
        derive_dependency.source = DependencySource::Path {
            path: derive_root.to_path_buf(),
        };
        let mut dependencies = Vec::new();
        for alias in SUPPORT_CRATES_EVERY_PROGRAM_LINKS {
            dependencies.push(match alias {
                CORE => DependencySpec {
                    crate_name: alias.to_string(),
                    package: Some(policy.package_name.to_string()),
                    version: Some(format!("={}", policy.version)),
                    features: Vec::new(),
                    default_features: true,
                    optional: false,
                    source: DependencySource::Path {
                        path: core_root.to_path_buf(),
                    },
                },
                DERIVE_CRATE => derive_dependency.clone(),
                _ => {
                    return Err(failure(
                        "generated support registry has an unbound mandatory native dependency",
                    ));
                }
            });
        }
        let owner = layout.verified_installation_root().map_err(failure)?.to_path_buf();
        let selected = Self {
            layout,
            core,
            derive,
            owner,
            dependencies,
        };
        selected.verify()?;
        Ok(Some(selected))
    }

    /// Revalidate the original executable geometry, declaration coordinates and exact original bytes.
    pub(crate) fn verify(&self) -> CliResult<()> {
        self.layout.verify().map_err(failure)?;
        self.core.verified_bytes().map_err(failure)?;
        self.derive.verified_bytes().map_err(failure)?;
        Ok(())
    }

    /// Borrow genuine mandatory requests after revalidation; no dependency resolution or native admission occurs.
    pub(crate) fn dependencies(&self) -> CliResult<&[DependencySpec]> {
        self.verify()?;
        Ok(&self.dependencies)
    }

    /// Borrow the original canonical declaration-owner anchor used by the existing native selector.
    pub(crate) fn declaration_owner(&self) -> CliResult<&Path> {
        self.verify()?;
        Ok(&self.owner)
    }

    /// Borrow the original standard source tree after verifying its executable owner and declarations.
    pub(crate) fn verified_standard_source_root(&self) -> CliResult<&Path> {
        self.verify()?;
        self.layout.verified_source_root().map_err(failure)
    }
}

/// Require the original declaration to have a package directory.
fn parent(path: &Path) -> CliResult<&Path> {
    path.parent()
        .ok_or_else(|| failure("mandatory native declaration has no parent"))
}

/// Preserve source selection and declaration errors in the command error domain.
fn failure(error: impl std::fmt::Display) -> CliError {
    CliError::failure(error.to_string())
}

#[cfg(all(test, unix))]
mod tests;
