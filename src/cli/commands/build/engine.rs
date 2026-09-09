//! Publisher-derived Oven module descriptors and borrowed artifact admission.
//!
//! This boundary records an explicit trusted module contract against a completed project output. It does not select a
//! module, authorize host operations, negotiate host compatibility, or execute the native file. A held store lease
//! prevents pruning; future execution must still enforce its own integrity, ABI and capability checks.

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
#[allow(dead_code, reason = "Pending #991: no ordinary command invokes the Engine publisher")]
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
        let descriptor = Self {
            schema_version: DESCRIPTOR_VERSION,
            artifact_kind: DESCRIPTOR_KIND.to_string(),
            contract,
            native_file_exchange_abi: 1,
            request_schemas: EXCHANGE_SCHEMAS.into_iter().map(str::to_string).collect(),
            response_schemas: EXCHANGE_SCHEMAS.into_iter().map(str::to_string).collect(),
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
        if header.schema_version != DESCRIPTOR_VERSION || header.artifact_kind != DESCRIPTOR_KIND {
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
        if self.schema_version != DESCRIPTOR_VERSION
            || self.artifact_kind != DESCRIPTOR_KIND
            || self.native_file_exchange_abi != 1
            || self.request_schemas != EXCHANGE_SCHEMAS
            || self.response_schemas != EXCHANGE_SCHEMAS
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

    /// Return both original owner identities for reporting and a later governed handoff.
    #[must_use]
    pub(crate) fn owner_identities(&self) -> (&str, &str) {
        (
            &self.engine_owner.manifest.identity,
            &self.output_owner.manifest.identity,
        )
    }
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

    use super::super::{
        OVEN_PROJECT_OUTPUT_ARTIFACT_PATH, OvenBakeProjectTarget, OvenProjectOutputPayload, OvenStoredProjectOutput,
    };
    use super::{
        AdmittedEngineArtifact, CompilerBinaryIdentity, CompilingBinary, DESCRIPTOR_LIMIT, EngineDescriptor,
        EngineModuleContract, EnginePublisher, EnginePublisherRequest,
    };
    use crate::generated_source::digest_file;
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
        let future = br#"{"schema_version":2,"artifact_kind":"incan.oven.engine","output":false}"#;
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
