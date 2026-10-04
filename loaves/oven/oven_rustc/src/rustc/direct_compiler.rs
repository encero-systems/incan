//! Store-owned `rustc` ownership and the just-equal compilation it authorizes.
//!
//! Split out of `rustc.rs` rather than annotated in place: that module was over twelve thousand lines, and a
//! module-wide allow there would also hide dead code in the live compiler paths. These items are one unit — a
//! compiler closure is retained under a lease, its evidence names the exact binary and closure digests, and a
//! prepared library binds to that evidence before it may compile.
//!
//! The separation between *prepared* and *bound* is deliberate: preparation records what a compilation would
//! read, and only binding — after `verify_current_inputs` re-checks them — produces something that may run.
#![allow(
    dead_code,
    reason = "Gates 6 and 7 of RFC 119 are the reader; this substrate lands before them"
)]

use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use oven_model::manifest::{RustFactArgument, RustFactEnvironment, RustFactLibraryKind, RustFactLink};
use oven_store::process::BoundedProcessLimits;
use oven_store::publisher_execution::{
    PublisherExecutionArgument, PublisherExecutionEnvironmentValue, PublisherExecutionInput, PublisherExecutionObject,
    PublisherExecutionReceipt, PublisherExecutionRequest, execute_publisher_work,
};

use super::{
    OvenDirectRustcBake, OvenRustcArtifactManifest, OvenRustcArtifactPlan, OvenRustcError,
    OvenTrustedDirectRustcTargetRequest, caller_output_path, canonical_directory, digest_regular_file,
    oven_profile_codegen_options, parse_rustc_diagnostics, resolve_compile_environment_value,
    trusted_artifact_plan_for_source, validate_edition, validate_rust_identifier, verified_regular_file,
    verify_rustc_identity,
};
use oven_store::digest_bytes;
use oven_store::store::OvenStoreExecutionPayload;

pub(crate) mod retention;

/// Publisher-only request to turn one admitted native-link fact into a receipted archive product.
pub struct OvenPublisherLinkBakeRequest<'a> {
    /// Fully validated typed link work from the selected fact record.
    pub link: &'a RustFactLink,
    /// Exact target selected for the consuming Rust unit.
    pub selected_target: &'a str,
    /// Archive format resolved from the selected rustc target specification.
    pub archive_format: &'a str,
    /// Exact toolchain identity selected for the consuming closure.
    pub toolchain: &'a str,
    /// Source-selected unit identity before this product owner is attached.
    pub consuming_unit_identity: &'a str,
    /// Immutable physical root corresponding to [`RustFactLink::executable`]'s owner.
    pub executable_owner_root: &'a Path,
    /// Immutable physical root containing the fact-declared source closure.
    pub source_owner_root: &'a Path,
    /// Fresh private product root. Existing content is an archive collision and is refused.
    pub output_root: &'a Path,
    /// Bounded process limits selected by the publisher policy.
    pub limits: BoundedProcessLimits,
}

/// Finished asset-side native-link product ready for publisher finalization.
#[derive(Debug)]
pub struct OvenPublisherLinkProduct {
    /// Linker-visible logical library name.
    pub library_name: String,
    /// Static or dynamic linkage class.
    pub library_kind: RustFactLibraryKind,
    /// Exact target association inherited from the selected unit.
    pub target: String,
    /// Portable archive path below the private product root.
    pub archive_relative_path: String,
    /// Physical verified archive path pending immutable publication.
    pub archive_path: PathBuf,
    /// Private product root that becomes the immutable asset root during publisher finalization.
    pub product_root: PathBuf,
    /// Exact archive byte identity.
    pub archive_digest: String,
    /// Receipt binding compiler, argv, sources, product and consumer source identity.
    pub receipt: PublisherExecutionReceipt,
}

/// Execute one link fact only at the explicit publisher boundary.
///
/// The executable receives exactly the declared arguments and an empty environment plus the declared entries. The
/// deterministic archive name is an output contract, not an injected argument: a declaration that needs an output
/// argument spells that relative name as a literal. This preserves argv equality between selection and execution.
pub fn bake_publisher_link(
    request: &OvenPublisherLinkBakeRequest<'_>,
) -> Result<OvenPublisherLinkProduct, OvenRustcError> {
    if request.link.library.kind == RustFactLibraryKind::Dynamic {
        return Err(OvenRustcError::InvalidInput {
            field: "publisher link library",
            message:
                "dynamic libraries are not supported; publisher link records currently produce static archives only"
                    .to_string(),
        });
    }
    if !link_target_matches(&request.link.target, request.selected_target)? {
        return Err(OvenRustcError::InvalidInput {
            field: "publisher link target",
            message: format!(
                "declared target `{}` does not match selected target `{}`",
                request.link.target, request.selected_target
            ),
        });
    }
    let executable = owner_relative_path(
        request.executable_owner_root,
        &request.link.executable.path,
        "publisher link executable",
    )?;
    let mut inputs = Vec::with_capacity(request.link.sources.len());
    for source in &request.link.sources {
        inputs.push(PublisherExecutionInput {
            name: &source.name,
            path: source_owner_path(request.source_owner_root, source)?,
            digest: source.digest.clone(),
            kind: source.kind,
            members: source.members.clone(),
        });
    }
    let objects = request
        .link
        .objects
        .iter()
        .map(|object| -> Result<PublisherExecutionObject<'_>, OvenRustcError> {
            let arguments = object
                .arguments
                .iter()
                .map(|argument| match argument {
                    RustFactArgument::Literal { literal } => PublisherExecutionArgument::Literal(literal),
                    RustFactArgument::Input { input } => PublisherExecutionArgument::Input(input),
                    RustFactArgument::Output { output } => PublisherExecutionArgument::Output(output),
                    RustFactArgument::Owner { owner } => PublisherExecutionArgument::Owner(owner),
                })
                .collect();
            Ok(PublisherExecutionObject {
                name: &object.name,
                arguments,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let environment = publisher_link_environment(&request.link.environment)?;
    let executable_owner_paths = std::iter::once(request.link.executable.path.as_str())
        .chain(request.link.objects.iter().flat_map(|object| {
            object.arguments.iter().filter_map(|argument| match argument {
                RustFactArgument::Owner { owner } => Some(owner.as_str()),
                _ => None,
            })
        }))
        .collect();
    let archive_relative_path = publisher_link_archive_name(&request.link.library.name);
    let execution = execute_publisher_work(&PublisherExecutionRequest {
        role: "link",
        name: &request.link.name,
        consuming_unit_identity: request.consuming_unit_identity,
        target: request.selected_target,
        archive_format: request.archive_format,
        toolchain: request.toolchain,
        executable: &executable,
        executable_owner_root: request.executable_owner_root,
        executable_owner: request.link.executable.owner.clone(),
        executable_owner_paths,
        executable_digest: &request.link.executable.digest,
        objects,
        environment,
        inputs,
        archive_relative_path: &archive_relative_path,
        output_root: request.output_root,
        limits: request.limits,
    })
    .map_err(|error| OvenRustcError::InvalidInput {
        field: "publisher link execution",
        message: error.to_string(),
    })?;
    let product = execution.outputs.first().ok_or_else(|| OvenRustcError::InvalidInput {
        field: "publisher link execution",
        message: "successful execution produced no archive record".to_string(),
    })?;
    Ok(OvenPublisherLinkProduct {
        library_name: request.link.library.name.clone(),
        library_kind: request.link.library.kind,
        target: request.selected_target.to_string(),
        archive_relative_path: product.path.clone(),
        archive_path: request.output_root.join(&product.path),
        product_root: request.output_root.to_path_buf(),
        archive_digest: product.digest.clone(),
        receipt: execution.receipt,
    })
}

/// Select the deterministic archive container required by one Rust target triple.
///
/// This mirrors rustc's object-format families without probing the host: Apple targets use indexed Darwin archives,
/// MSVC targets use COFF, BSD targets use the BSD variant, and the remaining supported Rust targets use GNU format.
pub fn publisher_archive_format(target: &str) -> &'static str {
    if target.contains("-apple-") {
        "darwin"
    } else if target.contains("-windows-msvc") {
        "coff"
    } else if target.contains("freebsd") || target.contains("netbsd") || target.contains("openbsd") {
        "bsd"
    } else {
        "gnu"
    }
}

/// Resolve a declared link source, allowing `.` only for a complete tree rooted at the source owner.
fn source_owner_path(root: &Path, source: &oven_model::manifest::RustFactArtifact) -> Result<PathBuf, OvenRustcError> {
    if source.kind == oven_model::manifest::RustFactArtifactKind::Tree && source.path == "." {
        return Ok(root.to_path_buf());
    }
    owner_relative_path(root, &source.path, "publisher link source")
}

/// Resolve one plain owner-relative path without accepting traversal or absolute paths.
fn owner_relative_path(root: &Path, relative: &str, field: &'static str) -> Result<PathBuf, OvenRustcError> {
    let relative = Path::new(relative);
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(OvenRustcError::InvalidInput {
            field,
            message: "must be a plain owner-relative path".to_string(),
        });
    }
    Ok(root.join(relative))
}

/// Convert declared environment entries without consulting ambient process state.
fn publisher_link_environment<'a>(
    declared: &'a [RustFactEnvironment],
) -> Result<BTreeMap<&'a str, PublisherExecutionEnvironmentValue<'a>>, OvenRustcError> {
    declared
        .iter()
        .map(|entry| {
            let value = match (&entry.literal, &entry.input) {
                (Some(value), None) => PublisherExecutionEnvironmentValue::Literal(value),
                (None, Some(input)) => PublisherExecutionEnvironmentValue::Input(input),
                _ => {
                    return Err(OvenRustcError::InvalidInput {
                        field: "publisher link environment",
                        message: format!("entry `{}` must declare exactly one value", entry.name),
                    });
                }
            };
            Ok((entry.name.as_str(), value))
        })
        .collect()
}

/// Derive the one portable static-archive name from the logical library contract.
fn publisher_link_archive_name(name: &str) -> String {
    format!("lib{name}.a")
}

/// Match an exact triple or the settled one-equality `cfg(...)` spelling against target evidence.
fn link_target_matches(predicate: &str, target: &str) -> Result<bool, OvenRustcError> {
    if !predicate.starts_with("cfg(") {
        return Ok(predicate == target);
    }
    let body = predicate
        .strip_prefix("cfg(")
        .and_then(|value| value.strip_suffix(')'))
        .ok_or_else(|| OvenRustcError::InvalidInput {
            field: "publisher link target",
            message: format!("unsupported target predicate `{predicate}`"),
        })?;
    let (key, quoted) = body.split_once('=').ok_or_else(|| OvenRustcError::InvalidInput {
        field: "publisher link target",
        message: format!("unsupported target predicate `{predicate}`"),
    })?;
    let value = quoted
        .trim()
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .ok_or_else(|| OvenRustcError::InvalidInput {
            field: "publisher link target",
            message: format!("unsupported target predicate `{predicate}`"),
        })?;
    let parts = target.split('-').collect::<Vec<_>>();
    let observed = match key.trim() {
        "target_arch" => parts.first().copied(),
        "target_vendor" => parts.get(1).copied(),
        "target_os" if target.contains("apple-darwin") => Some("macos"),
        "target_os" => parts.get(2).copied(),
        "target_env" => parts.get(3).copied(),
        _ => None,
    };
    Ok(observed == Some(value))
}

/// Schema of the owner descriptor a retained compiler closure is selected by.
///
/// A warm closure whose descriptor matches is the batch authority without the ambient sysroot being rescanned, so
/// the version moves whenever the closure's member set does: version 3 closures carry every compiler-owned host
/// helper below `lib/rustlib/<host>/bin`, and older closures are left unselected rather than admitted to a compile
/// whose strip, link, profile or component-link helper may be absent.
pub(crate) const OVEN_DIRECT_RUSTC_COMPILER_OWNER_SCHEMA_VERSION: u32 = 3;
pub(crate) const OVEN_DIRECT_RUSTC_COMPILER_DOMAIN_PREFIX: &str = "native-compiler";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OvenDirectRustcCompilerOwnerPayload {
    pub(crate) schema_version: u32,
    pub(crate) binary_digest: String,
    pub(crate) closure_digest: String,
    pub(crate) host: String,
    pub(crate) target: String,
    pub(crate) toolchain: String,
}

/// Toolchain-owned bytes that can affect a direct-Rustc compilation independently of package provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OvenDirectRustcCompilerEvidence {
    pub(crate) binary_digest: String,
    pub(crate) closure_digest: String,
    pub(crate) host: String,
    pub(crate) target: String,
    pub(crate) sysroot: PathBuf,
    pub(crate) members: Vec<OvenDirectRustcCompilerMember>,
}

/// One physical compiler member paired with its location-independent sysroot coordinate and byte identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OvenDirectRustcCompilerMember {
    pub(crate) relative_path: String,
    pub(crate) source_path: PathBuf,
    pub(crate) digest: String,
}

/// Store-owned compiler closure retained under one lease for an entire native provider batch.
pub struct OvenOwnedDirectRustcCompiler {
    /// The leased Store payload the compiler closure is read from; holding it keeps the lease for the batch.
    pub(crate) owner: OvenStoreExecutionPayload,
    pub(crate) evidence: OvenDirectRustcCompilerEvidence,
    pub(crate) rustc: PathBuf,
}

/// Outcome of trying to retain the selected compiler closure for JEC.
///
/// Unavailability is deliberately distinct from a hard receipt or intent mismatch: callers may keep compiling
/// deterministically through their admitted compiler, but must surface why byte-identical reuse was disabled.
///
/// `Retained` is boxed because it carries the whole owned compiler, several hundred bytes against `Unavailable`'s
/// single string. The unavailable arm is the common one on a cold store, and every caller moves the value.
pub enum OvenDirectRustcCompilerRetention {
    Retained(Box<OvenOwnedDirectRustcCompiler>),
    Unavailable { reason: String },
}

impl OvenDirectRustcCompilerEvidence {
    /// Content digest of the `rustc` binary itself, separate from the closure around it.
    pub fn binary_digest(&self) -> &str {
        &self.binary_digest
    }

    /// Digest over every member of the compiler closure, which is what makes two installations comparable.
    pub fn closure_digest(&self) -> &str {
        &self.closure_digest
    }

    /// Triple the retained compiler runs on, as opposed to the one it emits for.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Triple the retained compiler emits for, as opposed to the one it runs on.
    pub fn target(&self) -> &str {
        &self.target
    }
}

impl OvenOwnedDirectRustcCompiler {
    /// Path to the store-owned `rustc`, valid only while this closure holds its lease.
    pub fn rustc(&self) -> &Path {
        &self.rustc
    }

    /// Borrow the admitted identity of this retained closure, without exposing the lease that holds it.
    pub fn evidence(&self) -> &OvenDirectRustcCompilerEvidence {
        &self.evidence
    }

    /// Toolchain identity the closure was admitted under, as the receipt records it.
    pub fn toolchain(&self) -> &str {
        &self.owner.manifest.intent.toolchain
    }
}

/// Logical names paired with the exact physical members of one prepared invocation.
///
/// The admitted native/source-unit plan decides membership. This binding gives the native-compilation policy stable
/// names for those already selected paths; it cannot add an extern, search directory or source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OvenDirectRustcJecBindings {
    pub logical_source_root: String,
    pub logical_entrypoint: String,
    /// Logical native-member slot for every prepared `--extern`, in exact argument order.
    pub externs: Vec<String>,
    /// Logical search slot for every prepared `-L dependency`, in exact argument order.
    pub dependency_searches: Vec<String>,
    /// Logical search slot for every prepared `-L native`, in exact argument order.
    pub native_searches: Vec<String>,
}

/// A fully observed direct-Rustc library invocation before output lookup or compiler execution.
///
/// This owns the source-specific physical plan so the Incan projection and Rustc consume one preparation. Package
/// name/version and Store coordinates remain outside this value; callers retain them only in request bindings.
pub struct OvenPreparedDirectRustcLibrary {
    pub(crate) rustc: PathBuf,
    pub(crate) compiler: Option<OvenDirectRustcCompilerEvidence>,
    /// Lease-protected Store owner for `compiler`; production JEC preparations retain it instead of rereading its
    /// immutable bytes before every provider unit.
    pub(crate) compiler_owner: Option<Arc<OvenOwnedDirectRustcCompiler>>,
    pub(crate) source: PathBuf,
    pub(crate) source_root: PathBuf,
    pub(crate) source_digest: String,
    pub(crate) artifact_root: PathBuf,
    pub(crate) selected_artifacts: OvenRustcArtifactManifest,
    pub(crate) plan: OvenRustcArtifactPlan,
    pub(crate) target: String,
    pub(crate) profile: String,
    pub(crate) crate_name: String,
    pub(crate) edition: String,
    pub(crate) features: Vec<String>,
    /// Complete deterministic environment derived from the admitted native-plan values.
    ///
    /// Values remain process-local. Callers expose only their digests to the Incan projection.
    pub(crate) frozen_environment: Option<BTreeMap<String, PathBuf>>,
}

/// One environment name observed by Rustc, with a digest only when it was present in the frozen environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OvenRustcObservedEnvironment {
    pub name: String,
    pub value_digest: Option<String>,
}

/// Parsed Rustc dependency observations. Raw dep-info bytes and environment values never cross this boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OvenRustcDepInfoObservation {
    pub files: Vec<PathBuf>,
    pub environment: Vec<OvenRustcObservedEnvironment>,
}

/// Sanitized observation result. An unsupported dep-info shape disables reuse without failing a successful compile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OvenRustcDepInfoOutcome {
    Observed(OvenRustcDepInfoObservation),
    Unavailable { reason: String },
}

/// Fresh direct-Rustc output plus the sanitized observations needed for an Incan-owned compilation key.
pub struct OvenDirectRustcJecCompilation {
    pub bake: OvenDirectRustcBake,
    pub observation: OvenRustcDepInfoOutcome,
}

/// One immutable physical invocation, including its exact output path, argv and cwd.
///
/// Cacheable execution uses the prepared frozen environment. An explicitly uncacheable fallback retains the legacy
/// direct-Rustc inheritance contract.
pub struct OvenBoundDirectRustcLibrary<'prepared> {
    pub(crate) prepared: &'prepared OvenPreparedDirectRustcLibrary,
    pub(crate) output: PathBuf,
    pub(crate) arguments: Vec<OsString>,
    pub(crate) logical_arguments: Vec<Vec<String>>,
    pub(crate) path_effects_digest: Option<String>,
}

/// Prepare one provider-owned Rust library from the exact admitted source role and physical native plan.
///
/// The result is deliberately output-agnostic: Incan first projects its candidate identity, after which the caller
/// chooses the corresponding candidate directory for either retained-output validation or a fresh compile.
pub fn prepare_trusted_direct_rustc_library_with_artifact_role(
    request: &OvenTrustedDirectRustcTargetRequest<'_>,
    artifact_role: &str,
    source_root: &Path,
    compiler: Option<&Arc<OvenOwnedDirectRustcCompiler>>,
) -> Result<OvenPreparedDirectRustcLibrary, OvenRustcError> {
    request
        .receipt
        .verify_identity()
        .map_err(|error| OvenRustcError::InvalidInput {
            field: "receipt",
            message: error.to_string(),
        })?;
    if compiler.is_none() {
        verify_rustc_identity(request.rustc, &request.receipt.intent.toolchain)?;
    }
    validate_rust_identifier(request.crate_name)?;
    validate_edition(request.edition)?;
    if request.prefer_dynamic {
        return Err(OvenRustcError::InvalidInput {
            field: "JEC library",
            message: "does not support a dynamic Rust standard-library binding".to_string(),
        });
    }
    if let Some(compiler) = compiler {
        let evidence = compiler.evidence();
        let request_rustc = fs::canonicalize(request.rustc).map_err(|source| OvenRustcError::Io {
            path: request.rustc.to_path_buf(),
            source,
        })?;
        if request_rustc != compiler.rustc
            || evidence.target != request.receipt.intent.target
            || compiler.toolchain() != request.receipt.intent.toolchain
        {
            return Err(OvenRustcError::InvalidInput {
                field: "JEC compiler evidence",
                message: "is detached from the retained receipt-selected compiler owner".to_string(),
            });
        }
    }
    let source_root = canonical_directory(source_root, "JEC source root")?;
    let source = fs::canonicalize(verified_regular_file(request.source, "source")?).map_err(|source_error| {
        OvenRustcError::Io {
            path: request.source.to_path_buf(),
            source: source_error,
        }
    })?;
    if !source.starts_with(&source_root) {
        return Err(OvenRustcError::InvalidInput {
            field: "JEC source root",
            message: "does not contain the prepared source entrypoint".to_string(),
        });
    }
    let source_digest = digest_regular_file(&source, "source")?;
    let expected_source_digest = request
        .receipt
        .sources
        .supplemental_digests
        .get(request.source_evidence_key.trim())
        .ok_or_else(|| OvenRustcError::InvalidInput {
            field: "source evidence",
            message: format!("receipt does not declare `{}`", request.source_evidence_key),
        })?;
    if expected_source_digest != &source_digest {
        return Err(OvenRustcError::SourceEvidenceMismatch {
            key: request.source_evidence_key.to_string(),
            expected: expected_source_digest.clone(),
            actual: source_digest,
        });
    }
    let selected_artifacts = request.artifacts.for_source_evidence(artifact_role)?;
    let plan = if let Some(plan) = request.artifact_plan {
        trusted_artifact_plan_for_source(plan, request.artifacts, &selected_artifacts, artifact_role)?
    } else {
        selected_artifacts.materialize_trusted_store(request.artifact_root, &request.receipt.intent)?
    };
    let mut compile_environment = BTreeMap::new();
    for (name, value) in &plan.compile_environment {
        compile_environment.insert(name.clone(), resolve_compile_environment_value(name, value, &source)?);
    }
    let frozen_environment = freeze_direct_rustc_environment(env::vars_os(), &compile_environment);
    if request.features.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(OvenRustcError::InvalidInput {
            field: "JEC features",
            message: "must be sorted and unique".to_string(),
        });
    }
    Ok(OvenPreparedDirectRustcLibrary {
        rustc: fs::canonicalize(request.rustc).map_err(|source_error| OvenRustcError::Io {
            path: request.rustc.to_path_buf(),
            source: source_error,
        })?,
        compiler: compiler.map(|compiler| compiler.evidence.clone()),
        compiler_owner: compiler.cloned(),
        source,
        source_root,
        source_digest,
        artifact_root: canonical_directory(request.artifact_root, "artifact root")?,
        selected_artifacts,
        plan,
        target: request.receipt.intent.target.clone(),
        profile: request.receipt.intent.profile.clone(),
        crate_name: request.crate_name.to_string(),
        edition: request.edition.to_string(),
        features: request.features.to_vec(),
        frozen_environment,
    })
}

impl OvenPreparedDirectRustcLibrary {
    /// Canonical `rustc` this preparation will launch.
    pub fn rustc(&self) -> &Path {
        &self.rustc
    }

    /// Admitted compiler identity, absent when this preparation runs without a retained closure.
    pub fn compiler(&self) -> Option<&OvenDirectRustcCompilerEvidence> {
        self.compiler.as_ref()
    }

    /// Root module this library compiles, already resolved and digest-checked.
    pub fn source(&self) -> &Path {
        &self.source
    }

    /// Admitted source tree the root module sits in; nothing outside it may reach the compiler.
    pub fn source_root(&self) -> &Path {
        &self.source_root
    }

    /// Digest of that source tree as it was admitted, so a later step can prove it did not move underneath.
    pub fn source_digest(&self) -> &str {
        &self.source_digest
    }

    /// Sealed artifact manifest this preparation links against.
    pub fn selected_artifacts(&self) -> &OvenRustcArtifactManifest {
        &self.selected_artifacts
    }

    /// Store-owned root the sealed artifacts were materialized under.
    pub fn artifact_root(&self) -> &Path {
        &self.artifact_root
    }

    /// Verified compiler inputs — search paths, externs and environment — from the sealed plan.
    pub fn artifact_plan(&self) -> &OvenRustcArtifactPlan {
        &self.plan
    }

    /// Target triple from the receipt this preparation was bound to.
    pub fn target(&self) -> &str {
        &self.target
    }

    /// Named build profile, which decides the codegen flags rather than carrying them.
    pub fn profile(&self) -> &str {
        &self.profile
    }

    /// Crate name the compiler is told, which is a declared fact rather than one inferred from a path.
    pub fn crate_name(&self) -> &str {
        &self.crate_name
    }

    /// Rust edition this library is compiled under.
    pub fn edition(&self) -> &str {
        &self.edition
    }

    /// Resolved feature set, already unified; this is not the declared request.
    pub fn features(&self) -> &[String] {
        &self.features
    }

    /// Return the complete frozen environment for digest-only host projection.
    pub fn environment(&self) -> Option<&BTreeMap<String, PathBuf>> {
        self.frozen_environment.as_ref()
    }

    /// Freeze the complete typed projection and the exact physical argv for one output location.
    pub fn bind<'prepared>(
        &'prepared self,
        output: &Path,
        bindings: &OvenDirectRustcJecBindings,
    ) -> Result<OvenBoundDirectRustcLibrary<'prepared>, OvenRustcError> {
        self.verify_current_inputs()?;
        self.validate_bindings(bindings)?;
        let output = caller_output_path(output, &self.artifact_root)?;
        let mut arguments = Vec::new();
        let mut logical_arguments = vec![
            vec!["option".to_string(), "crate-name".to_string(), self.crate_name.clone()],
            vec!["option".to_string(), "crate-type".to_string(), "rlib".to_string()],
            vec!["option".to_string(), "edition".to_string(), self.edition.clone()],
            vec!["option".to_string(), "target".to_string(), self.target.clone()],
        ];
        arguments.extend([
            OsString::from("--crate-name"),
            OsString::from(&self.crate_name),
            OsString::from("--crate-type"),
            OsString::from("rlib"),
            OsString::from(format!("--edition={}", self.edition)),
            OsString::from("--target"),
            OsString::from(&self.target),
        ]);
        for (name, value) in oven_profile_codegen_options(&self.profile) {
            logical_arguments.push(vec!["option".to_string(), name.to_string(), value.to_string()]);
            arguments.extend([OsString::from("-C"), OsString::from(format!("{name}={value}"))]);
        }
        for feature in &self.features {
            let value = format!("feature={feature:?}");
            logical_arguments.push(vec!["option".to_string(), "cfg".to_string(), value.clone()]);
            arguments.extend([OsString::from("--cfg"), OsString::from(value)]);
        }
        logical_arguments.extend([
            vec!["switch".to_string(), "dep-info-stdout".to_string()],
            vec!["switch".to_string(), "json-diagnostics".to_string()],
            vec!["remap-source".to_string(), bindings.logical_source_root.clone()],
        ]);
        arguments.extend([
            OsString::from("--error-format=json"),
            OsString::from("--emit=dep-info=-,link"),
            joined_os_argument(
                "--remap-path-prefix=",
                &self.source_root,
                Some(&bindings.logical_source_root),
            ),
        ]);
        if let Some(compiler) = &self.compiler {
            logical_arguments.push(vec!["remap-toolchain".to_string(), "incan-toolchain".to_string()]);
            arguments.push(joined_os_argument(
                "--remap-path-prefix=",
                &compiler.sysroot,
                Some("incan-toolchain"),
            ));
        }
        for (path, logical) in self
            .plan
            .dependency_search_paths
            .iter()
            .zip(&bindings.dependency_searches)
        {
            logical_arguments.push(vec!["remap-search".to_string(), logical.clone()]);
            arguments.push(joined_os_argument(
                "--remap-path-prefix=",
                path,
                Some(&format!("incan-native/{logical}")),
            ));
            logical_arguments.push(vec!["search".to_string(), logical.clone()]);
            arguments.extend([OsString::from("-L"), joined_os_argument("dependency=", path, None)]);
        }
        for (path, logical) in self.plan.native_search_paths.iter().zip(&bindings.native_searches) {
            logical_arguments.push(vec!["remap-search".to_string(), logical.clone()]);
            arguments.push(joined_os_argument(
                "--remap-path-prefix=",
                path,
                Some(&format!("incan-native/{logical}")),
            ));
            logical_arguments.push(vec!["search".to_string(), logical.clone()]);
            arguments.extend([OsString::from("-L"), joined_os_argument("native=", path, None)]);
        }
        for ((crate_name, path), logical) in self.plan.externs.iter().zip(&bindings.externs) {
            logical_arguments.push(vec!["extern".to_string(), logical.clone()]);
            arguments.extend([
                OsString::from("--extern"),
                joined_os_argument(&format!("{crate_name}="), path, None),
            ]);
        }
        logical_arguments.extend([
            vec!["source".to_string(), bindings.logical_entrypoint.clone()],
            vec!["output".to_string(), "native".to_string()],
        ]);
        arguments.extend([
            self.source.as_os_str().to_os_string(),
            OsString::from("-o"),
            output.as_os_str().to_os_string(),
        ]);
        let path_effects_digest = self
            .compiler
            .as_ref()
            .map(|compiler| {
                serde_json::to_vec(&(
                    "incan.oven.checked-logical-path-effects/1",
                    compiler.closure_digest(),
                    &logical_arguments,
                ))
                .map(|material| digest_bytes(&material))
            })
            .transpose()
            .map_err(|error| OvenRustcError::InvalidInput {
                field: "JEC logical invocation",
                message: format!("cannot encode path effects: {error}"),
            })?;
        Ok(OvenBoundDirectRustcLibrary {
            prepared: self,
            output,
            arguments,
            logical_arguments,
            path_effects_digest,
        })
    }

    /// Re-check the compiler and sources against what was admitted, immediately before launching.
    ///
    /// Preparation validated them once; this runs again at use because nothing holds the filesystem still in
    /// between. The check is skipped when a store-retained closure owns the compiler, because its lease already
    /// does hold it, and re-digesting a multi-hundred-megabyte closure per invocation is the cost that lease
    /// exists to avoid.
    fn verify_current_inputs(&self) -> Result<(), OvenRustcError> {
        if self.compiler_owner.is_none() {
            let compiler = digest_regular_file(&self.rustc, "rustc")?;
            if let Some(expected) = &self.compiler
                && compiler != expected.binary_digest
            {
                return Err(OvenRustcError::ArtifactDigestMismatch {
                    path: self.rustc.clone(),
                    expected: expected.binary_digest.clone(),
                    actual: compiler,
                });
            }
        } else if self
            .compiler_owner
            .as_ref()
            .is_none_or(|owner| owner.rustc != self.rustc || Some(owner.evidence()) != self.compiler.as_ref())
        {
            return Err(OvenRustcError::InvalidInput {
                field: "JEC compiler owner",
                message: "is detached from the prepared compiler evidence".to_string(),
            });
        }
        let source = digest_regular_file(&self.source, "source")?;
        if source != self.source_digest {
            return Err(OvenRustcError::SourceEvidenceMismatch {
                key: "prepared JEC source".to_string(),
                expected: self.source_digest.clone(),
                actual: source,
            });
        }
        Ok(())
    }

    /// Refuse logical bindings that could not describe this compilation on another machine.
    ///
    /// The bindings are what make a recorded invocation comparable across hosts, so an empty or non-logical one is
    /// rejected here rather than producing a record that looks portable and is not.
    fn validate_bindings(&self, bindings: &OvenDirectRustcJecBindings) -> Result<(), OvenRustcError> {
        let invalid = |message: &str| OvenRustcError::InvalidInput {
            field: "JEC logical bindings",
            message: message.to_string(),
        };
        if bindings.logical_source_root.trim().is_empty()
            || bindings.logical_entrypoint.trim().is_empty()
            || bindings.externs.len() != self.plan.externs.len()
            || bindings.dependency_searches.len() != self.plan.dependency_search_paths.len()
            || bindings.native_searches.len() != self.plan.native_search_paths.len()
        {
            return Err(invalid("do not cover the prepared invocation"));
        }
        if bindings
            .externs
            .iter()
            .chain(&bindings.dependency_searches)
            .chain(&bindings.native_searches)
            .any(String::is_empty)
        {
            return Err(invalid("substitute or omit a prepared physical member"));
        }
        Ok(())
    }
}

impl OvenBoundDirectRustcLibrary<'_> {
    /// Path the compilation wrote, owned by the caller that asked for it.
    pub fn output(&self) -> &Path {
        &self.output
    }

    /// Compiler invocations as location-independent argument vectors, for comparison across machines.
    pub fn logical_arguments(&self) -> &[Vec<String>] {
        &self.logical_arguments
    }

    /// Digest of the observed path effects, absent when the compilation ran without observation.
    pub fn path_effects_digest(&self) -> Option<&str> {
        self.path_effects_digest.as_deref()
    }

    /// Rehash one retained output after the current invocation and Incan key have both been admitted.
    pub fn reuse_from(
        &self,
        retained_output: &Path,
        expected_digest: &str,
    ) -> Result<Option<OvenDirectRustcBake>, OvenRustcError> {
        let metadata = match fs::symlink_metadata(retained_output) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(OvenRustcError::Io {
                    path: retained_output.to_path_buf(),
                    source: error,
                });
            }
            Ok(metadata) => metadata,
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Ok(None);
        }
        let output_digest = digest_regular_file(retained_output, "retained output")?;
        if output_digest != expected_digest {
            return Ok(None);
        }
        let parent = self.output.parent().ok_or_else(|| OvenRustcError::InvalidInput {
            field: "output",
            message: "must have a parent directory".to_string(),
        })?;
        fs::create_dir_all(parent).map_err(|source| OvenRustcError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|source| OvenRustcError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
        let mut retained = fs::File::open(retained_output).map_err(|source| OvenRustcError::Io {
            path: retained_output.to_path_buf(),
            source,
        })?;
        io::copy(&mut retained, &mut temporary).map_err(|source| OvenRustcError::Io {
            path: retained_output.to_path_buf(),
            source,
        })?;
        temporary.as_file().sync_all().map_err(|source| OvenRustcError::Io {
            path: temporary.path().to_path_buf(),
            source,
        })?;
        if digest_regular_file(temporary.path(), "copied retained output")? != expected_digest {
            return Ok(None);
        }
        temporary.persist(&self.output).map_err(|error| OvenRustcError::Io {
            path: self.output.clone(),
            source: error.error,
        })?;
        Ok(Some(OvenDirectRustcBake {
            source_digest: self.prepared.source_digest.clone(),
            output: self.output.clone(),
            output_digest,
            cargo_process_started: false,
            reused: true,
            lease: None,
        }))
    }

    /// Execute the exact frozen argv under its captured environment and return only sanitized dep-info observations.
    pub fn compile(&self) -> Result<OvenDirectRustcJecCompilation, OvenRustcError> {
        self.compile_with_observation(true)
    }

    /// Execute an explicitly uncacheable invocation under the same compiler environment as a cacheable one.
    ///
    /// Cache availability cannot select program semantics. This path suppresses reusable observations, but it still
    /// clears ambient state and applies only the admitted plan values used by [`Self::compile`].
    pub fn compile_uncacheable(&self) -> Result<OvenDirectRustcJecCompilation, OvenRustcError> {
        self.compile_with_observation(false)
    }

    /// Run the prepared invocation, optionally recording the paths it touched.
    ///
    /// `observe` is a parameter rather than two functions because the compile is identical either way; only
    /// whether the effects are captured differs, and forking the launch would let the two drift.
    fn compile_with_observation(&self, observe: bool) -> Result<OvenDirectRustcJecCompilation, OvenRustcError> {
        self.prepared.verify_current_inputs()?;
        let mut command = Command::new(&self.prepared.rustc);
        apply_direct_rustc_execution_environment(&mut command, self.prepared.frozen_environment.as_ref())?;
        command.current_dir(&self.prepared.source_root).args(&self.arguments);
        let result = command.output().map_err(|source| OvenRustcError::Io {
            path: self.prepared.rustc.clone(),
            source,
        })?;
        if !result.status.success() {
            return Err(OvenRustcError::CompilationFailed {
                report: parse_rustc_diagnostics(&[], &result.stderr).with_invocation(&command),
            });
        }
        let observation = if observe {
            let frozen = self
                .prepared
                .frozen_environment
                .as_ref()
                .ok_or_else(|| OvenRustcError::InvalidInput {
                    field: "JEC environment",
                    message: "is unavailable for reusable compilation".to_string(),
                })?;
            match parse_rustc_dep_info(&result.stdout, frozen) {
                Ok(observation) => OvenRustcDepInfoOutcome::Observed(observation),
                Err(error) => OvenRustcDepInfoOutcome::Unavailable {
                    reason: error.to_string(),
                },
            }
        } else {
            OvenRustcDepInfoOutcome::Unavailable {
                reason: "reuse observation was deliberately suppressed".to_string(),
            }
        };
        verified_regular_file(&self.output, "output")?;
        let output_digest = digest_regular_file(&self.output, "output")?;
        Ok(OvenDirectRustcJecCompilation {
            bake: OvenDirectRustcBake {
                source_digest: self.prepared.source_digest.clone(),
                output: self.output.clone(),
                output_digest,
                cargo_process_started: false,
                reused: false,
                lease: None,
            },
            observation,
        })
    }
}

/// Apply the one deterministic direct-Rustc environment used with or without cache acceleration.
pub(crate) fn apply_direct_rustc_execution_environment(
    command: &mut Command,
    frozen_environment: Option<&BTreeMap<String, PathBuf>>,
) -> Result<(), OvenRustcError> {
    let frozen_environment = frozen_environment.ok_or_else(|| OvenRustcError::InvalidInput {
        field: "JEC environment",
        message: "is unavailable for direct compilation".to_string(),
    })?;
    command.env_clear().envs(frozen_environment);
    Ok(())
}

/// Construct one path-bearing argument without lossy UTF-8 conversion.
pub(crate) fn joined_os_argument(prefix: &str, path: &Path, suffix: Option<&str>) -> OsString {
    let mut value = OsString::from(prefix);
    value.push(path.as_os_str());
    if let Some(suffix) = suffix {
        value.push("=");
        value.push(suffix);
    }
    value
}

/// Parse Rustc's Makefile dep-info and discard every raw environment value before returning.
pub(crate) fn parse_rustc_dep_info(
    bytes: &[u8],
    environment: &BTreeMap<String, PathBuf>,
) -> Result<OvenRustcDepInfoObservation, OvenRustcError> {
    let text = std::str::from_utf8(bytes).map_err(|error| OvenRustcError::InvalidInput {
        field: "JEC dep-info",
        message: format!("is not UTF-8: {error}"),
    })?;
    let unfolded = text.replace("\\\r\n", "").replace("\\\n", "");
    let dependencies = unfolded
        .lines()
        .filter_map(|line| {
            let line = line.trim_end_matches('\r');
            (!line.trim().is_empty() && !line.starts_with('#')).then_some(line)
        })
        .find_map(|line| line.find(": ").map(|separator| &line[separator + 2..]))
        .ok_or_else(|| OvenRustcError::InvalidInput {
            field: "JEC dep-info",
            message: "has no dependency rule".to_string(),
        })?;
    let files = parse_makefile_words(dependencies)?;
    if files.is_empty() {
        return Err(OvenRustcError::InvalidInput {
            field: "JEC dep-info",
            message: "has an empty dependency rule".to_string(),
        });
    }
    let mut files = files.into_iter().map(PathBuf::from).collect::<Vec<_>>();
    files.sort();
    files.dedup();

    let mut observed = BTreeMap::new();
    for line in unfolded.lines() {
        let Some(value) = line.trim_end_matches('\r').strip_prefix("# env-dep:") else {
            continue;
        };
        let (name, supplied) = value
            .split_once('=')
            .map_or((value, None), |(name, value)| (name, Some(value)));
        if name.is_empty() || observed.contains_key(name) {
            return Err(OvenRustcError::InvalidInput {
                field: "JEC dep-info",
                message: "has an empty or repeated environment name".to_string(),
            });
        }
        let current = environment.get(name);
        let matches = match (supplied, current) {
            (None, None) => true,
            (Some(supplied), Some(current)) => current.to_str() == Some(supplied),
            _ => false,
        };
        if !matches {
            return Err(OvenRustcError::InvalidInput {
                field: "JEC dep-info",
                message: format!("does not match the frozen value presence for `{name}`"),
            });
        }
        observed.insert(
            name.to_string(),
            current.map(|value| digest_bytes(value.as_os_str().as_encoded_bytes())),
        );
    }
    Ok(OvenRustcDepInfoObservation {
        files,
        environment: observed
            .into_iter()
            .map(|(name, value_digest)| OvenRustcObservedEnvironment { name, value_digest })
            .collect(),
    })
}

/// Decode the subset of Makefile word escaping emitted by Rustc dep-info.
pub(crate) fn parse_makefile_words(value: &str) -> Result<Vec<String>, OvenRustcError> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\\' => {
                let escaped = characters.next().ok_or_else(|| OvenRustcError::InvalidInput {
                    field: "JEC dep-info",
                    message: "ends with an incomplete path escape".to_string(),
                })?;
                word.push(escaped);
            }
            '$' if characters.peek() == Some(&'$') => {
                characters.next();
                word.push('$');
            }
            character if character.is_whitespace() => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            character => word.push(character),
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    Ok(words)
}

/// Build the complete reusable compiler environment from admitted plan values only.
///
/// Ambient values are intentionally ignored. Rustc controls and dynamic-loader paths can change compiler behavior
/// without appearing as source `env!` dependencies, so inherited process state cannot participate in a reusable
/// compilation unless a later policy models it explicitly.
pub(crate) fn freeze_direct_rustc_environment(
    _inherited: impl IntoIterator<Item = (OsString, OsString)>,
    compile_environment: &BTreeMap<String, PathBuf>,
) -> Option<BTreeMap<String, PathBuf>> {
    Some(compile_environment.clone())
}

// These fixtures execute a publisher, which only a host with the confinement primitive can do.
#[cfg(all(test, target_os = "macos"))]
mod publisher_link_tests {
    use std::error::Error;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    use oven_model::manifest::{
        RustFactArgument, RustFactArtifact, RustFactArtifactKind, RustFactArtifactMember, RustFactExecutable,
        RustFactLibrary, RustFactLibraryKind, RustFactLink, RustFactLinkLanguage, RustFactLinkObject,
    };
    use oven_store::digest_bytes;
    use oven_store::process::BoundedProcessLimits;
    use tempfile::tempdir;

    use super::{OvenPublisherLinkBakeRequest, bake_publisher_link};

    /// Return only object members from `ar -t`; a symbol index is archive metadata rather than an object.
    fn archive_object_members(listing: &[u8]) -> Result<Vec<&str>, std::str::Utf8Error> {
        Ok(std::str::from_utf8(listing)?
            .lines()
            .filter(|name| !name.starts_with("__.SYMDEF") && *name != "/" && *name != "//")
            .collect())
    }

    /// Write a fake archive-producing compiler and its declared source record.
    ///
    /// The compiler copies with shell builtins only, because publisher confinement admits no undeclared executable
    /// such as `/bin/cp`.
    fn fixture_link(root: &std::path::Path) -> Result<RustFactLink, Box<dyn Error>> {
        let compiler = root.join("tool/bin/fake-cc");
        fs::create_dir_all(compiler.parent().ok_or("compiler has no parent")?)?;
        fs::write(
            &compiler,
            "#!/bin/sh\nset -eu\nIFS= read -r content < \"$1\" || true\nprintf '%s' \"$content\" > \"$2\"\n",
        )?;
        let mut permissions = fs::metadata(&compiler)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&compiler, permissions)?;
        fs::create_dir_all(root.join("source/native"))?;
        fs::write(root.join("source/native/fixture.c"), b"native-source")?;
        Ok(RustFactLink {
            name: "fixture-native".to_string(),
            target: "aarch64-apple-darwin".to_string(),
            executable: RustFactExecutable {
                name: "fake-cc".to_string(),
                owner: oven_store::publisher_owner::publisher_owner_identity(&root.join("tool"), ["bin/fake-cc"])?,
                path: "bin/fake-cc".to_string(),
                digest: digest_bytes(&fs::read(compiler)?),
            },
            objects: vec![RustFactLinkObject {
                name: "fixture.o".to_string(),
                language: RustFactLinkLanguage::C,
                arguments: vec![
                    RustFactArgument::Input {
                        input: "fixture-source".to_string(),
                    },
                    RustFactArgument::Output {
                        output: "fixture.o".to_string(),
                    },
                ],
            }],
            environment: Vec::new(),
            sources: vec![RustFactArtifact {
                name: "fixture-source".to_string(),
                kind: RustFactArtifactKind::File,
                path: "native/fixture.c".to_string(),
                digest: digest_bytes(b"native-source"),
                members: Vec::new(),
            }],
            library: RustFactLibrary {
                name: "fixture".to_string(),
                kind: RustFactLibraryKind::Static,
            },
        })
    }

    #[test]
    /// Publisher execution creates the deterministic archive and preserves selected logical argv.
    fn selected_unit_link_argv_equals_publisher_receipt() -> Result<(), Box<dyn Error>> {
        let root = tempdir()?;
        let link = fixture_link(root.path())?;
        let output = root.path().join("product");
        let product = match bake_publisher_link(&OvenPublisherLinkBakeRequest {
            link: &link,
            selected_target: "aarch64-apple-darwin",
            archive_format: "darwin",
            toolchain: "rustc fixture",
            consuming_unit_identity: &digest_bytes(b"consumer"),
            executable_owner_root: &root.path().join("tool"),
            source_owner_root: &root.path().join("source"),
            output_root: &output,
            limits: BoundedProcessLimits {
                stdout_bytes: 1024,
                stderr_bytes: 1024,
                timeout: Some(Duration::from_secs(2)),
            },
        }) {
            Ok(product) => product,
            Err(error) if error.to_string().contains("sandbox_apply: Operation not permitted") => return Ok(()),
            Err(error) => return Err(error.into()),
        };

        assert_eq!(product.archive_relative_path, "libfixture.a");
        let selected_argv = link.objects[0]
            .arguments
            .iter()
            .map(|argument| match argument {
                RustFactArgument::Literal { literal } => format!("literal:{literal}"),
                RustFactArgument::Input { input } => format!("input:{input}"),
                RustFactArgument::Output { output } => format!("output:{output}"),
                RustFactArgument::Owner { owner } => format!("owner:{owner}"),
            })
            .collect::<Vec<_>>();
        assert_eq!(product.receipt.objects[0].logical_argv, selected_argv);
        Ok(())
    }

    #[test]
    /// A root tree input resolves to the source owner itself while confinement still excludes its siblings.
    fn publisher_link_root_tree_input_stays_confined() -> Result<(), Box<dyn Error>> {
        let root = tempdir()?;
        let mut link = fixture_link(root.path())?;
        let source_root = root.path().join("source");
        fs::write(source_root.join("config.h"), b"root-config")?;
        fs::write(root.path().join("secret"), b"secret")?;
        let compiler = root.path().join("tool/bin/fake-cc");
        fs::write(
            &compiler,
            "#!/bin/sh\nset -eu\nsource_root=\"$1\"\nparent=${source_root%/*}\nif IFS= read -r secret < \"$parent/secret\"; then exit 41; fi\nIFS= read -r content < \"$source_root/config.h\" || true\nprintf '%s' \"$content\" > \"$2\"\n",
        )?;
        let mut permissions = fs::metadata(&compiler)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&compiler, permissions)?;
        link.executable.digest = digest_bytes(&fs::read(&compiler)?);
        link.executable.owner =
            oven_store::publisher_owner::publisher_owner_identity(&root.path().join("tool"), ["bin/fake-cc"])?;
        let members = vec![
            RustFactArtifactMember {
                path: "config.h".to_string(),
                digest: digest_bytes(b"root-config"),
            },
            RustFactArtifactMember {
                path: "native/fixture.c".to_string(),
                digest: digest_bytes(b"native-source"),
            },
        ];
        link.sources = vec![RustFactArtifact {
            name: "crate-root".to_string(),
            kind: RustFactArtifactKind::Tree,
            path: ".".to_string(),
            digest: digest_bytes(&serde_json::to_vec(&(
                "incan.oven.publisher-artifact-tree/1",
                &members,
            ))?),
            members,
        }];
        link.objects[0].arguments[0] = RustFactArgument::Input {
            input: "crate-root".to_string(),
        };
        let output = root.path().join("product");

        match bake_publisher_link(&OvenPublisherLinkBakeRequest {
            link: &link,
            selected_target: "aarch64-apple-darwin",
            archive_format: "darwin",
            toolchain: "rustc fixture",
            consuming_unit_identity: &digest_bytes(b"consumer"),
            executable_owner_root: &root.path().join("tool"),
            source_owner_root: &source_root,
            output_root: &output,
            limits: BoundedProcessLimits {
                stdout_bytes: 1024,
                stderr_bytes: 1024,
                timeout: Some(Duration::from_secs(2)),
            },
        }) {
            Ok(_) => Ok(()),
            Err(error) if error.to_string().contains("sandbox_apply: Operation not permitted") => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    #[test]
    /// Two explicit object compilations produce one byte-reproducible ordered archive and receipt.
    fn publisher_link_bake_archives_objects_in_name_order_reproducibly() -> Result<(), Box<dyn Error>> {
        let root = tempdir()?;
        let mut link = fixture_link(root.path())?;
        fs::write(root.path().join("source/native/another.c"), b"another-source")?;
        link.sources.push(RustFactArtifact {
            name: "another-source".to_string(),
            kind: RustFactArtifactKind::File,
            path: "native/another.c".to_string(),
            digest: digest_bytes(b"another-source"),
            members: Vec::new(),
        });
        link.sources.sort_by(|left, right| left.name.cmp(&right.name));
        link.objects = vec![
            RustFactLinkObject {
                name: "another.o".to_string(),
                language: RustFactLinkLanguage::C,
                arguments: vec![
                    RustFactArgument::Input {
                        input: "another-source".to_string(),
                    },
                    RustFactArgument::Output {
                        output: "another.o".to_string(),
                    },
                ],
            },
            link.objects[0].clone(),
        ];
        let bake = |output_root: &std::path::Path| {
            bake_publisher_link(&OvenPublisherLinkBakeRequest {
                link: &link,
                selected_target: "aarch64-apple-darwin",
                archive_format: "darwin",
                toolchain: "rustc fixture",
                consuming_unit_identity: &digest_bytes(b"consumer"),
                executable_owner_root: &root.path().join("tool"),
                source_owner_root: &root.path().join("source"),
                output_root,
                limits: BoundedProcessLimits {
                    stdout_bytes: 1024,
                    stderr_bytes: 1024,
                    timeout: Some(Duration::from_secs(2)),
                },
            })
        };
        let first = match bake(&root.path().join("first")) {
            Ok(product) => product,
            Err(error) if error.to_string().contains("sandbox_apply: Operation not permitted") => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        let second = bake(&root.path().join("second"))?;
        assert_eq!(fs::read(&first.archive_path)?, fs::read(&second.archive_path)?);
        assert_eq!(first.receipt, second.receipt);
        assert_eq!(
            first
                .receipt
                .objects
                .iter()
                .map(|object| object.name.as_str())
                .collect::<Vec<_>>(),
            ["another.o", "fixture.o"]
        );
        let listing = std::process::Command::new("/usr/bin/ar")
            .arg("-t")
            .arg(&first.archive_path)
            .output()?;
        assert!(listing.status.success());
        assert_eq!(archive_object_members(&listing.stdout)?, ["another.o", "fixture.o"]);
        Ok(())
    }

    #[test]
    /// Target substitution and pre-existing archive content fail before publication.
    fn publisher_link_bake_refuses_target_mismatch_and_archive_collision() -> Result<(), Box<dyn Error>> {
        let root = tempdir()?;
        let link = fixture_link(root.path())?;
        let product_root = root.path().join("product");
        fs::create_dir_all(&product_root)?;
        fs::write(product_root.join("libfixture.a"), b"collision")?;
        let request = OvenPublisherLinkBakeRequest {
            link: &link,
            selected_target: "x86_64-unknown-linux-gnu",
            archive_format: "gnu",
            toolchain: "rustc fixture",
            consuming_unit_identity: &digest_bytes(b"consumer"),
            executable_owner_root: &root.path().join("tool"),
            source_owner_root: &root.path().join("source"),
            output_root: &product_root,
            limits: BoundedProcessLimits {
                stdout_bytes: 1024,
                stderr_bytes: 1024,
                timeout: Some(Duration::from_secs(2)),
            },
        };
        assert!(bake_publisher_link(&request).is_err());

        let matching = OvenPublisherLinkBakeRequest {
            selected_target: "aarch64-apple-darwin",
            ..request
        };
        let error = bake_publisher_link(&matching)
            .err()
            .ok_or("archive collision was accepted")?;
        assert!(error.to_string().contains("collides"));
        Ok(())
    }

    #[test]
    /// A missing declared object and an undeclared extra object both refuse publication.
    fn publisher_link_bake_refuses_missing_and_extra_objects() -> Result<(), Box<dyn Error>> {
        let root = tempdir()?;
        let mut link = fixture_link(root.path())?;
        let compiler = root.path().join("tool/bin/fake-cc");
        fs::write(&compiler, "#!/bin/sh\nset -eu\n:\n")?;
        link.executable.digest = digest_bytes(&fs::read(&compiler)?);
        link.executable.owner =
            oven_store::publisher_owner::publisher_owner_identity(&root.path().join("tool"), ["bin/fake-cc"])?;
        let bake = |link: &RustFactLink, output_root: &std::path::Path| {
            bake_publisher_link(&OvenPublisherLinkBakeRequest {
                link,
                selected_target: "aarch64-apple-darwin",
                archive_format: "darwin",
                toolchain: "rustc fixture",
                consuming_unit_identity: &digest_bytes(b"consumer"),
                executable_owner_root: &root.path().join("tool"),
                source_owner_root: &root.path().join("source"),
                output_root,
                limits: BoundedProcessLimits {
                    stdout_bytes: 1024,
                    stderr_bytes: 1024,
                    timeout: Some(Duration::from_secs(2)),
                },
            })
        };
        let error = bake(&link, &root.path().join("missing"))
            .err()
            .ok_or("missing object was accepted")?;
        if error.to_string().contains("sandbox_apply: Operation not permitted") {
            return Ok(());
        }
        assert!(error.to_string().contains("missing"));

        fs::write(
            &compiler,
            "#!/bin/sh\nset -eu\nIFS= read -r content < \"$1\" || true\nprintf '%s' \"$content\" > \"$2\"\nprintf '%s' extra > extra.o\n",
        )?;
        link.executable.digest = digest_bytes(&fs::read(&compiler)?);
        link.executable.owner =
            oven_store::publisher_owner::publisher_owner_identity(&root.path().join("tool"), ["bin/fake-cc"])?;
        let error = bake(&link, &root.path().join("extra"))
            .err()
            .ok_or("extra object was accepted")?;
        assert!(error.to_string().contains("undeclared"));
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[test]
    /// Command Line Tools clang produces the same two-member archive bytes across independent bakes.
    fn publisher_link_bake_with_real_clang_is_reproducible() -> Result<(), Box<dyn Error>> {
        let owner = std::path::Path::new("/Library/Developer/CommandLineTools");
        let clang = owner.join("usr/bin/clang");
        let sdk_link = owner.join("SDKs/MacOSX.sdk");
        if !clang.is_file() || !sdk_link.exists() {
            return Ok(());
        }
        let sdk = fs::canonicalize(&sdk_link)?;
        let sdk_relative = sdk
            .strip_prefix(owner)?
            .to_str()
            .ok_or("versioned SDK path is not UTF-8")?
            .to_string();
        let resource_output = std::process::Command::new(&clang).arg("-print-resource-dir").output()?;
        if !resource_output.status.success() {
            return Err("clang did not report its resource directory".into());
        }
        let resource = String::from_utf8(resource_output.stdout)?.trim().to_string();
        let resource_relative = std::path::Path::new(&resource)
            .strip_prefix(owner)?
            .to_str()
            .ok_or("clang resource directory path is not UTF-8")?
            .to_string();
        let root = tempdir()?;
        let sources = root.path().join("source");
        fs::create_dir_all(&sources)?;
        fs::write(sources.join("first.c"), b"int first(void) { return 1; }\n")?;
        fs::write(sources.join("second.c"), b"int second(void) { return 2; }\n")?;
        let target = match std::env::consts::ARCH {
            "aarch64" => "aarch64-apple-darwin",
            "x86_64" => "x86_64-apple-darwin",
            architecture => return Err(format!("unsupported macOS test architecture `{architecture}`").into()),
        };
        let object = |name: &str, input: &str| RustFactLinkObject {
            name: name.to_string(),
            language: RustFactLinkLanguage::C,
            arguments: vec![
                RustFactArgument::Literal {
                    literal: "-isysroot".to_string(),
                },
                RustFactArgument::Owner {
                    owner: sdk_relative.clone(),
                },
                RustFactArgument::Literal {
                    literal: "-resource-dir".to_string(),
                },
                RustFactArgument::Owner {
                    owner: resource_relative.clone(),
                },
                RustFactArgument::Literal {
                    literal: "-c".to_string(),
                },
                RustFactArgument::Input {
                    input: input.to_string(),
                },
                RustFactArgument::Literal {
                    literal: "-o".to_string(),
                },
                RustFactArgument::Output {
                    output: name.to_string(),
                },
            ],
        };
        let link = RustFactLink {
            name: "pair".to_string(),
            target: target.to_string(),
            executable: RustFactExecutable {
                name: "clang".to_string(),
                owner: oven_store::publisher_owner::publisher_owner_identity(
                    owner,
                    ["usr/bin/clang", sdk_relative.as_str(), resource_relative.as_str()],
                )?,
                path: "usr/bin/clang".to_string(),
                digest: digest_bytes(&fs::read(&clang)?),
            },
            objects: vec![object("first.o", "first"), object("second.o", "second")],
            environment: Vec::new(),
            sources: vec![
                RustFactArtifact {
                    name: "first".to_string(),
                    kind: RustFactArtifactKind::File,
                    path: "first.c".to_string(),
                    digest: digest_bytes(b"int first(void) { return 1; }\n"),
                    members: Vec::new(),
                },
                RustFactArtifact {
                    name: "second".to_string(),
                    kind: RustFactArtifactKind::File,
                    path: "second.c".to_string(),
                    digest: digest_bytes(b"int second(void) { return 2; }\n"),
                    members: Vec::new(),
                },
            ],
            library: RustFactLibrary {
                name: "pair".to_string(),
                kind: RustFactLibraryKind::Static,
            },
        };
        let bake = |output_root: &std::path::Path| {
            bake_publisher_link(&OvenPublisherLinkBakeRequest {
                link: &link,
                selected_target: target,
                archive_format: "darwin",
                toolchain: "Command Line Tools clang",
                consuming_unit_identity: &digest_bytes(b"consumer"),
                executable_owner_root: owner,
                source_owner_root: &sources,
                output_root,
                limits: BoundedProcessLimits {
                    stdout_bytes: 1024 * 1024,
                    stderr_bytes: 1024 * 1024,
                    timeout: Some(Duration::from_secs(30)),
                },
            })
        };
        let first = match bake(&root.path().join("first-bake")) {
            Ok(product) => product,
            Err(error) if error.to_string().contains("sandbox_apply: Operation not permitted") => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        let second = bake(&root.path().join("second-bake"))?;
        assert_eq!(fs::read(&first.archive_path)?, fs::read(&second.archive_path)?);
        let listing = std::process::Command::new("/usr/bin/ar")
            .arg("-t")
            .arg(&first.archive_path)
            .output()?;
        assert!(listing.status.success());
        assert_eq!(archive_object_members(&listing.stdout)?, ["first.o", "second.o"]);
        Ok(())
    }
}
