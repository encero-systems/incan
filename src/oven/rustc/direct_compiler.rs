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
    reason = "Gates 6 and 7 of RFC 119 are the reader; this substrate lands first so their blockers have something to change"
)]

use super::*;

pub(super) const OVEN_DIRECT_RUSTC_COMPILER_OWNER_SCHEMA_VERSION: u32 = 1;
pub(super) const OVEN_DIRECT_RUSTC_COMPILER_DOMAIN_PREFIX: &str = "native-compiler";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OvenDirectRustcCompilerOwnerPayload {
    pub(super) schema_version: u32,
    pub(super) binary_digest: String,
    pub(super) closure_digest: String,
    pub(super) host: String,
    pub(super) target: String,
    pub(super) toolchain: String,
}

/// Toolchain-owned bytes that can affect a direct-Rustc compilation independently of package provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OvenDirectRustcCompilerEvidence {
    pub(super) binary_digest: String,
    pub(super) closure_digest: String,
    pub(super) host: String,
    pub(super) target: String,
    pub(super) sysroot: PathBuf,
    pub(super) members: Vec<OvenDirectRustcCompilerMember>,
}

/// One physical compiler member paired with its location-independent sysroot coordinate and byte identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OvenDirectRustcCompilerMember {
    pub(super) relative_path: String,
    pub(super) source_path: PathBuf,
    pub(super) digest: String,
}

/// Store-owned compiler closure retained under one lease for an entire native provider batch.
pub(crate) struct OvenOwnedDirectRustcCompiler {
    pub(super) _owner: OvenStoreExecutionPayload,
    pub(super) evidence: OvenDirectRustcCompilerEvidence,
    pub(super) rustc: PathBuf,
}

/// Outcome of trying to retain the selected compiler closure for JEC.
///
/// Unavailability is deliberately distinct from a hard receipt or intent mismatch: callers may keep compiling
/// deterministically through their admitted compiler, but must surface why byte-identical reuse was disabled.
///
/// `Retained` is boxed because it carries the whole owned compiler, several hundred bytes against `Unavailable`'s
/// single string. The unavailable arm is the common one on a cold store, and every caller moves the value.
pub(crate) enum OvenDirectRustcCompilerRetention {
    Retained(Box<OvenOwnedDirectRustcCompiler>),
    Unavailable { reason: String },
}

impl OvenDirectRustcCompilerEvidence {
    /// Content digest of the `rustc` binary itself, separate from the closure around it.
    pub(crate) fn binary_digest(&self) -> &str {
        &self.binary_digest
    }

    /// Digest over every member of the compiler closure, which is what makes two installations comparable.
    pub(crate) fn closure_digest(&self) -> &str {
        &self.closure_digest
    }

    /// Triple the retained compiler runs on, as opposed to the one it emits for.
    pub(crate) fn host(&self) -> &str {
        &self.host
    }

    /// Triple the retained compiler emits for, as opposed to the one it runs on.
    pub(crate) fn target(&self) -> &str {
        &self.target
    }
}

impl OvenOwnedDirectRustcCompiler {
    /// Path to the store-owned `rustc`, valid only while this closure holds its lease.
    pub(crate) fn rustc(&self) -> &Path {
        &self.rustc
    }

    /// Borrow the admitted identity of this retained closure, without exposing the lease that holds it.
    pub(crate) fn evidence(&self) -> &OvenDirectRustcCompilerEvidence {
        &self.evidence
    }

    /// Toolchain identity the closure was admitted under, as the receipt records it.
    pub(crate) fn toolchain(&self) -> &str {
        &self._owner.manifest.intent.toolchain
    }
}

/// Logical names paired with the exact physical members of one prepared invocation.
///
/// The admitted native/source-unit plan decides membership. This binding gives the native-compilation policy stable
/// names for those already selected paths; it cannot add an extern, search directory or source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OvenDirectRustcJecBindings {
    pub(crate) logical_source_root: String,
    pub(crate) logical_entrypoint: String,
    /// Logical native-member slot for every prepared `--extern`, in exact argument order.
    pub(crate) externs: Vec<String>,
    /// Logical search slot for every prepared `-L dependency`, in exact argument order.
    pub(crate) dependency_searches: Vec<String>,
    /// Logical search slot for every prepared `-L native`, in exact argument order.
    pub(crate) native_searches: Vec<String>,
}

/// A fully observed direct-Rustc library invocation before output lookup or compiler execution.
///
/// This owns the source-specific physical plan so the Incan projection and Rustc consume one preparation. Package
/// name/version and Store coordinates remain outside this value; callers retain them only in request bindings.
pub(crate) struct OvenPreparedDirectRustcLibrary {
    pub(super) rustc: PathBuf,
    pub(super) compiler: Option<OvenDirectRustcCompilerEvidence>,
    /// Lease-protected Store owner for `compiler`; production JEC preparations retain it instead of rereading its
    /// immutable bytes before every provider unit.
    pub(super) compiler_owner: Option<Arc<OvenOwnedDirectRustcCompiler>>,
    pub(super) source: PathBuf,
    pub(super) source_root: PathBuf,
    pub(super) source_digest: String,
    pub(super) artifact_root: PathBuf,
    pub(super) selected_artifacts: OvenRustcArtifactManifest,
    pub(super) plan: OvenRustcArtifactPlan,
    pub(super) target: String,
    pub(super) profile: String,
    pub(super) crate_name: String,
    pub(super) edition: String,
    pub(super) features: Vec<String>,
    /// Complete deterministic environment derived from the admitted native-plan values.
    ///
    /// Values remain process-local. Callers expose only their digests to the Incan projection.
    pub(super) frozen_environment: Option<BTreeMap<String, PathBuf>>,
}

/// One environment name observed by Rustc, with a digest only when it was present in the frozen environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OvenRustcObservedEnvironment {
    pub(crate) name: String,
    pub(crate) value_digest: Option<String>,
}

/// Parsed Rustc dependency observations. Raw dep-info bytes and environment values never cross this boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OvenRustcDepInfoObservation {
    pub(crate) files: Vec<PathBuf>,
    pub(crate) environment: Vec<OvenRustcObservedEnvironment>,
}

/// Sanitized observation result. An unsupported dep-info shape disables reuse without failing a successful compile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OvenRustcDepInfoOutcome {
    Observed(OvenRustcDepInfoObservation),
    Unavailable { reason: String },
}

/// Fresh direct-Rustc output plus the sanitized observations needed for an Incan-owned compilation key.
pub(crate) struct OvenDirectRustcJecCompilation {
    pub(crate) bake: OvenDirectRustcBake,
    pub(crate) observation: OvenRustcDepInfoOutcome,
}

/// One immutable physical invocation, including its exact output path, argv and cwd.
///
/// Cacheable execution uses the prepared frozen environment. An explicitly uncacheable fallback retains the legacy
/// direct-Rustc inheritance contract.
pub(crate) struct OvenBoundDirectRustcLibrary<'prepared> {
    pub(super) prepared: &'prepared OvenPreparedDirectRustcLibrary,
    pub(super) output: PathBuf,
    pub(super) arguments: Vec<OsString>,
    pub(super) logical_arguments: Vec<Vec<String>>,
    pub(super) path_effects_digest: Option<String>,
}

/// Prepare one provider-owned Rust library from the exact admitted source role and physical native plan.
///
/// The result is deliberately output-agnostic: Incan first projects its candidate identity, after which the caller
/// chooses the corresponding candidate directory for either retained-output validation or a fresh compile.
pub(crate) fn prepare_trusted_direct_rustc_library_with_artifact_role(
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
    pub(crate) fn rustc(&self) -> &Path {
        &self.rustc
    }

    /// Admitted compiler identity, absent when this preparation runs without a retained closure.
    pub(crate) fn compiler(&self) -> Option<&OvenDirectRustcCompilerEvidence> {
        self.compiler.as_ref()
    }

    /// Root module this library compiles, already resolved and digest-checked.
    pub(crate) fn source(&self) -> &Path {
        &self.source
    }

    /// Admitted source tree the root module sits in; nothing outside it may reach the compiler.
    pub(crate) fn source_root(&self) -> &Path {
        &self.source_root
    }

    /// Digest of that source tree as it was admitted, so a later step can prove it did not move underneath.
    pub(crate) fn source_digest(&self) -> &str {
        &self.source_digest
    }

    /// Sealed artifact manifest this preparation links against.
    pub(crate) fn selected_artifacts(&self) -> &OvenRustcArtifactManifest {
        &self.selected_artifacts
    }

    /// Store-owned root the sealed artifacts were materialized under.
    pub(crate) fn artifact_root(&self) -> &Path {
        &self.artifact_root
    }

    /// Verified compiler inputs — search paths, externs and environment — from the sealed plan.
    pub(crate) fn artifact_plan(&self) -> &OvenRustcArtifactPlan {
        &self.plan
    }

    /// Target triple from the receipt this preparation was bound to.
    pub(crate) fn target(&self) -> &str {
        &self.target
    }

    /// Named build profile, which decides the codegen flags rather than carrying them.
    pub(crate) fn profile(&self) -> &str {
        &self.profile
    }

    /// Crate name the compiler is told, which is a declared fact rather than one inferred from a path.
    pub(crate) fn crate_name(&self) -> &str {
        &self.crate_name
    }

    /// Rust edition this library is compiled under.
    pub(crate) fn edition(&self) -> &str {
        &self.edition
    }

    /// Resolved feature set, already unified; this is not the declared request.
    pub(crate) fn features(&self) -> &[String] {
        &self.features
    }

    /// Return the complete frozen environment for digest-only host projection.
    pub(crate) fn environment(&self) -> Option<&BTreeMap<String, PathBuf>> {
        self.frozen_environment.as_ref()
    }

    /// Freeze the complete typed projection and the exact physical argv for one output location.
    pub(crate) fn bind<'prepared>(
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
    pub(crate) fn output(&self) -> &Path {
        &self.output
    }

    /// Compiler invocations as location-independent argument vectors, for comparison across machines.
    pub(crate) fn logical_arguments(&self) -> &[Vec<String>] {
        &self.logical_arguments
    }

    /// Digest of the observed path effects, absent when the compilation ran without observation.
    pub(crate) fn path_effects_digest(&self) -> Option<&str> {
        self.path_effects_digest.as_deref()
    }

    /// Rehash one retained output after the current invocation and Incan key have both been admitted.
    pub(crate) fn reuse_from(
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
    pub(crate) fn compile(&self) -> Result<OvenDirectRustcJecCompilation, OvenRustcError> {
        self.compile_with_observation(true)
    }

    /// Execute an explicitly uncacheable invocation under the same compiler environment as a cacheable one.
    ///
    /// Cache availability cannot select program semantics. This path suppresses reusable observations, but it still
    /// clears ambient state and applies only the admitted plan values used by [`Self::compile`].
    pub(crate) fn compile_uncacheable(&self) -> Result<OvenDirectRustcJecCompilation, OvenRustcError> {
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
pub(super) fn apply_direct_rustc_execution_environment(
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
pub(super) fn joined_os_argument(prefix: &str, path: &Path, suffix: Option<&str>) -> OsString {
    let mut value = OsString::from(prefix);
    value.push(path.as_os_str());
    if let Some(suffix) = suffix {
        value.push("=");
        value.push(suffix);
    }
    value
}

/// Parse Rustc's Makefile dep-info and discard every raw environment value before returning.
pub(super) fn parse_rustc_dep_info(
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
pub(super) fn parse_makefile_words(value: &str) -> Result<Vec<String>, OvenRustcError> {
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
pub(super) fn freeze_direct_rustc_environment(
    _inherited: impl IntoIterator<Item = (OsString, OsString)>,
    compile_environment: &BTreeMap<String, PathBuf>,
) -> Option<BTreeMap<String, PathBuf>> {
    Some(compile_environment.clone())
}

/// Hash the selected compiler and the bounded sysroot closure that can affect one direct `rlib` compilation.
///
/// Package names, versions and installation locations are deliberately absent. The projection contains the invoked
/// compiler bytes, its actual sysroot compiler when distinct, the driver/LLVM libraries beside that compiler, and
/// the host/target Rust libraries selected by the invocation. Other installed targets are not members.
pub(crate) fn direct_rustc_compiler_evidence(
    rustc: &Path,
    target: &str,
) -> Result<OvenDirectRustcCompilerEvidence, OvenRustcError> {
    const MAX_MEMBERS: usize = 8_192;
    const MAX_BYTES: u64 = 16 * 1024 * 1024 * 1024;

    if target.trim().is_empty() {
        return Err(OvenRustcError::InvalidInput {
            field: "JEC compiler target",
            message: "must not be empty".to_string(),
        });
    }
    let rustc = fs::canonicalize(verified_regular_file(rustc, "rustc")?).map_err(|source| OvenRustcError::Io {
        path: rustc.to_path_buf(),
        source,
    })?;
    let host = rustc_host_target(&rustc)?;
    let sysroot = fs::canonicalize(rustc_sysroot(&rustc)?).map_err(|source| OvenRustcError::Io {
        path: rustc.clone(),
        source,
    })?;
    let binary_digest = digest_regular_file(&rustc, "rustc")?;
    let sysroot_rustc =
        fs::canonicalize(verified_regular_file(&sysroot.join("bin/rustc"), "sysroot rustc")?).map_err(|source| {
            OvenRustcError::Io {
                path: sysroot.join("bin/rustc"),
                source,
            }
        })?;
    if sysroot_rustc != rustc {
        return Err(OvenRustcError::InvalidInput {
            field: "JEC compiler closure",
            message: "requires the invoked compiler to be the selected sysroot's own bin/rustc".to_string(),
        });
    }
    let mut members = BTreeMap::from([("bin/rustc".to_string(), (rustc.clone(), binary_digest.clone()))]);
    let mut total_bytes = fs::metadata(&rustc)
        .map_err(|source| OvenRustcError::Io {
            path: rustc.clone(),
            source,
        })?
        .len();

    collect_compiler_closure_directory(
        &sysroot.join("lib"),
        "lib",
        false,
        &mut members,
        &mut total_bytes,
        MAX_MEMBERS,
        MAX_BYTES,
    )?;
    let mut targets = BTreeSet::from([host.clone(), target.to_string()]);
    for selected in std::mem::take(&mut targets) {
        let root = sysroot.join("lib/rustlib").join(&selected);
        for child in ["lib", "codegen-backends"] {
            let path = root.join(child);
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                    collect_compiler_closure_directory(
                        &path,
                        &format!("lib/rustlib/{selected}/{child}"),
                        true,
                        &mut members,
                        &mut total_bytes,
                        MAX_MEMBERS,
                        MAX_BYTES,
                    )?;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound && child == "codegen-backends" => {}
                Err(error) => {
                    return Err(OvenRustcError::Io { path, source: error });
                }
                _ => {
                    return Err(OvenRustcError::InvalidInput {
                        field: "JEC compiler closure",
                        message: format!("{} is not a non-symlink directory", path.display()),
                    });
                }
            }
        }
    }
    let member_digests = members
        .iter()
        .map(|(path, (_, digest))| (path.clone(), digest.clone()))
        .collect::<BTreeMap<_, _>>();
    let closure_digest = direct_rustc_compiler_closure_digest(&member_digests)?;
    let members = members
        .into_iter()
        .map(|(relative_path, (source_path, digest))| OvenDirectRustcCompilerMember {
            relative_path,
            source_path,
            digest,
        })
        .collect();
    Ok(OvenDirectRustcCompilerEvidence {
        binary_digest,
        closure_digest,
        host,
        target: target.to_string(),
        sysroot,
        members,
    })
}

/// Digest one compiler closure from its members' logical coordinates and byte identities.
///
/// The schema tag is folded in so a later change to what a closure contains cannot silently collide with an
/// identity minted under the old shape.
pub(super) fn direct_rustc_compiler_closure_digest(
    members: &BTreeMap<String, String>,
) -> Result<String, OvenRustcError> {
    let material = serde_json::to_vec(&("incan.oven.rustc-rlib-closure/1", members)).map_err(|error| {
        OvenRustcError::InvalidInput {
            field: "JEC compiler closure",
            message: format!("cannot encode compiler-member identities: {error}"),
        }
    })?;
    Ok(digest_bytes(&material))
}

/// Derive the store compatibility domain one compiler closure is published into.
///
/// Keying the domain on the closure digest is what keeps two installations of the same toolchain version from
/// sharing an entry when their closures actually differ.
pub(super) fn direct_rustc_compiler_domain(closure_digest: &str) -> Result<String, OvenRustcError> {
    let Some(hex) = closure_digest.strip_prefix("sha256:") else {
        return Err(OvenRustcError::InvalidInput {
            field: "JEC compiler closure",
            message: "has no SHA-256 identity prefix".to_string(),
        });
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(OvenRustcError::InvalidInput {
            field: "JEC compiler closure",
            message: "has a malformed SHA-256 identity".to_string(),
        });
    }
    Ok(format!("{OVEN_DIRECT_RUSTC_COMPILER_DOMAIN_PREFIX}.{hex}"))
}

/// Select or materialize one compiler closure in the immutable Store and retain its lease for native execution.
///
/// Discovery, capacity or Store availability failures disable JEC and leave ordinary deterministic direct-Rustc
/// compilation available. A single compatible Store closure is verified and preferred before the ambient compiler is
/// executed or its sysroot is hashed. Multiple byte-distinct closures with the same target/toolchain identity force a
/// cold observation so the ambient compiler bytes disambiguate them.
///
/// Once selected, that verified Store closure is Oven's compiler authority for the batch. The caller's mutable Rustc
/// path supplies identity and bytes only for a cold or ambiguous selection; it is never executed in place when a
/// unique warm owner can be admitted.
pub(crate) fn retain_direct_rustc_compiler(
    store: &OvenStore,
    receipt: &OvenReceipt,
    candidate_rustc: &Path,
    target: &str,
) -> Result<OvenDirectRustcCompilerRetention, OvenRustcError> {
    receipt
        .verify_identity()
        .map_err(|error| OvenRustcError::InvalidInput {
            field: "JEC compiler receipt",
            message: error.to_string(),
        })?;
    if receipt.intent.target != target {
        return Err(OvenRustcError::IntentMismatch);
    }
    let toolchain = receipt.intent.toolchain.clone();
    let mut selected = match store.select_payloads_matching_for_execution(|manifest| {
        manifest.kind == OvenArtifactKind::NativeCompilerClosure
            && manifest.intent.target == target
            && manifest.intent.toolchain == toolchain
    }) {
        Ok(mut selected) => {
            selected.sort_by(|left, right| left.manifest.identity.cmp(&right.manifest.identity));
            selected
        }
        Err(error) => {
            return Ok(OvenDirectRustcCompilerRetention::Unavailable {
                reason: format!("cannot inspect retained compiler closures: {error}"),
            });
        }
    };
    let compatible = selected
        .iter()
        .enumerate()
        .filter_map(|(index, owner)| {
            matching_direct_rustc_compiler_owner_payload(owner, target, &toolchain, None)
                .map(|payload| (index, payload))
        })
        .collect::<Vec<_>>();
    let closure_identities = compatible
        .iter()
        .map(|(_, payload)| {
            (
                payload.closure_digest.as_str(),
                payload.binary_digest.as_str(),
                payload.host.as_str(),
            )
        })
        .collect::<BTreeSet<_>>();
    if closure_identities.len() == 1
        && let Some((index, payload)) = compatible.first()
    {
        let host = payload.host.clone();
        let owner = selected.remove(*index);
        if let Some(owned) = admit_direct_rustc_compiler_owner(owner, target, &toolchain, &host, None)? {
            return Ok(OvenDirectRustcCompilerRetention::Retained(Box::new(owned)));
        }
    }

    let candidate_toolchain = rustc_identity(candidate_rustc)?;
    if toolchain != candidate_toolchain {
        return Err(OvenRustcError::ToolchainMismatch {
            expected: toolchain,
            actual: candidate_toolchain,
        });
    }
    let host = rustc_host_target(candidate_rustc)?;
    let evidence = match direct_rustc_compiler_evidence(candidate_rustc, target) {
        Ok(evidence) => evidence,
        Err(error) => {
            return Ok(OvenDirectRustcCompilerRetention::Unavailable {
                reason: format!("cannot admit the selected compiler closure: {error}"),
            });
        }
    };
    if evidence.host != host {
        return Ok(OvenDirectRustcCompilerRetention::Unavailable {
            reason: "the selected compiler changed its reported host during closure observation".to_string(),
        });
    }
    let payload = OvenDirectRustcCompilerOwnerPayload {
        schema_version: OVEN_DIRECT_RUSTC_COMPILER_OWNER_SCHEMA_VERSION,
        binary_digest: evidence.binary_digest.clone(),
        closure_digest: evidence.closure_digest.clone(),
        host: evidence.host.clone(),
        target: evidence.target.clone(),
        toolchain: receipt.intent.toolchain.clone(),
    };
    let domain = direct_rustc_compiler_domain(&evidence.closure_digest)?;
    if let Some(index) = selected.iter().position(|owner| {
        matching_direct_rustc_compiler_owner_payload(owner, target, &toolchain, Some(&host)).as_ref() == Some(&payload)
    }) {
        let owner = selected.remove(index);
        if let Some(owned) = admit_direct_rustc_compiler_owner(owner, target, &toolchain, &host, Some(&evidence))? {
            return Ok(OvenDirectRustcCompilerRetention::Retained(Box::new(owned)));
        }
    }

    let encoded = serde_json::to_vec(&payload).map_err(|error| OvenRustcError::InvalidInput {
        field: "JEC compiler owner",
        message: format!("cannot encode compiler closure metadata: {error}"),
    })?;
    let materialized_files = evidence
        .members
        .iter()
        .map(|member| OvenArtifactMaterializedFile {
            source_path: member.source_path.clone(),
            relative_path: member.relative_path.clone(),
        })
        .collect();
    let manifest = match store.publish(&OvenArtifactPublishRequest {
        receipt: receipt.clone(),
        domain: domain.clone(),
        kind: OvenArtifactKind::NativeCompilerClosure,
        payload: encoded,
        materialized_files,
    }) {
        Ok(manifest) => manifest,
        Err(error) => {
            return Ok(OvenDirectRustcCompilerRetention::Unavailable {
                reason: format!("cannot publish the selected compiler closure: {error}"),
            });
        }
    };
    let mut selected = match store.select_payloads_for_execution(std::slice::from_ref(&manifest.identity)) {
        Ok(selected) => selected,
        Err(error) => {
            return Ok(OvenDirectRustcCompilerRetention::Unavailable {
                reason: format!("cannot retain the published compiler closure: {error}"),
            });
        }
    };
    let Some(owner) = selected.pop() else {
        return Ok(OvenDirectRustcCompilerRetention::Unavailable {
            reason: "the published compiler closure has no execution owner".to_string(),
        });
    };
    match admit_direct_rustc_compiler_owner(owner, target, &toolchain, &host, Some(&evidence))? {
        Some(owned) => Ok(OvenDirectRustcCompilerRetention::Retained(Box::new(owned))),
        None => Ok(OvenDirectRustcCompilerRetention::Unavailable {
            reason: "the published compiler closure failed final admission".to_string(),
        }),
    }
}

/// Decode the small owner descriptor used to select a warm compiler without reading its materialized closure.
pub(super) fn matching_direct_rustc_compiler_owner_payload(
    owner: &OvenStoreExecutionPayload,
    target: &str,
    toolchain: &str,
    host: Option<&str>,
) -> Option<OvenDirectRustcCompilerOwnerPayload> {
    if owner.manifest.kind != OvenArtifactKind::NativeCompilerClosure
        || owner.manifest.intent.target != target
        || owner.manifest.intent.toolchain != toolchain
    {
        return None;
    }
    let payload = serde_json::from_slice::<OvenDirectRustcCompilerOwnerPayload>(&owner.payload).ok()?;
    if payload.schema_version != OVEN_DIRECT_RUSTC_COMPILER_OWNER_SCHEMA_VERSION
        || payload.target != target
        || payload.toolchain != toolchain
        || host.is_some_and(|host| payload.host != host)
        || direct_rustc_compiler_domain(&payload.closure_digest).ok().as_deref() != Some(owner.manifest.domain.as_str())
    {
        return None;
    }
    Some(payload)
}

/// Admit one retained store entry as this batch's compiler, or say why it cannot serve.
///
/// The entry's recorded host, target and toolchain must match the request, and where the caller already holds
/// expected evidence it must match that too. A mismatch is reported as unavailability rather than as an error:
/// the caller can still compile through its own admitted compiler, it just loses byte-identical reuse.
pub(super) fn admit_direct_rustc_compiler_owner(
    owner: OvenStoreExecutionPayload,
    target: &str,
    toolchain: &str,
    host: &str,
    expected: Option<&OvenDirectRustcCompilerEvidence>,
) -> Result<Option<OvenOwnedDirectRustcCompiler>, OvenRustcError> {
    let Some(payload) = matching_direct_rustc_compiler_owner_payload(&owner, target, toolchain, Some(host)) else {
        return Ok(None);
    };
    if expected.is_some_and(|expected| {
        payload.binary_digest != expected.binary_digest
            || payload.closure_digest != expected.closure_digest
            || payload.host != expected.host
            || payload.target != expected.target
    }) {
        return Ok(None);
    }
    if owner.verify_materialized_files().is_err() {
        return Ok(None);
    }
    let mut member_digests = BTreeMap::new();
    let mut members = Vec::with_capacity(owner.manifest.materialized_files.len());
    let mut rustc = None;
    for member in &owner.manifest.materialized_files {
        if member_digests
            .insert(member.relative_path.clone(), member.digest.clone())
            .is_some()
        {
            return Ok(None);
        }
        let source_path = owner.artifact_root.join(&member.relative_path);
        if member.relative_path == "bin/rustc" {
            if !member.executable || member.digest != payload.binary_digest {
                return Ok(None);
            }
            rustc = Some(source_path.clone());
        }
        members.push(OvenDirectRustcCompilerMember {
            relative_path: member.relative_path.clone(),
            source_path,
            digest: member.digest.clone(),
        });
    }
    if direct_rustc_compiler_closure_digest(&member_digests)? != payload.closure_digest {
        return Ok(None);
    }
    let Some(rustc) = rustc else {
        return Ok(None);
    };
    let sysroot = match fs::canonicalize(&owner.artifact_root) {
        Ok(sysroot) => sysroot,
        Err(_) => return Ok(None),
    };
    if fs::canonicalize(rustc_sysroot(&rustc)?).ok().as_deref() != Some(sysroot.as_path())
        || rustc_host_target(&rustc)? != payload.host
        || rustc_identity(&rustc)? != payload.toolchain
    {
        return Ok(None);
    }
    Ok(Some(OvenOwnedDirectRustcCompiler {
        _owner: owner,
        evidence: OvenDirectRustcCompilerEvidence {
            binary_digest: payload.binary_digest.clone(),
            closure_digest: payload.closure_digest.clone(),
            host: payload.host.clone(),
            target: payload.target.clone(),
            sysroot,
            members,
        },
        rustc,
    }))
}

#[allow(clippy::too_many_arguments)]
/// Walk one directory of the compiler installation into logical members, bounded by count and bytes.
///
/// The bounds are the point: this reads a directory the caller named, and an unbounded walk of a sysroot would
/// admit an arbitrary amount of material into an identity. `logical_root` keeps each member's coordinate
/// independent of where the installation happens to live.
pub(super) fn collect_compiler_closure_directory(
    root: &Path,
    logical_root: &str,
    recursive: bool,
    members: &mut BTreeMap<String, (PathBuf, String)>,
    total_bytes: &mut u64,
    max_members: usize,
    max_bytes: u64,
) -> Result<(), OvenRustcError> {
    let mut entries = fs::read_dir(root)
        .map_err(|source| OvenRustcError::Io {
            path: root.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| OvenRustcError::Io {
            path: root.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| OvenRustcError::InvalidInput {
                field: "JEC compiler closure",
                message: "contains a non-UTF-8 member name".to_string(),
            })?;
        let logical = format!("{logical_root}/{name}");
        let metadata = fs::symlink_metadata(&path).map_err(|source| OvenRustcError::Io {
            path: path.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() {
            return Err(OvenRustcError::InvalidInput {
                field: "JEC compiler closure",
                message: format!("contains a symlink at {logical}"),
            });
        }
        if metadata.is_file() {
            collect_compiler_closure_file(&path, logical, members, total_bytes, max_members, max_bytes)?;
        } else if metadata.is_dir() && recursive {
            collect_compiler_closure_directory(&path, &logical, true, members, total_bytes, max_members, max_bytes)?;
        }
    }
    Ok(())
}

/// Admit one compiler-closure file under its logical coordinate, enforcing the member and byte bounds.
pub(super) fn collect_compiler_closure_file(
    path: &Path,
    logical: String,
    members: &mut BTreeMap<String, (PathBuf, String)>,
    total_bytes: &mut u64,
    max_members: usize,
    max_bytes: u64,
) -> Result<(), OvenRustcError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| OvenRustcError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(OvenRustcError::InvalidArtifactPath {
            kind: "JEC compiler closure",
            path: path.to_path_buf(),
            message: "must contain only non-symlink regular files".to_string(),
        });
    }
    *total_bytes = total_bytes
        .checked_add(metadata.len())
        .ok_or_else(|| OvenRustcError::InvalidInput {
            field: "JEC compiler closure",
            message: "byte count overflowed".to_string(),
        })?;
    if members.len() >= max_members || *total_bytes > max_bytes {
        return Err(OvenRustcError::InvalidInput {
            field: "JEC compiler closure",
            message: "exceeds its bounded member or byte budget".to_string(),
        });
    }
    let digest = digest_regular_file(path, "JEC compiler closure member")?;
    if members.insert(logical, (path.to_path_buf(), digest)).is_some() {
        return Err(OvenRustcError::InvalidInput {
            field: "JEC compiler closure",
            message: "contains a duplicate logical member".to_string(),
        });
    }
    Ok(())
}
