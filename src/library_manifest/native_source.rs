//! Versioned physical definitions for emitted library source units.
//!
//! These definitions retain source bytes and dependency requests. They neither select a Rust closure nor confer
//! native readiness; selected provider identities remain in the containing checked manifest.

use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{LibraryManifest, ProviderDependencyKind};
use crate::generated_source;

/// Artifact-relative location of an emitted library's physical source definition.
pub const NATIVE_SOURCE_UNIT_PATH: &str = "native/source-unit.json";
/// Supported physical source-unit schema; independent of semantic executable formats.
pub const NATIVE_SOURCE_UNIT_SCHEMA_VERSION: u32 = 1;
/// Maximum accepted source-definition payload size.
const MAX_DEFINITION_BYTES: usize = 4 * 1024 * 1024;

/// A malformed, unsupported or physically inconsistent source-unit definition.
#[derive(Debug, thiserror::Error)]
pub enum NativeSourceDefinitionError {
    /// A present document uses a schema this reader cannot interpret.
    #[error("unsupported native source-unit schema {0}")]
    UnsupportedVersion(u64),
    /// The definition or its binding violates the physical metadata contract.
    #[error("invalid native source-unit definition: {0}")]
    Invalid(String),
    /// A present definition or explicitly named source could not be read.
    #[error("failed to read native source-unit input {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    /// A present definition is not valid JSON with the required field types.
    #[error("invalid native source-unit JSON: {0}")]
    Json(#[from] serde_json::Error),
}

/// One emitted unit definition, independent of target/profile selection and containing artifact identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeSourceUnitDefinition {
    /// Physical record format, checked before interpreting its payload.
    pub schema_version: u32,
    /// Checked authoring package identity.
    pub package: NativeSourcePackage,
    /// Actual emitted Rust target name.
    pub crate_name: String,
    /// Actual producer operation's library kind.
    pub crate_kind: NativeSourceCrateKind,
    /// Actual Rust edition chosen by the producer.
    pub edition: String,
    /// Existing authored Incan/manifest digest, excluding unresolved Rust source closures.
    pub authored_source_digest: String,
    /// The explicitly named emitted crate root and its file digest.
    pub entrypoint: NativeSourceInput,
    /// The explicitly named generated tree and its portable tree digest.
    pub source_tree: NativeSourceInput,
    /// Retained requirements, sorted by role and alias without merging those roles.
    pub requirements: Vec<NativeSourceRequirement>,
}

/// Name and version bound to the containing checked manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeSourcePackage {
    /// Published package name.
    pub name: String,
    /// Published package version.
    pub version: String,
}

/// Native library operation represented by the emitted unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeSourceCrateKind {
    /// Ordinary reusable Rust library.
    Rlib,
    /// Host procedural macro library; callers must supply this kind from producer evidence.
    ProcMacro,
}

/// One artifact-relative physical source binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeSourceInput {
    /// Fixed artifact-relative location, never an authoring checkout path.
    pub path: String,
    /// SHA-256 under the corresponding generated file/tree digest contract.
    pub digest: String,
}

/// Dependency role retained from the producer's resolved input records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeRequirementRole {
    /// Ordinary library dependency.
    Normal,
    /// Development/test dependency, not automatically part of a native library closure.
    Dev,
}

impl NativeRequirementRole {
    /// Stable request-slot prefix; it identifies a requirement, not selected source bytes.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Dev => "dev",
        }
    }
}

/// A retained Rust requirement without implied resolution or native admission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeSourceRequirement {
    /// Original normal/development role.
    pub role: NativeRequirementRole,
    /// Rust dependency alias used by the emitted source.
    pub alias: String,
    /// Explicit package rename, if any.
    pub package: Option<String>,
    /// Authored or checked requirement, never an invented selected version.
    pub version_requirement: Option<String>,
    /// Sorted unique requested Rust feature names.
    pub features: Vec<String>,
    /// Whether the request enables default Rust features.
    pub default_features: bool,
    /// Whether the retained request is optional.
    pub optional: bool,
    /// Existing provider edge reference or explicitly unresolved source request.
    pub source: NativeRequirementSource,
}

/// Portable dependency origin information; no variant is a complete selected Rust closure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeRequirementSource {
    /// Reference to one exact checked provider edge in the containing manifest.
    ProviderEdge {
        /// Existing public/private provider edge role.
        edge_kind: ProviderDependencyKind,
        /// Existing provider edge key; exact identities stay in that edge.
        dependency_key: String,
    },
    /// Unresolved registry requirement.
    RegistryRequest,
    /// Unresolved Git URL and reference request.
    GitRequest {
        /// Declared repository URL.
        url: String,
        /// Declared reference, not an admitted checkout identity.
        reference: NativeGitReference,
    },
    /// A portable request relative to the original authoring project, never the published artifact.
    AuthoredPathRequest {
        /// Fixed authoring-coordinate anchor, which grants no filesystem access.
        anchor: NativePathAnchor,
        /// Relative declarative coordinate; parent components do not authorize artifact traversal.
        path: String,
    },
    /// A retained request whose portable source binding is not available.
    UnboundPathRequest {
        /// Exact role/alias slot that a later selected producer must bind.
        request_key: String,
        /// Why no portable source reference can be published.
        reason: NativeUnboundPathReason,
    },
}

/// Coordinate origin for an unresolved authored path request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativePathAnchor {
    /// Relative to the original authoring project, whose path is not serialized.
    AuthoringProject,
}

/// Missing evidence category; this is not a permissive native fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeUnboundPathReason {
    /// Effective source coordinates cannot be expressed as a checked provider edge or portable authored request.
    PortableSourceBindingUnavailable,
}

/// Declared Git reference; no resolution to a commit occurs while recording it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeGitReference {
    /// Branch request.
    Branch(String),
    /// Tag request.
    Tag(String),
    /// Revision request.
    Rev(String),
}

impl NativeSourceUnitDefinition {
    /// Decode a bounded document, rejecting its version before interpreting version-specific fields.
    pub fn from_json_bytes(bytes: &[u8]) -> Result<Self, NativeSourceDefinitionError> {
        if bytes.len() > MAX_DEFINITION_BYTES {
            return Err(invalid("document exceeds the source-definition size limit"));
        }
        let value: serde_json::Value = serde_json::from_slice(bytes)?;
        let version = value
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| invalid("schema_version must be an unsigned integer"))?;
        if version != u64::from(NATIVE_SOURCE_UNIT_SCHEMA_VERSION) {
            return Err(NativeSourceDefinitionError::UnsupportedVersion(version));
        }
        let definition: Self = serde_json::from_value(value)?;
        definition.validate()?;
        Ok(definition)
    }

    /// Encode a structurally valid definition without conferring source/provider admission.
    pub fn to_json_bytes(&self) -> Result<Vec<u8>, NativeSourceDefinitionError> {
        self.validate()?;
        let bytes = serde_json::to_vec_pretty(self)?;
        if bytes.len() > MAX_DEFINITION_BYTES {
            return Err(invalid("document exceeds the source-definition size limit"));
        }
        Ok(bytes)
    }

    /// Locate the fixed sidecar without allowing an existing parent or file symlink to escape the artifact.
    ///
    /// Missing native directories/files are allowed for initial publication and legacy absence. This method does
    /// not create directories, read payloads or authorize concurrent mutation of the artifact during publication.
    pub fn path_in(artifact_root: &Path) -> Result<PathBuf, NativeSourceDefinitionError> {
        require_directory(artifact_root)?;
        let native = artifact_root.join("native");
        match fs::symlink_metadata(&native) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => return Err(NativeSourceDefinitionError::Io { path: native, source }),
            Ok(_) => require_directory(&native)?,
        }
        let path = artifact_root.join(NATIVE_SOURCE_UNIT_PATH);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(invalid("source-unit sidecar must not be a symlink"));
            }
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(NativeSourceDefinitionError::Io { path, source: error });
            }
            _ => {}
        }
        Ok(path)
    }

    /// Read an optional legacy-compatible sidecar; malformed present files never become absence.
    pub fn read_optional(artifact_root: &Path) -> Result<Option<Self>, NativeSourceDefinitionError> {
        let path = Self::path_in(artifact_root)?;
        let metadata = match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(NativeSourceDefinitionError::Io { path, source }),
            Ok(metadata) => metadata,
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(invalid("source-unit sidecar must be a regular non-symlink file"));
        }
        let read_limit = u64::try_from(MAX_DEFINITION_BYTES + 1)
            .map_err(|_| invalid("source-definition size limit is not representable"))?;
        let mut bytes = Vec::new();
        fs::File::open(&path)
            .and_then(|file| file.take(read_limit).read_to_end(&mut bytes))
            .map_err(|source| NativeSourceDefinitionError::Io { path, source })?;
        Self::from_json_bytes(&bytes).map(Some)
    }

    /// Validate the schema and portable record shape without reading unresolved dependency sources.
    pub fn validate(&self) -> Result<(), NativeSourceDefinitionError> {
        if self.schema_version != NATIVE_SOURCE_UNIT_SCHEMA_VERSION {
            return Err(NativeSourceDefinitionError::UnsupportedVersion(
                self.schema_version.into(),
            ));
        }
        for (label, value) in [
            ("package name", &self.package.name),
            ("package version", &self.package.version),
            ("crate name", &self.crate_name),
        ] {
            require_text(label, value)?;
        }
        if !matches!(self.edition.as_str(), "2015" | "2018" | "2021" | "2024") {
            return Err(invalid("unsupported Rust edition"));
        }
        for digest in [
            &self.authored_source_digest,
            &self.entrypoint.digest,
            &self.source_tree.digest,
        ] {
            require_digest(digest)?;
        }
        if self.entrypoint.path != "src/lib.rs" || self.source_tree.path != "src" {
            return Err(invalid("source inputs must name src/lib.rs and src"));
        }
        let mut keys = BTreeSet::new();
        let mut previous = None;
        for requirement in &self.requirements {
            require_text("dependency alias", &requirement.alias)?;
            for value in [requirement.package.as_ref(), requirement.version_requirement.as_ref()]
                .into_iter()
                .flatten()
            {
                require_text("dependency request", value)?;
            }
            let key = (requirement.role, requirement.alias.as_str());
            if !keys.insert(key) || previous.is_some_and(|previous| previous >= key) {
                return Err(invalid("requirements must have unique sorted role/alias keys"));
            }
            previous = Some(key);
            for feature in &requirement.features {
                require_text("feature", feature)?;
            }
            if requirement.features.windows(2).any(|pair| pair[0] >= pair[1]) {
                return Err(invalid("features must be sorted and unique"));
            }
            match &requirement.source {
                NativeRequirementSource::ProviderEdge { dependency_key, .. } => {
                    if dependency_key != &requirement.alias {
                        return Err(invalid("provider edge key differs from its requirement alias"));
                    }
                }
                NativeRequirementSource::RegistryRequest => {}
                NativeRequirementSource::GitRequest { url, reference } => {
                    require_text("Git URL", url)?;
                    let (NativeGitReference::Branch(value)
                    | NativeGitReference::Tag(value)
                    | NativeGitReference::Rev(value)) = reference;
                    require_text("Git reference", value)?;
                }
                NativeRequirementSource::AuthoredPathRequest { path, .. } => {
                    require_text("authored path request", path)?;
                    if path.starts_with('/') || path.contains('\\') || path.contains(':') {
                        return Err(invalid("authored path request must be portable and relative"));
                    }
                }
                NativeRequirementSource::UnboundPathRequest { request_key, .. } => {
                    if request_key != &format!("{}:{}", requirement.role.as_str(), requirement.alias) {
                        return Err(invalid("unbound path request key differs from its role/alias"));
                    }
                }
            }
        }
        Ok(())
    }

    /// Bind package/source facts and provider-edge references to the containing checked manifest.
    pub fn validate_against_manifest(&self, manifest: &LibraryManifest) -> Result<(), NativeSourceDefinitionError> {
        self.validate()?;
        if self.package.name != manifest.name
            || self.package.version != manifest.version
            || Some(self.authored_source_digest.as_str())
                != manifest.contract_metadata.provider.semantic_source_digest.as_deref()
        {
            return Err(invalid(
                "package or authored-source identity differs from the checked manifest",
            ));
        }
        for requirement in &self.requirements {
            if let NativeRequirementSource::ProviderEdge {
                edge_kind,
                dependency_key,
            } = &requirement.source
            {
                let mut edges = manifest
                    .contract_metadata
                    .provider
                    .provider_dependencies
                    .iter()
                    .filter(|edge| &edge.kind == edge_kind && &edge.dependency_key == dependency_key);
                let edge = edges
                    .next()
                    .ok_or_else(|| invalid(format!("missing checked provider edge {dependency_key}")))?;
                if edges.next().is_some()
                    || edge.provider_name != requirement.package.as_deref().unwrap_or(&requirement.alias)
                {
                    return Err(invalid(format!(
                        "ambiguous or mismatched checked provider edge {dependency_key}"
                    )));
                }
            }
        }
        Ok(())
    }

    /// Verify the explicitly named generated bytes under this artifact root, without selecting dependencies.
    pub fn validate_sources(&self, artifact_root: &Path) -> Result<(), NativeSourceDefinitionError> {
        self.validate()?;
        require_directory(artifact_root)?;
        let tree = artifact_root.join(&self.source_tree.path);
        // Validate the parent before opening src/lib.rs so a symlinked src cannot escape the artifact.
        let tree_digest = generated_source::digest_tree(&tree).map_err(|error| invalid(error.to_string()))?;
        let root_digest = generated_source::digest_file(&artifact_root.join(&self.entrypoint.path))
            .map_err(|error| invalid(error.to_string()))?;
        if tree_digest != self.source_tree.digest || root_digest != self.entrypoint.digest {
            return Err(invalid("generated source bytes differ from their recorded digests"));
        }
        Ok(())
    }
}

/// Construct a precise structural/binding error without assigning native meaning.
fn invalid(message: impl Into<String>) -> NativeSourceDefinitionError {
    NativeSourceDefinitionError::Invalid(message.into())
}

/// Reject empty, padded or NUL-bearing request values without normalizing declared intent.
fn require_text(label: &str, value: &str) -> Result<(), NativeSourceDefinitionError> {
    if value.is_empty() || value.trim() != value || value.contains('\0') {
        return Err(invalid(format!("{label} must be nonempty, unpadded and NUL-free")));
    }
    Ok(())
}

/// Validate the established lowercase SHA-256 digest rendering.
fn require_digest(value: &str) -> Result<(), NativeSourceDefinitionError> {
    if !value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }) {
        return Err(invalid("expected lowercase sha256 digest"));
    }
    Ok(())
}

/// Prove a named artifact directory is real before traversing its fixed child paths.
fn require_directory(path: &Path) -> Result<(), NativeSourceDefinitionError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| NativeSourceDefinitionError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(invalid(format!("{} must be a non-symlink directory", path.display())));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{ProviderDependencyMetadata, digest_provider_artifact};
    use super::*;

    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

    /// Construct a real emitted-source binding without needing a compiler, SDK or native selector.
    fn fixture(root: &Path) -> TestResult<(LibraryManifest, NativeSourceUnitDefinition)> {
        fs::create_dir_all(root.join("src"))?;
        fs::write(root.join("src/lib.rs"), "pub fn answer() -> i64 { 42 }\n")?;
        let mut manifest = LibraryManifest::new("pricing", "0.1.0");
        manifest.contract_metadata.provider.semantic_source_digest =
            Some(generated_source::digest_bytes(b"authored Incan"));
        let definition = NativeSourceUnitDefinition {
            schema_version: NATIVE_SOURCE_UNIT_SCHEMA_VERSION,
            package: NativeSourcePackage {
                name: manifest.name.clone(),
                version: manifest.version.clone(),
            },
            crate_name: "pricing".to_string(),
            crate_kind: NativeSourceCrateKind::Rlib,
            edition: "2024".to_string(),
            authored_source_digest: manifest
                .contract_metadata
                .provider
                .semantic_source_digest
                .clone()
                .ok_or("authored digest missing")?,
            entrypoint: NativeSourceInput {
                path: "src/lib.rs".to_string(),
                digest: generated_source::digest_file(&root.join("src/lib.rs"))?,
            },
            source_tree: NativeSourceInput {
                path: "src".to_string(),
                digest: generated_source::digest_tree(&root.join("src"))?,
            },
            requirements: Vec::new(),
        };
        Ok((manifest, definition))
    }

    /// Keep test requests explicit so source kinds cannot be mistaken for selected input facts.
    fn request(role: NativeRequirementRole, source: NativeRequirementSource) -> NativeSourceRequirement {
        NativeSourceRequirement {
            role,
            alias: "stock".to_string(),
            package: Some("catalog".to_string()),
            version_requirement: Some("1".to_string()),
            features: vec!["retail".to_string()],
            default_features: false,
            optional: false,
            source,
        }
    }

    #[test]
    fn native_source_definition_preserves_requests_and_separate_roles() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let (_, mut definition) = fixture(tmp.path())?;
        for source in [
            NativeRequirementSource::RegistryRequest,
            NativeRequirementSource::GitRequest {
                url: "https://example.invalid/catalog".to_string(),
                reference: NativeGitReference::Branch("main".to_string()),
            },
            NativeRequirementSource::GitRequest {
                url: "https://example.invalid/catalog".to_string(),
                reference: NativeGitReference::Tag("v1".to_string()),
            },
            NativeRequirementSource::GitRequest {
                url: "https://example.invalid/catalog".to_string(),
                reference: NativeGitReference::Rev("abc".to_string()),
            },
            NativeRequirementSource::AuthoredPathRequest {
                anchor: NativePathAnchor::AuthoringProject,
                path: "../catalog".to_string(),
            },
            NativeRequirementSource::UnboundPathRequest {
                request_key: "normal:stock".to_string(),
                reason: NativeUnboundPathReason::PortableSourceBindingUnavailable,
            },
        ] {
            definition.requirements = vec![
                request(NativeRequirementRole::Normal, source),
                request(NativeRequirementRole::Dev, NativeRequirementSource::RegistryRequest),
            ];
            let encoded = definition.to_json_bytes()?;
            assert_eq!(NativeSourceUnitDefinition::from_json_bytes(&encoded)?, definition);
            assert!(!String::from_utf8(encoded)?.contains(tmp.path().to_string_lossy().as_ref()));
        }
        definition.requirements.push(definition.requirements[0].clone());
        assert!(definition.to_json_bytes().is_err());
        Ok(())
    }

    #[test]
    fn native_source_definition_refuses_version_before_payload_and_malformed_present() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let (_, definition) = fixture(tmp.path())?;
        assert!(NativeSourceUnitDefinition::read_optional(tmp.path())?.is_none());
        assert!(matches!(
            NativeSourceUnitDefinition::from_json_bytes(br#"{"schema_version":99,"crate_kind":null}"#),
            Err(NativeSourceDefinitionError::UnsupportedVersion(99))
        ));
        fs::create_dir(tmp.path().join("native"))?;
        fs::write(tmp.path().join(NATIVE_SOURCE_UNIT_PATH), b"{")?;
        assert!(NativeSourceUnitDefinition::read_optional(tmp.path()).is_err());
        fs::write(tmp.path().join(NATIVE_SOURCE_UNIT_PATH), definition.to_json_bytes()?)?;
        assert_eq!(
            NativeSourceUnitDefinition::read_optional(tmp.path())?,
            Some(definition.clone())
        );
        for (field, value) in [
            ("edition", "2027"),
            ("crate_kind", "bin"),
            ("authored_source_digest", "SHA256:0"),
        ] {
            let mut json = serde_json::to_value(&definition)?;
            json[field] = value.into();
            assert!(NativeSourceUnitDefinition::from_json_bytes(&serde_json::to_vec(&json)?).is_err());
        }
        Ok(())
    }

    /// Reject malformed portable request data without opening any path or resolving any requirement.
    #[test]
    fn native_source_definition_rejects_unsafe_or_ambiguous_request_shapes() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let (_, mut definition) = fixture(tmp.path())?;
        for path in ["/absolute/source", "C:\\outside", "../bad\0path"] {
            definition.requirements = vec![request(
                NativeRequirementRole::Normal,
                NativeRequirementSource::AuthoredPathRequest {
                    anchor: NativePathAnchor::AuthoringProject,
                    path: path.to_string(),
                },
            )];
            assert!(definition.to_json_bytes().is_err());
        }
        definition.requirements = vec![request(
            NativeRequirementRole::Normal,
            NativeRequirementSource::RegistryRequest,
        )];
        for features in [
            vec!["z".to_string(), "a".to_string()],
            vec!["same".to_string(), "same".to_string()],
            vec![" ".to_string()],
        ] {
            definition.requirements[0].features = features;
            assert!(definition.to_json_bytes().is_err());
        }
        assert!(NativeSourceUnitDefinition::from_json_bytes(&vec![b' '; MAX_DEFINITION_BYTES + 1]).is_err());
        Ok(())
    }

    #[test]
    fn native_source_definition_binds_manifest_and_exact_provider_edge() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let (mut manifest, mut definition) = fixture(tmp.path())?;
        definition.requirements.push(request(
            NativeRequirementRole::Normal,
            NativeRequirementSource::ProviderEdge {
                edge_kind: ProviderDependencyKind::PublicPackage,
                dependency_key: "stock".to_string(),
            },
        ));
        assert!(definition.validate_against_manifest(&manifest).is_err());
        manifest
            .contract_metadata
            .provider
            .provider_dependencies
            .push(ProviderDependencyMetadata {
                kind: ProviderDependencyKind::PublicPackage,
                dependency_key: "stock".to_string(),
                provider_name: "catalog".to_string(),
                provider_version: "1.2.0".to_string(),
                artifact_digest: generated_source::digest_bytes(b"admitted artifact"),
                relative_artifact_path: "../../catalog".to_string(),
                requested_features: BTreeSet::from(["public_feature".to_string()]),
                default_features: false,
                optional: false,
            });
        // Public provider features and emitted Rust requirement features are deliberately distinct domains.
        definition.validate_against_manifest(&manifest)?;
        for field in [
            "name",
            "version",
            "source",
            "missing_source",
            "edge_kind",
            "edge_key",
            "edge_package",
            "duplicate",
        ] {
            let mut changed = manifest.clone();
            match field {
                "name" => changed.name = "other".to_string(),
                "version" => changed.version = "9".to_string(),
                "source" => {
                    changed.contract_metadata.provider.semantic_source_digest =
                        Some(generated_source::digest_bytes(b"different source"))
                }
                "missing_source" => changed.contract_metadata.provider.semantic_source_digest = None,
                "edge_kind" => {
                    changed.contract_metadata.provider.provider_dependencies[0].kind =
                        ProviderDependencyKind::PrivateImplementation
                }
                "edge_key" => {
                    changed.contract_metadata.provider.provider_dependencies[0].dependency_key = "other".to_string()
                }
                "edge_package" => {
                    changed.contract_metadata.provider.provider_dependencies[0].provider_name = "other".to_string()
                }
                _ => changed
                    .contract_metadata
                    .provider
                    .provider_dependencies
                    .push(changed.contract_metadata.provider.provider_dependencies[0].clone()),
            }
            assert!(
                definition.validate_against_manifest(&changed).is_err(),
                "must refuse {field}"
            );
        }
        Ok(())
    }

    #[test]
    fn native_source_definition_is_portable_and_detects_source_and_sidecar_mutation() -> TestResult {
        let first = tempfile::tempdir()?;
        let second = tempfile::tempdir()?;
        let (_, definition) = fixture(first.path())?;
        let (_, relocated) = fixture(second.path())?;
        assert_eq!(definition, relocated);
        definition.validate_sources(second.path())?;
        fs::create_dir(first.path().join("native"))?;
        fs::write(first.path().join(NATIVE_SOURCE_UNIT_PATH), definition.to_json_bytes()?)?;
        let before = digest_provider_artifact(first.path())?;
        let mut changed = definition.clone();
        changed.edition = "2021".to_string();
        fs::write(first.path().join(NATIVE_SOURCE_UNIT_PATH), changed.to_json_bytes()?)?;
        assert_ne!(digest_provider_artifact(first.path())?, before);
        fs::write(second.path().join("src/lib.rs"), "pub fn answer() -> i64 { 99 }\n")?;
        assert!(definition.validate_sources(second.path()).is_err());
        let mut escaped = definition;
        escaped.entrypoint.path = "../src/lib.rs".to_string();
        assert!(escaped.validate_sources(first.path()).is_err());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn native_source_definition_refuses_symlinked_source_and_sidecar_parents() -> TestResult {
        let tmp = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        let (_, definition) = fixture(outside.path())?;
        std::os::unix::fs::symlink(outside.path().join("src"), tmp.path().join("src"))?;
        assert!(definition.validate_sources(tmp.path()).is_err());
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("native"))?;
        assert!(NativeSourceUnitDefinition::read_optional(tmp.path()).is_err());
        Ok(())
    }
}
