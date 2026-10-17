//! Explicit full producer-request authority for ordinary checked library metadata (#1337/#1698).
//!
//! Selected physical roots alone do not cover semantic or macro demands. This boundary retains every actual
//! producer request, including unselected units, and the genuine compiler-owned support sources. Callers must still
//! establish checked ABI, derive and vocabulary demand coverage before using it for a library preparation.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::build::ordinary_support::CompilerSupportSources;
use crate::error::CliResult;
use oven_rustc::native_loaf::NativeLoafRequestObservation;
#[cfg(feature = "rust_inspect")]
use oven_rustc::rustc::{OvenRustInspectionToolchain, prepare_rust_inspection_toolchain};
use oven_rustc::rustc::{rustc_host_target, rustc_identity};
use oven_store::{OvenBuildIntent, project_source_tree_evidence};

use super::invalid;

/// Original full native requests for exact requested profiles, retaining all producer and support source owners.
///
/// No public DTO, selected-root hint or SDK catalog can construct an original producer observation. This capability
/// proves current physical requests and mandatory support selection, not arbitrary checked program demand coverage.
pub(crate) struct OrdinaryNativeMetadataAuthority {
    support: Arc<CompilerSupportSources>,
    observations: BTreeMap<String, NativeLoafRequestObservation>,
    rustc: PathBuf,
    target: String,
    toolchain: String,
    host: String,
    #[cfg(feature = "rust_inspect")]
    inspection_toolchain: Arc<OvenRustInspectionToolchain>,
}

impl OrdinaryNativeMetadataAuthority {
    /// Bind actual full producer observations to the explicit consumer and genuine mandatory source declarations.
    /// Empty, unsupported or mismatched profile requests refuse without selecting ambient native authority.
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "#1698 ordinary library publication: only its tests call it until the ordinary command cutover does"
        )
    )]
    pub(crate) fn new(
        support: Arc<CompilerSupportSources>,
        observations: BTreeMap<String, NativeLoafRequestObservation>,
        rustc: &Path,
        target: &str,
        toolchain: &str,
    ) -> CliResult<Self> {
        validate_profiles(observations.keys().map(String::as_str))?;
        let rustc = rustc.canonicalize().map_err(|error| invalid(error.to_string()))?;
        let host = rustc_host_target(&rustc).map_err(|error| invalid(error.to_string()))?;
        #[cfg(feature = "rust_inspect")]
        let inspection_toolchain = Arc::new(
            prepare_rust_inspection_toolchain(&crate::oven_store::open_default_oven_store()?, &rustc, target)
                .map_err(|error| invalid(error.to_string()))?,
        );
        let selected = Self {
            support,
            observations,
            rustc,
            target: target.to_string(),
            toolchain: toolchain.to_string(),
            host,
            #[cfg(feature = "rust_inspect")]
            inspection_toolchain,
        };
        selected.verify()?;
        Ok(selected)
    }

    /// Borrow the original compiler coordinate; no active-toolchain discovery participates in this route.
    pub(crate) fn rustc(&self) -> &Path {
        &self.rustc
    }

    /// Borrow the explicit requested consumer target independently of host proc-macro units.
    pub(crate) fn target(&self) -> &str {
        &self.target
    }

    /// Borrow complete original requests for current-profile planning without reacquiring native owners.
    pub(crate) fn observations(&self) -> &BTreeMap<String, NativeLoafRequestObservation> {
        &self.observations
    }

    /// Borrow an originally requested inspection profile, preferring debug only when it is already included.
    /// This does not prepare another profile; the inspection handoff still verifies the original observation.
    pub(crate) fn inspection_request(&self) -> CliResult<(&str, &NativeLoafRequestObservation)> {
        self.observations
            .get_key_value("debug")
            .or_else(|| self.observations.first_key_value())
            .map(|(profile, observation)| (profile.as_str(), observation))
            .ok_or_else(|| invalid("ordinary inspection lacks an original requested native profile"))
    }

    /// Borrow the genuine support source capability for explicit inspection and planning integration.
    pub(crate) fn support(&self) -> &Arc<CompilerSupportSources> {
        &self.support
    }

    /// Borrow the exact immutable Rust source owner bound into this metadata recipe, without selecting it again.
    #[cfg(feature = "rust_inspect")]
    pub(crate) fn inspection_toolchain(&self) -> &Arc<OvenRustInspectionToolchain> {
        &self.inspection_toolchain
    }

    /// Include actual Rust inspection source ownership only in a build that can produce Rust ABI metadata.
    fn inspection_identity(&self) -> Option<&str> {
        #[cfg(feature = "rust_inspect")]
        {
            Some(self.inspection_toolchain.identity())
        }
        #[cfg(not(feature = "rust_inspect"))]
        {
            None
        }
    }

    /// Revalidate full requests, exact compiler/profile intent and genuine mandatory forward root selections.
    #[track_caller]
    pub(crate) fn verify(&self) -> CliResult<()> {
        self.verified_requests().map(|_| ())
    }

    /// Produce deterministic full-request semantic inputs, preserving compile-time coordinates and unselected units.
    #[track_caller]
    pub(super) fn semantic_inputs(&self) -> CliResult<serde_json::Value> {
        Ok(serde_json::json!({
            "contract": "ordinary-native-full-producer-requests-v2",
            "requests": self.verified_requests()?,
            "rust_inspection_toolchain": self.inspection_identity(),
            "target": self.target,
            "toolchain": self.toolchain,
            "host": self.host,
        }))
    }

    /// Bind compiler-owned source while each selected package's full recipe independently owns its canonical lock.
    pub(super) fn standard_source_digest(&self) -> CliResult<String> {
        shared_standard_source_digest(self.support.verified_standard_source_root()?)
    }

    /// Verify the requested full producer observation at a physical profile handoff, retaining its original owners.
    ///
    /// Metadata identity still calls `verified_requests` for every supplied profile. A debug-only physical plan does
    /// not consume release bytes, but never narrows the selected profile's complete request to its linked roots.
    #[track_caller]
    pub(crate) fn verify_profile(&self, intent: &OvenBuildIntent) -> CliResult<&NativeLoafRequestObservation> {
        self.verify_compiler()?;
        if intent.target != self.target || intent.toolchain != self.toolchain {
            return Err(invalid(
                "ordinary native profile differs from the requested compiler intent",
            ));
        }
        let observation = self
            .observations
            .get(&intent.profile)
            .ok_or_else(|| invalid("ordinary library lacks the requested native profile"))?;
        self.verify_original_profile(&intent.profile, observation)?;
        Ok(observation)
    }

    /// Check shared original compiler and mandatory source authority before observing any profile.
    fn verify_compiler(&self) -> CliResult<()> {
        validate_profiles(self.observations.keys().map(String::as_str))?;
        self.support.verify()?;
        if self.target.is_empty()
            || rustc_identity(&self.rustc).map_err(|error| invalid(error.to_string()))? != self.toolchain
            || rustc_host_target(&self.rustc).map_err(|error| invalid(error.to_string()))? != self.host
        {
            return Err(invalid("ordinary metadata native compiler/target authority changed"));
        }
        Ok(())
    }

    /// Verify every profile's original request and support selection, returning only validated complete digests.
    #[track_caller]
    fn verified_requests(&self) -> CliResult<BTreeMap<String, String>> {
        let caller = std::panic::Location::caller();
        let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_driver::verify_ordinary_metadata_requests", profiles = self.observations.len(), caller_file = caller.file(), caller_line = u64::from(caller.line())).entered();
        self.verify_compiler()?;
        #[cfg(feature = "rust_inspect")]
        self.inspection_toolchain
            .verify_compiler(&self.rustc, &self.target)
            .map_err(|error| invalid(error.to_string()))?;
        self.observations
            .iter()
            .map(|(profile, observation)| {
                self.verify_original_profile(profile, observation)
                    .map(|digest| (profile.clone(), digest.to_string()))
            })
            .collect()
    }

    /// Recheck one complete original request, including unlinked units and the actual mandatory support roots.
    fn verify_original_profile<'a>(
        &self,
        profile: &str,
        observation: &'a NativeLoafRequestObservation,
    ) -> CliResult<&'a str> {
        let dependencies = self.support.dependencies()?;
        let owner = self.support.declaration_owner()?;
        #[cfg(test)]
        METADATA_REQUEST_VERIFICATIONS.with(|work| work.set(work.get() + 1));
        let digest = observation
            .verify_intent(
                &self.rustc,
                &OvenBuildIntent {
                    target: self.target.clone(),
                    toolchain: self.toolchain.clone(),
                    profile: profile.to_string(),
                    features: Vec::new(),
                },
            )
            .map_err(|error| invalid(error.to_string()))?;
        let roots = observation
            .graph()
            .select_dependency_roots(dependencies, owner, "target")
            .map_err(|error| invalid(error.to_string()))?;
        if roots.len() != dependencies.len() {
            return Err(invalid(
                "ordinary metadata lacks complete mandatory native support roots",
            ));
        }
        for unit in observation.graph().units().values() {
            let record = unit.record();
            let target = match record.source.domain.as_str() {
                "target" => &self.target,
                "host" => &self.host,
                _ => {
                    return Err(invalid(
                        "ordinary metadata native record has an unknown compilation domain",
                    ));
                }
            };
            if record.recipe.intent.target != *target
                || record.recipe.intent.toolchain != self.toolchain
                || record.recipe.intent.profile != profile
            {
                return Err(invalid(
                    "ordinary metadata native record differs from requested profile/target",
                ));
            }
        }
        Ok(digest)
    }
}

/// Bind the shared compiler source family while canonical registered locks remain in their owners' full recipes.
///
/// Callers retain genuine compiler-owned source authority. Exact embedded declarations identify the only package
/// coordinates eligible for projection; unknown and nested lock files remain authored input. Full observation still
/// rejects symlinks. This digest neither admits dependencies nor authorizes any package lock writer or transition.
pub(in crate::build::library_project) fn shared_standard_source_digest(standard_root: &Path) -> CliResult<String> {
    // ---- Complete source evidence, including every file before projection ----
    let standard_root = standard_root
        .canonicalize()
        .map_err(|error| invalid(error.to_string()))?;
    let tree = project_source_tree_evidence(&standard_root).map_err(|error| invalid(error.to_string()))?;
    let mut locks = Vec::new();

    // ---- Only compiler-registered declarations identify canonical package locks ----
    for policy in incan_lang::lang::standard_packages::STANDARD_PACKAGE_NAMESPACE_POLICIES {
        let declaration = Path::new(policy.source_directory).join("loaf.toml");
        let expected = oven_store::digest_bytes(policy.declaration.as_bytes());
        if tree
            .file_digest(&declaration)
            .map_err(|error| invalid(error.to_string()))?
            != Some(expected.as_str())
        {
            return Err(invalid(
                "standard source declaration differs from compiler-pinned policy",
            ));
        }
        let lock = super::lock_transition::canonical_lock_coordinate(&standard_root.join(policy.source_directory))?;
        if let Ok(relative) = lock.strip_prefix(&standard_root) {
            locks.push(relative.to_path_buf());
        }
    }
    tree.digest_except_exact_files(&locks)
        .map_err(|error| invalid(error.to_string()))
}

#[cfg(test)]
thread_local! {
    /// Actual full-profile request verification attempts made by this test thread's metadata authority.
    static METADATA_REQUEST_VERIFICATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Read the existing verification-work counter without resetting the surrounding publication measurement.
#[cfg(test)]
pub(crate) fn metadata_request_verifications() -> usize {
    METADATA_REQUEST_VERIFICATIONS.with(std::cell::Cell::get)
}

/// Take measured complete-request verification work without caching authority or changing refusal policy.
#[cfg(test)]
pub(crate) fn take_metadata_request_verifications() -> usize {
    METADATA_REQUEST_VERIFICATIONS.with(|work| work.replace(0))
}

/// Validate exact caller-selected profiles without consulting the ambient bake-profile selector.
fn validate_profiles<'a>(profiles: impl IntoIterator<Item = &'a str>) -> CliResult<()> {
    let mut count = 0;
    for profile in profiles {
        if !matches!(profile, "debug" | "release") {
            return Err(invalid("ordinary metadata native request has an unsupported profile"));
        }
        count += 1;
    }
    if count == 0 {
        return Err(invalid("ordinary metadata lacks original native producer requests"));
    }
    Ok(())
}
