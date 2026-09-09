//! Publisher-derived Oven module descriptors and borrowed artifact admission.
//!
//! This boundary records an explicit trusted module contract against a completed project output. It does not select an
//! arbitrary module or issue host permission. The fixed toolchain installer retains original Engine/output owners;
//! ordinary admission borrows them, and the caller separately enforces ABI, integrity and capability checks.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{
    BackendExecutionReceipt, OVEN_PROJECT_OUTPUT_ARTIFACT_PATH, OvenBakeProjectTarget, OvenProjectOutputPayload,
    OvenStoredProjectOutput, project_relative_entrypoint, validated_project_output_native_path,
    validated_project_output_relative_path,
};
use crate::cli::{CliError, CliResult};
use crate::generated_source::digest_file;
use crate::oven::OvenReceipt;
use crate::oven::store::{
    OvenArtifactKind, OvenArtifactManifest, OvenArtifactPublishRequest, OvenStore, OvenStoreExecutionPayload,
};
use crate::provider::FeatureSelection;

const DESCRIPTOR_VERSION: u32 = 1;
const DESCRIPTOR_KIND: &str = "incan.oven.engine";
const DESCRIPTOR_LIMIT: usize = 1024 * 1024;
const EXCHANGE_SCHEMAS: [&str; 2] = ["incan.oven.selection/1", "incan.oven.source-unit-batch/1"];

/// Content identity of the actual Incan executable that performed source checking and emission.
///
/// This is neither a compiler checkout identity nor a requirement that the hosting compiler have identical bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CompilerBinaryIdentity {
    digest: String,
}

impl CompilerBinaryIdentity {
    /// Return the publisher-observed SHA-256 identity of the compiling executable.
    #[must_use]
    #[allow(dead_code, reason = "Pending #991: compiler identity reporting is not connected")]
    pub(crate) fn digest(&self) -> &str {
        &self.digest
    }
}

/// Compiler-owned module role explicitly declared by the trusted publisher.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EngineModuleContract {
    /// The reviewed selection and source-unit batch adapter using bounded native JSON file exchange.
    OvenSourceUnitBatchV1,
    /// Explicit version 2 selection evidence and source-unit batch exchange, retaining the original version 1
    /// protocols.
    OvenSourceUnitBatchV2,
    /// Explicit compiler support requests and original runtime grants in source-unit batch version 3.
    OvenSourceUnitBatchV3,
}

impl EngineModuleContract {
    /// Bind an explicit publisher declaration to its descriptor version and exact supported protocols.
    fn wire_contract(self) -> (u32, Vec<String>) {
        let mut schemas: Vec<String> = EXCHANGE_SCHEMAS.into_iter().map(str::to_string).collect();
        let version = match self {
            Self::OvenSourceUnitBatchV1 => DESCRIPTOR_VERSION,
            Self::OvenSourceUnitBatchV2 | Self::OvenSourceUnitBatchV3 => {
                schemas.extend([
                    "incan.oven.selection/2".to_string(),
                    "incan.oven.source-unit-batch/2".to_string(),
                ]);
                if self == Self::OvenSourceUnitBatchV3 {
                    schemas.push("incan.oven.source-unit-batch/3".to_string());
                    3
                } else {
                    2
                }
            }
        };
        (version, schemas)
    }
}

/// Explicit trusted bootstrap request; source paths and script names never infer a module role.
///
/// Constructing a request is not permission to execute the resulting module. No public CLI or environment selector uses
/// this request; it is an insertion point for the compiler-owned publisher.
pub(crate) struct EnginePublisherRequest {
    contract: EngineModuleContract,
    entrypoint_relative_path: String,
}

impl EnginePublisherRequest {
    /// Declare a module role for one project-relative executable entrypoint.
    #[allow(dead_code, reason = "Pending #991: trusted bootstrap request is not connected")]
    pub(crate) fn new(contract: EngineModuleContract, entrypoint_relative_path: &str) -> CliResult<Self> {
        if entrypoint_relative_path.trim().is_empty() {
            return Err(CliError::failure(
                "Engine request requires a nonempty relative entrypoint",
            ));
        }
        let _ = validated_project_output_relative_path(entrypoint_relative_path, "Engine entrypoint")?;
        Ok(Self {
            contract,
            entrypoint_relative_path: entrypoint_relative_path.to_string(),
        })
    }
}

/// Publish descriptors through the ordinary explicit project baker for a trusted Engine request.
///
/// Fresh compilation observes this process's executable before checking/emission and verifies it again before
/// publication. Completed-output reuse keeps the original observation and refuses a legacy output that lacks it.
/// Returned Engine owners retain their leases; later admission must separately retain each referenced ProjectOutput.
pub(crate) fn publish_engine_project(
    project: &Path,
    request: EnginePublisherRequest,
) -> CliResult<Vec<OvenStoreExecutionPayload>> {
    let mut publisher = EnginePublisher {
        request,
        compiler: None,
        published: Vec::new(),
    };
    super::bake_oven_project_targets_with_engine(project, &FeatureSelection::default(), Some(&mut publisher))?;
    if publisher.published.is_empty() {
        return Err(CliError::failure(
            "Engine publication produced no completed executable descriptors",
        ));
    }
    Ok(publisher.published)
}

/// Command-local compiler observation. Physical paths never enter a reusable receipt or descriptor identity.
struct CompilingBinary {
    path: PathBuf,
    identity: CompilerBinaryIdentity,
}

impl CompilingBinary {
    /// Observe only this trusted publisher process, without an executable override or checkout inference.
    fn observe_current() -> CliResult<Self> {
        let path = std::env::current_exe()
            .and_then(|path| path.canonicalize())
            .map_err(|error| CliError::failure(format!("cannot observe the compiling Incan executable: {error}")))?;
        let digest = digest_file(&path).map_err(|error| CliError::failure(error.to_string()))?;
        Ok(Self {
            path,
            identity: CompilerBinaryIdentity { digest },
        })
    }

    /// Refuse publication if the observed compiler file changed during source checking or native preparation.
    fn verify_unchanged(&self) -> CliResult<()> {
        let digest = digest_file(&self.path).map_err(|error| CliError::failure(error.to_string()))?;
        if digest != self.identity.digest {
            return Err(CliError::failure(
                "compiling Incan executable changed before Engine publication",
            ));
        }
        Ok(())
    }
}

/// Mutable publisher state held only by the explicit trusted bootstrap operation.
pub(super) struct EnginePublisher {
    request: EnginePublisherRequest,
    compiler: Option<CompilingBinary>,
    published: Vec<OvenStoreExecutionPayload>,
}

impl EnginePublisher {
    /// Require an exact declared executable target before the explicit baker can have effects.
    pub(super) fn validate_targets(&self, root: &Path, targets: &[(OvenBakeProjectTarget, PathBuf)]) -> CliResult<()> {
        let count = targets
            .iter()
            .filter(|(target, entrypoint)| {
                *target == OvenBakeProjectTarget::Executable
                    && project_relative_entrypoint(root, entrypoint).as_deref()
                        == Some(self.request.entrypoint_relative_path.as_str())
            })
            .count();
        if count != 1 {
            return Err(CliError::failure(
                "Engine request must name exactly one discovered executable entrypoint",
            ));
        }
        Ok(())
    }

    /// Keep only the explicitly declared Engine entrypoint after normal target discovery and validation.
    pub(super) fn retain_requested_target(
        &self,
        root: &Path,
        targets: &mut Vec<(OvenBakeProjectTarget, PathBuf)>,
    ) -> CliResult<()> {
        self.validate_targets(root, targets)?;
        targets.retain(|(kind, entrypoint)| {
            *kind == OvenBakeProjectTarget::Executable
                && project_relative_entrypoint(root, entrypoint).as_deref()
                    == Some(self.request.entrypoint_relative_path.as_str())
        });
        Ok(())
    }

    /// Record the compiling binary once after completed-output reuse has missed.
    pub(super) fn begin_fresh_compilation(&mut self) -> CliResult<()> {
        self.compiler = Some(CompilingBinary::observe_current()?);
        Ok(())
    }

    /// Attach the observation only to the explicitly requested module entrypoint.
    pub(super) fn compiler_identity_for(&self, root: &Path, entrypoint: &Path) -> Option<CompilerBinaryIdentity> {
        if project_relative_entrypoint(root, entrypoint).as_deref()
            != Some(self.request.entrypoint_relative_path.as_str())
        {
            return None;
        }
        self.compiler.as_ref().map(|compiler| compiler.identity.clone())
    }

    /// Complete the fresh-publisher binary check before sealing any output or descriptor.
    pub(super) fn verify_compiling_binary(&self) -> CliResult<()> {
        self.compiler
            .as_ref()
            .ok_or_else(|| CliError::failure("fresh Engine publisher has no compiling binary observation"))?
            .verify_unchanged()
    }

    /// Publish from an existing completed-output owner while that owner remains leased.
    pub(super) fn publish_if_requested(
        &mut self,
        store: &OvenStore,
        receipt: &OvenReceipt,
        output: &OvenStoredProjectOutput,
    ) -> CliResult<()> {
        if output.payload.project_target != OvenBakeProjectTarget::Executable.as_str()
            || output.payload.entrypoint_relative_path != self.request.entrypoint_relative_path
        {
            return Ok(());
        }
        // The existing completed-output publisher or warm materializer has validated these bytes. Keep its original
        // owner leased through descriptor publication; do not relabel an older output with this process's compiler.
        let descriptor =
            EngineDescriptor::from_output(self.request.contract, &output.manifest, &output.payload, receipt)?;
        verify_native_executable_mode(&output.native_output)?;
        let payload = serde_json::to_vec(&descriptor).map_err(|error| CliError::failure(error.to_string()))?;
        if payload.len() > DESCRIPTOR_LIMIT {
            return Err(CliError::failure("Engine descriptor exceeds its bounded wire size"));
        }
        let manifest = store
            .publish(&OvenArtifactPublishRequest {
                receipt: receipt.clone(),
                domain: output.manifest.domain.clone(),
                kind: OvenArtifactKind::Engine,
                payload: payload.clone(),
                materialized_files: Vec::new(),
            })
            .map_err(|error| CliError::failure(error.to_string()))?;
        let mut selected = store
            .select_payloads_matching_for_execution(|candidate| candidate.identity == manifest.identity)
            .map_err(|error| CliError::failure(error.to_string()))?;
        if selected.len() != 1 {
            return Err(CliError::failure(
                "published Engine descriptor lost its exact store owner",
            ));
        }
        let owner = selected
            .pop()
            .ok_or_else(|| CliError::failure("published Engine descriptor is unavailable"))?;
        if owner.manifest != manifest || owner.payload != payload {
            return Err(CliError::failure(
                "Engine descriptor changed during publication admission",
            ));
        }
        self.published.push(owner);
        Ok(())
    }
}

/// Exact completed-output facts repeated by the descriptor for owner-bound admission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EngineOutputBinding {
    identity: String,
    receipt: OvenReceipt,
    plan_identity: String,
    project_identity: String,
    target_identity: String,
    entrypoint_relative_path: String,
    source_authority_digest: String,
    backend_receipt: BackendExecutionReceipt,
    compiler_binary_identity: CompilerBinaryIdentity,
    native_relative_path: String,
    native_digest: String,
    native_logical_bytes: u64,
}

/// Versioned module declaration derived by the completed-output publisher.
///
/// It supplies module/source/compiler-binary evidence for a later handshake. Compiler checkout provenance is not
/// supplied by this publisher. The hosting process must enforce the declared ABI separately from binary identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EngineDescriptor {
    schema_version: u32,
    artifact_kind: String,
    contract: EngineModuleContract,
    native_file_exchange_abi: u32,
    request_schemas: Vec<String>,
    response_schemas: Vec<String>,
    output: EngineOutputBinding,
}

impl EngineDescriptor {
    /// Project the original executable receipt and materialized native record without reading source again.
    fn from_output(
        contract: EngineModuleContract,
        manifest: &OvenArtifactManifest,
        payload: &OvenProjectOutputPayload,
        receipt: &OvenReceipt,
    ) -> CliResult<Self> {
        receipt
            .verify_identity()
            .map_err(|error| CliError::failure(error.to_string()))?;
        payload
            .backend_receipt
            .verify_identity()
            .map_err(|error| CliError::failure(error.to_string()))?;
        if manifest.kind != OvenArtifactKind::ProjectOutput
            || payload.project_target != OvenBakeProjectTarget::Executable.as_str()
            || receipt.identity != manifest.receipt_identity
            || receipt.build_unit_identity != manifest.build_unit_identity
            || receipt.intent != manifest.intent
            || payload.receipt_identity != receipt.identity
            || payload.build_unit_identity != receipt.build_unit_identity
        {
            return Err(CliError::failure(
                "Engine requires the exact receipt-bound completed executable output",
            ));
        }
        let compiler_binary_identity = payload.compiler_binary_identity.clone().ok_or_else(|| {
            CliError::failure("completed output has no original compiling Incan binary observation; Engine publication cannot retag it")
        })?;
        let mut native = payload
            .files
            .iter()
            .filter(|file| file.output_relative_path == OVEN_PROJECT_OUTPUT_ARTIFACT_PATH);
        let native = native
            .next()
            .filter(|_| native.next().is_none())
            .ok_or_else(|| CliError::failure("Engine requires exactly one sealed native output"))?;
        let mut materialized = manifest
            .materialized_files
            .iter()
            .filter(|file| file.relative_path == native.output_relative_path);
        let materialized = materialized
            .next()
            .filter(|_| materialized.next().is_none())
            .ok_or_else(|| {
                CliError::failure("Engine native output is absent or duplicated in its materialized owner")
            })?;
        if materialized.digest != native.digest
            || materialized.logical_bytes != native.logical_bytes
            || !materialized.executable
        {
            return Err(CliError::failure(
                "Engine native output digest, length or executable mode differs from its completed owner",
            ));
        }
        let (schema_version, schemas) = contract.wire_contract();
        let descriptor = Self {
            schema_version,
            artifact_kind: DESCRIPTOR_KIND.to_string(),
            contract,
            native_file_exchange_abi: 1,
            request_schemas: schemas.clone(),
            response_schemas: schemas,
            output: EngineOutputBinding {
                identity: manifest.identity.clone(),
                receipt: receipt.clone(),
                plan_identity: payload.plan_identity.clone(),
                project_identity: payload.project_identity.clone(),
                target_identity: payload.target_identity.clone(),
                entrypoint_relative_path: payload.entrypoint_relative_path.clone(),
                source_authority_digest: payload.source_authority_digest.clone(),
                backend_receipt: payload.backend_receipt.clone(),
                compiler_binary_identity,
                native_relative_path: native.output_relative_path.clone(),
                native_digest: native.digest.clone(),
                native_logical_bytes: native.logical_bytes,
            },
        };
        descriptor.validate_shape()?;
        Ok(descriptor)
    }

    /// Decode bounded descriptor bytes, refusing unsupported versions before decoding the typed body.
    ///
    /// Decoding does not admit an artifact or authorize a module. Use [`AdmittedEngineArtifact::borrow`] with the
    /// original selected Engine and completed-output owners for artifact admission.
    #[allow(dead_code, reason = "Pending #991: descriptor admission is not connected")]
    pub(crate) fn from_json(bytes: &[u8]) -> CliResult<Self> {
        if bytes.len() > DESCRIPTOR_LIMIT {
            return Err(CliError::failure("Engine descriptor exceeds its bounded wire size"));
        }
        #[derive(Deserialize)]
        struct Header {
            schema_version: u32,
            artifact_kind: String,
        }
        let header: Header = serde_json::from_slice(bytes)
            .map_err(|error| CliError::failure(format!("invalid Engine descriptor header: {error}")))?;
        if ![DESCRIPTOR_VERSION, 2, 3].contains(&header.schema_version) || header.artifact_kind != DESCRIPTOR_KIND {
            return Err(CliError::failure(
                "unsupported Engine descriptor kind or schema version",
            ));
        }
        let descriptor: Self = serde_json::from_slice(bytes)
            .map_err(|error| CliError::failure(format!("invalid Engine descriptor: {error}")))?;
        descriptor.validate_shape()?;
        Ok(descriptor)
    }

    /// Check only the declared wire contract; original store owners are required for artifact admission.
    fn validate_shape(&self) -> CliResult<()> {
        self.output
            .receipt
            .verify_identity()
            .map_err(|error| CliError::failure(error.to_string()))?;
        self.output
            .backend_receipt
            .verify_identity()
            .map_err(|error| CliError::failure(error.to_string()))?;
        let (version, schemas) = self.contract.wire_contract();
        if self.schema_version != version
            || self.artifact_kind != DESCRIPTOR_KIND
            || self.native_file_exchange_abi != 1
            || self.request_schemas != schemas
            || self.response_schemas != schemas
            || self.output.native_relative_path != OVEN_PROJECT_OUTPUT_ARTIFACT_PATH
            || [
                &self.output.identity,
                &self.output.plan_identity,
                &self.output.project_identity,
                &self.output.target_identity,
                &self.output.source_authority_digest,
                &self.output.entrypoint_relative_path,
            ]
            .into_iter()
            .any(|value| value.trim().is_empty())
            || !is_sha256(&self.output.compiler_binary_identity.digest)
            || !is_sha256(&self.output.native_digest)
        {
            return Err(CliError::failure(
                "Engine descriptor contains an incomplete or unsupported module binding",
            ));
        }
        let _ = validated_project_output_relative_path(&self.output.entrypoint_relative_path, "Engine entrypoint")?;
        Ok(())
    }

    /// Return the declared module role, without selecting an operation or granting its capabilities.
    #[must_use]
    #[allow(dead_code, reason = "Pending #991: reporting accessor has no ordinary caller")]
    pub(crate) fn module_contract(&self) -> EngineModuleContract {
        self.contract
    }

    /// Return the declared file-exchange ABI revision for the later host compatibility check.
    #[must_use]
    #[allow(dead_code, reason = "Pending #991: reporting accessor has no ordinary caller")]
    pub(crate) fn native_file_exchange_abi(&self) -> u32 {
        self.native_file_exchange_abi
    }

    /// Return the original authored-source authority, separate from the compiler executable observation.
    #[must_use]
    #[allow(dead_code, reason = "Pending #991: reporting accessor has no ordinary caller")]
    pub(crate) fn source_authority_digest(&self) -> &str {
        &self.output.source_authority_digest
    }

    /// Return the recorded module entrypoint relative to its original logical project.
    #[must_use]
    #[allow(dead_code, reason = "Pending #991: reporting accessor has no ordinary caller")]
    pub(crate) fn entrypoint_relative_path(&self) -> &str {
        &self.output.entrypoint_relative_path
    }

    /// Return the exact ProjectOutput identity; the caller must already hold that original store owner.
    #[must_use]
    #[allow(dead_code, reason = "Pending #991: reporting accessor has no ordinary caller")]
    pub(crate) fn project_output_identity(&self) -> &str {
        &self.output.identity
    }

    /// Return the actual compiler binary observation without claiming compiler checkout provenance.
    #[must_use]
    #[allow(dead_code, reason = "Pending #991: reporting accessor has no ordinary caller")]
    pub(crate) fn compiler_binary_identity(&self) -> &CompilerBinaryIdentity {
        &self.output.compiler_binary_identity
    }

    /// Return the original completed-output receipt, including target, toolchain, profile and feature intent.
    #[must_use]
    #[allow(dead_code, reason = "Pending #991: reporting accessor has no ordinary caller")]
    pub(crate) fn receipt(&self) -> &OvenReceipt {
        &self.output.receipt
    }
}

/// Accept a complete digest encoding, without treating well-formed text as artifact authority.
fn is_sha256(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|digest| digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

/// Validate the native permission fact that the store records but its byte-integrity walk does not recheck.
fn verify_native_executable_mode(path: &Path) -> CliResult<()> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| CliError::failure(format!("cannot read Engine native output metadata: {error}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 {
            return Ok(());
        }
    }
    #[cfg(not(unix))]
    let _ = metadata;
    Err(CliError::failure(
        "Engine native output has no verified regular executable-mode fact",
    ))
}

/// Borrowed artifact evidence retaining both exact selected owners for the entire use of its native coordinate.
///
/// This is not an execution capability or a completed compatibility handshake. No effective filesystem, environment or
/// host-operation grants are constructed here; native file exchange alone provides no process sandbox.
#[allow(dead_code, reason = "Pending #991: borrowed admission has no ordinary consumer")]
pub(crate) struct AdmittedEngineArtifact<'a> {
    engine_owner: &'a OvenStoreExecutionPayload,
    output_owner: &'a OvenStoreExecutionPayload,
    descriptor: EngineDescriptor,
    native_output: PathBuf,
}

#[allow(
    dead_code,
    reason = "Pending #991: borrowed admission and reporting are not connected"
)]
impl<'a> AdmittedEngineArtifact<'a> {
    /// Validate original leased owners and join the descriptor to their existing completed-output authority.
    ///
    /// Each owner's existing physical validator runs once. This method performs no store selection, copying,
    /// publication or execution. A missing output cannot be substituted by an arbitrary file and digest.
    pub(crate) fn borrow(
        engine_owner: &'a OvenStoreExecutionPayload,
        output_owner: &'a OvenStoreExecutionPayload,
    ) -> CliResult<Self> {
        engine_owner
            .verify_admitted_payload()
            .map_err(|error| CliError::failure(error.to_string()))?;
        output_owner
            .verify_admitted_payload()
            .map_err(|error| CliError::failure(error.to_string()))?;
        if engine_owner.manifest.kind != OvenArtifactKind::Engine
            || !engine_owner.manifest.materialized_files.is_empty()
        {
            return Err(CliError::failure(
                "selected Engine owner must contain only its descriptor",
            ));
        }
        let descriptor = EngineDescriptor::from_json(&engine_owner.payload)?;
        let payload: OvenProjectOutputPayload = serde_json::from_slice(&output_owner.payload)
            .map_err(|error| CliError::failure(format!("invalid completed Engine output: {error}")))?;
        let native_output =
            validated_project_output_native_path(&output_owner.manifest, &output_owner.artifact_root, &payload)?;
        verify_native_executable_mode(&native_output)?;
        let expected = EngineDescriptor::from_output(
            descriptor.contract,
            &output_owner.manifest,
            &payload,
            &descriptor.output.receipt,
        )?;
        if descriptor != expected
            || engine_owner.manifest.receipt_identity != descriptor.output.receipt.identity
            || engine_owner.manifest.build_unit_identity != descriptor.output.receipt.build_unit_identity
            || engine_owner.manifest.intent != descriptor.output.receipt.intent
            || engine_owner.manifest.domain != output_owner.manifest.domain
        {
            return Err(CliError::failure(
                "Engine descriptor differs from its exact completed-output owner",
            ));
        }
        Ok(Self {
            engine_owner,
            output_owner,
            descriptor,
            native_output,
        })
    }

    /// Return validated module/source/compiler evidence without adding host authority.
    #[must_use]
    pub(crate) fn descriptor(&self) -> &EngineDescriptor {
        &self.descriptor
    }

    /// Return the native coordinate under the borrowed ProjectOutput lease, without permission to execute it.
    #[must_use]
    pub(crate) fn native_output(&self) -> &Path {
        &self.native_output
    }

    /// Recheck the exact sealed executable immediately before launch; a pruning lease does not prevent file mutation.
    pub(crate) fn verify_native_for_execution(&self) -> CliResult<()> {
        verify_native_executable_mode(&self.native_output)?;
        let metadata = std::fs::symlink_metadata(&self.native_output)
            .map_err(|error| CliError::failure(format!("cannot inspect Engine native output: {error}")))?;
        if metadata.len() != self.descriptor.output.native_logical_bytes
            || digest_file(&self.native_output).map_err(|error| CliError::failure(error.to_string()))?
                != self.descriptor.output.native_digest
        {
            return Err(CliError::failure(
                "Engine native output changed after borrowed admission",
            ));
        }
        Ok(())
    }

    /// Return the original sealed native digest for the kernel exchange report.
    pub(crate) fn native_digest(&self) -> &str {
        &self.descriptor.output.native_digest
    }

    /// Return both original owner identities for reporting and a later governed handoff.
    #[must_use]
    pub(crate) fn owner_identities(&self) -> (&str, &str) {
        (
            &self.engine_owner.manifest.identity,
            &self.output_owner.manifest.identity,
        )
    }
}

const CORE_ENGINE_ROOT: &str = "share/incan/oven/engines/core";
const CORE_ENGINE_SOURCE: &str = "share/incan/oven/core-source";
const CORE_ENGINE_ENTRYPOINT: &str = "src/plan_json_main.incn";
const CORE_ENGINE_INDEX: &str = "installed.json";

/// Toolchain installation selects original content owners; it does not grant host execution permission.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CoreEngineInstallation {
    schema_version: u32,
    installing_compiler: CompilerBinaryIdentity,
    contract: EngineModuleContract,
    engine_identity: String,
    output_identity: String,
    source_authority_digest: String,
    receipt_identity: String,
}

/// Exactly selected installed owners, held together through borrowed admission and the complete exchange.
pub(super) struct InstalledCoreEngine {
    installation: CoreEngineInstallation,
    engine: OvenStoreExecutionPayload,
    output: OvenStoreExecutionPayload,
}

impl CoreEngineInstallation {
    /// Reject future versions before decoding fields and require complete content coordinates.
    fn decode(bytes: &[u8]) -> CliResult<Self> {
        #[derive(Deserialize)]
        struct Header {
            schema_version: u32,
        }
        if bytes.len() > DESCRIPTOR_LIMIT {
            return Err(CliError::failure("installed Engine index exceeds its size limit"));
        }
        let header: Header = serde_json::from_slice(bytes)
            .map_err(|error| CliError::failure(format!("invalid installed Engine header: {error}")))?;
        if header.schema_version != 1 {
            return Err(CliError::failure("unsupported installed Engine index version"));
        }
        let value: Self = serde_json::from_slice(bytes)
            .map_err(|error| CliError::failure(format!("invalid installed Engine index: {error}")))?;
        if value.contract != EngineModuleContract::OvenSourceUnitBatchV3
            || [
                &value.installing_compiler.digest,
                &value.engine_identity,
                &value.output_identity,
                &value.source_authority_digest,
                &value.receipt_identity,
            ]
            .into_iter()
            .any(|value| !is_sha256(value))
        {
            return Err(CliError::failure(
                "installed Engine index has unsupported or incomplete identity bindings",
            ));
        }
        Ok(value)
    }
}

impl InstalledCoreEngine {
    /// Load only the actual canonical executable's installed toolchain, with no environment or project override.
    pub(super) fn load_current() -> CliResult<Self> {
        let executable =
            std::fs::canonicalize(std::env::current_exe().map_err(|error| CliError::failure(error.to_string()))?)
                .map_err(|error| CliError::failure(error.to_string()))?;
        let binary_directory = executable.parent().filter(|parent| parent.file_name().is_some_and(|name| name == "bin")).ok_or_else(|| CliError::failure("the active compiler has no installed core Engine layout; explicitly publish it through the toolchain installer"))?;
        let root = binary_directory
            .parent()
            .ok_or_else(|| CliError::failure("installed compiler has no toolchain root"))?;
        let compiler = CompilerBinaryIdentity {
            digest: digest_file(&executable).map_err(|error| CliError::failure(error.to_string()))?,
        };
        Self::load(root, &compiler)
    }

    /// Read the optional current index without creating a directory or following a redirected component.
    fn read_index(root: &Path) -> CliResult<Option<(PathBuf, CoreEngineInstallation)>> {
        use std::io::Read;
        let root = std::fs::canonicalize(root).map_err(|error| CliError::failure(error.to_string()))?;
        let component = root.join(CORE_ENGINE_ROOT);
        match std::fs::symlink_metadata(&component) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(CliError::failure(error.to_string())),
            Ok(_) => {}
        }
        if std::fs::canonicalize(&component).map_err(|error| CliError::failure(error.to_string()))? != component {
            return Err(CliError::failure(
                "installed core Engine root must not redirect outside its toolchain",
            ));
        }
        let index = component.join(CORE_ENGINE_INDEX);
        let metadata = match std::fs::symlink_metadata(&index) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(CliError::failure(error.to_string())),
            Ok(metadata) => metadata,
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(CliError::failure(
                "installed Engine index must be an original regular file",
            ));
        }
        let mut bytes = Vec::new();
        std::fs::File::open(&index)
            .and_then(|file| file.take((DESCRIPTOR_LIMIT + 1) as u64).read_to_end(&mut bytes))
            .map_err(|error| CliError::failure(error.to_string()))?;
        Ok(Some((component, CoreEngineInstallation::decode(&bytes)?)))
    }

    /// Select exact installation records once, retaining their original leases without modifying installed files.
    fn load(root: &Path, compiler: &CompilerBinaryIdentity) -> CliResult<Self> {
        let (component, installation) =
            Self::read_index(root)?.ok_or_else(|| CliError::failure("core Engine is not installed"))?;
        if installation.installing_compiler != *compiler {
            return Err(CliError::failure(
                "core Engine installation belongs to a different compiler binary",
            ));
        }
        Self::select_original_owners(&component, installation)
    }

    /// Retain a previous valid installation throughout replacement, even when the installing compiler has changed.
    ///
    /// This protects rollback referents only; it does not grant execution under the current compiler. Ordinary load
    /// continues to enforce the installation's original compiler binding before owner selection.
    fn retain_previous(root: &Path) -> CliResult<Option<Self>> {
        let Some((component, installation)) = Self::read_index(root)? else {
            return Ok(None);
        };
        let previous = Self::select_original_owners(&component, installation)?;
        previous.with_admitted(|_| Ok(()))?;
        Ok(Some(previous))
    }

    /// Select only the two exact records already named by a validated installation index.
    fn select_original_owners(component: &Path, installation: CoreEngineInstallation) -> CliResult<Self> {
        let mut selected = crate::oven::store::PublishedOvenStore::new(component.join("store"))
            .select_payloads_matching_for_execution(|manifest| {
                manifest.identity == installation.engine_identity || manifest.identity == installation.output_identity
            })
            .map_err(|error| CliError::failure(error.to_string()))?;
        if selected.len() != 2 {
            return Err(CliError::failure(
                "installed core Engine requires exactly its two original owners",
            ));
        }
        let engine_index = selected
            .iter()
            .position(|owner| owner.manifest.identity == installation.engine_identity)
            .ok_or_else(|| CliError::failure("installed Engine owner is missing"))?;
        let engine = selected.swap_remove(engine_index);
        let output = selected
            .pop()
            .ok_or_else(|| CliError::failure("installed Engine output owner is missing"))?;
        Ok(Self {
            installation,
            engine,
            output,
        })
    }

    /// Borrow the original installed descriptor/output pair for one command; this callback grants no permission.
    pub(super) fn with_admitted<T>(
        &self,
        consume: impl FnOnce(&AdmittedEngineArtifact<'_>) -> CliResult<T>,
    ) -> CliResult<T> {
        let admitted = AdmittedEngineArtifact::borrow(&self.engine, &self.output)?;
        if admitted.descriptor.module_contract() != self.installation.contract
            || admitted.descriptor.source_authority_digest() != self.installation.source_authority_digest
            || admitted.descriptor.receipt().identity != self.installation.receipt_identity
            || admitted.owner_identities()
                != (
                    self.installation.engine_identity.as_str(),
                    self.installation.output_identity.as_str(),
                )
        {
            return Err(CliError::failure(
                "installed core Engine record differs from its original admitted owners",
            ));
        }
        consume(&admitted)
    }
}

/// Publish the one toolchain-owned core selector and install its original Engine/output owners.
///
/// This explicit administrative command accepts no project, executable, contract or plugin selector. Its fixed source
/// project belongs to the toolchain being staged; normal commands only read the completed installation. The installing
/// compiler and original compiling compiler identities remain distinct. Archive authenticity belongs to the installer,
/// not to a content hash or a claim of operating-system isolation.
pub(crate) fn install_core_engine(toolchain_root: &Path) -> CliResult<()> {
    let root = std::fs::canonicalize(toolchain_root).map_err(|error| CliError::failure(error.to_string()))?;
    let current = CompilingBinary::observe_current()?;
    let installed_compiler =
        std::fs::canonicalize(root.join("bin").join(if cfg!(windows) { "incan.exe" } else { "incan" }))
            .map_err(|error| CliError::failure(error.to_string()))?;
    if std::fs::canonicalize(&current.path).map_err(|error| CliError::failure(error.to_string()))? != installed_compiler
    {
        return Err(CliError::failure(
            "core Engine installation must run through this toolchain's actual compiler",
        ));
    }
    let project = root.join(CORE_ENGINE_SOURCE);
    let mut engines = publish_engine_project(
        &project,
        EnginePublisherRequest::new(EngineModuleContract::OvenSourceUnitBatchV3, CORE_ENGINE_ENTRYPOINT)?,
    )?
    .into_iter()
    .filter(|owner| owner.manifest.intent.profile == "release")
    .collect::<Vec<_>>();
    if engines.len() != 1 {
        return Err(CliError::failure(
            "core Engine publisher did not produce exactly one release descriptor",
        ));
    }
    let engine_owner = engines
        .pop()
        .ok_or_else(|| CliError::failure("core Engine publication is absent"))?;
    let descriptor = EngineDescriptor::from_json(&engine_owner.payload)?;
    let source_store = super::open_default_oven_store()?;
    let mut outputs = source_store
        .select_payloads_matching_for_execution(|manifest| manifest.identity == descriptor.output.identity)
        .map_err(|error| CliError::failure(error.to_string()))?;
    if outputs.len() != 1 {
        return Err(CliError::failure("core Engine publisher output owner is unavailable"));
    }
    let output_owner = outputs
        .pop()
        .ok_or_else(|| CliError::failure("core Engine output is absent"))?;
    install_core_engine_owners(&root, &source_store, engine_owner, output_owner, &current)
}

/// Serialize replacement of the component index while its rollback and new Store owners are leased.
fn lock_core_engine_installation(root: &Path) -> CliResult<std::fs::File> {
    let mut directory = std::fs::canonicalize(root).map_err(|error| CliError::failure(error.to_string()))?;
    for component in Path::new(CORE_ENGINE_ROOT).components() {
        directory.push(component);
        match std::fs::create_dir(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(CliError::failure(error.to_string())),
        }
        let metadata = std::fs::symlink_metadata(&directory).map_err(|error| CliError::failure(error.to_string()))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(CliError::failure(
                "core Engine installation component is not an original directory",
            ));
        }
    }
    let path = directory.join(".installation.lock");
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => {
            return Err(CliError::failure(
                "core Engine installation lock is not an original regular file",
            ));
        }
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(CliError::failure(error.to_string())),
        _ => {}
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|error| CliError::failure(error.to_string()))?;
    file.lock().map_err(|error| CliError::failure(error.to_string()))?;
    Ok(file)
}

/// Copy only the genuine publisher's already selected owners, then commit their validated installation index.
fn install_core_engine_owners(
    root: &Path,
    source_store: &OvenStore,
    engine_owner: OvenStoreExecutionPayload,
    output_owner: OvenStoreExecutionPayload,
    current: &CompilingBinary,
) -> CliResult<()> {
    use std::io::Write;
    let admitted = AdmittedEngineArtifact::borrow(&engine_owner, &output_owner)?;
    if admitted.descriptor().module_contract() != EngineModuleContract::OvenSourceUnitBatchV3 {
        return Err(CliError::failure(
            "core Engine installation requires an explicitly published batch version 3 role",
        ));
    }
    let descriptor = admitted.descriptor().clone();
    current.verify_unchanged()?;
    let installing_compiler = current.identity.clone();
    let installation = CoreEngineInstallation {
        schema_version: 1,
        installing_compiler: installing_compiler.clone(),
        contract: EngineModuleContract::OvenSourceUnitBatchV3,
        engine_identity: engine_owner.manifest.identity.clone(),
        output_identity: output_owner.manifest.identity.clone(),
        source_authority_digest: admitted.descriptor.source_authority_digest().to_string(),
        receipt_identity: descriptor.output.receipt.identity.clone(),
    };
    let installation_lock = lock_core_engine_installation(root)?;
    // Pruning during either new publication must not destroy the index's rollback referents.
    let previous = InstalledCoreEngine::retain_previous(root)?;
    let component = root.join(CORE_ENGINE_ROOT);
    let destination = OvenStore::new(component.join("store"), *source_store.limits());
    let mut retained = Vec::with_capacity(2);
    for owner in [output_owner, engine_owner] {
        let identity = owner.manifest.identity.clone();
        let kind = owner.manifest.kind;
        let copied = super::publish_selected_provider_loaf(
            owner,
            &destination,
            &descriptor.output.receipt,
            &identity,
            kind,
            "core Engine installation",
        )?;
        if copied.identity != identity {
            return Err(CliError::failure(
                "core Engine installation changed an original owner identity",
            ));
        }
        // Publication returns an immutable record, not a lease. Acquire its execution owner before another publish
        // can prune it; if a concurrent pruner wins this interval, fail while the previous pair is still held.
        let mut selected = destination
            .select_payloads_for_execution(&[identity])
            .map_err(|error| CliError::failure(error.to_string()))?;
        if selected.len() != 1 {
            return Err(CliError::failure(
                "new core Engine member is unavailable after publication",
            ));
        }
        retained.push(
            selected
                .pop()
                .ok_or_else(|| CliError::failure("new core Engine owner is absent"))?,
        );
    }
    let engine = retained
        .pop()
        .ok_or_else(|| CliError::failure("new core Engine descriptor owner is absent"))?;
    let output = retained
        .pop()
        .ok_or_else(|| CliError::failure("new core Engine output owner is absent"))?;
    let installed = InstalledCoreEngine {
        installation: installation.clone(),
        engine,
        output,
    };
    installed.with_admitted(|_| Ok(()))?;
    let bytes = serde_json::to_vec(&installation).map_err(|error| CliError::failure(error.to_string()))?;
    let _ = CoreEngineInstallation::decode(&bytes)?;
    // The index is the commit point. A failed copy preserves the previous index and its referenced immutable owners.
    let mut temporary =
        tempfile::NamedTempFile::new_in(&component).map_err(|error| CliError::failure(error.to_string()))?;
    temporary
        .write_all(&bytes)
        .and_then(|_| temporary.as_file().sync_all())
        .map_err(|error| CliError::failure(error.to_string()))?;
    current.verify_unchanged()?;
    temporary
        .persist(component.join(CORE_ENGINE_INDEX))
        .map_err(|error| CliError::failure(error.to_string()))?;
    // Both destination owners and the prior pair remain leased through the successful index commit.
    drop(installed);
    drop(previous);
    drop(installation_lock);
    Ok(())
}

// The existing store records executable permissions only on Unix. These are metadata/owner controls, not native module
// execution tests; a non-Unix publisher cannot currently establish this descriptor's executable-mode fact.
#[cfg(test)]
#[cfg(unix)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs::{self, File};
    use std::os::unix::fs::{PermissionsExt, symlink};

    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use super::super::engine_exchange::{ExchangeOutcome, ExchangePhase, exchange};
    use super::super::{EngineBootstrapPermit, EngineCommandAuthority, EngineKernelCeiling, EngineKernelOperation};
    use super::super::{
        OVEN_PROJECT_OUTPUT_ARTIFACT_PATH, OvenBakeProjectTarget, OvenProjectOutputPayload, OvenStoredProjectOutput,
    };
    use super::{
        AdmittedEngineArtifact, CORE_ENGINE_INDEX, CORE_ENGINE_ROOT, CompilerBinaryIdentity, CompilingBinary,
        CoreEngineInstallation, DESCRIPTOR_LIMIT, EngineDescriptor, EngineModuleContract, EnginePublisher,
        EnginePublisherRequest, InstalledCoreEngine, install_core_engine_owners,
    };
    use crate::generated_source::{digest_bytes, digest_file};
    use crate::oven::OvenReceipt;
    use crate::oven::store::{
        OvenArtifactKind, OvenArtifactPublishRequest, OvenStore, OvenStoreExecutionPayload, OvenStoreLimits,
    };

    type TestResult = Result<(), Box<dyn std::error::Error>>;
    type FileInventory = BTreeMap<PathBuf, (Vec<u8>, u32)>;

    struct Fixture {
        project_root: tempfile::TempDir,
        store: OvenStore,
        receipt: OvenReceipt,
        completed: OvenStoredProjectOutput,
    }

    impl Fixture {
        /// Use the existing completed-output fixture and real store publisher; the native file is test data only.
        fn new(label: &str, compiler_observed: bool) -> Result<Self, Box<dyn std::error::Error>> {
            Self::with_native(label, compiler_observed, None)
        }

        /// Supply an explicit process-test script before ordinary output sealing; this is not Incan native proof.
        fn with_native(
            label: &str,
            compiler_observed: bool,
            native: Option<&[u8]>,
        ) -> Result<Self, Box<dyn std::error::Error>> {
            let root = tempfile::tempdir()?;
            fs::create_dir(root.path().join("src"))?;
            fs::write(root.path().join("loaf.toml"), "[project]\nname = \"fixture\"\n")?;
            fs::write(root.path().join("src/main.incn"), "def main() -> None:\n    pass\n")?;
            let store = OvenStore::new(
                root.path().join("store"),
                OvenStoreLimits::new(16 << 20, 16 << 20, 16 << 20),
            );
            let (receipt, mut payload, files) =
                super::super::tests::fixture_project_output_publication(root.path(), "debug", label)?;
            if compiler_observed {
                // In a libtest this is the libtest process, not a claimed standalone compiler/native build proof.
                payload.compiler_binary_identity = Some(CompilingBinary::observe_current()?.identity);
            }
            for file in &files {
                if file.output_relative_path == OVEN_PROJECT_OUTPUT_ARTIFACT_PATH {
                    if let Some(bytes) = native {
                        fs::write(&file.source_path, bytes)?;
                        for record in &mut payload.files {
                            if record.output_relative_path == file.output_relative_path {
                                record.digest = digest_bytes(bytes);
                                record.logical_bytes = bytes.len() as u64;
                            }
                        }
                    }
                    fs::set_permissions(&file.source_path, fs::Permissions::from_mode(0o755))?;
                }
            }
            let completed = super::super::publish_project_output_loaf(&store, &receipt, &payload, &files)?;
            Ok(Self {
                project_root: root,
                store,
                receipt,
                completed,
            })
        }

        /// Publish only through the same private adapter used by the explicit bake completion seam.
        fn publish_engine(&self) -> Result<OvenStoreExecutionPayload, Box<dyn std::error::Error>> {
            let mut publisher = publisher()?;
            publisher.publish_if_requested(&self.store, &self.receipt, &self.completed)?;
            Ok(publisher.published.pop().ok_or("fixture did not publish an Engine")?)
        }

        /// Select an exact original output owner, without copying or reconstructing its lease.
        fn output_owner(&self) -> Result<OvenStoreExecutionPayload, Box<dyn std::error::Error>> {
            select_exact(&self.store, &self.completed.identity)
        }
    }

    /// The fixture does not use filename recognition or a public executable-path override.
    fn publisher() -> Result<EnginePublisher, Box<dyn std::error::Error>> {
        Ok(EnginePublisher {
            request: EnginePublisherRequest::new(EngineModuleContract::OvenSourceUnitBatchV1, "src/main.incn")?,
            compiler: None,
            published: Vec::new(),
        })
    }

    /// Publish a version 3 descriptor through the same completion adapter used by the explicit installer.
    fn publish_core_fixture(fixture: &Fixture) -> Result<OvenStoreExecutionPayload, Box<dyn std::error::Error>> {
        let mut publisher = EnginePublisher {
            request: EnginePublisherRequest::new(EngineModuleContract::OvenSourceUnitBatchV3, "src/main.incn")?,
            compiler: None,
            published: Vec::new(),
        };
        publisher.publish_if_requested(&fixture.store, &fixture.receipt, &fixture.completed)?;
        Ok(publisher.published.pop().ok_or("core fixture descriptor missing")?)
    }

    /// Exact publisher owners survive installation/relocation; host binary drift and redirected indexes refuse.
    #[test]
    fn core_engine_installation_preserves_original_owners_and_rejects_substitutions() -> TestResult {
        let fixture = Fixture::new("core-install", true)?;
        let root = tempfile::tempdir()?;
        let current = CompilingBinary::observe_current()?;
        let owner = publish_core_fixture(&fixture)?;
        let expected_engine = owner.manifest.identity.clone();
        let expected_output = fixture.completed.identity.clone();
        install_core_engine_owners(root.path(), &fixture.store, owner, fixture.output_owner()?, &current)?;
        let installed = InstalledCoreEngine::load(root.path(), &current.identity)?;
        installed.with_admitted(|admitted| {
            assert_eq!(
                admitted.owner_identities(),
                (expected_engine.as_str(), expected_output.as_str())
            );
            assert_eq!(admitted.descriptor().receipt(), &fixture.receipt);
            Ok(())
        })?;
        let index = root.path().join(CORE_ENGINE_ROOT).join(CORE_ENGINE_INDEX);
        let bytes = fs::read(&index)?;
        let changed_compiler = CompilerBinaryIdentity {
            digest: digest_bytes(b"different-host"),
        };
        assert!(InstalledCoreEngine::load(root.path(), &changed_compiler).is_err());
        let mut changed = CoreEngineInstallation::decode(&bytes)?;
        changed.source_authority_digest = digest_bytes(b"different-module-source");
        fs::write(&index, serde_json::to_vec(&changed)?)?;
        let substituted = InstalledCoreEngine::load(root.path(), &current.identity)?;
        assert!(substituted.with_admitted(|_| Ok(())).is_err());
        fs::write(&index, &bytes)?;
        drop(substituted);
        drop(installed);
        let moved = root.path().join("relocated-toolchain");
        fs::create_dir(&moved)?;
        fs::rename(root.path().join("share"), moved.join("share"))?;
        InstalledCoreEngine::load(&moved, &current.identity)?.with_admitted(|_| Ok(()))?;
        let moved_index = moved.join(CORE_ENGINE_ROOT).join(CORE_ENGINE_INDEX);
        fs::remove_file(&moved_index)?;
        let external_index = root.path().join("external-index.json");
        fs::write(&external_index, bytes)?;
        symlink(&external_index, &moved_index)?;
        assert!(InstalledCoreEngine::load(&moved, &current.identity).is_err());
        Ok(())
    }

    /// Legacy roles cannot replace an installed core; failed admission leaves the prior index byte-identical.
    #[test]
    fn core_engine_installer_keeps_prior_index_on_invalid_original_owner() -> TestResult {
        let fixture = Fixture::new("core-install-rollback", true)?;
        let root = tempfile::tempdir()?;
        let current = CompilingBinary::observe_current()?;
        install_core_engine_owners(
            root.path(),
            &fixture.store,
            publish_core_fixture(&fixture)?,
            fixture.output_owner()?,
            &current,
        )?;
        let index = root.path().join(CORE_ENGINE_ROOT).join(CORE_ENGINE_INDEX);
        let before = fs::read(&index)?;
        assert!(
            install_core_engine_owners(
                root.path(),
                &fixture.store,
                fixture.publish_engine()?,
                fixture.output_owner()?,
                &current
            )
            .is_err()
        );
        assert_eq!(fs::read(index)?, before);
        InstalledCoreEngine::load(root.path(), &current.identity)?.with_admitted(|_| Ok(()))?;
        assert!(CoreEngineInstallation::decode(br#"{"schema_version":99,"invalid_body":true}"#).is_err());
        Ok(())
    }

    /// A capacity refusal on the second copy preserves both prior referents and the first new leased owner.
    #[test]
    fn core_engine_replacement_pressure_preserves_rollback_owners() -> TestResult {
        let old = Fixture::new("old_core_pressure", true)?;
        let new = Fixture::new("new_core_pressure", true)?;
        let root = tempfile::tempdir()?;
        let current = CompilingBinary::observe_current()?;
        let old_engine = publish_core_fixture(&old)?;
        let old_engine_id = old_engine.manifest.identity.clone();
        install_core_engine_owners(root.path(), &old.store, old_engine, old.output_owner()?, &current)?;
        let component = root.path().join(CORE_ENGINE_ROOT);
        let before = fs::read(component.join(CORE_ENGINE_INDEX))?;
        let destination = OvenStore::new(component.join("store"), *old.store.limits());
        let prior = destination.inspect()?;
        let new_engine = publish_core_fixture(&new)?;
        let new_engine_id = new_engine.manifest.identity.clone();
        let new_output = new.output_owner()?;
        assert_eq!(new_engine.manifest.domain, new_output.manifest.domain);
        assert!(
            prior
                .entries
                .iter()
                .all(|entry| entry.manifest.domain == new_engine.manifest.domain)
        );
        let output_logical = new_output.manifest.payload.logical_bytes
            + new_output
                .manifest
                .materialized_files
                .iter()
                .map(|file| file.logical_bytes)
                .sum::<u64>();
        let engine_logical = new_engine.manifest.payload.logical_bytes;
        assert!(engine_logical > 1);
        let tight = OvenStore::new(
            new.store.root(),
            OvenStoreLimits::new(
                16 << 20,
                16 << 20,
                prior.logical_bytes + output_logical + engine_logical - 1,
            ),
        );
        let error = install_core_engine_owners(root.path(), &tight, new_engine, new_output, &current)
            .err()
            .ok_or("second publication unexpectedly passed the tight capacity bound")?;
        assert!(error.to_string().contains("capacity blocked"), "{error}");
        assert_eq!(fs::read(component.join(CORE_ENGINE_INDEX))?, before);
        let entries = destination.inspect()?.entries;
        assert!(entries.iter().any(|entry| entry.manifest.identity == old_engine_id));
        assert!(
            entries
                .iter()
                .any(|entry| entry.manifest.identity == old.completed.identity)
        );
        assert!(
            entries
                .iter()
                .any(|entry| entry.manifest.identity == new.completed.identity)
        );
        assert!(!entries.iter().any(|entry| entry.manifest.identity == new_engine_id));
        InstalledCoreEngine::load(root.path(), &current.identity)?.with_admitted(|_| Ok(()))?;
        Ok(())
    }

    /// Replacement prunes only inactive pressure entries and publishes a complete newly admissible pair.
    #[test]
    fn core_engine_replacement_under_pressure_retains_new_pair() -> TestResult {
        let old = Fixture::new("old_core_success", true)?;
        let new = Fixture::new("new_core_success", true)?;
        let root = tempfile::tempdir()?;
        let current = CompilingBinary::observe_current()?;
        install_core_engine_owners(
            root.path(),
            &old.store,
            publish_core_fixture(&old)?,
            old.output_owner()?,
            &current,
        )?;
        let component = root.path().join(CORE_ENGINE_ROOT);
        let destination = OvenStore::new(component.join("store"), *old.store.limits());
        let prior_logical = destination.inspect()?.logical_bytes;
        let new_engine = publish_core_fixture(&new)?;
        let new_engine_id = new_engine.manifest.identity.clone();
        let new_output = new.output_owner()?;
        let pair_logical = new_engine.manifest.payload.logical_bytes
            + new_output.manifest.payload.logical_bytes
            + new_output
                .manifest
                .materialized_files
                .iter()
                .map(|file| file.logical_bytes)
                .sum::<u64>();
        let pressure = destination.publish(&OvenArtifactPublishRequest {
            receipt: new.receipt.clone(),
            domain: new_engine.manifest.domain.clone(),
            kind: OvenArtifactKind::ProjectPayload,
            payload: vec![b'x'; usize::try_from(pair_logical)?],
            materialized_files: Vec::new(),
        })?;
        let tight = OvenStore::new(
            new.store.root(),
            OvenStoreLimits::new(16 << 20, 16 << 20, prior_logical + pair_logical),
        );
        install_core_engine_owners(root.path(), &tight, new_engine, new_output, &current)?;
        let installed = InstalledCoreEngine::load(root.path(), &current.identity)?;
        installed.with_admitted(|admitted| {
            assert_eq!(
                admitted.owner_identities(),
                (new_engine_id.as_str(), new.completed.identity.as_str())
            );
            Ok(())
        })?;
        let entries = destination.inspect()?.entries;
        assert!(!entries.iter().any(|entry| entry.manifest.identity == pressure.identity));
        assert!(entries.iter().any(|entry| entry.manifest.identity == new_engine_id));
        assert!(
            entries
                .iter()
                .any(|entry| entry.manifest.identity == new.completed.identity)
        );
        Ok(())
    }

    /// The real issuer separates explicit host policy from descriptors, bounds, cancellation and ABI facts.
    #[test]
    fn core_engine_permit_requires_command_policy_and_exact_invocation() -> TestResult {
        let fixture = Fixture::new("core-permit", true)?;
        let engine_owner = publish_core_fixture(&fixture)?;
        let output_owner = fixture.output_owner()?;
        let admitted = AdmittedEngineArtifact::borrow(&engine_owner, &output_owner)?;
        let scope = fs::canonicalize(fixture.project_root.path())?;
        let cancelled = AtomicBool::new(false);
        let authority = EngineCommandAuthority {
            operation: EngineKernelOperation::SelectProviderSources,
            ceiling: EngineKernelCeiling::BoundedCoreSelection,
        };
        let issue = |authority: &EngineCommandAuthority, target: &str, input: &[u8], deadline| {
            authority.issue(&admitted, &fixture.receipt, input, &scope, target, deadline, &cancelled)
        };
        let target = fixture.receipt.intent.target.as_str();
        let permit = issue(&authority, target, b"{}", Instant::now() + Duration::from_secs(30))?;
        assert_eq!(permit.command_receipt.identity, fixture.receipt.identity);
        assert_eq!(permit.engine_identity, engine_owner.manifest.identity);
        assert_eq!(permit.request_digest, digest_bytes(b"{}"));
        assert_eq!(permit.request_limit, 1024 * 1024);
        let denied = EngineCommandAuthority {
            operation: EngineKernelOperation::SelectProviderSources,
            ceiling: EngineKernelCeiling::Denied,
        };
        assert!(issue(&denied, target, b"{}", Instant::now() + Duration::from_secs(30)).is_err());
        let unrelated = EngineCommandAuthority {
            operation: EngineKernelOperation::PublishModule,
            ceiling: EngineKernelCeiling::BoundedCoreSelection,
        };
        assert!(issue(&unrelated, target, b"{}", Instant::now() + Duration::from_secs(30)).is_err());
        assert!(
            issue(
                &authority,
                "wrong-host",
                b"{}",
                Instant::now() + Duration::from_secs(30)
            )
            .is_err()
        );
        assert!(
            issue(
                &authority,
                target,
                &vec![0; 1024 * 1024 + 1],
                Instant::now() + Duration::from_secs(30)
            )
            .is_err()
        );
        assert!(issue(&authority, target, b"{}", Instant::now()).is_err());
        cancelled.store(true, Ordering::Release);
        assert!(issue(&authority, target, b"{}", Instant::now() + Duration::from_secs(30)).is_err());
        Ok(())
    }

    /// An installed module uses the real issuer and preserves exact exchange bytes and terminal diagnostics.
    #[test]
    fn core_engine_installed_exchange_persists_exact_result_without_source_receipt_claim() -> TestResult {
        let fixture = Fixture::with_native(
            "installed-exchange",
            true,
            Some(b"#!/bin/sh\nprintf '{\"schema\":\"fixture\",\"ok\":true}' > \"$2\"\n"),
        )?;
        let root = tempfile::tempdir()?;
        let current = CompilingBinary::observe_current()?;
        install_core_engine_owners(
            root.path(),
            &fixture.store,
            publish_core_fixture(&fixture)?,
            fixture.output_owner()?,
            &current,
        )?;
        let installed = InstalledCoreEngine::load(root.path(), &current.identity)?;
        let scope = fs::canonicalize(root.path())?;
        let cancelled = AtomicBool::new(false);
        let authority = EngineCommandAuthority {
            operation: EngineKernelOperation::SelectProviderSources,
            ceiling: EngineKernelCeiling::BoundedCoreSelection,
        };
        installed.with_admitted(|admitted| {
            let permit = authority.issue(
                admitted,
                &fixture.receipt,
                b"{}",
                &scope,
                &fixture.receipt.intent.target,
                Instant::now() + Duration::from_secs(30),
                &cancelled,
            )?;
            let report = exchange(admitted, permit, b"{}", &cancelled);
            assert_eq!(report.outcome, ExchangeOutcome::Completed);
            assert_eq!(
                report.response.as_deref(),
                Some(br#"{"schema":"fixture","ok":true}"#.as_slice())
            );
            super::super::persist_engine_exchange_observation(&scope, &report)?;
            Ok(())
        })?;
        let reports = fs::read_dir(scope.join(".incan/engine-exchange"))?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("engine-exchange-"))
            .collect::<Vec<_>>();
        assert_eq!(reports.len(), 1);
        let value: serde_json::Value = serde_json::from_slice(&fs::read(reports[0].path())?)?;
        assert_eq!(value["kind"], "incan.oven.engine-exchange-observation");
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["outcome"], "completed");
        assert_eq!(
            value["response_digest"],
            digest_bytes(br#"{"schema":"fixture","ok":true}"#)
        );
        assert_eq!(
            value["response"],
            serde_json::to_value(br#"{"schema":"fixture","ok":true}"#.as_slice())?
        );
        assert!(value.get("canonical_operation_id").is_none());
        Ok(())
    }

    /// Explicit Engine publication filters only its declared entrypoint, avoiding unrelated acceptance targets.
    #[test]
    fn core_engine_publisher_keeps_only_its_declared_target() -> TestResult {
        let root = tempfile::tempdir()?;
        let publisher = publisher()?;
        let mut targets = vec![
            (
                OvenBakeProjectTarget::Executable,
                root.path().join("src/acceptance.incn"),
            ),
            (OvenBakeProjectTarget::Executable, root.path().join("src/main.incn")),
            (OvenBakeProjectTarget::Library, root.path().join("src/lib.incn")),
        ];
        publisher.retain_requested_target(root.path(), &mut targets)?;
        assert_eq!(
            targets,
            vec![(OvenBakeProjectTarget::Executable, root.path().join("src/main.incn"))]
        );
        Ok(())
    }

    /// Read a fixture's original admitted payload through the real store selection API.
    fn select_exact(
        store: &OvenStore,
        identity: &str,
    ) -> Result<OvenStoreExecutionPayload, Box<dyn std::error::Error>> {
        let mut owners = store.select_payloads_matching_for_execution(|manifest| manifest.identity == identity)?;
        assert_eq!(owners.len(), 1);
        Ok(owners.pop().ok_or("fixture owner is missing")?)
    }

    /// Capture every materialized file and mode around descriptor publication and admission.
    fn inventory(root: &Path) -> Result<FileInventory, Box<dyn std::error::Error>> {
        let mut result = BTreeMap::new();
        let mut directories = vec![root.to_path_buf()];
        while let Some(directory) = directories.pop() {
            for entry in fs::read_dir(directory)? {
                let path = entry?.path();
                let metadata = fs::symlink_metadata(&path)?;
                if metadata.is_dir() {
                    directories.push(path);
                } else {
                    result.insert(
                        path.strip_prefix(root)?.to_path_buf(),
                        (fs::read(&path)?, metadata.permissions().mode()),
                    );
                }
            }
        }
        Ok(result)
    }

    /// Check active lease contention through a read-only descriptor, without the writable pruner path.
    fn active_lock(owner: &OvenStoreExecutionPayload) -> Result<File, Box<dyn std::error::Error>> {
        Ok(File::open(
            owner
                .artifact_root
                .parent()
                .ok_or("artifact root has no owning entry")?
                .join(".active.lock"),
        )?)
    }

    /// Construct a permit only inside this trusted-parent fixture; production exchange has no issuing constructor.
    fn exchange_permit<'a>(
        fixture: &'a Fixture,
        admitted: &AdmittedEngineArtifact<'_>,
        request: &[u8],
    ) -> Result<EngineBootstrapPermit<'a>, Box<dyn std::error::Error>> {
        let (engine, output) = admitted.owner_identities();
        Ok(EngineBootstrapPermit {
            permit_id: "fixture-permit".into(),
            invocation_id: "fixture-invocation".into(),
            command_receipt: &fixture.receipt,
            engine_identity: engine.into(),
            output_identity: output.into(),
            contract: admitted.descriptor().module_contract(),
            host_target: admitted.descriptor().receipt().intent.target.clone(),
            request_digest: digest_bytes(request),
            scratch_parent: fixture.project_root.path().canonicalize()?,
            deadline: Instant::now() + Duration::from_secs(5),
            request_limit: 1024 * 1024,
            response_limit: 1024 * 1024,
            stdout_limit: 64 * 1024,
            stderr_limit: 64 * 1024,
        })
    }

    /// Preserve exact bytes and original store inventories while reporting a separate caller permit and invocation.
    #[test]
    fn engine_exchange_preserves_exact_response_and_original_owners() -> TestResult {
        let fixture = Fixture::with_native(
            "exchange_exact",
            true,
            Some(b"#!/bin/sh\n/bin/cat \"$1\" > \"$2\"\nprintf output\nprintf diagnostic >&2\n"),
        )?;
        let engine = fixture.publish_engine()?;
        let output = fixture.output_owner()?;
        let admitted = AdmittedEngineArtifact::borrow(&engine, &output)?;
        let before_output = inventory(&output.artifact_root)?;
        let before_engine = inventory(&engine.artifact_root)?;
        let request = b"{\"value\": 42}\n";
        let report = exchange(
            &admitted,
            exchange_permit(&fixture, &admitted, request)?,
            request,
            &AtomicBool::new(false),
        );
        assert_eq!(report.outcome, ExchangeOutcome::Completed, "{report:?}");
        assert_eq!(report.response.as_deref(), Some(request.as_slice()));
        assert_eq!(report.response_digest.as_deref(), Some(digest_bytes(request).as_str()));
        assert_eq!(report.stdout.bytes, b"output");
        assert_eq!(report.stderr.bytes, b"diagnostic");
        assert_eq!(report.permit_id, "fixture-permit");
        assert_eq!(report.invocation_id, "fixture-invocation");
        assert_eq!(report.command_receipt_identity, fixture.receipt.identity);
        assert_eq!(report.engine_identity, engine.manifest.identity);
        assert_eq!(report.output_identity, output.manifest.identity);
        assert!(
            report
                .phases
                .iter()
                .all(|phase| phase.outcome == Some(ExchangeOutcome::Completed))
        );
        assert_eq!(before_output, inventory(&output.artifact_root)?);
        assert_eq!(before_engine, inventory(&engine.artifact_root)?);
        assert!(!report.scratch_path.ok_or("missing scratch observation")?.exists());
        Ok(())
    }

    /// Reject independent owner, request, budget, deadline and cancellation failures without file or process effects.
    #[test]
    fn engine_exchange_refuses_unmatched_or_interrupted_permits_before_effects() -> TestResult {
        let fixture = Fixture::with_native("exchange_permit", true, Some(b"#!/bin/sh\nexit 0\n"))?;
        let engine = fixture.publish_engine()?;
        let output = fixture.output_owner()?;
        let admitted = AdmittedEngineArtifact::borrow(&engine, &output)?;
        let before = inventory(fixture.project_root.path())?;
        for case in 0..9 {
            let mut permit = exchange_permit(&fixture, &admitted, b"{}")?;
            let cancelled = AtomicBool::new(false);
            let expected = match case {
                0 => {
                    permit.engine_identity = "different-engine".into();
                    ExchangeOutcome::Refused
                }
                1 => {
                    permit.output_identity = "different-output".into();
                    ExchangeOutcome::Refused
                }
                2 => {
                    permit.request_digest = digest_bytes(b"changed");
                    ExchangeOutcome::Refused
                }
                3 => {
                    permit.stdout_limit = 0;
                    ExchangeOutcome::Refused
                }
                4 => {
                    permit.response_limit = 1024 * 1024 + 1;
                    ExchangeOutcome::Refused
                }
                5 => {
                    permit.deadline = Instant::now();
                    ExchangeOutcome::TimedOut
                }
                6 => {
                    cancelled.store(true, Ordering::Release);
                    ExchangeOutcome::Cancelled
                }
                7 => {
                    permit.host_target = "unrelated-host-target".into();
                    ExchangeOutcome::Refused
                }
                _ => {
                    permit.contract = EngineModuleContract::OvenSourceUnitBatchV2;
                    ExchangeOutcome::Refused
                }
            };
            let report = exchange(&admitted, permit, b"{}", &cancelled);
            assert_eq!(report.outcome, expected, "case {case}: {report:?}");
            assert!(report.child_id.is_none());
            assert!(report.scratch_path.is_none());
            assert_eq!(report.phases.len(), 1);
            assert_eq!(report.phases[0].phase, ExchangePhase::Admission);
        }
        assert_eq!(before, inventory(fixture.project_root.path())?);
        Ok(())
    }

    /// A lease prevents pruning but does not authorize executing bytes changed after initial admission.
    #[test]
    fn engine_exchange_rechecks_sealed_native_bytes_before_spawn() -> TestResult {
        let fixture = Fixture::with_native("exchange_tamper", true, Some(b"#!/bin/sh\nexit 0\n"))?;
        let engine = fixture.publish_engine()?;
        let output = fixture.output_owner()?;
        let admitted = AdmittedEngineArtifact::borrow(&engine, &output)?;
        fs::set_permissions(admitted.native_output(), fs::Permissions::from_mode(0o755))?;
        fs::write(admitted.native_output(), b"#!/bin/sh\nexit 7\n")?;
        let report = exchange(
            &admitted,
            exchange_permit(&fixture, &admitted, b"{}")?,
            b"{}",
            &AtomicBool::new(false),
        );
        assert_eq!(report.outcome, ExchangeOutcome::Failed);
        assert!(
            report
                .detail
                .as_deref()
                .is_some_and(|message| message.contains("changed after borrowed admission"))
        );
        assert!(report.child_id.is_none());
        assert!(!report.scratch_path.ok_or("missing scratch observation")?.exists());
        Ok(())
    }

    /// Missing, oversized, indirect, invalid-text and failed-child responses cannot become accepted exchange bytes.
    #[test]
    fn engine_exchange_rejects_invalid_response_and_child_failures() -> TestResult {
        let cases: [(&str, &[u8]); 7] = [
            ("missing", b"#!/bin/sh\nexit 0\n"),
            ("exit", b"#!/bin/sh\nprintf refused >&2\nexit 7\n"),
            ("utf8", b"#!/bin/sh\nprintf '\\377' > \"$2\"\n"),
            ("symlink", b"#!/bin/sh\n/bin/ln -s /dev/null \"$2\"\n"),
            ("hardlink", b"#!/bin/sh\n/bin/ln \"$1\" \"$2\"\n"),
            ("size", b"#!/bin/sh\nprintf 12345 > \"$2\"\n"),
            ("request", b"#!/bin/sh\nprintf changed > \"$1\"\nprintf ok > \"$2\"\n"),
        ];
        for (label, script) in cases {
            let fixture = Fixture::with_native(label, true, Some(script))?;
            let engine = fixture.publish_engine()?;
            let output = fixture.output_owner()?;
            let admitted = AdmittedEngineArtifact::borrow(&engine, &output)?;
            let mut permit = exchange_permit(&fixture, &admitted, b"{}")?;
            permit.response_limit = 4;
            let report = exchange(&admitted, permit, b"{}", &AtomicBool::new(false));
            assert_eq!(report.outcome, ExchangeOutcome::Failed, "{label}: {report:?}");
            let detail = report.detail.as_deref().ok_or("missing failure reason")?;
            let expected = match label {
                "missing" => detail.contains("No such file"),
                "exit" => {
                    detail.contains("Engine child exited")
                        && report.child_status.is_some_and(|status| status.code() == Some(7))
                }
                "utf8" => detail.contains("utf-8"),
                "symlink" => detail.contains("indirect") || detail.contains("Too many levels"),
                "hardlink" => detail.contains("bounded private regular file"),
                "size" => detail.contains("byte limit") || detail.contains("bounded private regular file"),
                "request" => detail.contains("changed the original request file"),
                _ => false,
            };
            assert!(expected, "{label}: unexpected failure {report:?}");
            assert!(report.response.is_none());
            assert!(report.response_digest.is_none());
            assert!(
                report
                    .phases
                    .iter()
                    .any(|phase| phase.phase == ExchangePhase::ProcessCleanup
                        && phase.outcome == Some(ExchangeOutcome::Completed))
            );
            assert!(!report.scratch_path.ok_or("missing scratch observation")?.exists());
            assert!(!crate::oven::process::process_is_running(
                report.child_id.ok_or("missing child ID")?
            )?);
        }
        Ok(())
    }

    /// Failed launch and incomplete file cleanup retain distinct phase failures without accepting response bytes.
    #[test]
    fn engine_exchange_reports_spawn_and_scope_cleanup_failures() -> TestResult {
        for (label, script) in [
            ("spawn", b"#!/missing-engine-test-interpreter\n".as_slice()),
            (
                "cleanup",
                b"#!/bin/sh\nprintf extra > extra\nprintf 42 > \"$2\"\n".as_slice(),
            ),
        ] {
            let fixture = Fixture::with_native(label, true, Some(script))?;
            let engine = fixture.publish_engine()?;
            let output = fixture.output_owner()?;
            let admitted = AdmittedEngineArtifact::borrow(&engine, &output)?;
            let report = exchange(
                &admitted,
                exchange_permit(&fixture, &admitted, b"{}")?,
                b"{}",
                &AtomicBool::new(false),
            );
            assert_eq!(report.outcome, ExchangeOutcome::Failed, "{report:?}");
            assert!(report.response.is_none());
            let scope = report.scratch_path.as_ref().ok_or("missing scope observation")?;
            if label == "spawn" {
                assert!(report.child_id.is_none());
                assert!(
                    report
                        .phases
                        .iter()
                        .any(|phase| phase.phase == ExchangePhase::Spawn
                            && phase.outcome == Some(ExchangeOutcome::Failed))
                );
                assert!(
                    !report
                        .phases
                        .iter()
                        .any(|phase| phase.phase == ExchangePhase::ProcessCleanup)
                );
                assert!(!scope.exists());
            } else {
                assert!(report.child_id.is_some());
                assert!(
                    report
                        .phases
                        .iter()
                        .any(|phase| phase.phase == ExchangePhase::ResponseRead
                            && phase.outcome == Some(ExchangeOutcome::Completed))
                );
                assert!(
                    report
                        .phases
                        .iter()
                        .any(|phase| phase.phase == ExchangePhase::FileCleanup
                            && phase.outcome == Some(ExchangeOutcome::Failed))
                );
                assert_eq!(fs::read(scope.join("extra"))?, b"extra");
                assert!(!scope.join("request.json").exists());
                assert!(!scope.join("response.json").exists());
            }
        }
        Ok(())
    }

    /// Overflow retains a bounded diagnostic prefix and still cleans up a blocked writer.
    #[test]
    fn engine_exchange_bounds_each_diagnostic_stream() -> TestResult {
        for (label, script) in [
            ("stdout", b"#!/bin/sh\nwhile :; do printf 0123456789; done\n".as_slice()),
            (
                "stderr",
                b"#!/bin/sh\nwhile :; do printf 0123456789 >&2; done\n".as_slice(),
            ),
        ] {
            let fixture = Fixture::with_native(label, true, Some(script))?;
            let engine = fixture.publish_engine()?;
            let output = fixture.output_owner()?;
            let admitted = AdmittedEngineArtifact::borrow(&engine, &output)?;
            let mut permit = exchange_permit(&fixture, &admitted, b"{}")?;
            permit.stdout_limit = 64;
            permit.stderr_limit = 64;
            let report = exchange(&admitted, permit, b"{}", &AtomicBool::new(false));
            assert_eq!(report.outcome, ExchangeOutcome::Failed, "{report:?}");
            let capture = if label == "stdout" {
                &report.stdout
            } else {
                &report.stderr
            };
            assert!(capture.exceeded);
            assert_eq!(capture.bytes.len(), 64);
            assert!(report.response.is_none());
            assert!(!crate::oven::process::process_is_running(
                report.child_id.ok_or("missing child ID")?
            )?);
        }
        Ok(())
    }

    /// Both timeout and caller cancellation kill normal descendants and remain distinct from completed exchange.
    #[test]
    fn engine_exchange_cancels_and_times_out_process_groups() -> TestResult {
        for cancel in [false, true] {
            let fixture = Fixture::with_native(
                "interrupt",
                true,
                Some(b"#!/bin/sh\n/bin/sleep 30 &\nprintf '%s' \"$!\"\nprintf ready > \"$2\"\nwait\n"),
            )?;
            let engine = fixture.publish_engine()?;
            let output = fixture.output_owner()?;
            let admitted = AdmittedEngineArtifact::borrow(&engine, &output)?;
            let mut permit = exchange_permit(&fixture, &admitted, b"{}")?;
            permit.deadline = Instant::now() + Duration::from_secs(if cancel { 5 } else { 1 });
            let cancelled = AtomicBool::new(false);
            let scratch_parent = permit.scratch_parent.clone();
            let report = std::thread::scope(|scope| {
                if cancel {
                    let cancelled = &cancelled;
                    scope.spawn(move || {
                        // The fixture cancels after actual child work, rather than racing process startup with a sleep.
                        let until = Instant::now() + Duration::from_secs(4);
                        while Instant::now() < until {
                            if fs::read_dir(&scratch_parent).is_ok_and(|entries| {
                                entries.flatten().any(|entry| {
                                    entry.file_name().to_string_lossy().starts_with("engine-")
                                        && entry.path().join("response.json").exists()
                                })
                            }) {
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        cancelled.store(true, Ordering::Release);
                    });
                }
                exchange(&admitted, permit, b"{}", &cancelled)
            });
            assert_eq!(
                report.outcome,
                if cancel {
                    ExchangeOutcome::Cancelled
                } else {
                    ExchangeOutcome::TimedOut
                },
                "{report:?}"
            );
            assert!(report.response.is_none());
            let descendant: u32 = std::str::from_utf8(&report.stdout.bytes)?.parse()?;
            assert!(!crate::oven::process::process_is_running(descendant)?);
            assert!(!crate::oven::process::process_is_running(
                report.child_id.ok_or("missing child ID")?
            )?);
            assert!(!report.scratch_path.ok_or("missing scratch observation")?.exists());
        }
        Ok(())
    }

    /// A successful parent cannot leave its normal group descendants running or holding diagnostic pipes open.
    #[test]
    fn engine_exchange_cleans_descendants_after_successful_child_exit() -> TestResult {
        let fixture = Fixture::with_native(
            "descendant",
            true,
            Some(b"#!/bin/sh\n/bin/sleep 30 &\nprintf '%s' \"$!\"\nprintf 42 > \"$2\"\nexit 0\n"),
        )?;
        let engine = fixture.publish_engine()?;
        let output = fixture.output_owner()?;
        let admitted = AdmittedEngineArtifact::borrow(&engine, &output)?;
        let report = exchange(
            &admitted,
            exchange_permit(&fixture, &admitted, b"{}")?,
            b"{}",
            &AtomicBool::new(false),
        );
        assert_eq!(report.outcome, ExchangeOutcome::Completed, "{report:?}");
        assert_eq!(report.response.as_deref(), Some(b"42".as_slice()));
        let descendant: u32 = std::str::from_utf8(&report.stdout.bytes)?.parse()?;
        assert!(!crate::oven::process::process_is_running(descendant)?);
        Ok(())
    }

    /// Explicit V2/V3 declarations preserve prior descriptor bytes and reject role/version relabeling.
    #[test]
    fn engine_exchange_role_versions_preserve_original_descriptor_evidence() -> TestResult {
        let fixture = Fixture::new("role", true)?;
        let engine = fixture.publish_engine()?;
        let original = engine.payload.clone();
        let first = EngineDescriptor::from_json(&original)?;
        assert_eq!(first.schema_version, 1);
        let second = EngineDescriptor::from_output(
            EngineModuleContract::OvenSourceUnitBatchV2,
            &fixture.completed.manifest,
            &fixture.completed.payload,
            &fixture.receipt,
        )?;
        assert_eq!(second.schema_version, 2);
        assert_eq!(second.output, first.output);
        assert!(second.request_schemas.contains(&"incan.oven.selection/2".to_string()));
        assert_eq!(EngineDescriptor::from_json(&serde_json::to_vec(&second)?)?, second);
        let mut crossed = first;
        crossed.contract = EngineModuleContract::OvenSourceUnitBatchV2;
        assert!(EngineDescriptor::from_json(&serde_json::to_vec(&crossed)?).is_err());
        assert_eq!(original, engine.payload);
        let second_fixture = Fixture::new("role_v2", true)?;
        let mut second_publisher = EnginePublisher {
            request: EnginePublisherRequest::new(EngineModuleContract::OvenSourceUnitBatchV2, "src/main.incn")?,
            compiler: None,
            published: Vec::new(),
        };
        second_publisher.publish_if_requested(
            &second_fixture.store,
            &second_fixture.receipt,
            &second_fixture.completed,
        )?;
        let second_owner = second_publisher.published.pop().ok_or("missing V2 owner")?;
        let second_bytes = second_owner.payload.clone();
        let second_output = second_fixture.output_owner()?;
        let admitted = AdmittedEngineArtifact::borrow(&second_owner, &second_output)?;
        assert_eq!(
            admitted.descriptor().module_contract(),
            EngineModuleContract::OvenSourceUnitBatchV2
        );
        let mut third_publisher = EnginePublisher {
            request: EnginePublisherRequest::new(EngineModuleContract::OvenSourceUnitBatchV3, "src/main.incn")?,
            compiler: None,
            published: Vec::new(),
        };
        third_publisher.publish_if_requested(
            &second_fixture.store,
            &second_fixture.receipt,
            &second_fixture.completed,
        )?;
        let third_owner = third_publisher.published.pop().ok_or("missing V3 owner")?;
        let third = AdmittedEngineArtifact::borrow(&third_owner, &second_output)?;
        assert_eq!(third.descriptor().schema_version, 3);
        assert_eq!(
            third.descriptor().module_contract(),
            EngineModuleContract::OvenSourceUnitBatchV3
        );
        assert!(
            third
                .descriptor()
                .request_schemas
                .contains(&"incan.oven.source-unit-batch/3".to_string())
        );
        let mut retagged = EngineDescriptor::from_json(&second_bytes)?;
        retagged.contract = EngineModuleContract::OvenSourceUnitBatchV3;
        assert!(EngineDescriptor::from_json(&serde_json::to_vec(&retagged)?).is_err());
        assert_eq!(original, engine.payload);
        assert_eq!(second_bytes, second_owner.payload);
        Ok(())
    }

    /// Verify descriptor admission preserves native inventory and holds both original store leases.
    #[test]
    fn engine_descriptor_borrows_both_original_owners_without_copying_native_bytes() -> TestResult {
        let fixture = Fixture::new("engine", true)?;
        let before = inventory(&fixture.completed.artifact_root)?;
        let engine = fixture.publish_engine()?;
        assert!(engine.manifest.materialized_files.is_empty());
        let output = fixture.output_owner()?;
        let admitted = AdmittedEngineArtifact::borrow(&engine, &output)?;
        assert_eq!(
            admitted.owner_identities(),
            (engine.manifest.identity.as_str(), output.manifest.identity.as_str())
        );
        assert_eq!(admitted.native_output(), fixture.completed.native_output);
        assert_eq!(admitted.descriptor().receipt(), &fixture.receipt);
        assert_eq!(
            admitted.descriptor().output.source_authority_digest,
            fixture.completed.payload.source_authority_digest
        );
        assert_eq!(
            admitted.descriptor().output.backend_receipt,
            fixture.completed.payload.backend_receipt
        );
        assert_eq!(
            admitted.descriptor().compiler_binary_identity(),
            fixture
                .completed
                .payload
                .compiler_binary_identity
                .as_ref()
                .ok_or("missing fixture compiler")?
        );
        assert_eq!(inventory(&fixture.completed.artifact_root)?, before);
        let engine_lock = active_lock(&engine)?;
        let output_lock = active_lock(&output)?;
        assert!(engine_lock.try_lock().is_err());
        assert!(output_lock.try_lock().is_err());
        drop(admitted);
        drop(engine);
        drop(output);
        drop(fixture.completed);
        engine_lock.try_lock()?;
        output_lock.try_lock()?;
        Ok(())
    }

    /// Verify legacy output reuse cannot borrow a current compiler identity that was never recorded.
    #[test]
    fn engine_legacy_completed_output_keeps_missing_compiler_observation() -> TestResult {
        let fixture = Fixture::new("legacy", false)?;
        let encoded = serde_json::to_vec(&fixture.completed.payload)?;
        let old_shape: serde_json::Value = serde_json::from_slice(&encoded)?;
        assert!(old_shape.get("compiler_binary_identity").is_none());
        let decoded: OvenProjectOutputPayload = serde_json::from_slice(&encoded)?;
        assert_eq!(decoded, fixture.completed.payload);
        super::super::validated_project_output_native_path(
            &fixture.completed.manifest,
            &fixture.completed.artifact_root,
            &decoded,
        )?;
        let mut explicit = publisher()?;
        // Even an available current observation cannot be assigned to an already completed legacy output.
        explicit.begin_fresh_compilation()?;
        let Err(error) = explicit.publish_if_requested(&fixture.store, &fixture.receipt, &fixture.completed) else {
            return Err("legacy output acquired a fabricated compiling binary".into());
        };
        assert!(error.to_string().contains("cannot retag"));
        assert!(explicit.published.is_empty());
        assert!(
            !fixture
                .store
                .manifests_for_selection()?
                .iter()
                .any(|manifest| manifest.kind == OvenArtifactKind::Engine)
        );
        Ok(())
    }

    /// Check version-first decoding and bounded wire refusal independently of artifact admission.
    #[test]
    fn engine_wire_requires_supported_version_before_body_and_rejects_invalid_fields() -> TestResult {
        let fixture = Fixture::new("wire", true)?;
        let owner = fixture.publish_engine()?;
        let descriptor = EngineDescriptor::from_json(&owner.payload)?;
        assert_eq!(serde_json::to_vec(&descriptor)?, owner.payload);
        // Invalid typed body deliberately accompanies a future header: version refusal must win.
        let future = br#"{"schema_version":4,"artifact_kind":"incan.oven.engine","output":false}"#;
        let Err(error) = EngineDescriptor::from_json(future) else {
            return Err("future Engine schema accepted".into());
        };
        assert!(
            error
                .to_string()
                .contains("unsupported Engine descriptor kind or schema")
        );
        let valid: serde_json::Value = serde_json::from_slice(&owner.payload)?;
        for (key, value) in [
            ("artifact_kind", serde_json::json!("unrelated")),
            ("extra", serde_json::json!(true)),
            ("native_file_exchange_abi", serde_json::json!(2)),
            ("request_schemas", serde_json::json!(["incan.oven.selection/2"])),
        ] {
            let mut changed = valid.clone();
            changed[key] = value;
            assert!(
                EngineDescriptor::from_json(&serde_json::to_vec(&changed)?).is_err(),
                "accepted {key}"
            );
        }
        assert!(EngineDescriptor::from_json(b"{").is_err());
        assert!(EngineDescriptor::from_json(&vec![b' '; DESCRIPTOR_LIMIT + 1]).is_err());
        Ok(())
    }

    /// Reject a valid but different owner or altered authority despite matching module naming.
    #[test]
    fn engine_requires_exact_output_and_receipt_owner_even_for_the_same_module_name() -> TestResult {
        let first = Fixture::new("first", true)?;
        let second = Fixture::new("second", true)?;
        let engine = first.publish_engine()?;
        let output = second.output_owner()?;
        assert!(AdmittedEngineArtifact::borrow(&engine, &output).is_err());
        let mut descriptor = EngineDescriptor::from_json(&engine.payload)?;
        // A well-formed different receipt cannot authorize the original output, even if the descriptor is sealed.
        descriptor.output.receipt = second.receipt.clone();
        let changed = first.store.publish(&OvenArtifactPublishRequest {
            receipt: second.receipt.clone(),
            domain: engine.manifest.domain.clone(),
            kind: OvenArtifactKind::Engine,
            payload: serde_json::to_vec(&descriptor)?,
            materialized_files: Vec::new(),
        })?;
        let changed_owner = select_exact(&first.store, &changed.identity)?;
        assert!(AdmittedEngineArtifact::borrow(&changed_owner, &first.output_owner()?).is_err());
        let original = EngineDescriptor::from_json(&engine.payload)?;
        for alteration in ["source", "plan", "native", "compiler"] {
            let mut changed = original.clone();
            match alteration {
                "source" => changed.output.source_authority_digest = crate::oven::digest_bytes(b"other source"),
                "plan" => changed.output.plan_identity = "other-plan".to_string(),
                "native" => changed.output.native_digest = crate::oven::digest_bytes(b"other native bytes"),
                "compiler" => {
                    changed.output.compiler_binary_identity.digest = crate::oven::digest_bytes(b"other compiler")
                }
                _ => return Err("unknown authority alteration".into()),
            }
            changed.validate_shape()?;
            let sealed = first.store.publish(&OvenArtifactPublishRequest {
                receipt: first.receipt.clone(),
                domain: engine.manifest.domain.clone(),
                kind: OvenArtifactKind::Engine,
                payload: serde_json::to_vec(&changed)?,
                materialized_files: Vec::new(),
            })?;
            let wrong_owner = select_exact(&first.store, &sealed.identity)?;
            assert!(
                AdmittedEngineArtifact::borrow(&wrong_owner, &first.output_owner()?).is_err(),
                "accepted changed {alteration}"
            );
        }
        Ok(())
    }

    /// Ensure mutable public fields cannot retarget the original owners retained by their leases.
    #[test]
    fn engine_admission_rejects_mutated_public_owners_and_payload() -> TestResult {
        let fixture = Fixture::new("owners", true)?;
        let mut engine = fixture.publish_engine()?;
        let mut output = fixture.output_owner()?;
        AdmittedEngineArtifact::borrow(&engine, &output)?;
        let original = engine.payload.clone();
        engine.payload.push(b' ');
        assert!(AdmittedEngineArtifact::borrow(&engine, &output).is_err());
        engine.payload = original;
        output.manifest.intent.profile = "release".to_string();
        assert!(AdmittedEngineArtifact::borrow(&engine, &output).is_err());
        output.manifest.intent.profile = "debug".to_string();
        output.artifact_root = engine.artifact_root.clone();
        assert!(AdmittedEngineArtifact::borrow(&engine, &output).is_err());
        Ok(())
    }

    /// Reject native-file tampering under live leases, then readmit only the restored original facts.
    #[test]
    fn engine_native_tampering_length_mode_and_symlink_refuse_under_original_leases() -> TestResult {
        let fixture = Fixture::new("native", true)?;
        let engine = fixture.publish_engine()?;
        let output = fixture.output_owner()?;
        let path = &fixture.completed.native_output;
        let original = fs::read(path)?;
        for changed in [vec![b'x'; original.len()], b"different length".to_vec()] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
            fs::write(path, changed)?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o555))?;
            assert!(AdmittedEngineArtifact::borrow(&engine, &output).is_err());
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
        fs::write(path, &original)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o444))?;
        assert!(AdmittedEngineArtifact::borrow(&engine, &output).is_err());
        fs::remove_file(path)?;
        assert!(AdmittedEngineArtifact::borrow(&engine, &output).is_err());
        let other = fixture.project_root.path().join("outside-native");
        fs::write(&other, &original)?;
        symlink(&other, path)?;
        assert!(AdmittedEngineArtifact::borrow(&engine, &output).is_err());
        fs::remove_file(path)?;
        fs::write(path, original)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o555))?;
        AdmittedEngineArtifact::borrow(&engine, &output)?;
        Ok(())
    }

    /// Reject incomplete, non-executable or escaping output declarations at the publisher projection.
    #[test]
    fn engine_projection_refuses_library_duplicate_missing_and_escaped_native_authority() -> TestResult {
        let fixture = Fixture::new("shape", true)?;
        EngineDescriptor::from_output(
            EngineModuleContract::OvenSourceUnitBatchV1,
            &fixture.completed.manifest,
            &fixture.completed.payload,
            &fixture.receipt,
        )?;
        for alteration in ["library", "duplicate", "missing", "escaped"] {
            let mut payload = fixture.completed.payload.clone();
            match alteration {
                "library" => payload.project_target = "library".to_string(),
                "duplicate" => payload
                    .files
                    .push(payload.files.first().ok_or("missing fixture file")?.clone()),
                "missing" => payload.files.clear(),
                "escaped" => payload.entrypoint_relative_path = "../src/main.incn".to_string(),
                _ => return Err("unknown test alteration".into()),
            }
            assert!(
                EngineDescriptor::from_output(
                    EngineModuleContract::OvenSourceUnitBatchV1,
                    &fixture.completed.manifest,
                    &payload,
                    &fixture.receipt
                )
                .is_err(),
                "accepted {alteration}"
            );
        }
        Ok(())
    }

    /// Require an explicit, unique executable target before observing a compiler or publishing an Engine.
    #[test]
    fn engine_request_requires_explicit_exact_executable_target_before_publication() -> TestResult {
        let fixture = Fixture::new("targets", true)?;
        let publisher = publisher()?;
        let root = fixture.project_root.path();
        publisher.validate_targets(root, &[(OvenBakeProjectTarget::Executable, root.join("src/main.incn"))])?;
        for targets in [
            Vec::new(),
            vec![(OvenBakeProjectTarget::Library, root.join("src/main.incn"))],
            vec![(OvenBakeProjectTarget::Executable, root.join("src/plan_json_main.incn"))],
            vec![(OvenBakeProjectTarget::Executable, root.join("src/main.incn")); 2],
        ] {
            assert!(publisher.validate_targets(root, &targets).is_err());
        }
        assert!(EnginePublisherRequest::new(EngineModuleContract::OvenSourceUnitBatchV1, "../main.incn").is_err());
        assert!(publisher.compiler.is_none());
        Ok(())
    }

    /// Verify the actual publisher executable observation and detect mutation before completion.
    #[test]
    fn engine_compiler_observation_uses_current_binary_and_detects_mid_build_mutation() -> TestResult {
        let actual = CompilingBinary::observe_current()?;
        actual.verify_unchanged()?;
        assert_eq!(actual.path, std::env::current_exe()?.canonicalize()?);
        let root = tempfile::tempdir()?;
        let path = root.path().join("compiler-copy");
        fs::write(&path, b"original compiler")?;
        // The synthetic command-local observation tests mutation; no production constructor accepts this path.
        let observation = CompilingBinary {
            identity: CompilerBinaryIdentity {
                digest: digest_file(&path)?,
            },
            path: path.clone(),
        };
        observation.verify_unchanged()?;
        fs::write(&path, b"replaced compiler")?;
        assert!(observation.verify_unchanged().is_err());
        Ok(())
    }

    /// Preserve descriptor identity across real store relocation while retaining the compiling binary binding.
    #[test]
    fn engine_descriptor_reference_does_not_salt_identity_with_physical_store_coordinates() -> TestResult {
        let fixture = Fixture::new("relocation", true)?;
        let owner = fixture.publish_engine()?;
        let descriptor = EngineDescriptor::from_json(&owner.payload)?;
        let bytes = String::from_utf8(serde_json::to_vec(&descriptor)?)?;
        assert!(!bytes.contains(fixture.project_root.path().to_string_lossy().as_ref()));
        assert!(!bytes.contains(std::env::current_exe()?.to_string_lossy().as_ref()));
        assert!(!bytes.contains("compiler_source"));
        let relocated = tempfile::tempdir()?;
        let relocated_store = OvenStore::new(relocated.path(), *fixture.store.limits());
        let files = fixture
            .completed
            .payload
            .files
            .iter()
            .map(|file| super::super::OvenProjectOutputBakeFile {
                source_path: fixture.completed.artifact_root.join(&file.output_relative_path),
                caller_relative_path: file.caller_relative_path.clone(),
                output_relative_path: file.output_relative_path.clone(),
            })
            .collect::<Vec<_>>();
        let completed = super::super::publish_project_output_loaf(
            &relocated_store,
            &fixture.receipt,
            &fixture.completed.payload,
            &files,
        )?;
        let mut relocated_publisher = publisher()?;
        relocated_publisher.publish_if_requested(&relocated_store, &fixture.receipt, &completed)?;
        let relocated_engine = relocated_publisher.published.pop().ok_or("missing relocated Engine")?;
        let relocated_output = select_exact(&relocated_store, &completed.identity)?;
        let admitted = AdmittedEngineArtifact::borrow(&relocated_engine, &relocated_output)?;
        assert_eq!(owner.manifest.identity, relocated_engine.manifest.identity);
        assert_eq!(owner.payload, relocated_engine.payload);
        assert_ne!(admitted.native_output(), fixture.completed.native_output);
        let mut other_compiler = descriptor.clone();
        other_compiler.output.compiler_binary_identity.digest = crate::oven::digest_bytes(b"another compiling binary");
        other_compiler.validate_shape()?;
        assert_ne!(serde_json::to_vec(&descriptor)?, serde_json::to_vec(&other_compiler)?);
        assert_eq!(
            other_compiler.native_file_exchange_abi,
            descriptor.native_file_exchange_abi
        );
        Ok(())
    }
}
