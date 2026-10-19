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

use incan_frontend::library_manifest::digest_provider_artifact;
use incan_frontend::loaf_index::LIBRARY_MANIFEST_EXTENSION;
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
