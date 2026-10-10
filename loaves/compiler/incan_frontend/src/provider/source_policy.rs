//! Opaque trusted source-publication selection for compiler-pinned ordinary standard packages (#1337/#1698).
//!
//! Selection binds the actual executable-relative source coordinate and original declaration to compiler policy.
//! It authorizes only publication from that source, not installed package metadata, namespace grants or semantic
//! completeness. Matching public names, manifests, paths and SDK catalogs cannot publicly construct this capability.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use incan_lang::lang::standard_packages::{StandardPackageNamespacePolicy, standard_package_namespace_policy};
use oven_model::manifest::{ManifestError, ProjectManifest};
use oven_model::toolchain_layout::{
    CompilerOwnedSourceLayout, CompilerOwnedSourceLayoutError, CompilerOwnedSourceMember,
};

/// Original canonical standard-package source and declaration retained against compiler-pinned ownership policy.
pub struct TrustedStandardSourcePublication {
    policy: &'static StandardPackageNamespacePolicy,
    declaration: CompilerOwnedSourceMember,
    policy_bytes: Vec<u8>,
}

impl std::fmt::Debug for TrustedStandardSourcePublication {
    /// Describe pinned identity and original coordinates without exposing or formatting retained file handles.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TrustedStandardSourcePublication")
            .field("package_name", &self.policy.package_name)
            .field("version", &self.policy.version)
            .field("package_root", &self.declaration.path().parent())
            .finish()
    }
}

/// Refusal to select trusted publication source or authorize another package's namespace roots.
#[derive(Debug, thiserror::Error)]
pub enum TrustedStandardSourcePublicationError {
    /// The actual executable-relative layout or retained original file failed verification.
    #[error(transparent)]
    Layout(#[from] CompilerOwnedSourceLayoutError),
    /// The pinned authored declaration does not follow the existing manifest grammar.
    #[error(transparent)]
    Declaration(#[from] ManifestError),
    /// Exact pinned policy could not be serialized for retention.
    #[error("could not encode compiler-pinned standard package policy: {0}")]
    PolicyEncoding(#[from] serde_json::Error),
    /// Public spelling is not an ordinary standard package named by the compiler.
    #[error("package `{package}` has no compiler-pinned standard namespace policy")]
    UnknownPackage {
        /// Rejected package selector.
        package: String,
    },
    /// Original declaration, selected package or requested namespace authority disagrees with compiler policy.
    #[error("standard package `{package}` publication refused: {reason}")]
    Invalid {
        /// Selected compiler-owned package.
        package: &'static str,
        /// Failed exact policy/declaration invariant.
        reason: &'static str,
    },
}

impl TrustedStandardSourcePublication {
    /// Read pinned own roots without filesystem work; the retained source must still be verified at each handoff.
    pub fn namespace_roots(&self) -> &'static [&'static str] {
        self.policy.namespace_roots
    }

    /// Select this compiler's own publication source by pinned package name and actual canonical executable only.
    ///
    /// The name selects policy, never a path. Absence is returned only when no supported source layout is installed.
    /// A present invalid layout/declaration refuses rather than falling through to another root, SDK or environment.
    pub fn discover(package_name: &str) -> Result<Option<Self>, TrustedStandardSourcePublicationError> {
        let policy = standard_package_namespace_policy(package_name).ok_or_else(|| {
            TrustedStandardSourcePublicationError::UnknownPackage {
                package: package_name.to_string(),
            }
        })?;
        let Some(layout) = CompilerOwnedSourceLayout::discover()? else {
            return Ok(None);
        };
        let layout = Arc::new(layout);
        let declaration = layout.open_member(&Path::new(policy.source_directory).join("loaf.toml"))?;
        if declaration.verified_bytes()? != policy.declaration.as_bytes() {
            return Err(invalid(
                policy,
                "source declaration bytes differ from the compiler-pinned declaration",
            ));
        }
        let parsed = ProjectManifest::from_str(policy.declaration, declaration.path())?;
        let project = parsed
            .project
            .as_ref()
            .ok_or_else(|| invalid(policy, "pinned declaration has no project identity"))?;
        if project.name.as_deref() != Some(policy.package_name) || project.version.as_deref() != Some(policy.version) {
            return Err(invalid(
                policy,
                "pinned package name/version disagree with the authored declaration",
            ));
        }
        let policy_bytes = serde_json::to_vec(&(
            "incan-standard-source-policy-v1",
            policy.package_name,
            policy.version,
            policy.source_directory,
            policy.namespace_roots,
            policy.declaration,
        ))?;
        let selected = Self {
            policy,
            declaration,
            policy_bytes,
        };
        selected.verify()?;
        Ok(Some(selected))
    }

    /// Revalidate the original executable/source coordinate, declaration handle and exact current pinned bytes.
    pub fn verify(&self) -> Result<(), TrustedStandardSourcePublicationError> {
        self.verified_declaration_bytes().map(|_| ())
    }

    /// Return the original canonical package source coordinate after revalidating its declaration and layout.
    pub fn verified_package_root(&self) -> Result<&Path, TrustedStandardSourcePublicationError> {
        self.verify()?;
        self.declaration
            .path()
            .parent()
            .ok_or_else(|| invalid(self.policy, "declaration parent missing"))
    }

    /// Return exact retained compiler policy bytes after source revalidation; a digest alone cannot replace them.
    pub fn verified_policy_bytes(&self) -> Result<&[u8], TrustedStandardSourcePublicationError> {
        self.verify()?;
        Ok(&self.policy_bytes)
    }

    /// Return exact retained authored declaration bytes after original-owner and source-coordinate revalidation.
    pub fn verified_declaration_bytes(&self) -> Result<&[u8], TrustedStandardSourcePublicationError> {
        let bytes = self.declaration.verified_bytes()?;
        if bytes != self.policy.declaration.as_bytes() {
            return Err(invalid(
                self.policy,
                "original declaration no longer matches compiler policy",
            ));
        }
        Ok(bytes)
    }

    /// Permit only this exact pinned package/version's reserved top-level namespace roots for source publication.
    ///
    /// Checked exports and active feature semantics must still be validated by the publisher. Passing this check
    /// neither issues a frontend namespace grant nor establishes installed package or semantic completeness authority.
    pub fn validate_namespace_roots(
        &self,
        package_name: &str,
        package_version: &str,
        roots: &BTreeSet<String>,
    ) -> Result<(), TrustedStandardSourcePublicationError> {
        self.verify()?;
        if package_name != self.policy.package_name || package_version != self.policy.version {
            return Err(invalid(
                self.policy,
                "publication package name/version differ from the selected source",
            ));
        }
        if roots
            .iter()
            .any(|root| !self.policy.namespace_roots.contains(&root.as_str()))
        {
            return Err(invalid(
                self.policy,
                "publication requests a namespace root not owned by this package",
            ));
        }
        Ok(())
    }
}

/// Refuse an exact compiler policy or original declaration contract violation.
fn invalid(
    policy: &'static StandardPackageNamespacePolicy,
    reason: &'static str,
) -> TrustedStandardSourcePublicationError {
    TrustedStandardSourcePublicationError::Invalid {
        package: policy.package_name,
        reason,
    }
}

#[cfg(test)]
mod tests;
