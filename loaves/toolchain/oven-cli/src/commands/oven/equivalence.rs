//! Literal raw-byte equivalence between Cargo harvest and Cargo-free Oven publisher captures.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use oven_model::digest::digest_bytes;
use oven_model::manifest::is_sha256_identity;
use serde::{Deserialize, Serialize};

use super::{CliError, CliResult, ExitCode};

/// Stable schema version of capture manifests and emitted attestations.
const EQUIVALENCE_SCHEMA: u64 = 1;

/// Allowed logical roles for metadata-reachable capture products.
const ARTIFACT_ROLES: &[&str] = &[
    "dylib",
    "executable",
    "generated-input",
    "proc-macro",
    "rlib",
    "rmeta",
    "static-archive",
];

/// One exact executable identity in the deterministic capture setup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecutableIdentity {
    version: String,
    digest: String,
}

/// Recorded Cargo, rustc, platform, and profile setup shared by both captures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CaptureSetup {
    cargo: ExecutableIdentity,
    rustc: ExecutableIdentity,
    host: String,
    target: String,
    profile: String,
}

/// One path-named digest-bearing `out` identity input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PathDigest {
    path: String,
    digest: String,
}

/// One name-bearing native or tool identity input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NameDigest {
    name: String,
    digest: String,
}

/// One dependency identity entering a captured unit identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DependencyIdentity {
    name: String,
    unit_identity: String,
}

/// Complete negative identity inputs for one captured unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct IdentityInputs {
    cfg: Vec<String>,
    out: Vec<PathDigest>,
    native: Vec<NameDigest>,
    tools: Vec<NameDigest>,
    dependencies: Vec<DependencyIdentity>,
}

/// One declared metadata-reachable product in a capture manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArtifactManifest {
    role: String,
    path: String,
    digest: String,
}

/// One Oven asset archive declared beside a captured unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchiveManifest {
    path: String,
    digest: String,
}

/// One binding and every identity input and product its metadata reaches.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct UnitManifest {
    binding: String,
    package: String,
    version: String,
    unit_identity: String,
    features: Vec<String>,
    source_digest: String,
    manifest_digest: String,
    identity_inputs: IdentityInputs,
    artifacts: Vec<ArtifactManifest>,
    #[serde(default)]
    asset_archive: Option<ArchiveManifest>,
}

/// One complete Cargo harvest or Oven publisher capture manifest.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CaptureManifest {
    schema: u64,
    producer: String,
    cargo_free: bool,
    artifact_root: String,
    setup: CaptureSetup,
    units: Vec<UnitManifest>,
}

/// One verified metadata-reachable product and its raw-byte digest.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CapturedArtifact {
    digest: String,
}

/// One validated binding, its identity facts, and its raw products.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CapturedUnit {
    package: String,
    version: String,
    unit_identity: String,
    features: Vec<String>,
    source_digest: String,
    manifest_digest: String,
    identity_inputs: IdentityInputs,
    artifacts: BTreeMap<(String, String), CapturedArtifact>,
    archive_digest: Option<String>,
}

/// One fully validated capture.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Capture {
    producer: String,
    cargo_free: bool,
    setup: CaptureSetup,
    units: BTreeMap<String, CapturedUnit>,
}

/// Producer summary retained in attestation evidence.
#[derive(Debug, Serialize)]
struct EvidenceCapture {
    producer: String,
    cargo_free: bool,
}

/// Both producer summaries retained in attestation evidence.
#[derive(Debug, Serialize)]
struct EvidenceCaptures {
    cargo: EvidenceCapture,
    oven: EvidenceCapture,
}

/// One literally compared artifact in attestation evidence.
#[derive(Debug, Serialize)]
struct EvidenceArtifact {
    role: String,
    path: String,
    cargo_digest: String,
    oven_digest: String,
}

/// One compared binding in attestation evidence.
#[derive(Debug, Serialize)]
struct EvidenceBinding {
    binding: String,
    package: String,
    version: String,
    unit_identity: String,
    archive_digest: String,
    features: Vec<String>,
    source_digest: String,
    manifest_digest: String,
    identity_inputs: IdentityInputs,
    artifacts: Vec<EvidenceArtifact>,
}

/// Schema-1 RFC 125 attestation evidence emitted only after complete comparison.
#[derive(Debug, Serialize)]
struct EquivalenceEvidence {
    schema: u64,
    event: &'static str,
    status: &'static str,
    captures: EvidenceCaptures,
    setup: CaptureSetup,
    bindings: Vec<EvidenceBinding>,
    #[serde(skip_serializing_if = "Option::is_none")]
    evidence_digest: Option<String>,
}

/// Compare Cargo and Oven captures literally and atomically publish attestation evidence.
pub fn oven_equivalence(cargo_manifest: &Path, oven_manifest: &Path, output: &Path) -> CliResult<ExitCode> {
    let cargo = load_capture(cargo_manifest, false).map_err(equivalence_error)?;
    let oven = load_capture(oven_manifest, true).map_err(equivalence_error)?;
    let mut evidence = compare_captures(cargo, oven).map_err(equivalence_error)?;
    let digest_value = serde_json::to_value(&evidence)
        .map_err(|error| CliError::failure(format!("oven artifact equivalence: could not encode evidence: {error}")))?;
    let digest_bytes_value = serde_json::to_vec(&digest_value)
        .map_err(|error| CliError::failure(format!("oven artifact equivalence: could not encode evidence: {error}")))?;
    evidence.evidence_digest = Some(digest_bytes(&digest_bytes_value));
    write_evidence(output, &evidence)?;
    println!(
        "oven artifact equivalence: attested {} binding(s) -> {}",
        evidence.bindings.len(),
        output.display()
    );
    Ok(ExitCode::SUCCESS)
}

/// Load and validate one complete capture manifest and all declared bytes.
fn load_capture(path: &Path, oven: bool) -> Result<Capture, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("could not read capture manifest {}: {error}", path.display()))?;
    let raw: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("could not read capture manifest {}: {error}", path.display()))?;
    if !oven
        && raw
            .get("units")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|units| units.iter().any(|unit| unit.get("asset_archive").is_some()))
    {
        return Err(format!("{} Cargo units must not contain asset_archive", path.display()));
    }
    let manifest: CaptureManifest = serde_json::from_slice(&bytes)
        .map_err(|error| format!("could not read capture manifest {}: {error}", path.display()))?;
    if manifest.schema != EQUIVALENCE_SCHEMA {
        return Err(format!(
            "{} uses unsupported schema {}",
            path.display(),
            manifest.schema
        ));
    }
    let expected_producer = if oven { "oven-publisher" } else { "cargo-harvest" };
    if manifest.producer != expected_producer {
        return Err(format!("{} producer must be {expected_producer}", path.display()));
    }
    if manifest.cargo_free != oven {
        return Err(format!("{} cargo_free must be {oven}", path.display()));
    }
    validate_setup(&manifest.setup, &format!("{}.setup", path.display()))?;
    let artifact_root = safe_relative_path(&manifest.artifact_root, &format!("{}.artifact_root", path.display()))?;
    let root = path.parent().unwrap_or_else(|| Path::new(".")).join(artifact_root);
    if !root.is_dir() {
        return Err(format!(
            "{} artifact root is not a directory: {}",
            path.display(),
            manifest.artifact_root
        ));
    }
    if manifest.units.is_empty() {
        return Err(format!("{}.units must be a nonempty list", path.display()));
    }
    let mut units = BTreeMap::new();
    let mut prior_binding: Option<&str> = None;
    for (index, unit) in manifest.units.iter().enumerate() {
        let field = format!("{}.units[{index}]", path.display());
        if prior_binding.is_some_and(|prior| prior >= unit.binding.as_str()) {
            return Err(format!(
                "{}.units must be sorted and duplicate-free by binding",
                path.display()
            ));
        }
        prior_binding = Some(&unit.binding);
        let captured = capture_unit(&root, unit, &field, oven)?;
        if units.insert(unit.binding.clone(), captured).is_some() {
            return Err(format!("{}.units repeats binding {}", path.display(), unit.binding));
        }
    }
    Ok(Capture {
        producer: manifest.producer,
        cargo_free: manifest.cargo_free,
        setup: manifest.setup,
        units,
    })
}

/// Validate one capture's deterministic setup identities.
fn validate_setup(setup: &CaptureSetup, field: &str) -> Result<(), String> {
    for (name, identity) in [("cargo", &setup.cargo), ("rustc", &setup.rustc)] {
        if identity.version.is_empty() {
            return Err(format!("{field}.{name}.version must be a nonempty string"));
        }
        validate_digest(&identity.digest, &format!("{field}.{name}.digest"))?;
    }
    for (name, value) in [
        ("host", setup.host.as_str()),
        ("target", setup.target.as_str()),
        ("profile", setup.profile.as_str()),
    ] {
        if value.is_empty() {
            return Err(format!("{field}.{name} must be a nonempty string"));
        }
    }
    Ok(())
}

/// Validate one binding and hash every product its metadata reaches.
fn capture_unit(root: &Path, unit: &UnitManifest, field: &str, oven: bool) -> Result<CapturedUnit, String> {
    for (name, value) in [
        ("binding", unit.binding.as_str()),
        ("package", unit.package.as_str()),
        ("version", unit.version.as_str()),
    ] {
        if value.is_empty() {
            return Err(format!("{field}.{name} must be a nonempty string"));
        }
    }
    validate_digest(&unit.unit_identity, &format!("{field}.unit_identity"))?;
    validate_digest(&unit.source_digest, &format!("{field}.source_digest"))?;
    validate_digest(&unit.manifest_digest, &format!("{field}.manifest_digest"))?;
    validate_sorted_strings(&unit.features, &format!("{field}.features"))?;
    validate_identity_inputs(&unit.identity_inputs, &format!("{field}.identity_inputs"))?;
    let mut artifacts = BTreeMap::new();
    let mut prior_key: Option<(String, String)> = None;
    for (index, artifact) in unit.artifacts.iter().enumerate() {
        let artifact_field = format!("{field}.artifacts[{index}]");
        if !ARTIFACT_ROLES.contains(&artifact.role.as_str()) {
            return Err(format!("{artifact_field}.role is unsupported: {}", artifact.role));
        }
        let relative = safe_relative_path(&artifact.path, &format!("{artifact_field}.path"))?;
        validate_digest(&artifact.digest, &format!("{artifact_field}.digest"))?;
        let actual = digest_bytes(
            &fs::read(regular_file(root, &relative, &artifact_field)?)
                .map_err(|error| format!("could not read {artifact_field}: {error}"))?,
        );
        if actual != artifact.digest {
            return Err(format!(
                "{artifact_field} declared digest {}, but raw bytes have {actual}",
                artifact.digest
            ));
        }
        let key = (artifact.role.clone(), artifact.path.clone());
        if prior_key.as_ref().is_some_and(|prior| prior >= &key) {
            return Err(format!(
                "{field}.artifacts must be sorted and duplicate-free by role/path"
            ));
        }
        prior_key = Some(key.clone());
        artifacts.insert(key, CapturedArtifact { digest: actual });
    }
    let archive_digest = match (&unit.asset_archive, oven) {
        (Some(archive), true) => Some(capture_archive(root, archive, &format!("{field}.asset_archive"))?),
        (None, true) => return Err(format!("{field} must contain asset_archive")),
        (Some(_), false) => return Err(format!("{field} Cargo unit must not contain asset_archive")),
        (None, false) => None,
    };
    Ok(CapturedUnit {
        package: unit.package.clone(),
        version: unit.version.clone(),
        unit_identity: unit.unit_identity.clone(),
        features: unit.features.clone(),
        source_digest: unit.source_digest.clone(),
        manifest_digest: unit.manifest_digest.clone(),
        identity_inputs: unit.identity_inputs.clone(),
        artifacts,
        archive_digest,
    })
}

/// Validate and hash one Oven asset archive without transforming its bytes.
fn capture_archive(root: &Path, archive: &ArchiveManifest, field: &str) -> Result<String, String> {
    let relative = safe_relative_path(&archive.path, &format!("{field}.path"))?;
    validate_digest(&archive.digest, &format!("{field}.digest"))?;
    let actual = digest_bytes(
        &fs::read(regular_file(root, &relative, field)?).map_err(|error| format!("could not read {field}: {error}"))?,
    );
    if actual != archive.digest {
        return Err(format!(
            "{field} declared digest {}, but raw bytes have {actual}",
            archive.digest
        ));
    }
    Ok(actual)
}

/// Validate cfg, out, native, tool, and dependency identity inputs.
fn validate_identity_inputs(inputs: &IdentityInputs, field: &str) -> Result<(), String> {
    validate_sorted_strings(&inputs.cfg, &format!("{field}.cfg"))?;
    validate_path_digests(&inputs.out, &format!("{field}.out"))?;
    validate_name_digests(&inputs.native, &format!("{field}.native"))?;
    validate_name_digests(&inputs.tools, &format!("{field}.tools"))?;
    let mut prior: Option<&str> = None;
    for (index, dependency) in inputs.dependencies.iter().enumerate() {
        if dependency.name.is_empty() {
            return Err(format!("{field}.dependencies[{index}].name must be a nonempty string"));
        }
        validate_digest(
            &dependency.unit_identity,
            &format!("{field}.dependencies[{index}].unit_identity"),
        )?;
        if prior.is_some_and(|name| name >= dependency.name.as_str()) {
            return Err(format!(
                "{field}.dependencies must be sorted and duplicate-free by name"
            ));
        }
        prior = Some(&dependency.name);
    }
    Ok(())
}

/// Validate a sorted path-digest list.
fn validate_path_digests(entries: &[PathDigest], field: &str) -> Result<(), String> {
    let mut prior: Option<&str> = None;
    for (index, entry) in entries.iter().enumerate() {
        if entry.path.is_empty() {
            return Err(format!("{field}[{index}].path must be a nonempty string"));
        }
        validate_digest(&entry.digest, &format!("{field}[{index}].digest"))?;
        if prior.is_some_and(|previous| previous >= entry.path.as_str()) {
            return Err(format!("{field} must be sorted and duplicate-free by path"));
        }
        prior = Some(&entry.path);
    }
    Ok(())
}

/// Validate a sorted name-digest list.
fn validate_name_digests(entries: &[NameDigest], field: &str) -> Result<(), String> {
    let mut prior: Option<&str> = None;
    for (index, entry) in entries.iter().enumerate() {
        if entry.name.is_empty() {
            return Err(format!("{field}[{index}].name must be a nonempty string"));
        }
        validate_digest(&entry.digest, &format!("{field}[{index}].digest"))?;
        if prior.is_some_and(|previous| previous >= entry.name.as_str()) {
            return Err(format!("{field} must be sorted and duplicate-free by name"));
        }
        prior = Some(&entry.name);
    }
    Ok(())
}

/// Validate a sorted, duplicate-free list of nonempty strings.
fn validate_sorted_strings(values: &[String], field: &str) -> Result<(), String> {
    if values.iter().any(String::is_empty) {
        return Err(format!("{field} must be a list of nonempty strings"));
    }
    if values.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(format!("{field} must be sorted and duplicate-free"));
    }
    Ok(())
}

/// Validate one canonical lowercase SHA-256 identity.
fn validate_digest(value: &str, field: &str) -> Result<(), String> {
    if !is_sha256_identity(value) {
        return Err(format!("{field} must be a lowercase sha256: identity"));
    }
    Ok(())
}

/// Validate one normalized, nonempty POSIX-style path below an artifact root.
fn safe_relative_path(value: &str, field: &str) -> Result<PathBuf, String> {
    if value.is_empty() || value.contains('\\') {
        return Err(format!("{field} must be a nonempty relative path"));
    }
    let path = Path::new(value);
    let normalized = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    if path.is_absolute()
        || !path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
        || normalized != value
    {
        return Err(format!("{field} must be a normalized relative path"));
    }
    Ok(path.to_path_buf())
}

/// Resolve one declared product while refusing path escape, symlinks, and non-files.
fn regular_file(root: &Path, relative: &Path, field: &str) -> Result<PathBuf, String> {
    let root = fs::canonicalize(root).map_err(|error| format!("could not resolve artifact root: {error}"))?;
    let mut path = root.clone();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(format!("{field} escapes its artifact root: {}", relative.display()));
        };
        path.push(component);
        let metadata = fs::symlink_metadata(&path)
            .map_err(|_| format!("{field} is not a regular file: {}", relative.display()))?;
        if metadata.file_type().is_symlink() {
            return Err(format!("{field} traverses a symlink: {}", relative.display()));
        }
    }
    let resolved =
        fs::canonicalize(&path).map_err(|_| format!("{field} is not a regular file: {}", relative.display()))?;
    if !resolved.starts_with(&root) {
        return Err(format!("{field} escapes its artifact root: {}", relative.display()));
    }
    if !resolved.is_file() {
        return Err(format!("{field} is not a regular file: {}", relative.display()));
    }
    Ok(resolved)
}

/// Compare two validated captures and construct attestation evidence.
fn compare_captures(cargo: Capture, oven: Capture) -> Result<EquivalenceEvidence, String> {
    if cargo.setup != oven.setup {
        return Err("recorded Cargo/rustc/host/target/profile setup differs".to_string());
    }
    let cargo_bindings: BTreeSet<_> = cargo.units.keys().cloned().collect();
    let oven_bindings: BTreeSet<_> = oven.units.keys().cloned().collect();
    if cargo_bindings != oven_bindings {
        return Err(format!(
            "binding inventory differs: missing from Oven={:?}, extra in Oven={:?}",
            cargo_bindings.difference(&oven_bindings).collect::<Vec<_>>(),
            oven_bindings.difference(&cargo_bindings).collect::<Vec<_>>()
        ));
    }
    let mut bindings = Vec::new();
    for binding in cargo_bindings {
        let cargo_unit = cargo
            .units
            .get(&binding)
            .ok_or_else(|| format!("missing Cargo binding {binding}"))?;
        let oven_unit = oven
            .units
            .get(&binding)
            .ok_or_else(|| format!("missing Oven binding {binding}"))?;
        compare_unit_identity(&binding, cargo_unit, oven_unit)?;
        let cargo_artifacts: BTreeSet<_> = cargo_unit.artifacts.keys().cloned().collect();
        let oven_artifacts: BTreeSet<_> = oven_unit.artifacts.keys().cloned().collect();
        if cargo_artifacts != oven_artifacts {
            return Err(format!(
                "{binding}: artifact inventory differs: missing from Oven={:?}, extra in Oven={:?}",
                cargo_artifacts.difference(&oven_artifacts).collect::<Vec<_>>(),
                oven_artifacts.difference(&cargo_artifacts).collect::<Vec<_>>()
            ));
        }
        let mut artifacts = Vec::new();
        for (role, path) in cargo_artifacts {
            let key = (role.clone(), path.clone());
            let cargo_digest = &cargo_unit.artifacts[&key].digest;
            let oven_digest = &oven_unit.artifacts[&key].digest;
            if cargo_digest != oven_digest {
                return Err(format!(
                    "{binding}: determinism conflict for equal unit identity {}: {role} {path} has Cargo {cargo_digest}, Oven {oven_digest}",
                    cargo_unit.unit_identity
                ));
            }
            artifacts.push(EvidenceArtifact {
                role,
                path,
                cargo_digest: cargo_digest.clone(),
                oven_digest: oven_digest.clone(),
            });
        }
        let archive_digest = oven_unit
            .archive_digest
            .clone()
            .ok_or_else(|| format!("{binding}: Oven unit has no asset archive digest"))?;
        bindings.push(EvidenceBinding {
            binding,
            package: oven_unit.package.clone(),
            version: oven_unit.version.clone(),
            unit_identity: oven_unit.unit_identity.clone(),
            archive_digest,
            features: oven_unit.features.clone(),
            source_digest: oven_unit.source_digest.clone(),
            manifest_digest: oven_unit.manifest_digest.clone(),
            identity_inputs: oven_unit.identity_inputs.clone(),
            artifacts,
        });
    }
    Ok(EquivalenceEvidence {
        schema: EQUIVALENCE_SCHEMA,
        event: "attest",
        status: "attested",
        captures: EvidenceCaptures {
            cargo: EvidenceCapture {
                producer: cargo.producer,
                cargo_free: cargo.cargo_free,
            },
            oven: EvidenceCapture {
                producer: oven.producer,
                cargo_free: oven.cargo_free,
            },
        },
        setup: cargo.setup,
        bindings,
        evidence_digest: None,
    })
}

/// Compare every non-artifact identity field of one binding.
fn compare_unit_identity(binding: &str, cargo: &CapturedUnit, oven: &CapturedUnit) -> Result<(), String> {
    for (field, equal) in [
        ("package", cargo.package == oven.package),
        ("version", cargo.version == oven.version),
        ("features", cargo.features == oven.features),
        ("source digest", cargo.source_digest == oven.source_digest),
        ("manifest digest", cargo.manifest_digest == oven.manifest_digest),
    ] {
        if !equal {
            return Err(format!("{binding}: {field} differs"));
        }
    }
    if cargo.identity_inputs != oven.identity_inputs {
        return Err(format!("{binding}: identity inputs differ"));
    }
    if cargo.unit_identity != oven.unit_identity {
        return Err(format!("{binding}: unit identity differs"));
    }
    Ok(())
}

/// Atomically replace the output only after complete comparison and encoding succeed.
fn write_evidence(path: &Path, evidence: &EquivalenceEvidence) -> CliResult<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|error| {
        CliError::failure(format!(
            "oven artifact equivalence: could not create {}: {error}",
            parent.display()
        ))
    })?;
    let value = serde_json::to_value(evidence)
        .map_err(|error| CliError::failure(format!("oven artifact equivalence: could not encode evidence: {error}")))?;
    let mut payload = serde_json::to_string_pretty(&value)
        .map_err(|error| CliError::failure(format!("oven artifact equivalence: could not encode evidence: {error}")))?;
    payload.push('\n');
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| CliError::failure(format!("oven artifact equivalence: could not stage evidence: {error}")))?;
    temporary
        .write_all(payload.as_bytes())
        .map_err(|error| CliError::failure(format!("oven artifact equivalence: could not stage evidence: {error}")))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| CliError::failure(format!("oven artifact equivalence: could not sync evidence: {error}")))?;
    temporary.persist(path).map_err(|error| {
        CliError::failure(format!(
            "oven artifact equivalence: could not publish {}: {error}",
            path.display()
        ))
    })?;
    Ok(())
}

/// Prefix one validation failure with the command's stable diagnostic label.
fn equivalence_error(error: String) -> CliError {
    CliError::failure(format!("oven artifact equivalence: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// Synthetic Cargo/Oven capture pair rooted at different physical paths.
    struct Fixture {
        root: tempfile::TempDir,
        cargo: PathBuf,
        oven: PathBuf,
        cargo_manifest: PathBuf,
        oven_manifest: PathBuf,
        output: PathBuf,
    }

    impl Fixture {
        /// Create matching Cargo and Oven evidence with one registry unit.
        fn new() -> Result<Self, Box<dyn std::error::Error>> {
            let root = tempfile::tempdir()?;
            let cargo = root.path().join("cargo-capture");
            let oven = root.path().join("oven-bake");
            let cargo_manifest = write_capture(&cargo, "cargo-harvest", false)?;
            let oven_manifest = write_capture(&oven, "oven-publisher", true)?;
            let output = root.path().join("attestation.json");
            Ok(Self {
                root,
                cargo,
                oven,
                cargo_manifest,
                oven_manifest,
                output,
            })
        }

        /// Run the Rust comparator over the current fixture state.
        fn run(&self) -> Result<(), String> {
            oven_equivalence(&self.cargo_manifest, &self.oven_manifest, &self.output)
                .map(|_| ())
                .map_err(|error| error.to_string())
        }
    }

    /// Write one base capture manifest and all bytes it declares.
    fn write_capture(root: &Path, producer: &str, cargo_free: bool) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let artifact_root = root.join("artifacts");
        let artifact = artifact_root.join("deps/libfixture.rlib");
        let archive = artifact_root.join("assets/fixture.loaf");
        fs::create_dir_all(artifact.parent().ok_or("artifact has no parent")?)?;
        fs::create_dir_all(archive.parent().ok_or("archive has no parent")?)?;
        fs::write(&artifact, b"fixture rlib bytes\n")?;
        fs::write(&archive, b"oven asset archive\n")?;
        let mut unit = serde_json::json!({
            "binding": "fixture 1.0.0 target release",
            "package": "fixture",
            "version": "1.0.0",
            "unit_identity": digest_bytes(b"unit identity"),
            "features": ["feature-a"],
            "source_digest": digest_bytes(b"source"),
            "manifest_digest": digest_bytes(b"manifest"),
            "identity_inputs": {
                "cfg": ["feature=\"feature-a\""],
                "out": [{"path": "generated/config.rs", "digest": digest_bytes(b"out")}],
                "native": [{"name": "fixture", "digest": digest_bytes(b"native")}],
                "tools": [{"name": "generator", "digest": digest_bytes(b"tool")}],
                "dependencies": [{"name": "dependency", "unit_identity": digest_bytes(b"dependency")}]
            },
            "artifacts": [{
                "role": "rlib",
                "path": "deps/libfixture.rlib",
                "digest": digest_bytes(&fs::read(&artifact)?)
            }]
        });
        if producer == "oven-publisher" {
            unit["asset_archive"] = serde_json::json!({
                "path": "assets/fixture.loaf",
                "digest": digest_bytes(&fs::read(&archive)?)
            });
        }
        let manifest = serde_json::json!({
            "schema": 1,
            "producer": producer,
            "cargo_free": cargo_free,
            "artifact_root": "artifacts",
            "setup": {
                "cargo": {"version": "cargo 1.fixture", "digest": digest_bytes(b"cargo")},
                "rustc": {"version": "rustc 1.fixture", "digest": digest_bytes(b"rustc")},
                "host": "fixture-host",
                "target": "fixture-target",
                "profile": "release"
            },
            "units": [unit]
        });
        fs::create_dir_all(root)?;
        let path = root.join("capture.json");
        save(&path, &manifest)?;
        Ok(path)
    }

    /// Load one mutable capture manifest.
    fn load(path: &Path) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }

    /// Save one capture manifest with canonical pretty JSON.
    fn save(path: &Path, value: &serde_json::Value) -> TestResult {
        let mut encoded = serde_json::to_string_pretty(value)?;
        encoded.push('\n');
        fs::write(path, encoded)?;
        Ok(())
    }

    #[test]
    fn matching_raw_artifacts_emit_registry_attestation_evidence() -> TestResult {
        let fixture = Fixture::new()?;
        fixture.run()?;
        let evidence: serde_json::Value = serde_json::from_slice(&fs::read(&fixture.output)?)?;
        assert_eq!(evidence["status"], "attested");
        let binding = &evidence["bindings"][0];
        assert_eq!(binding["package"], "fixture");
        assert_eq!(binding["version"], "1.0.0");
        assert_eq!(binding["unit_identity"], digest_bytes(b"unit identity"));
        assert_eq!(binding["archive_digest"], digest_bytes(b"oven asset archive\n"));
        assert_eq!(
            binding["artifacts"][0]["cargo_digest"],
            digest_bytes(b"fixture rlib bytes\n")
        );
        assert_eq!(
            binding["artifacts"][0]["oven_digest"],
            digest_bytes(b"fixture rlib bytes\n")
        );
        Ok(())
    }

    #[test]
    fn every_metadata_reachable_artifact_role_is_compared() -> TestResult {
        let fixture = Fixture::new()?;
        let roles = ARTIFACT_ROLES;
        for (manifest_path, capture_root) in [
            (&fixture.cargo_manifest, &fixture.cargo),
            (&fixture.oven_manifest, &fixture.oven),
        ] {
            let mut manifest = load(manifest_path)?;
            let mut artifacts = Vec::new();
            for role in roles {
                let relative = format!("products/{role}.bin");
                let payload = format!("{role} bytes\n");
                let path = capture_root.join("artifacts").join(&relative);
                fs::create_dir_all(path.parent().ok_or("product has no parent")?)?;
                fs::write(&path, payload.as_bytes())?;
                artifacts.push(serde_json::json!({
                    "role": role,
                    "path": relative,
                    "digest": digest_bytes(payload.as_bytes())
                }));
            }
            manifest["units"][0]["artifacts"] = serde_json::Value::Array(artifacts);
            save(manifest_path, &manifest)?;
        }
        fixture.run()?;
        let evidence: serde_json::Value = serde_json::from_slice(&fs::read(&fixture.output)?)?;
        let actual: Vec<_> = evidence["bindings"][0]["artifacts"]
            .as_array()
            .ok_or("artifacts is not an array")?
            .iter()
            .filter_map(|artifact| artifact["role"].as_str())
            .collect();
        assert_eq!(actual, roles);
        Ok(())
    }

    #[test]
    fn missing_extra_and_renamed_artifacts_refuse() -> TestResult {
        for label in ["missing", "extra", "renamed"] {
            let fixture = Fixture::new()?;
            let mut manifest = load(&fixture.oven_manifest)?;
            match label {
                "missing" => manifest["units"][0]["artifacts"] = serde_json::json!([]),
                "extra" => {
                    let extra_path = fixture.oven.join("artifacts/deps/libextra.rmeta");
                    fs::write(&extra_path, b"extra")?;
                    manifest["units"][0]["artifacts"]
                        .as_array_mut()
                        .ok_or("artifacts is not an array")?
                        .push(serde_json::json!({
                            "role": "rmeta",
                            "path": "deps/libextra.rmeta",
                            "digest": digest_bytes(b"extra")
                        }));
                }
                "renamed" => {
                    manifest["units"][0]["artifacts"][0]["path"] = serde_json::json!("deps/librenamed.rlib");
                    fs::write(
                        fixture.oven.join("artifacts/deps/librenamed.rlib"),
                        b"fixture rlib bytes\n",
                    )?;
                }
                _ => return Err("unknown mutation".into()),
            }
            save(&fixture.oven_manifest, &manifest)?;
            let error = fixture.run().err().ok_or("artifact inventory drift was accepted")?;
            assert!(error.contains("artifact inventory differs"), "{label}: {error}");
            assert!(!fixture.output.exists());
        }
        Ok(())
    }

    #[test]
    fn archive_member_order_or_raw_digest_drift_is_a_determinism_conflict() -> TestResult {
        let fixture = Fixture::new()?;
        let cargo_archive = b"!<arch>\nmember-a\nmember-b\n";
        let oven_archive = b"!<arch>\nmember-b\nmember-a\n";
        for (manifest_path, capture_root, payload) in [
            (&fixture.cargo_manifest, &fixture.cargo, cargo_archive.as_slice()),
            (&fixture.oven_manifest, &fixture.oven, oven_archive.as_slice()),
        ] {
            let path = capture_root.join("artifacts/deps/libfixture.a");
            fs::write(&path, payload)?;
            let mut manifest = load(manifest_path)?;
            manifest["units"][0]["artifacts"] = serde_json::json!([{
                "role": "static-archive",
                "path": "deps/libfixture.a",
                "digest": digest_bytes(payload)
            }]);
            save(manifest_path, &manifest)?;
        }
        let error = fixture.run().err().ok_or("archive order drift was accepted")?;
        assert!(error.contains("determinism conflict"), "{error}");
        assert!(!fixture.output.exists());
        Ok(())
    }

    #[test]
    fn equal_unit_identity_never_permits_unequal_raw_bytes() -> TestResult {
        let fixture = Fixture::new()?;
        let artifact = fixture.oven.join("artifacts/deps/libfixture.rlib");
        fs::write(&artifact, b"different but internally valid rlib bytes\n")?;
        let mut manifest = load(&fixture.oven_manifest)?;
        manifest["units"][0]["artifacts"][0]["digest"] = serde_json::json!(digest_bytes(&fs::read(&artifact)?));
        save(&fixture.oven_manifest, &manifest)?;
        let error = fixture.run().err().ok_or("unequal raw bytes were accepted")?;
        assert!(error.contains("determinism conflict"), "{error}");
        assert!(!fixture.output.exists());
        Ok(())
    }

    #[test]
    fn capture_roots_may_relocate_without_changing_evidence() -> TestResult {
        let mut fixture = Fixture::new()?;
        fixture.run()?;
        let expected = fs::read(&fixture.output)?;
        let relocated = fixture.root.path().join("relocated");
        fs::create_dir(&relocated)?;
        let new_cargo = relocated.join("cargo");
        let new_oven = relocated.join("oven");
        fs::rename(&fixture.cargo, &new_cargo)?;
        fs::rename(&fixture.oven, &new_oven)?;
        fixture.cargo_manifest = new_cargo.join("capture.json");
        fixture.oven_manifest = new_oven.join("capture.json");
        fixture.run()?;
        assert_eq!(fs::read(&fixture.output)?, expected);
        Ok(())
    }

    #[test]
    fn every_changed_identity_input_refuses_negative_identity() -> TestResult {
        for field in ["cfg", "out", "native", "tools", "dependencies"] {
            let fixture = Fixture::new()?;
            let mut manifest = load(&fixture.oven_manifest)?;
            manifest["units"][0]["identity_inputs"][field] = match field {
                "cfg" => serde_json::json!(["changed"]),
                "out" => serde_json::json!([{"path": "generated/config.rs", "digest": digest_bytes(b"changed out")}]),
                "native" => serde_json::json!([{"name": "fixture", "digest": digest_bytes(b"changed native")}]),
                "tools" => serde_json::json!([{"name": "generator", "digest": digest_bytes(b"changed tool")}]),
                "dependencies" => {
                    serde_json::json!([{"name": "dependency", "unit_identity": digest_bytes(b"changed dependency")}])
                }
                _ => return Err("unknown identity field".into()),
            };
            save(&fixture.oven_manifest, &manifest)?;
            let error = fixture.run().err().ok_or("changed identity input was accepted")?;
            assert!(error.contains("identity inputs differ"), "{field}: {error}");
            assert!(!fixture.output.exists());
        }
        Ok(())
    }

    #[test]
    fn changed_unit_identity_and_tampered_product_refuse() -> TestResult {
        let fixture = Fixture::new()?;
        let mut manifest = load(&fixture.oven_manifest)?;
        manifest["units"][0]["unit_identity"] = serde_json::json!(digest_bytes(b"different unit"));
        save(&fixture.oven_manifest, &manifest)?;
        let identity_error = fixture.run().err().ok_or("different unit identity was accepted")?;
        assert!(identity_error.contains("unit identity differs"), "{identity_error}");

        let fixture = Fixture::new()?;
        fs::write(fixture.oven.join("artifacts/deps/libfixture.rlib"), b"tampered")?;
        let tamper_error = fixture.run().err().ok_or("tampered product was accepted")?;
        assert!(tamper_error.contains("declared digest"), "{tamper_error}");
        assert!(!fixture.output.exists());
        Ok(())
    }
}
