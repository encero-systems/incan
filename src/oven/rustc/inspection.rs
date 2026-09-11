//! Project-inspection authority payloads.
//!
//! The sealed description of what a project's inspection authority covers -- its sources, constituents, root and
//! test dependencies -- as recorded for one schema version.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::super::store::{OvenArtifactKind, OvenStoreExecutionPayload, OvenStoreLease};
use super::super::{OvenBuildIntent, OvenReceipt};
use super::artifact::OvenRustcRegistrySourcePackage;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Wire schema for one project-level Rust inspection authority.
pub(crate) const OVEN_PROJECT_INSPECTION_AUTHORITY_SCHEMA_VERSION: u32 = 2;

#[allow(dead_code, reason = "first selected-input producer slice; production wiring follows")]
mod selected_rust_facet_graph {
    use super::*;

    /// Wire schema for the portable Rust facet graph selected before physical rust-analyzer projection.
    pub(crate) const OVEN_SELECTED_RUST_FACET_GRAPH_SCHEMA_VERSION: u32 = 1;
    const OVEN_SELECTED_RUST_FACET_GRAPH_DIGEST_DOMAIN: &str = "incan.oven.selected-rust-facet-graph/1";
    const OVEN_SELECTED_RUST_FACET_UNIT_DIGEST_DOMAIN: &str = "incan.oven.selected-rust-facet-unit/1";

    /// Command purpose whose dependency roles and feature activation produced a selected Rust graph.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub(crate) enum OvenSelectedRustFacetPurpose {
        /// Ordinary build, run, inspection, or publication inputs.
        Normal,
        /// Test inputs, including development dependencies selected for the owning test unit.
        Test,
    }

    /// Portable provenance class for one immutable physical owner retained outside the graph.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub(crate) enum OvenSelectedRustFacetOwnerKind {
        /// The singular project inspection authority owns the referenced bytes directly.
        ProjectAuthority,
        /// One exact Store or Loaf constituent owns the referenced bytes.
        Constituent,
        /// A selected toolchain inspection closure owns compiler source or target facts.
        Toolchain,
        /// An admitted provider execution owns generated output bytes.
        GeneratedOutput,
    }

    /// One immutable owner identity; its physical root is supplied separately under a retained lease.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub(crate) struct OvenSelectedRustFacetOwner {
        /// Content-addressed Loaf, authority, or provider-output identity.
        ///
        /// A `Toolchain` owner uses `OvenRustInspectionToolchainPayload::closure_digest`; it is deliberately distinct
        /// from both the Store manifest identity and the human-readable Rust release in `selection.intent.toolchain`.
        pub identity: String,
        /// Provenance class used by physical admission to choose the correct owner table.
        pub kind: OvenSelectedRustFacetOwnerKind,
    }

    /// Selected source provenance without a machine-local checkout or Store path.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub(crate) enum OvenSelectedRustFacetSourceKind {
        /// Immutable registry package source.
        Registry,
        /// Immutable Git revision source.
        Git,
        /// Author-owned path source admitted by content.
        Path,
        /// Rust emitted by a checked Incan or provider compilation.
        Generated,
        /// Compiler-owned support source selected from the Toolchain owner's fixed `sysroot_units` and `rust_src`.
        Compiler,
    }

    /// Complete portable source-tree identity for one selected unit.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub(crate) struct OvenSelectedRustFacetSource {
        /// Source category retained from the selected producer.
        pub kind: OvenSelectedRustFacetSourceKind,
        /// Producer-established source coordinate, such as a locked registry package or compiler component.
        pub identity: String,
        /// Stable owner reference resolved only by the later physical adapter.
        pub owner: String,
        /// Portable directory below the owner's immutable root; `.` denotes the owner root itself.
        pub root: String,
        /// Complete source-tree digest computed from `source_members`.
        pub digest: String,
    }

    /// One exact regular file below a unit's portable source root.
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub(crate) struct OvenSelectedRustFacetSourceMember {
        /// Portable path relative to the selected source root.
        pub path: String,
        /// Exact file byte digest.
        pub digest: String,
    }

    /// Portable reference to a path below one retained owner.
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub(crate) struct OvenSelectedRustFacetPath {
        /// Content identity from the graph's complete owner table.
        pub owner: String,
        /// Portable owner-relative path; `.` denotes the owner root itself.
        pub path: String,
    }

    /// Environment value retained without embedding a machine-local delivery path.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
    pub(crate) enum OvenSelectedRustFacetEnvironmentValue {
        /// Explicit portable non-path value, including an explicitly selected empty string.
        Text { value: String },
        /// Path rebound through one retained owner by the physical projection adapter.
        Path { value: OvenSelectedRustFacetPath },
        /// Selection-only identity for a sensitive value that must never enter the projection payload.
        SensitiveDigest { hmac_sha256: String },
    }

    /// One selected generated input and the exact owner whose receipt produced it.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub(crate) struct OvenSelectedRustFacetGeneratedInput {
        /// Stable name used by cfg, environment, or source projection policy.
        pub name: String,
        /// Portable path and exact owner of the generated file or tree.
        pub source: OvenSelectedRustFacetPath,
        /// Digest of the selected generated bytes or complete generated tree.
        pub digest: String,
    }

    /// Exact dependency alias and selected child unit.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub(crate) struct OvenSelectedRustFacetDependency {
        /// Rust-facing alias used by the parent source, including an admitted package rename.
        pub alias: String,
        /// Stable ID of the exact selected unit.
        pub unit: String,
    }

    /// Rust crate output kind selected for one unit.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub(crate) enum OvenSelectedRustFacetCrateKind {
        /// Reusable Rust library.
        Rlib,
        /// Rust executable.
        Binary,
        /// Host procedural-macro library.
        ProcMacro,
    }

    /// Inspection role controlling why a Rust crate unit participates in this graph.
    ///
    /// RFC 119 build scripts are provider candidates whose checked cfg, environment and generated outputs enter the
    /// consuming unit; they are not rust-analyzer crate units in this v1 projection. Generated sources are represented
    /// by `source_members` and `generated_inputs`, rather than a synthetic unit. Carrier-only `dylib`, `cdylib` and
    /// `staticlib` outputs likewise belong to bake/link planning; inspection consumes their underlying library or
    /// caller-projection unit. Those roles must refuse at the producer until a later schema explicitly represents them.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub(crate) enum OvenSelectedRustFacetUnitRole {
        /// Ordinary library root or dependency.
        Library,
        /// Ordinary executable root.
        Binary,
        /// Unit-test harness compiled with its owning crate root.
        UnitTest,
        /// Separate integration-test crate.
        IntegrationTest,
        /// Named executable example.
        Example,
        /// Generated documentation-test crate.
        Doctest,
        /// Named benchmark executable.
        Benchmark,
        /// Procedural-macro host unit.
        ProcMacro,
        /// Compiler support projected from the selected Toolchain owner's fixed sysroot-unit catalog.
        ///
        /// The producer copies the catalog's root, edition and dependency facts; it must not rediscover sysroot units.
        CompilerSupport,
        /// Target library projected for a Rust-hosted Incan caller.
        CallerProjection,
    }

    /// Whether a selected unit executes for the build host or compiles for the target.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub(crate) enum OvenSelectedRustFacetDomain {
        /// Build script, procedural macro, or other host unit.
        Host,
        /// Ordinary target-linked unit.
        Target,
    }

    /// One stable crate unit selected by the Oven-native Rust facet.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub(crate) struct OvenSelectedRustFacetUnit {
        /// Content-derived unit identity used by dependency and exposed-root references.
        pub identity: String,
        /// Selected package identity.
        pub package: String,
        /// Exact selected package version.
        pub package_version: String,
        /// Rust crate name passed to compiler and inspection consumers.
        pub crate_name: String,
        /// Selected crate output kind.
        pub crate_kind: OvenSelectedRustFacetCrateKind,
        /// Reason this unit participates in the graph.
        pub role: OvenSelectedRustFacetUnitRole,
        /// Host or target compilation domain.
        pub domain: OvenSelectedRustFacetDomain,
        /// Selected Rust edition.
        pub edition: String,
        /// Complete source identity and owner reference.
        pub source: OvenSelectedRustFacetSource,
        /// Portable source-root-relative crate root, which must occur in `source_members`.
        pub root_module: String,
        /// Complete sorted source-file catalog. Absence cannot be represented as an empty leaf.
        pub source_members: Vec<OvenSelectedRustFacetSourceMember>,
        /// Sorted effective feature set for this exact unit.
        pub features: Vec<String>,
        /// Whether this unit's selected activation includes default features.
        pub default_features: bool,
        /// Sorted complete cfg facts supplied to inspection; an explicitly checked empty set remains empty.
        pub cfg: Vec<String>,
        /// Complete selected environment. Empty means the producer checked and selected no values.
        pub environment: BTreeMap<String, OvenSelectedRustFacetEnvironmentValue>,
        /// Sorted, nonempty source directories visible to rust-analyzer.
        pub include_dirs: Vec<OvenSelectedRustFacetPath>,
        /// Sorted source directories explicitly excluded from rust-analyzer.
        pub exclude_dirs: Vec<OvenSelectedRustFacetPath>,
        /// Exact sorted dependency aliases. An empty vector is a checked leaf, not missing evidence.
        pub dependencies: Vec<OvenSelectedRustFacetDependency>,
        /// Sorted generated inputs with their exact output owners.
        pub generated_inputs: Vec<OvenSelectedRustFacetGeneratedInput>,
    }

    /// Strict wire form of the existing target/toolchain/profile/root-feature build intent.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub(crate) struct OvenSelectedRustFacetIntent {
        /// Target triple selected for the requested graph.
        pub target: String,
        /// Exact selected Rust toolchain release identity.
        pub toolchain: String,
        /// Named Oven profile.
        pub profile: String,
        /// Deterministically ordered enabled root feature set.
        pub features: Vec<String>,
    }

    impl From<&OvenBuildIntent> for OvenSelectedRustFacetIntent {
        fn from(intent: &OvenBuildIntent) -> Self {
            Self {
                target: intent.target.clone(),
                toolchain: intent.toolchain.clone(),
                profile: intent.profile.clone(),
                features: intent.features.clone(),
            }
        }
    }

    /// Exact target-spec bytes selected from one Store-owned toolchain closure.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub(crate) struct OvenSelectedRustFacetTargetSpec {
        /// Toolchain-owner-relative target-spec JSON file, matching the inspection-toolchain payload's recorded path.
        pub source: OvenSelectedRustFacetPath,
        /// Digest of the exact target-spec JSON bytes recorded by that same toolchain payload.
        pub digest: String,
    }

    /// Complete selection context shared by every unit in one graph.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub(crate) struct OvenSelectedRustFacetSelection {
        /// Strict target/toolchain/profile/root-feature wire intent.
        pub intent: OvenSelectedRustFacetIntent,
        /// Exact build host triple; host and target remain distinct.
        pub host: String,
        /// Dependency-role activation purpose.
        pub purpose: OvenSelectedRustFacetPurpose,
        /// Whether the selected root activation includes default features.
        pub default_features: bool,
        /// Semver-only compiler version required by rust-analyzer.
        pub toolchain_version: String,
        /// Exact target-spec file bound to the selected Store-owned toolchain closure.
        pub target_spec: OvenSelectedRustFacetTargetSpec,
    }

    /// Portable selected Rust graph; all physical roots and leases remain outside this payload.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code, reason = "first selected-input producer slice; production wiring follows")]
    pub(crate) struct OvenSelectedRustFacetGraph {
        /// Exact wire schema checked before any other field is consumed.
        pub schema_version: u32,
        /// Complete host/target/profile/purpose/feature/toolchain selection.
        pub selection: OvenSelectedRustFacetSelection,
        /// Sorted complete owner table referenced by sources and generated inputs, with exactly one Toolchain closure.
        pub owners: Vec<OvenSelectedRustFacetOwner>,
        /// Sorted complete selected unit table.
        pub units: Vec<OvenSelectedRustFacetUnit>,
        /// Exact Rust-facing root alias to selected unit ID bindings.
        pub exposed_roots: BTreeMap<String, String>,
    }

    /// Failure to admit a portable selected Rust graph.
    #[derive(Debug, thiserror::Error)]
    pub(crate) enum OvenSelectedRustFacetGraphError {
        /// A required producer fact was not supplied.
        #[error("missing selected Rust facet graph evidence: {field}")]
        Missing { field: String },
        /// Supplied facts are malformed, inconsistent, noncanonical, or reference absent records.
        #[error("invalid selected Rust facet graph evidence for {field}: {message}")]
        Invalid { field: String, message: String },
        /// The graph uses a wire schema this reader does not implement.
        #[error("unsupported selected Rust facet graph schema {found}; expected {expected}")]
        UnsupportedSchema { found: u32, expected: u32 },
    }

    /// Validated portable graph paired with its relocation-independent content identity.
    #[derive(Debug, Clone)]
    #[allow(dead_code, reason = "first selected-input producer slice; production wiring follows")]
    pub(crate) struct ValidatedOvenSelectedRustFacetGraph {
        graph: OvenSelectedRustFacetGraph,
        digest: String,
    }

    /// Minimal header decoded before any version-specific graph fields.
    #[derive(Deserialize)]
    struct OvenSelectedRustFacetGraphHeader {
        schema_version: u32,
    }

    fn selected_graph_missing(field: impl Into<String>) -> OvenSelectedRustFacetGraphError {
        OvenSelectedRustFacetGraphError::Missing { field: field.into() }
    }

    fn selected_graph_invalid(field: impl Into<String>, message: impl Into<String>) -> OvenSelectedRustFacetGraphError {
        OvenSelectedRustFacetGraphError::Invalid {
            field: field.into(),
            message: message.into(),
        }
    }

    fn validate_selected_graph_text(value: &str, field: &str) -> Result<(), OvenSelectedRustFacetGraphError> {
        if value.is_empty() {
            return Err(selected_graph_missing(field));
        }
        if value.trim() != value {
            return Err(selected_graph_invalid(field, "has leading or trailing whitespace"));
        }
        Ok(())
    }

    fn validate_selected_graph_digest(value: &str, field: &str) -> Result<(), OvenSelectedRustFacetGraphError> {
        if value.is_empty() {
            return Err(selected_graph_missing(field));
        }
        let valid = value.strip_prefix("sha256:").is_some_and(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        });
        if !valid {
            return Err(selected_graph_invalid(
                field,
                "must be `sha256:` followed by 64 lowercase hexadecimal digits",
            ));
        }
        Ok(())
    }

    fn validate_selected_graph_path(
        value: &str,
        field: &str,
        allow_owner_root: bool,
    ) -> Result<(), OvenSelectedRustFacetGraphError> {
        validate_selected_graph_text(value, field)?;
        if allow_owner_root && value == "." {
            return Ok(());
        }
        if value.starts_with('/')
            || value.contains('\\')
            || value.contains(':')
            || value.split('/').any(|part| matches!(part, "" | "." | ".."))
        {
            return Err(selected_graph_invalid(field, "is not a portable relative path"));
        }
        Ok(())
    }

    /// Reject every host-specific spelling a source identity could smuggle in.
    ///
    /// The checks read the string itself rather than going through `std::path`, because `Path::is_absolute` answers
    /// for the *validating* host: a Windows drive or UNC identity looks like an ordinary relative name to a
    /// macOS or Linux validator, and a POSIX absolute looks ordinary to a Windows one. A portable graph must refuse
    /// both wherever it is checked, so the rule is spelled out once and applied uniformly.
    ///
    /// Logical coordinates keep their own punctuation -- a Git URL's `://`, a registry's `@version` -- because those
    /// name a remote or locked coordinate rather than a location on this machine. A `file:` URI is the exception
    /// that proves the rule and is handled by `selected_graph_coordinate_names_a_local_file_uri`.
    fn validate_selected_graph_portable_coordinate(
        value: &str,
        field: &str,
    ) -> Result<(), OvenSelectedRustFacetGraphError> {
        if value.is_empty() {
            return Err(selected_graph_missing(field));
        }
        if value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        {
            return Err(selected_graph_invalid(
                field,
                "contains control or whitespace characters",
            ));
        }
        let bytes = value.as_bytes();
        let windows_drive = bytes.len() >= 2
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && bytes.get(2).is_none_or(|byte| matches!(byte, b'/' | b'\\'));
        let host_specific = value.starts_with('/')
            || value.starts_with('~')
            || value.contains('\\')
            || windows_drive
            || matches!(value, "." | "..")
            || value.starts_with("./")
            || value.starts_with("../")
            || value.contains("/./")
            || value.contains("/../")
            || value.ends_with("/.")
            || value.ends_with("/..")
            || selected_graph_coordinate_names_a_local_file_uri(value)
            || selected_graph_coordinate_opens_with_an_encoded_separator(value);
        if host_specific {
            return Err(selected_graph_invalid(
                field,
                "is a machine-local location; a portable identity must not name an absolute, drive-qualified, UNC, home-relative, `file:`-URI or traversing path",
            ));
        }
        Ok(())
    }

    /// Return whether a coordinate names a `file:` URI at any scheme position.
    ///
    /// A `file:` URI is an absolute machine-local path wearing URI clothing, and the shape checks above cannot see
    /// it: `file:///tmp/project` begins with a scheme letter, not a separator, so nothing about it looks like a
    /// location until the scheme is read. It says exactly what `/tmp/project` says, `file:///C:/Users/alice` exactly
    /// what the drive spelling says, and `jar:file:/...` or `git+file://...` hide the same thing one scheme deeper.
    /// URI schemes are case-insensitive under RFC 3986, so this comparison is too.
    ///
    /// Remote schemes are untouched: `https://`, `ssh://` and `git://` name a coordinate every machine resolves the
    /// same way, which is the whole distinction this rule draws.
    fn selected_graph_coordinate_names_a_local_file_uri(value: &str) -> bool {
        let lowered = value.to_ascii_lowercase();
        lowered.starts_with("file:") || lowered.contains(":file:") || lowered.contains("+file:")
    }

    /// Return whether a coordinate opens with a percent-encoded separator or drive colon.
    ///
    /// `%2F` and `%5C` at the front decode to the same absolute path their plain spellings name, so without this the
    /// leading-position checks would sit one encoding away from a bypass. Only the leading position is refused:
    /// percent-encoding deeper in a coordinate is ordinary URL syntax, and a GitLab group path such as
    /// `group%2Fsubgroup` names a remote coordinate rather than a location on this machine.
    fn selected_graph_coordinate_opens_with_an_encoded_separator(value: &str) -> bool {
        let lowered = value.to_ascii_lowercase();
        let bytes = lowered.as_bytes();
        bytes.starts_with(b"%2f")
            || bytes.starts_with(b"%5c")
            || (bytes.first().is_some_and(u8::is_ascii_alphabetic) && bytes.get(1..4) == Some(b"%3a".as_slice()))
    }

    /// Return the one identity vocabulary each source kind may name itself in.
    ///
    /// Every kind carries a scheme so an identity states what it is rather than being inferred from its shape. A
    /// `Path` source is the reason this exists: its physical checkout differs per machine, so its identity must be a
    /// logical name such as `path:foo`, never `/Users/alice/project` or `D:\work\project`. `Compiler` sources are
    /// named by the content digest of the selected `rust-src` tree, which is already the strongest portable identity
    /// available and is what the toolchain producer records.
    const fn selected_graph_source_identity_scheme(kind: OvenSelectedRustFacetSourceKind) -> &'static str {
        match kind {
            OvenSelectedRustFacetSourceKind::Registry => "registry:",
            OvenSelectedRustFacetSourceKind::Git => "git:",
            OvenSelectedRustFacetSourceKind::Path => "path:",
            OvenSelectedRustFacetSourceKind::Generated => "generated:",
            OvenSelectedRustFacetSourceKind::Compiler => "sha256:",
        }
    }

    /// Admit one source identity as portable for its declared kind.
    fn validate_selected_graph_source_identity(
        source: &OvenSelectedRustFacetSource,
        field: &str,
    ) -> Result<(), OvenSelectedRustFacetGraphError> {
        validate_selected_graph_text(&source.identity, field)?;
        if matches!(source.kind, OvenSelectedRustFacetSourceKind::Compiler) {
            return validate_selected_graph_digest(&source.identity, field);
        }
        let scheme = selected_graph_source_identity_scheme(source.kind);
        let coordinate = source.identity.strip_prefix(scheme).ok_or_else(|| {
            selected_graph_invalid(
                field,
                format!(
                    "must name its portable {:?} coordinate as `{scheme}<coordinate>`",
                    source.kind
                ),
            )
        })?;
        validate_selected_graph_portable_coordinate(coordinate, field)
    }

    fn validate_selected_graph_alias(value: &str, field: &str) -> Result<(), OvenSelectedRustFacetGraphError> {
        validate_selected_graph_text(value, field)?;
        if value.contains("::") || value.chars().any(char::is_whitespace) {
            return Err(selected_graph_invalid(field, "is not one Rust-facing alias"));
        }
        Ok(())
    }

    fn validate_selected_graph_sorted_strings(
        values: &[String],
        field: &str,
    ) -> Result<(), OvenSelectedRustFacetGraphError> {
        for value in values {
            validate_selected_graph_text(value, field)?;
        }
        if values.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(selected_graph_invalid(field, "must be sorted and unique"));
        }
        Ok(())
    }

    pub(crate) fn selected_graph_sha256(bytes: &[u8]) -> String {
        format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
    }

    pub(crate) fn selected_graph_source_digest(
        members: &[OvenSelectedRustFacetSourceMember],
    ) -> Result<String, OvenSelectedRustFacetGraphError> {
        if members.is_empty() {
            return Err(selected_graph_missing("source_members"));
        }
        let mut records = BTreeMap::new();
        for (index, member) in members.iter().enumerate() {
            validate_selected_graph_path(&member.path, &format!("source_members[{index}].path"), false)?;
            validate_selected_graph_digest(&member.digest, &format!("source_members[{index}].digest"))?;
            if records.insert(member.path.as_str(), member.digest.as_str()).is_some() {
                return Err(selected_graph_invalid(
                    "source_members",
                    "repeat a portable source path",
                ));
            }
        }
        let bytes = serde_json::to_vec(&records)
            .map_err(|error| selected_graph_invalid("source_members", format!("cannot encode: {error}")))?;
        Ok(selected_graph_sha256(&bytes))
    }

    fn validate_selected_graph_hmac_sha256(value: &str, field: &str) -> Result<(), OvenSelectedRustFacetGraphError> {
        if value.is_empty() {
            return Err(selected_graph_missing(field));
        }
        let valid = value.strip_prefix("hmac-sha256:").is_some_and(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        });
        if !valid {
            return Err(selected_graph_invalid(
                field,
                "must be a producer-keyed `hmac-sha256:` identity, never a raw sensitive value or unkeyed digest",
            ));
        }
        Ok(())
    }

    fn validate_selected_graph_environment_name(
        value: &str,
        field: &str,
    ) -> Result<(), OvenSelectedRustFacetGraphError> {
        validate_selected_graph_text(value, field)?;
        let mut bytes = value.bytes();
        let first = bytes.next().ok_or_else(|| selected_graph_missing(field))?;
        if !(first == b'_' || first.is_ascii_uppercase())
            || bytes.any(|byte| !(byte == b'_' || byte.is_ascii_uppercase() || byte.is_ascii_digit()))
        {
            return Err(selected_graph_invalid(
                field,
                "must use portable uppercase ASCII environment vocabulary",
            ));
        }
        Ok(())
    }

    /// Environment names whose meaning is a public, portable compiler or build fact.
    ///
    /// This registry is the whole of the `Text` capability: an environment value may only be retained verbatim when
    /// its name appears here (or matches the `CARGO_FEATURE_` activation prefix). Secrecy is a property of a value
    /// and its semantics, not of its name, so a name-shaped denylist cannot decide it -- `DATABASE_URL` and
    /// `PRIVATE_MATERIAL` both carry secrets under names no denylist predicts. Everything absent from this registry
    /// therefore fails closed to `SensitiveDigest`, and adding a name here is a deliberate, reviewable act.
    ///
    /// Deliberate omissions: `CARGO_PKG_AUTHORS`, `CARGO_PKG_HOMEPAGE` and their neighbours describe people and
    /// services rather than compilation, and no build fact needs them. Kept alphabetical for review, though nothing
    /// depends on that order.
    const OVEN_SELECTED_RUST_FACET_PUBLIC_ENVIRONMENT_NAMES: &[&str] = &[
        "CARGO_BIN_NAME",
        "CARGO_CFG_PANIC",
        "CARGO_CFG_TARGET_ABI",
        "CARGO_CFG_TARGET_ARCH",
        "CARGO_CFG_TARGET_ENDIAN",
        "CARGO_CFG_TARGET_ENV",
        "CARGO_CFG_TARGET_FAMILY",
        "CARGO_CFG_TARGET_FEATURE",
        "CARGO_CFG_TARGET_HAS_ATOMIC",
        "CARGO_CFG_TARGET_OS",
        "CARGO_CFG_TARGET_POINTER_WIDTH",
        "CARGO_CFG_TARGET_VENDOR",
        "CARGO_CFG_UNIX",
        "CARGO_CFG_WINDOWS",
        "CARGO_CRATE_NAME",
        "CARGO_PKG_NAME",
        "CARGO_PKG_VERSION",
        "CARGO_PKG_VERSION_MAJOR",
        "CARGO_PKG_VERSION_MINOR",
        "CARGO_PKG_VERSION_PATCH",
        "CARGO_PKG_VERSION_PRE",
        "DEBUG",
        "HOST",
        "NUM_JOBS",
        "OPT_LEVEL",
        "PROFILE",
        "TARGET",
    ];

    /// Prefix of the Cargo feature-activation flags, whose semantics are fixed by the prefix itself.
    const OVEN_SELECTED_RUST_FACET_FEATURE_ENVIRONMENT_PREFIX: &str = "CARGO_FEATURE_";

    /// Byte budget for one retained public value.
    ///
    /// The longest genuine public fact is a `CARGO_CFG_TARGET_FEATURE` list, comfortably inside this bound. The limit
    /// exists so a registered public name cannot become a channel for a large opaque blob.
    const OVEN_SELECTED_RUST_FACET_PUBLIC_ENVIRONMENT_TEXT_LIMIT: usize = 512;

    /// The one representation the graph admits for a given environment name.
    ///
    /// Classification is total and closed: every name lands in exactly one class, and an unrecognized name lands in
    /// `Sensitive` rather than in any retaining class.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum OvenSelectedRustFacetEnvironmentClass {
        /// A registered public compiler or build fact, retained verbatim as `Text`.
        PublicText,
        /// A Cargo feature-activation flag, retained as the fixed `Text` value `1`.
        FeatureActivation,
        /// A delivery location, retained as an owner-relative `Path` the physical adapter rebinds.
        OwnerPath,
        /// Anything else, retained only as a keyed `SensitiveDigest` that binds identity without the material.
        Sensitive,
    }

    impl OvenSelectedRustFacetEnvironmentClass {
        /// Describe the only admitted representation, for a refusal a producer can act on.
        const fn required_representation(self) -> &'static str {
            match self {
                Self::PublicText => "a `text` value, because the name is a registered public compiler fact",
                Self::FeatureActivation => "a `text` value of `1`, the only meaning a Cargo feature flag carries",
                Self::OwnerPath => "a `path` value bound to a retained owner, never a machine-local location",
                Self::Sensitive => {
                    "a keyed `sensitive_digest`, because the name is not a registered public compiler fact"
                }
            }
        }
    }

    /// Names whose value is a delivery location and must be rebound through a retained owner.
    fn selected_graph_environment_requires_path(name: &str) -> bool {
        matches!(
            name,
            "OUT_DIR" | "CARGO_MANIFEST_DIR" | "CARGO_MANIFEST_PATH" | "RUSTC" | "RUSTDOC"
        ) || name.ends_with("_DIR")
            || name.ends_with("_PATH")
            || name.ends_with("_ROOT")
            || name.ends_with("_FILE")
    }

    fn selected_graph_environment_is_path_list(name: &str) -> bool {
        matches!(
            name,
            "PATH" | "LD_LIBRARY_PATH" | "DYLD_LIBRARY_PATH" | "DYLD_FALLBACK_LIBRARY_PATH"
        )
    }

    /// Classify one environment name into the single representation the graph admits for it.
    fn selected_graph_environment_class(name: &str) -> OvenSelectedRustFacetEnvironmentClass {
        if OVEN_SELECTED_RUST_FACET_PUBLIC_ENVIRONMENT_NAMES.contains(&name) {
            return OvenSelectedRustFacetEnvironmentClass::PublicText;
        }
        if name.starts_with(OVEN_SELECTED_RUST_FACET_FEATURE_ENVIRONMENT_PREFIX) {
            return OvenSelectedRustFacetEnvironmentClass::FeatureActivation;
        }
        if selected_graph_environment_requires_path(name) {
            return OvenSelectedRustFacetEnvironmentClass::OwnerPath;
        }
        OvenSelectedRustFacetEnvironmentClass::Sensitive
    }

    /// Admit one retained public value against the closed portable alphabet.
    ///
    /// Public compiler facts are short unquoted tokens, version fragments, or comma-separated lists, so the alphabet
    /// is `A-Za-z0-9` plus `. , + - _`. Excluding `/`, `\` and `:` is what makes the rule closed rather than
    /// advisory: it refuses machine-local paths and credential-bearing URIs such as
    /// `postgres://user:password@host/db` by construction, instead of by recognizing them. An empty value stays
    /// admissible -- `CARGO_PKG_VERSION_PRE` is legitimately empty, and that is a checked fact, not missing evidence.
    ///
    /// A refusal never echoes the offending value: these errors reach logs, and the value may be exactly what must
    /// not escape.
    fn validate_selected_graph_public_environment_text(
        value: &str,
        field: &str,
    ) -> Result<(), OvenSelectedRustFacetGraphError> {
        if value.len() > OVEN_SELECTED_RUST_FACET_PUBLIC_ENVIRONMENT_TEXT_LIMIT {
            return Err(selected_graph_invalid(
                field,
                format!(
                    "exceeds the {OVEN_SELECTED_RUST_FACET_PUBLIC_ENVIRONMENT_TEXT_LIMIT}-byte budget for a retained public value"
                ),
            ));
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b',' | b'+' | b'-' | b'_'))
        {
            return Err(selected_graph_invalid(
                field,
                "leaves the portable public alphabet `A-Za-z0-9.,+-_`; a location must use a path value and anything else a keyed sensitive identity",
            ));
        }
        Ok(())
    }

    fn canonicalize_selected_graph_selection(selection: &mut OvenSelectedRustFacetSelection) {
        selection.intent.features.sort();
    }

    fn canonicalize_selected_graph_unit(unit: &mut OvenSelectedRustFacetUnit) {
        unit.source_members.sort();
        unit.features.sort();
        unit.cfg.sort();
        unit.include_dirs.sort();
        unit.exclude_dirs.sort();
        unit.dependencies
            .sort_by(|left, right| left.alias.cmp(&right.alias).then_with(|| left.unit.cmp(&right.unit)));
        unit.generated_inputs.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.source.cmp(&right.source))
                .then_with(|| left.digest.cmp(&right.digest))
        });
    }

    fn canonicalize_selected_graph(graph: &mut OvenSelectedRustFacetGraph) {
        canonicalize_selected_graph_selection(&mut graph.selection);
        graph.owners.sort_by(|left, right| {
            left.identity
                .cmp(&right.identity)
                .then_with(|| left.kind.cmp(&right.kind))
        });
        for unit in &mut graph.units {
            canonicalize_selected_graph_unit(unit);
        }
        graph.units.sort_by(|left, right| left.identity.cmp(&right.identity));
    }

    #[derive(Serialize)]
    struct OvenSelectedRustFacetUnitIdentityInput<'a> {
        toolchain: &'a str,
        toolchain_owner: &'a str,
        toolchain_version: &'a str,
        profile: &'a str,
        compilation_target: &'a str,
        target_spec: Option<&'a OvenSelectedRustFacetTargetSpec>,
        package: &'a str,
        package_version: &'a str,
        crate_name: &'a str,
        crate_kind: OvenSelectedRustFacetCrateKind,
        role: OvenSelectedRustFacetUnitRole,
        domain: OvenSelectedRustFacetDomain,
        edition: &'a str,
        source: &'a OvenSelectedRustFacetSource,
        root_module: &'a str,
        source_members: &'a [OvenSelectedRustFacetSourceMember],
        features: &'a [String],
        default_features: bool,
        cfg: &'a [String],
        environment: &'a BTreeMap<String, OvenSelectedRustFacetEnvironmentValue>,
        include_dirs: &'a [OvenSelectedRustFacetPath],
        exclude_dirs: &'a [OvenSelectedRustFacetPath],
        dependencies: &'a [OvenSelectedRustFacetDependency],
        generated_inputs: &'a [OvenSelectedRustFacetGeneratedInput],
    }

    /// Derive one canonical unit identity from effective compiler inputs and canonical dependency identities.
    pub(crate) fn selected_graph_unit_identity(
        selection: &OvenSelectedRustFacetSelection,
        unit: &OvenSelectedRustFacetUnit,
    ) -> Result<String, OvenSelectedRustFacetGraphError> {
        let mut selection = selection.clone();
        canonicalize_selected_graph_selection(&mut selection);
        let mut unit = unit.clone();
        canonicalize_selected_graph_unit(&mut unit);
        let (compilation_target, target_spec) = match unit.domain {
            OvenSelectedRustFacetDomain::Host => (selection.host.as_str(), None),
            OvenSelectedRustFacetDomain::Target => (selection.intent.target.as_str(), Some(&selection.target_spec)),
        };
        let input = OvenSelectedRustFacetUnitIdentityInput {
            toolchain: &selection.intent.toolchain,
            toolchain_owner: &selection.target_spec.source.owner,
            toolchain_version: &selection.toolchain_version,
            profile: &selection.intent.profile,
            compilation_target,
            target_spec,
            package: &unit.package,
            package_version: &unit.package_version,
            crate_name: &unit.crate_name,
            crate_kind: unit.crate_kind,
            role: unit.role,
            domain: unit.domain,
            edition: &unit.edition,
            source: &unit.source,
            root_module: &unit.root_module,
            source_members: &unit.source_members,
            features: &unit.features,
            default_features: unit.default_features,
            cfg: &unit.cfg,
            environment: &unit.environment,
            include_dirs: &unit.include_dirs,
            exclude_dirs: &unit.exclude_dirs,
            dependencies: &unit.dependencies,
            generated_inputs: &unit.generated_inputs,
        };
        let bytes = serde_json::to_vec(&(OVEN_SELECTED_RUST_FACET_UNIT_DIGEST_DOMAIN, input))
            .map_err(|error| selected_graph_invalid("unit identity", format!("cannot encode: {error}")))?;
        Ok(selected_graph_sha256(&bytes))
    }

    /// Return whether the explicit role, domain and crate-kind fields describe one RFC 119 inspection unit.
    pub(crate) const fn selected_graph_unit_role_is_valid(
        role: OvenSelectedRustFacetUnitRole,
        domain: OvenSelectedRustFacetDomain,
        crate_kind: OvenSelectedRustFacetCrateKind,
    ) -> bool {
        matches!(
            (role, domain, crate_kind),
            (
                OvenSelectedRustFacetUnitRole::Library | OvenSelectedRustFacetUnitRole::CompilerSupport,
                OvenSelectedRustFacetDomain::Host | OvenSelectedRustFacetDomain::Target,
                OvenSelectedRustFacetCrateKind::Rlib
            ) | (
                OvenSelectedRustFacetUnitRole::Binary
                    | OvenSelectedRustFacetUnitRole::UnitTest
                    | OvenSelectedRustFacetUnitRole::IntegrationTest
                    | OvenSelectedRustFacetUnitRole::Example
                    | OvenSelectedRustFacetUnitRole::Doctest
                    | OvenSelectedRustFacetUnitRole::Benchmark,
                OvenSelectedRustFacetDomain::Target,
                OvenSelectedRustFacetCrateKind::Binary
            ) | (
                OvenSelectedRustFacetUnitRole::ProcMacro,
                OvenSelectedRustFacetDomain::Host,
                OvenSelectedRustFacetCrateKind::ProcMacro
            ) | (
                OvenSelectedRustFacetUnitRole::CallerProjection,
                OvenSelectedRustFacetDomain::Target,
                OvenSelectedRustFacetCrateKind::Rlib
            )
        )
    }

    fn validate_selected_graph_owner_reference(
        owner: &str,
        owners: &BTreeMap<&str, OvenSelectedRustFacetOwnerKind>,
        field: &str,
    ) -> Result<OvenSelectedRustFacetOwnerKind, OvenSelectedRustFacetGraphError> {
        validate_selected_graph_digest(owner, field)?;
        owners
            .get(owner)
            .copied()
            .ok_or_else(|| selected_graph_missing(format!("{field} owner `{owner}`")))
    }

    fn validate_selected_graph_path_reference(
        value: &OvenSelectedRustFacetPath,
        owners: &BTreeMap<&str, OvenSelectedRustFacetOwnerKind>,
        field: &str,
        allow_owner_root: bool,
    ) -> Result<OvenSelectedRustFacetOwnerKind, OvenSelectedRustFacetGraphError> {
        let kind = validate_selected_graph_owner_reference(&value.owner, owners, field)?;
        validate_selected_graph_path(&value.path, field, allow_owner_root)?;
        Ok(kind)
    }

    /// Admit one environment binding against the closed name classification.
    ///
    /// The match below is the whole policy: each class has exactly one admitting arm, and every other pairing falls
    /// to the refusal arm. A name nobody has classified as public therefore cannot become serializable by omission,
    /// which is the property a denylist could never give.
    fn validate_selected_graph_environment(
        name: &str,
        value: &OvenSelectedRustFacetEnvironmentValue,
        owners: &BTreeMap<&str, OvenSelectedRustFacetOwnerKind>,
        referenced_owners: &mut BTreeSet<String>,
        field: &str,
    ) -> Result<(), OvenSelectedRustFacetGraphError> {
        validate_selected_graph_environment_name(name, field)?;
        if selected_graph_environment_is_path_list(name) {
            return Err(selected_graph_invalid(
                field,
                "ambient path-list environment is not representable in an inspection graph",
            ));
        }
        let class = selected_graph_environment_class(name);
        match (class, value) {
            (
                OvenSelectedRustFacetEnvironmentClass::PublicText,
                OvenSelectedRustFacetEnvironmentValue::Text { value },
            ) => validate_selected_graph_public_environment_text(value, field),
            (
                OvenSelectedRustFacetEnvironmentClass::FeatureActivation,
                OvenSelectedRustFacetEnvironmentValue::Text { value },
            ) if value == "1" => Ok(()),
            (
                OvenSelectedRustFacetEnvironmentClass::OwnerPath,
                OvenSelectedRustFacetEnvironmentValue::Path { value },
            ) => {
                validate_selected_graph_path_reference(value, owners, field, true)?;
                referenced_owners.insert(value.owner.clone());
                Ok(())
            }
            (
                OvenSelectedRustFacetEnvironmentClass::Sensitive,
                OvenSelectedRustFacetEnvironmentValue::SensitiveDigest { hmac_sha256 },
            ) => validate_selected_graph_hmac_sha256(hmac_sha256, field),
            (class, _) => Err(selected_graph_invalid(
                field,
                format!("must be retained as {}", class.required_representation()),
            )),
        }
    }

    impl OvenSelectedRustFacetGraph {
        /// Decode a graph only after its minimal schema header selects this reader.
        pub(crate) fn decode_validated(
            bytes: &[u8],
        ) -> Result<ValidatedOvenSelectedRustFacetGraph, OvenSelectedRustFacetGraphError> {
            let header = serde_json::from_slice::<OvenSelectedRustFacetGraphHeader>(bytes).map_err(|error| {
                selected_graph_invalid("graph header", format!("cannot decode schema version: {error}"))
            })?;
            if header.schema_version != OVEN_SELECTED_RUST_FACET_GRAPH_SCHEMA_VERSION {
                return Err(OvenSelectedRustFacetGraphError::UnsupportedSchema {
                    found: header.schema_version,
                    expected: OVEN_SELECTED_RUST_FACET_GRAPH_SCHEMA_VERSION,
                });
            }
            let graph = serde_json::from_slice::<Self>(bytes)
                .map_err(|error| selected_graph_invalid("graph", format!("cannot decode v1 payload: {error}")))?;
            graph.validated()
        }

        /// Validate complete evidence after canonical insertion-order normalization.
        fn validate_shape(&self) -> Result<(), OvenSelectedRustFacetGraphError> {
            if self.schema_version != OVEN_SELECTED_RUST_FACET_GRAPH_SCHEMA_VERSION {
                return Err(OvenSelectedRustFacetGraphError::UnsupportedSchema {
                    found: self.schema_version,
                    expected: OVEN_SELECTED_RUST_FACET_GRAPH_SCHEMA_VERSION,
                });
            }
            for (field, value) in [
                ("selection.host", self.selection.host.as_str()),
                ("selection.intent.target", self.selection.intent.target.as_str()),
                ("selection.intent.toolchain", self.selection.intent.toolchain.as_str()),
                ("selection.intent.profile", self.selection.intent.profile.as_str()),
                ("selection.toolchain_version", self.selection.toolchain_version.as_str()),
            ] {
                validate_selected_graph_text(value, field)?;
            }
            semver::Version::parse(&self.selection.toolchain_version).map_err(|error| {
                selected_graph_invalid(
                    "selection.toolchain_version",
                    format!("is not semantic version evidence: {error}"),
                )
            })?;
            validate_selected_graph_digest(&self.selection.target_spec.digest, "selection.target_spec.digest")?;
            validate_selected_graph_sorted_strings(&self.selection.intent.features, "selection.intent.features")?;

            if self.owners.is_empty() {
                return Err(selected_graph_missing("owners"));
            }
            let mut owners = BTreeMap::new();
            let mut toolchain_owner_count = 0usize;
            for (index, owner) in self.owners.iter().enumerate() {
                let field = format!("owners[{index}].identity");
                validate_selected_graph_digest(&owner.identity, &field)?;
                if owners.insert(owner.identity.as_str(), owner.kind).is_some() {
                    return Err(selected_graph_invalid(field, "repeats an owner identity"));
                }
                if owner.kind == OvenSelectedRustFacetOwnerKind::Toolchain {
                    toolchain_owner_count += 1;
                }
            }
            if toolchain_owner_count != 1 {
                return Err(selected_graph_invalid(
                    "owners",
                    format!("must contain exactly one Toolchain owner; found {toolchain_owner_count}"),
                ));
            }
            let mut referenced_owners = BTreeSet::new();
            let target_spec_kind = validate_selected_graph_path_reference(
                &self.selection.target_spec.source,
                &owners,
                "selection.target_spec.source",
                false,
            )?;
            if target_spec_kind != OvenSelectedRustFacetOwnerKind::Toolchain {
                return Err(selected_graph_invalid(
                    "selection.target_spec.source.owner",
                    "must reference a Toolchain owner",
                ));
            }
            referenced_owners.insert(self.selection.target_spec.source.owner.clone());

            if self.units.is_empty() {
                return Err(selected_graph_missing("units"));
            }
            let mut unit_indexes = BTreeMap::new();
            for (index, unit) in self.units.iter().enumerate() {
                let field = format!("units[{index}].identity");
                validate_selected_graph_digest(&unit.identity, &field)?;
                if unit_indexes.insert(unit.identity.as_str(), index).is_some() {
                    return Err(selected_graph_invalid(field, "repeats a unit identity"));
                }
            }

            for (index, unit) in self.units.iter().enumerate() {
                let field = format!("units[{index}]");
                for (name, value) in [
                    ("package", unit.package.as_str()),
                    ("package_version", unit.package_version.as_str()),
                    ("crate_name", unit.crate_name.as_str()),
                    ("edition", unit.edition.as_str()),
                ] {
                    validate_selected_graph_text(value, &format!("{field}.{name}"))?;
                }
                semver::Version::parse(&unit.package_version).map_err(|error| {
                    selected_graph_invalid(
                        format!("{field}.package_version"),
                        format!("is not an exact semantic version: {error}"),
                    )
                })?;
                if !matches!(unit.edition.as_str(), "2015" | "2018" | "2021" | "2024") {
                    return Err(selected_graph_invalid(format!("{field}.edition"), "is unsupported"));
                }
                if !selected_graph_unit_role_is_valid(unit.role, unit.domain, unit.crate_kind) {
                    return Err(selected_graph_invalid(
                        format!("{field}.role"),
                        format!(
                            "role {:?} cannot use {:?} domain with {:?} crate kind",
                            unit.role, unit.domain, unit.crate_kind
                        ),
                    ));
                }

                let source_owner_kind = validate_selected_graph_owner_reference(
                    &unit.source.owner,
                    &owners,
                    &format!("{field}.source.owner"),
                )?;
                if matches!(unit.source.kind, OvenSelectedRustFacetSourceKind::Compiler)
                    != matches!(source_owner_kind, OvenSelectedRustFacetOwnerKind::Toolchain)
                {
                    return Err(selected_graph_invalid(
                        format!("{field}.source.owner"),
                        "Compiler source must use the Toolchain owner and other source kinds must not",
                    ));
                }
                // Owner coherence settles what kind of source this really is before its identity spelling is judged.
                validate_selected_graph_source_identity(&unit.source, &format!("{field}.source.identity"))?;
                referenced_owners.insert(unit.source.owner.clone());
                validate_selected_graph_path(&unit.source.root, &format!("{field}.source.root"), true)?;
                validate_selected_graph_digest(&unit.source.digest, &format!("{field}.source.digest"))?;
                validate_selected_graph_path(&unit.root_module, &format!("{field}.root_module"), false)?;
                if unit.source_members.is_empty() {
                    return Err(selected_graph_missing(format!("{field}.source_members")));
                }
                for (member_index, member) in unit.source_members.iter().enumerate() {
                    validate_selected_graph_path(
                        &member.path,
                        &format!("{field}.source_members[{member_index}].path"),
                        false,
                    )?;
                    validate_selected_graph_digest(
                        &member.digest,
                        &format!("{field}.source_members[{member_index}].digest"),
                    )?;
                }
                if unit.source_members.windows(2).any(|pair| pair[0].path >= pair[1].path) {
                    return Err(selected_graph_invalid(
                        format!("{field}.source_members"),
                        "repeat a portable source path",
                    ));
                }
                if !unit.source_members.iter().any(|member| member.path == unit.root_module) {
                    return Err(selected_graph_missing(format!("{field}.root_module source member")));
                }
                if selected_graph_source_digest(&unit.source_members)? != unit.source.digest {
                    return Err(selected_graph_invalid(
                        format!("{field}.source.digest"),
                        "does not bind the complete source member catalog",
                    ));
                }

                validate_selected_graph_sorted_strings(&unit.features, &format!("{field}.features"))?;
                validate_selected_graph_sorted_strings(&unit.cfg, &format!("{field}.cfg"))?;
                for (name, value) in &unit.environment {
                    validate_selected_graph_environment(
                        name,
                        value,
                        &owners,
                        &mut referenced_owners,
                        &format!("{field}.environment.{name}"),
                    )?;
                }
                if unit.include_dirs.is_empty() {
                    return Err(selected_graph_missing(format!("{field}.include_dirs")));
                }
                for (name, paths) in [
                    ("include_dirs", unit.include_dirs.as_slice()),
                    ("exclude_dirs", unit.exclude_dirs.as_slice()),
                ] {
                    if paths.windows(2).any(|pair| pair[0] >= pair[1]) {
                        return Err(selected_graph_invalid(
                            format!("{field}.{name}"),
                            "repeat an owner-relative path",
                        ));
                    }
                    for path in paths {
                        validate_selected_graph_path_reference(path, &owners, &format!("{field}.{name}"), true)?;
                        referenced_owners.insert(path.owner.clone());
                    }
                }
                for (dependency_index, dependency) in unit.dependencies.iter().enumerate() {
                    validate_selected_graph_alias(
                        &dependency.alias,
                        &format!("{field}.dependencies[{dependency_index}].alias"),
                    )?;
                    validate_selected_graph_digest(
                        &dependency.unit,
                        &format!("{field}.dependencies[{dependency_index}].unit"),
                    )?;
                    if !unit_indexes.contains_key(dependency.unit.as_str()) {
                        return Err(selected_graph_missing(format!(
                            "{field}.dependencies[{dependency_index}] unit `{}`",
                            dependency.unit
                        )));
                    }
                }
                if unit.dependencies.windows(2).any(|pair| pair[0].alias >= pair[1].alias) {
                    return Err(selected_graph_invalid(
                        format!("{field}.dependencies"),
                        "repeat a Rust-facing alias",
                    ));
                }
                for (generated_index, generated) in unit.generated_inputs.iter().enumerate() {
                    validate_selected_graph_text(
                        &generated.name,
                        &format!("{field}.generated_inputs[{generated_index}].name"),
                    )?;
                    validate_selected_graph_path_reference(
                        &generated.source,
                        &owners,
                        &format!("{field}.generated_inputs[{generated_index}].source"),
                        true,
                    )?;
                    referenced_owners.insert(generated.source.owner.clone());
                    validate_selected_graph_digest(
                        &generated.digest,
                        &format!("{field}.generated_inputs[{generated_index}].digest"),
                    )?;
                }
                if unit
                    .generated_inputs
                    .windows(2)
                    .any(|pair| pair[0].name >= pair[1].name)
                {
                    return Err(selected_graph_invalid(
                        format!("{field}.generated_inputs"),
                        "repeat a generated-input name",
                    ));
                }
            }

            if self.exposed_roots.is_empty() {
                return Err(selected_graph_missing("exposed_roots"));
            }
            for (alias, unit) in &self.exposed_roots {
                validate_selected_graph_alias(alias, "exposed_roots alias")?;
                validate_selected_graph_digest(unit, &format!("exposed_roots.{alias}"))?;
                if !unit_indexes.contains_key(unit.as_str()) {
                    return Err(selected_graph_missing(format!("exposed_roots.{alias} unit `{unit}`")));
                }
            }

            let mut incoming = self
                .units
                .iter()
                .map(|unit| unit.dependencies.len())
                .collect::<Vec<_>>();
            let mut dependents = vec![Vec::new(); self.units.len()];
            for (index, unit) in self.units.iter().enumerate() {
                for dependency in &unit.dependencies {
                    let selected = unit_indexes.get(dependency.unit.as_str()).copied().ok_or_else(|| {
                        selected_graph_missing(format!("units[{index}] dependency unit `{}`", dependency.unit))
                    })?;
                    dependents[selected].push(index);
                }
            }
            let mut ready = incoming
                .iter()
                .enumerate()
                .filter_map(|(index, count)| (*count == 0).then_some(index))
                .collect::<Vec<_>>();
            let mut visited = 0usize;
            while let Some(index) = ready.pop() {
                visited += 1;
                for dependent in &dependents[index] {
                    incoming[*dependent] -= 1;
                    if incoming[*dependent] == 0 {
                        ready.push(*dependent);
                    }
                }
            }
            if visited != self.units.len() {
                return Err(selected_graph_invalid("units.dependencies", "contain a cycle"));
            }

            let mut reachable = BTreeSet::new();
            let mut pending = self
                .exposed_roots
                .values()
                .filter_map(|identity| unit_indexes.get(identity.as_str()).copied())
                .collect::<Vec<_>>();
            while let Some(index) = pending.pop() {
                if !reachable.insert(index) {
                    continue;
                }
                pending.extend(
                    self.units[index]
                        .dependencies
                        .iter()
                        .filter_map(|dependency| unit_indexes.get(dependency.unit.as_str()).copied()),
                );
            }
            if reachable.len() != self.units.len() {
                let detached = self
                    .units
                    .iter()
                    .enumerate()
                    .find(|(index, _)| !reachable.contains(index))
                    .map(|(_, unit)| unit.identity.as_str())
                    .ok_or_else(|| selected_graph_invalid("units", "reachability accounting failed"))?;
                return Err(selected_graph_invalid(
                    "units",
                    format!("unit `{detached}` is not reachable from an exposed root"),
                ));
            }

            if let Some(detached) = self
                .owners
                .iter()
                .find(|owner| !referenced_owners.contains(owner.identity.as_str()))
            {
                return Err(selected_graph_invalid(
                    "owners",
                    format!("owner `{}` is not referenced by the selection", detached.identity),
                ));
            }

            for (index, unit) in self.units.iter().enumerate() {
                let expected = selected_graph_unit_identity(&self.selection, unit)?;
                if unit.identity != expected {
                    return Err(selected_graph_invalid(
                        format!("units[{index}].identity"),
                        format!("does not match canonical unit identity {expected}"),
                    ));
                }
            }
            Ok(())
        }

        /// Canonicalize, validate and hash the portable graph without ambient discovery.
        pub(crate) fn validated(
            mut self,
        ) -> Result<ValidatedOvenSelectedRustFacetGraph, OvenSelectedRustFacetGraphError> {
            canonicalize_selected_graph(&mut self);
            self.validate_shape()?;
            let bytes =
                serde_json::to_vec(&(OVEN_SELECTED_RUST_FACET_GRAPH_DIGEST_DOMAIN, &self)).map_err(|error| {
                    selected_graph_invalid("graph", format!("cannot encode deterministic digest input: {error}"))
                })?;
            Ok(ValidatedOvenSelectedRustFacetGraph {
                graph: self,
                digest: selected_graph_sha256(&bytes),
            })
        }
    }

    #[allow(dead_code, reason = "first selected-input producer slice; production wiring follows")]
    impl ValidatedOvenSelectedRustFacetGraph {
        /// Borrow the validated portable graph.
        pub(crate) fn graph(&self) -> &OvenSelectedRustFacetGraph {
            &self.graph
        }

        /// Return the graph's location-independent content identity.
        pub(crate) fn digest(&self) -> &str {
            &self.digest
        }

        /// Encode the admitted graph for a Store payload or inspection sidecar.
        pub(crate) fn to_json_bytes(&self) -> Result<Vec<u8>, OvenSelectedRustFacetGraphError> {
            serde_json::to_vec(&self.graph)
                .map_err(|error| selected_graph_invalid("graph", format!("cannot encode wire payload: {error}")))
        }

        /// Recover the validated graph for ownership transfer into a publisher.
        pub(crate) fn into_graph(self) -> OvenSelectedRustFacetGraph {
            self.graph
        }
    }
}

#[allow(unused_imports, reason = "production producer wiring follows this contract slice")]
pub(crate) use selected_rust_facet_graph::*;

/// Exact immutable source owner named by a project inspection authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "owner", rename_all = "snake_case")]
pub(crate) enum OvenProjectInspectionSourceOwner {
    /// The small project authority entry materializes this source fragment itself.
    Authority,
    /// One exact constituent supplies the source tree at the catalog's relative root.
    Constituent { index: usize },
}

/// One exact immutable constituent of a project inspection authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum OvenProjectInspectionConstituent {
    /// A compiler-shipped release Loaf retained by its immutable toolchain generation.
    ReleaseLoaf {
        loaf_identity: String,
        build_unit_identity: String,
        receipt: OvenReceipt,
    },
    /// A receipt-bound entry in the bounded project store.
    Stored {
        identity: String,
        artifact_kind: OvenArtifactKind,
        receipt: OvenReceipt,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base_loaf_identity: Option<String>,
    },
}

/// One canonical registry source and the exact immutable root that owns its bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OvenProjectInspectionSource {
    pub package: OvenRustcRegistrySourcePackage,
    pub owner: OvenProjectInspectionSourceOwner,
}

/// One exact normal or dev registry root selected for project Rust inspection.
///
/// The locked package identity proves which source tree Cargo selected at the explicit bake boundary. The requested
/// feature contract remains separate because two source-identical root edges can expose different Rust APIs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OvenProjectInspectionRootDependency {
    pub alias: String,
    pub package: String,
    pub version: String,
    pub registry: String,
    pub checksum: String,
    pub requested_features: Vec<String>,
    pub default_features: bool,
}

/// Exact project-owned dependency envelope used only by generated native tests.
///
/// The envelope promotes the canonical normal and dev dependency surface into one checked debug executable closure at
/// the explicit project-bake boundary. Its dependency digest is deliberately independent of authored test bytes, so
/// unchanged dependency declarations reuse the same Loaf while every generated harness remains caller-owned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OvenProjectInspectionTestDependencyEnvelope {
    pub constituent_index: usize,
    pub dependency_surface_digest: String,
    pub dependency_roots: BTreeMap<String, OvenProjectInspectionTestDependencyRoot>,
}

/// Exact per-root evidence admitted by the generated native-test dependency envelope.
///
/// Registry roots retain Cargo's locked package/source identity as well as the declared edge digest. Path and Git
/// roots carry their complete portable declaration/source digest; for paths that digest includes the source tree,
/// never its machine-local spelling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub(crate) enum OvenProjectInspectionTestDependencyRoot {
    Registry {
        dependency_digest: String,
        locked: OvenProjectInspectionRootDependency,
    },
    Path {
        dependency_digest: String,
    },
    Git {
        dependency_digest: String,
    },
}

/// Singular source authority published once by an explicit project bake.
///
/// The authority owns one canonical publisher lock and exact normal/dev root-edge records. Its source catalog is a
/// composition of named immutable constituents plus only those bounded source fragments absent from every
/// constituent; normal commands never union independent locks or search the store by dependency compatibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OvenProjectInspectionAuthorityPayload {
    pub schema_version: u32,
    pub project_identity: String,
    pub source_authority_digest: String,
    pub compiler_version: String,
    pub registry_lock_digest: String,
    #[serde(default)]
    pub registry_source_dependencies: Vec<OvenProjectInspectionRootDependency>,
    #[serde(default)]
    pub dev_registry_source_dependencies: Vec<OvenProjectInspectionRootDependency>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_dependency_envelope: Option<OvenProjectInspectionTestDependencyEnvelope>,
    #[serde(default)]
    pub constituents: Vec<OvenProjectInspectionConstituent>,
    #[serde(default)]
    pub registry_sources: Vec<OvenProjectInspectionSource>,
}

/// Exact authority entry named by a source-current completed project output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OvenProjectInspectionAuthorityRef {
    pub identity: String,
    pub receipt_identity: String,
    pub build_unit_identity: String,
}

/// Source-current singular project authority with every bounded-store constituent leased in one batch.
pub(crate) struct OvenLoadedProjectInspectionAuthority {
    source_owner: OvenStoreExecutionPayload,
    pub(crate) payload: OvenProjectInspectionAuthorityPayload,
    pub(crate) stored_constituents: Vec<OvenStoreExecutionPayload>,
    pub(super) lineage_leases: Vec<OvenStoreLease>,
}

impl OvenLoadedProjectInspectionAuthority {
    /// Retain one validated authority owner together with every store-owned constituent it names.
    pub(super) fn new(
        source_owner: OvenStoreExecutionPayload,
        payload: OvenProjectInspectionAuthorityPayload,
        stored_constituents: Vec<OvenStoreExecutionPayload>,
    ) -> Self {
        Self {
            source_owner,
            payload,
            stored_constituents,
            lineage_leases: Vec::new(),
        }
    }

    /// Return the immutable Store identity that owns the authority payload and materialized files.
    pub(crate) fn identity(&self) -> &str {
        &self.source_owner.manifest.identity
    }

    /// Return the materialized root derived from the retained authority owner.
    pub(crate) fn artifact_root(&self) -> &Path {
        &self.source_owner.artifact_root
    }

    /// Retain completed-output leases for the complete inspection command.
    pub(crate) fn retain_lineage_leases(&mut self, leases: Vec<OvenStoreLease>) {
        self.lineage_leases = leases;
    }
}

#[cfg(test)]
mod selected_rust_facet_graph_tests {
    use std::fs;
    use std::path::Path;

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn dependency_owner_identity() -> String {
        selected_graph_sha256(b"dependency owner")
    }

    fn project_owner_identity(root_bytes: &[u8]) -> String {
        selected_graph_sha256(root_bytes)
    }

    fn toolchain_owner_identity() -> String {
        selected_graph_sha256(b"toolchain inspection closure")
    }

    fn member(path: &str, bytes: &[u8]) -> OvenSelectedRustFacetSourceMember {
        OvenSelectedRustFacetSourceMember {
            path: path.to_string(),
            digest: selected_graph_sha256(bytes),
        }
    }

    fn source(
        kind: OvenSelectedRustFacetSourceKind,
        identity: &str,
        owner: &str,
        members: &[OvenSelectedRustFacetSourceMember],
    ) -> Result<OvenSelectedRustFacetSource, OvenSelectedRustFacetGraphError> {
        Ok(OvenSelectedRustFacetSource {
            kind,
            identity: identity.to_string(),
            owner: owner.to_string(),
            root: ".".to_string(),
            digest: selected_graph_source_digest(members)?,
        })
    }

    fn selection() -> OvenSelectedRustFacetSelection {
        OvenSelectedRustFacetSelection {
            intent: OvenSelectedRustFacetIntent {
                target: "x86_64-unknown-linux-gnu".to_string(),
                toolchain: "rustc 1.85.0 (fixture)".to_string(),
                profile: "dev".to_string(),
                features: vec!["root-feature".to_string()],
            },
            host: "aarch64-apple-darwin".to_string(),
            purpose: OvenSelectedRustFacetPurpose::Normal,
            default_features: true,
            toolchain_version: "1.85.0".to_string(),
            target_spec: OvenSelectedRustFacetTargetSpec {
                source: OvenSelectedRustFacetPath {
                    owner: toolchain_owner_identity(),
                    path: "target-specs/x86_64-unknown-linux-gnu.json".to_string(),
                },
                digest: selected_graph_sha256(b"fixture target spec"),
            },
        }
    }

    fn leaf_unit(
        selection: &OvenSelectedRustFacetSelection,
    ) -> Result<OvenSelectedRustFacetUnit, OvenSelectedRustFacetGraphError> {
        let source_members = vec![member("src/lib.rs", b"pub struct Dependency;\n")];
        let mut unit = OvenSelectedRustFacetUnit {
            identity: String::new(),
            package: "dependency-package".to_string(),
            package_version: "1.2.3".to_string(),
            crate_name: "dependency_crate".to_string(),
            crate_kind: OvenSelectedRustFacetCrateKind::Rlib,
            role: OvenSelectedRustFacetUnitRole::Library,
            domain: OvenSelectedRustFacetDomain::Target,
            edition: "2021".to_string(),
            source: source(
                OvenSelectedRustFacetSourceKind::Registry,
                "registry:dependency-package@1.2.3",
                &dependency_owner_identity(),
                &source_members,
            )?,
            root_module: "src/lib.rs".to_string(),
            source_members,
            features: Vec::new(),
            default_features: false,
            cfg: Vec::new(),
            environment: BTreeMap::new(),
            include_dirs: vec![OvenSelectedRustFacetPath {
                owner: dependency_owner_identity(),
                path: ".".to_string(),
            }],
            exclude_dirs: Vec::new(),
            dependencies: Vec::new(),
            generated_inputs: Vec::new(),
        };
        unit.identity = selected_graph_unit_identity(selection, &unit)?;
        Ok(unit)
    }

    fn root_unit(
        selection: &OvenSelectedRustFacetSelection,
        dependency_identity: Option<&str>,
        bytes: &[u8],
    ) -> Result<OvenSelectedRustFacetUnit, OvenSelectedRustFacetGraphError> {
        let source_members = vec![member("src/lib.rs", bytes)];
        let dependencies = dependency_identity.map_or_else(Vec::new, |identity| {
            vec![OvenSelectedRustFacetDependency {
                alias: "renamed_dep".to_string(),
                unit: identity.to_string(),
            }]
        });
        let owner = project_owner_identity(bytes);
        let mut unit = OvenSelectedRustFacetUnit {
            identity: String::new(),
            package: "fixture".to_string(),
            package_version: "0.1.0".to_string(),
            crate_name: "fixture".to_string(),
            crate_kind: OvenSelectedRustFacetCrateKind::Rlib,
            role: OvenSelectedRustFacetUnitRole::Library,
            domain: OvenSelectedRustFacetDomain::Target,
            edition: "2024".to_string(),
            source: source(
                OvenSelectedRustFacetSourceKind::Generated,
                "generated:fixture",
                &owner,
                &source_members,
            )?,
            root_module: "src/lib.rs".to_string(),
            source_members,
            features: vec!["root-feature".to_string()],
            default_features: true,
            cfg: vec!["feature=\"root-feature\"".to_string()],
            environment: BTreeMap::new(),
            include_dirs: vec![OvenSelectedRustFacetPath {
                owner,
                path: ".".to_string(),
            }],
            exclude_dirs: Vec::new(),
            dependencies,
            generated_inputs: Vec::new(),
        };
        unit.identity = selected_graph_unit_identity(selection, &unit)?;
        Ok(unit)
    }

    fn graph(root_bytes: &[u8]) -> Result<OvenSelectedRustFacetGraph, OvenSelectedRustFacetGraphError> {
        let selection = selection();
        let leaf = leaf_unit(&selection)?;
        let root = root_unit(&selection, Some(&leaf.identity), root_bytes)?;
        let exposed_root = root.identity.clone();
        Ok(OvenSelectedRustFacetGraph {
            schema_version: OVEN_SELECTED_RUST_FACET_GRAPH_SCHEMA_VERSION,
            selection,
            owners: vec![
                OvenSelectedRustFacetOwner {
                    identity: project_owner_identity(root_bytes),
                    kind: OvenSelectedRustFacetOwnerKind::ProjectAuthority,
                },
                OvenSelectedRustFacetOwner {
                    identity: toolchain_owner_identity(),
                    kind: OvenSelectedRustFacetOwnerKind::Toolchain,
                },
                OvenSelectedRustFacetOwner {
                    identity: dependency_owner_identity(),
                    kind: OvenSelectedRustFacetOwnerKind::Constituent,
                },
            ],
            units: vec![root, leaf],
            exposed_roots: BTreeMap::from([("fixture".to_string(), exposed_root)]),
        })
    }

    fn single_unit_graph(
        role: OvenSelectedRustFacetUnitRole,
        domain: OvenSelectedRustFacetDomain,
        crate_kind: OvenSelectedRustFacetCrateKind,
    ) -> Result<OvenSelectedRustFacetGraph, OvenSelectedRustFacetGraphError> {
        let selection = selection();
        let bytes = b"pub fn fixture() {}\n";
        let mut unit = root_unit(&selection, None, bytes)?;
        unit.role = role;
        unit.domain = domain;
        unit.crate_kind = crate_kind;
        unit.identity = selected_graph_unit_identity(&selection, &unit)?;
        let identity = unit.identity.clone();
        Ok(OvenSelectedRustFacetGraph {
            schema_version: OVEN_SELECTED_RUST_FACET_GRAPH_SCHEMA_VERSION,
            selection,
            owners: vec![
                OvenSelectedRustFacetOwner {
                    identity: project_owner_identity(bytes),
                    kind: OvenSelectedRustFacetOwnerKind::ProjectAuthority,
                },
                OvenSelectedRustFacetOwner {
                    identity: toolchain_owner_identity(),
                    kind: OvenSelectedRustFacetOwnerKind::Toolchain,
                },
            ],
            units: vec![unit],
            exposed_roots: BTreeMap::from([("fixture".to_string(), identity)]),
        })
    }

    fn unit_index(graph: &OvenSelectedRustFacetGraph, package: &str) -> Result<usize, &'static str> {
        graph
            .units
            .iter()
            .position(|unit| unit.package == package)
            .ok_or("fixture graph lost a unit")
    }

    fn reidentify_unit(
        graph: &mut OvenSelectedRustFacetGraph,
        index: usize,
    ) -> Result<(), OvenSelectedRustFacetGraphError> {
        let old = graph.units[index].identity.clone();
        let new = selected_graph_unit_identity(&graph.selection, &graph.units[index])?;
        graph.units[index].identity = new.clone();
        for unit in &mut graph.units {
            for dependency in &mut unit.dependencies {
                if dependency.unit == old {
                    dependency.unit = new.clone();
                }
            }
        }
        for identity in graph.exposed_roots.values_mut() {
            if *identity == old {
                *identity = new.clone();
            }
        }
        Ok(())
    }

    fn environment_text(value: &str) -> OvenSelectedRustFacetEnvironmentValue {
        OvenSelectedRustFacetEnvironmentValue::Text {
            value: value.to_string(),
        }
    }

    /// Stand in for the producer's keyed redaction; a fixture only needs well-formed, distinguishable identities.
    fn environment_sensitive_digest(material: &[u8]) -> OvenSelectedRustFacetEnvironmentValue {
        OvenSelectedRustFacetEnvironmentValue::SensitiveDigest {
            hmac_sha256: selected_graph_sha256(material).replace("sha256:", "hmac-sha256:"),
        }
    }

    fn graph_with_root_environment(
        environment: &[(&str, OvenSelectedRustFacetEnvironmentValue)],
    ) -> Result<OvenSelectedRustFacetGraph, Box<dyn std::error::Error>> {
        let mut graph = graph(b"pub fn use_dependency() {}\n")?;
        let root = unit_index(&graph, "fixture")?;
        for (name, value) in environment {
            graph.units[root].environment.insert((*name).to_string(), value.clone());
        }
        reidentify_unit(&mut graph, root)?;
        Ok(graph)
    }

    /// Build a one-unit graph whose source carries the supplied kind and logical identity.
    ///
    /// The owner identity is derived from the source bytes, never from where those bytes were read, which is what
    /// lets the same logical source relocate between physical roots.
    fn kind_source_graph(
        kind: OvenSelectedRustFacetSourceKind,
        source_identity: &str,
        bytes: &[u8],
    ) -> Result<OvenSelectedRustFacetGraph, OvenSelectedRustFacetGraphError> {
        let selection = selection();
        let mut unit = root_unit(&selection, None, bytes)?;
        unit.source.kind = kind;
        unit.source.identity = source_identity.to_string();
        // A delivery location the physical adapter rebinds; relocation must not reach the portable graph through it.
        unit.environment.insert(
            "OUT_DIR".to_string(),
            OvenSelectedRustFacetEnvironmentValue::Path {
                value: OvenSelectedRustFacetPath {
                    owner: project_owner_identity(bytes),
                    path: "generated/out".to_string(),
                },
            },
        );
        unit.identity = selected_graph_unit_identity(&selection, &unit)?;
        let identity = unit.identity.clone();
        Ok(OvenSelectedRustFacetGraph {
            schema_version: OVEN_SELECTED_RUST_FACET_GRAPH_SCHEMA_VERSION,
            selection,
            owners: vec![
                OvenSelectedRustFacetOwner {
                    identity: project_owner_identity(bytes),
                    kind: OvenSelectedRustFacetOwnerKind::ProjectAuthority,
                },
                OvenSelectedRustFacetOwner {
                    identity: toolchain_owner_identity(),
                    kind: OvenSelectedRustFacetOwnerKind::Toolchain,
                },
            ],
            units: vec![unit],
            exposed_roots: BTreeMap::from([("fixture".to_string(), identity)]),
        })
    }

    fn path_source_graph(
        source_identity: &str,
        bytes: &[u8],
    ) -> Result<OvenSelectedRustFacetGraph, OvenSelectedRustFacetGraphError> {
        kind_source_graph(OvenSelectedRustFacetSourceKind::Path, source_identity, bytes)
    }

    /// Return the refusal field for a graph that must not validate, whichever refusal variant it produces.
    fn refusal_field(graph: OvenSelectedRustFacetGraph, subject: &str) -> Result<String, Box<dyn std::error::Error>> {
        match graph.validated() {
            Ok(_) => Err(format!("{subject} was admitted into a portable graph").into()),
            Err(
                OvenSelectedRustFacetGraphError::Invalid { field, .. }
                | OvenSelectedRustFacetGraphError::Missing { field },
            ) => Ok(field),
            Err(other) => Err(format!("{subject} was refused for an unrelated reason: {other}").into()),
        }
    }

    #[test]
    fn selected_graph_preserves_renamed_dependency_alias() -> TestResult {
        let selected = graph(b"pub fn use_dependency() {}\n")?.validated()?;
        let root = selected
            .graph()
            .units
            .iter()
            .find(|unit| unit.package == "fixture")
            .ok_or("validated graph lost its root unit")?;
        let dependency = selected
            .graph()
            .units
            .iter()
            .find(|unit| unit.package == "dependency-package")
            .ok_or("validated graph lost its dependency unit")?;
        assert_eq!(
            root.dependencies,
            vec![OvenSelectedRustFacetDependency {
                alias: "renamed_dep".to_string(),
                unit: dependency.identity.clone(),
            }]
        );
        let encoded = selected.to_json_bytes()?;
        let decoded = OvenSelectedRustFacetGraph::decode_validated(&encoded)?;
        assert_eq!(decoded.graph(), selected.graph());
        assert_eq!(decoded.digest(), selected.digest());
        Ok(())
    }

    #[test]
    fn selected_graph_retains_explicit_empty_leaves_and_rejects_missing_leaf() -> TestResult {
        let selected = graph(b"pub fn use_dependency() {}\n")?.validated()?;
        let encoded = selected.to_json_bytes()?;
        let mut document = serde_json::from_slice::<serde_json::Value>(&encoded)?;
        let leaf = document["units"]
            .as_array()
            .ok_or("serialized graph has no unit array")?
            .iter()
            .find(|unit| unit["package"].as_str() == Some("dependency-package"))
            .ok_or("serialized graph lost its leaf unit")?;
        for field in ["dependencies", "features", "cfg", "exclude_dirs", "generated_inputs"] {
            assert_eq!(leaf[field].as_array().map(Vec::len), Some(0));
        }
        assert_eq!(leaf["environment"].as_object().map(serde_json::Map::len), Some(0));

        let missing_leaf = document["units"]
            .as_array_mut()
            .ok_or("serialized graph has no mutable unit array")?
            .iter_mut()
            .find(|unit| unit["package"].as_str() == Some("dependency-package"))
            .and_then(serde_json::Value::as_object_mut)
            .ok_or("serialized graph lost its mutable leaf unit")?;
        assert!(missing_leaf.remove("dependencies").is_some());
        let missing_bytes = serde_json::to_vec(&document)?;
        assert!(matches!(
            OvenSelectedRustFacetGraph::decode_validated(&missing_bytes),
            Err(OvenSelectedRustFacetGraphError::Invalid { .. })
        ));
        Ok(())
    }

    #[test]
    fn selected_graph_accepts_host_and_target_units_sharing_one_source_root() -> TestResult {
        let mut graph = graph(b"pub fn shared() {}\n")?;
        let root_index = unit_index(&graph, "fixture")?;
        let mut host = graph.units[root_index].clone();
        host.domain = OvenSelectedRustFacetDomain::Host;
        host.dependencies.clear();
        host.identity = selected_graph_unit_identity(&graph.selection, &host)?;
        graph
            .exposed_roots
            .insert("fixture_host".to_string(), host.identity.clone());
        graph.units.push(host);

        let selected = graph.validated()?;
        let shared = selected
            .graph()
            .units
            .iter()
            .filter(|unit| unit.package == "fixture")
            .collect::<Vec<_>>();
        assert_eq!(shared.len(), 2);
        let first = shared.first().ok_or("validated graph lost first shared-root unit")?;
        let second = shared.get(1).ok_or("validated graph lost second shared-root unit")?;
        assert_eq!(first.source.owner, second.source.owner);
        assert_eq!(first.source.root, second.source.root);
        assert_eq!(first.root_module, second.root_module);
        assert_ne!(first.domain, second.domain);
        Ok(())
    }

    #[test]
    fn selected_graph_enforces_every_inspection_role_combination() -> TestResult {
        let roles = [
            OvenSelectedRustFacetUnitRole::Library,
            OvenSelectedRustFacetUnitRole::Binary,
            OvenSelectedRustFacetUnitRole::UnitTest,
            OvenSelectedRustFacetUnitRole::IntegrationTest,
            OvenSelectedRustFacetUnitRole::Example,
            OvenSelectedRustFacetUnitRole::Doctest,
            OvenSelectedRustFacetUnitRole::Benchmark,
            OvenSelectedRustFacetUnitRole::ProcMacro,
            OvenSelectedRustFacetUnitRole::CompilerSupport,
            OvenSelectedRustFacetUnitRole::CallerProjection,
        ];
        let domains = [OvenSelectedRustFacetDomain::Host, OvenSelectedRustFacetDomain::Target];
        let crate_kinds = [
            OvenSelectedRustFacetCrateKind::Rlib,
            OvenSelectedRustFacetCrateKind::Binary,
            OvenSelectedRustFacetCrateKind::ProcMacro,
        ];
        for role in roles {
            for domain in domains {
                for crate_kind in crate_kinds {
                    let expected = matches!(
                        (role, domain, crate_kind),
                        (
                            OvenSelectedRustFacetUnitRole::Library | OvenSelectedRustFacetUnitRole::CompilerSupport,
                            OvenSelectedRustFacetDomain::Host | OvenSelectedRustFacetDomain::Target,
                            OvenSelectedRustFacetCrateKind::Rlib
                        ) | (
                            OvenSelectedRustFacetUnitRole::Binary
                                | OvenSelectedRustFacetUnitRole::UnitTest
                                | OvenSelectedRustFacetUnitRole::IntegrationTest
                                | OvenSelectedRustFacetUnitRole::Example
                                | OvenSelectedRustFacetUnitRole::Doctest
                                | OvenSelectedRustFacetUnitRole::Benchmark,
                            OvenSelectedRustFacetDomain::Target,
                            OvenSelectedRustFacetCrateKind::Binary
                        ) | (
                            OvenSelectedRustFacetUnitRole::ProcMacro,
                            OvenSelectedRustFacetDomain::Host,
                            OvenSelectedRustFacetCrateKind::ProcMacro
                        ) | (
                            OvenSelectedRustFacetUnitRole::CallerProjection,
                            OvenSelectedRustFacetDomain::Target,
                            OvenSelectedRustFacetCrateKind::Rlib
                        )
                    );
                    let actual = single_unit_graph(role, domain, crate_kind)?.validated().is_ok();
                    assert_eq!(
                        actual, expected,
                        "role={role:?} domain={domain:?} crate_kind={crate_kind:?}"
                    );
                }
            }
        }

        let selected = single_unit_graph(
            OvenSelectedRustFacetUnitRole::Library,
            OvenSelectedRustFacetDomain::Target,
            OvenSelectedRustFacetCrateKind::Rlib,
        )?
        .validated()?;
        for unsupported in ["build_script", "generated_source", "carrier"] {
            let mut document = serde_json::from_slice::<serde_json::Value>(&selected.to_json_bytes()?)?;
            let unit = document["units"]
                .as_array_mut()
                .and_then(|units| units.first_mut())
                .ok_or("serialized role graph lost its unit")?;
            unit["role"] = serde_json::json!(unsupported);
            let bytes = serde_json::to_vec(&document)?;
            assert!(matches!(
                OvenSelectedRustFacetGraph::decode_validated(&bytes),
                Err(OvenSelectedRustFacetGraphError::Invalid { .. })
            ));
        }
        Ok(())
    }

    #[test]
    fn selected_graph_binds_target_spec_to_a_toolchain_owner() -> TestResult {
        let selected = graph(b"pub fn use_dependency() {}\n")?.validated()?;
        let target_owner = &selected.graph().selection.target_spec.source.owner;
        let owner = selected
            .graph()
            .owners
            .iter()
            .find(|owner| &owner.identity == target_owner)
            .ok_or("validated graph lost target-spec owner")?;
        assert_eq!(owner.kind, OvenSelectedRustFacetOwnerKind::Toolchain);

        let mut wrong_target_spec_owner = graph(b"pub fn use_dependency() {}\n")?;
        wrong_target_spec_owner.selection.target_spec.source.owner = dependency_owner_identity();
        assert!(matches!(
            wrong_target_spec_owner.validated(),
            Err(OvenSelectedRustFacetGraphError::Invalid { field, .. })
                if field == "selection.target_spec.source.owner"
        ));

        let mut missing_toolchain_kind = graph(b"pub fn use_dependency() {}\n")?;
        let toolchain = toolchain_owner_identity();
        let owner = missing_toolchain_kind
            .owners
            .iter_mut()
            .find(|owner| owner.identity == toolchain)
            .ok_or("fixture graph lost toolchain owner")?;
        owner.kind = OvenSelectedRustFacetOwnerKind::Constituent;
        assert!(matches!(
            missing_toolchain_kind.validated(),
            Err(OvenSelectedRustFacetGraphError::Invalid { .. })
        ));

        let mut duplicate_toolchain_kind = graph(b"pub fn use_dependency() {}\n")?;
        duplicate_toolchain_kind.owners.push(OvenSelectedRustFacetOwner {
            identity: selected_graph_sha256(b"second toolchain closure"),
            kind: OvenSelectedRustFacetOwnerKind::Toolchain,
        });
        assert!(matches!(
            duplicate_toolchain_kind.validated(),
            Err(OvenSelectedRustFacetGraphError::Invalid { .. })
        ));

        let mut missing_owner = graph(b"pub fn use_dependency() {}\n")?;
        missing_owner.selection.target_spec.source.owner = selected_graph_sha256(b"absent toolchain owner");
        assert!(matches!(
            missing_owner.validated(),
            Err(OvenSelectedRustFacetGraphError::Missing { .. })
        ));

        let mut compiler_from_constituent = graph(b"pub fn use_dependency() {}\n")?;
        let dependency = unit_index(&compiler_from_constituent, "dependency-package")?;
        compiler_from_constituent
            .units
            .get_mut(dependency)
            .ok_or("fixture graph lost dependency unit")?
            .source
            .kind = OvenSelectedRustFacetSourceKind::Compiler;
        assert!(matches!(
            compiler_from_constituent.validated(),
            Err(OvenSelectedRustFacetGraphError::Invalid { field, .. }) if field.ends_with(".source.owner")
        ));

        let mut registry_from_toolchain = graph(b"pub fn use_dependency() {}\n")?;
        let dependency = unit_index(&registry_from_toolchain, "dependency-package")?;
        registry_from_toolchain
            .units
            .get_mut(dependency)
            .ok_or("fixture graph lost dependency unit")?
            .source
            .owner = toolchain_owner_identity();
        assert!(matches!(
            registry_from_toolchain.validated(),
            Err(OvenSelectedRustFacetGraphError::Invalid { field, .. }) if field.ends_with(".source.owner")
        ));
        Ok(())
    }

    #[test]
    fn selected_graph_rejects_detached_units_and_owners() -> TestResult {
        let mut detached_unit = graph(b"pub fn use_dependency() {}\n")?;
        let selection = detached_unit.selection.clone();
        let mut unit = leaf_unit(&selection)?;
        unit.domain = OvenSelectedRustFacetDomain::Host;
        unit.identity = selected_graph_unit_identity(&selection, &unit)?;
        detached_unit.units.push(unit);
        assert!(matches!(
            detached_unit.validated(),
            Err(OvenSelectedRustFacetGraphError::Invalid { .. })
        ));

        let mut detached_owner = graph(b"pub fn use_dependency() {}\n")?;
        detached_owner.owners.push(OvenSelectedRustFacetOwner {
            identity: selected_graph_sha256(b"detached owner"),
            kind: OvenSelectedRustFacetOwnerKind::GeneratedOutput,
        });
        assert!(matches!(
            detached_owner.validated(),
            Err(OvenSelectedRustFacetGraphError::Invalid { .. })
        ));
        Ok(())
    }

    #[test]
    fn selected_graph_checks_schema_before_strict_nested_wire_fields() -> TestResult {
        let selected = graph(b"pub fn use_dependency() {}\n")?.validated()?;
        let mut current = serde_json::from_slice::<serde_json::Value>(&selected.to_json_bytes()?)?;
        current["selection"]["intent"]["future_field"] = serde_json::json!(true);
        let current_bytes = serde_json::to_vec(&current)?;
        assert!(matches!(
            OvenSelectedRustFacetGraph::decode_validated(&current_bytes),
            Err(OvenSelectedRustFacetGraphError::Invalid { .. })
        ));

        let future = serde_json::to_vec(&serde_json::json!({
            "schema_version": OVEN_SELECTED_RUST_FACET_GRAPH_SCHEMA_VERSION + 1,
            "selection": { "intent": { "future_field": true } },
            "future_top_level": true
        }))?;
        assert!(matches!(
            OvenSelectedRustFacetGraph::decode_validated(&future),
            Err(OvenSelectedRustFacetGraphError::UnsupportedSchema { .. })
        ));
        Ok(())
    }

    #[test]
    fn selected_graph_eliminates_alpha_labels_and_normalizes_insertion_order() -> TestResult {
        let first = graph(b"pub fn use_dependency() {}\n")?.validated()?;
        let mut reordered = graph(b"pub fn use_dependency() {}\n")?;
        reordered.owners.reverse();
        reordered.units.reverse();
        reordered.selection.intent.features.reverse();
        for unit in &mut reordered.units {
            unit.features.reverse();
            unit.cfg.reverse();
            unit.include_dirs.reverse();
            unit.exclude_dirs.reverse();
            unit.dependencies.reverse();
            unit.generated_inputs.reverse();
            unit.source_members.reverse();
        }
        let reordered = reordered.validated()?;
        assert_eq!(first.digest(), reordered.digest());
        assert_eq!(first.to_json_bytes()?, reordered.to_json_bytes()?);
        let encoded = String::from_utf8(first.to_json_bytes()?)?;
        assert!(!encoded.contains("unit-root"));
        assert!(!encoded.contains("owner-project"));
        assert!(!encoded.contains("\"id\""));
        Ok(())
    }

    #[test]
    fn selected_graph_rejects_cycles_and_duplicates() -> TestResult {
        let mut cycle = graph(b"pub fn use_dependency() {}\n")?;
        let root = unit_index(&cycle, "fixture")?;
        let leaf = unit_index(&cycle, "dependency-package")?;
        let root_identity = cycle
            .units
            .get(root)
            .ok_or("fixture graph lost root unit")?
            .identity
            .clone();
        cycle
            .units
            .get_mut(leaf)
            .ok_or("fixture graph lost leaf unit")?
            .dependencies
            .push(OvenSelectedRustFacetDependency {
                alias: "fixture".to_string(),
                unit: root_identity,
            });
        assert!(matches!(
            cycle.validated(),
            Err(OvenSelectedRustFacetGraphError::Invalid { .. })
        ));

        let mut duplicate_feature = graph(b"pub fn use_dependency() {}\n")?;
        duplicate_feature
            .selection
            .intent
            .features
            .push("root-feature".to_string());
        assert!(matches!(
            duplicate_feature.validated(),
            Err(OvenSelectedRustFacetGraphError::Invalid { .. })
        ));

        let mut duplicate_owner = graph(b"pub fn use_dependency() {}\n")?;
        let owner = duplicate_owner
            .owners
            .first()
            .ok_or("fixture graph lost first owner")?
            .clone();
        duplicate_owner.owners.push(owner);
        assert!(matches!(
            duplicate_owner.validated(),
            Err(OvenSelectedRustFacetGraphError::Invalid { .. })
        ));

        let mut duplicate_alias = graph(b"pub fn use_dependency() {}\n")?;
        let root = unit_index(&duplicate_alias, "fixture")?;
        let dependency = duplicate_alias
            .units
            .get(root)
            .and_then(|unit| unit.dependencies.first())
            .ok_or("fixture graph lost root dependency")?
            .clone();
        duplicate_alias
            .units
            .get_mut(root)
            .ok_or("fixture graph lost root unit")?
            .dependencies
            .push(dependency);
        assert!(matches!(
            duplicate_alias.validated(),
            Err(OvenSelectedRustFacetGraphError::Invalid { .. })
        ));
        Ok(())
    }

    #[test]
    fn selected_graph_rejects_invalid_portable_paths() -> TestResult {
        let mut absolute_source = graph(b"pub fn use_dependency() {}\n")?;
        let root = unit_index(&absolute_source, "fixture")?;
        absolute_source.units[root].source.root = "/tmp/fixture".to_string();
        assert!(matches!(
            absolute_source.validated(),
            Err(OvenSelectedRustFacetGraphError::Invalid { .. })
        ));

        let mut owner_root_target_spec = graph(b"pub fn use_dependency() {}\n")?;
        owner_root_target_spec.selection.target_spec.source.path = ".".to_string();
        assert!(matches!(
            owner_root_target_spec.validated(),
            Err(OvenSelectedRustFacetGraphError::Invalid { .. })
        ));

        let mut escaping_include = graph(b"pub fn use_dependency() {}\n")?;
        let root = unit_index(&escaping_include, "fixture")?;
        escaping_include
            .units
            .get_mut(root)
            .and_then(|unit| unit.include_dirs.first_mut())
            .ok_or("fixture graph lost root include directory")?
            .path = "../src".to_string();
        assert!(matches!(
            escaping_include.validated(),
            Err(OvenSelectedRustFacetGraphError::Invalid { .. })
        ));
        Ok(())
    }

    #[test]
    fn selected_graph_enforces_portable_and_sensitive_environment_vocabulary() -> TestResult {
        let mut out_dir_text = graph(b"pub fn use_dependency() {}\n")?;
        let root = unit_index(&out_dir_text, "fixture")?;
        out_dir_text.units[root].environment.insert(
            "OUT_DIR".to_string(),
            OvenSelectedRustFacetEnvironmentValue::Text {
                value: "/tmp/out".to_string(),
            },
        );
        assert!(matches!(
            out_dir_text.validated(),
            Err(OvenSelectedRustFacetGraphError::Invalid { .. })
        ));

        let mut absolute_text = graph(b"pub fn use_dependency() {}\n")?;
        let root = unit_index(&absolute_text, "fixture")?;
        absolute_text.units[root].environment.insert(
            "FIXTURE_VALUE".to_string(),
            OvenSelectedRustFacetEnvironmentValue::Text {
                value: "/tmp/private".to_string(),
            },
        );
        assert!(matches!(
            absolute_text.validated(),
            Err(OvenSelectedRustFacetGraphError::Invalid { .. })
        ));

        let mut sensitive_text = graph(b"pub fn use_dependency() {}\n")?;
        let root = unit_index(&sensitive_text, "fixture")?;
        sensitive_text.units[root].environment.insert(
            "API_TOKEN".to_string(),
            OvenSelectedRustFacetEnvironmentValue::Text {
                value: "do-not-serialize".to_string(),
            },
        );
        assert!(matches!(
            sensitive_text.validated(),
            Err(OvenSelectedRustFacetGraphError::Invalid { .. })
        ));

        let mut admitted = graph(b"pub fn use_dependency() {}\n")?;
        let root = unit_index(&admitted, "fixture")?;
        let project_owner = admitted.units[root].source.owner.clone();
        admitted.units[root].environment.insert(
            "OUT_DIR".to_string(),
            OvenSelectedRustFacetEnvironmentValue::Path {
                value: OvenSelectedRustFacetPath {
                    owner: project_owner,
                    path: "generated/out".to_string(),
                },
            },
        );
        admitted.units[root].environment.insert(
            "API_TOKEN".to_string(),
            OvenSelectedRustFacetEnvironmentValue::SensitiveDigest {
                hmac_sha256: format!("hmac-sha256:{}", "a".repeat(64)),
            },
        );
        reidentify_unit(&mut admitted, root)?;
        let encoded = String::from_utf8(admitted.validated()?.to_json_bytes()?)?;
        assert!(!encoded.contains("do-not-serialize"));
        assert!(encoded.contains("hmac-sha256:"));
        Ok(())
    }

    #[test]
    fn selected_graph_digest_changes_after_semantic_exposure_mutation() -> TestResult {
        let first = graph(b"pub fn use_dependency() {}\n")?.validated()?;
        let mut changed = graph(b"pub fn use_dependency() {}\n")?;
        let root = changed
            .exposed_roots
            .get("fixture")
            .cloned()
            .ok_or("fixture graph lost exposed root")?;
        changed.exposed_roots.insert("fixture_alias".to_string(), root);
        let changed = changed.validated()?;
        assert_ne!(first.digest(), changed.digest());
        Ok(())
    }

    #[test]
    fn selected_graph_refuses_missing_units_owners_digests_and_root_references() -> TestResult {
        let baseline = graph(b"pub fn use_dependency() {}\n")?;
        let root = unit_index(&baseline, "fixture")?;

        let mut missing_unit = baseline.clone();
        missing_unit
            .units
            .get_mut(root)
            .and_then(|unit| unit.dependencies.first_mut())
            .ok_or("fixture graph lost root dependency")?
            .unit = selected_graph_sha256(b"absent unit");
        assert!(matches!(
            missing_unit.validated(),
            Err(OvenSelectedRustFacetGraphError::Missing { .. })
        ));

        let mut missing_owner = baseline.clone();
        missing_owner.units[root].source.owner = selected_graph_sha256(b"absent owner");
        assert!(matches!(
            missing_owner.validated(),
            Err(OvenSelectedRustFacetGraphError::Missing { .. })
        ));

        let mut missing_digest = baseline.clone();
        missing_digest.units[root].source.digest.clear();
        assert!(matches!(
            missing_digest.validated(),
            Err(OvenSelectedRustFacetGraphError::Missing { .. })
        ));

        let mut missing_root = baseline;
        missing_root
            .exposed_roots
            .insert("fixture".to_string(), selected_graph_sha256(b"absent root"));
        assert!(matches!(
            missing_root.validated(),
            Err(OvenSelectedRustFacetGraphError::Missing { .. })
        ));
        Ok(())
    }

    fn graph_from_relocated_source(root: &Path) -> Result<OvenSelectedRustFacetGraph, Box<dyn std::error::Error>> {
        let bytes = fs::read(root.join("src/lib.rs"))?;
        Ok(graph(&bytes)?)
    }

    #[test]
    fn selected_graph_digest_is_stable_across_physical_relocation() -> TestResult {
        let first = tempfile::tempdir()?;
        let second = tempfile::tempdir()?;
        for root in [first.path(), second.path()] {
            fs::create_dir(root.join("src"))?;
            fs::write(root.join("src/lib.rs"), b"pub fn use_dependency() {}\n")?;
        }

        let first_selected = graph_from_relocated_source(first.path())?.validated()?;
        let second_selected = graph_from_relocated_source(second.path())?.validated()?;
        assert_eq!(first_selected.digest(), second_selected.digest());
        assert_eq!(first_selected.to_json_bytes()?, second_selected.to_json_bytes()?);
        let encoded = String::from_utf8(first_selected.to_json_bytes()?)?;
        assert!(!encoded.contains(&first.path().to_string_lossy().to_string()));
        assert!(!encoded.contains(&second.path().to_string_lossy().to_string()));
        Ok(())
    }

    #[test]
    fn selected_graph_retains_registered_public_compiler_facts() -> TestResult {
        let facts = [
            ("CARGO_CFG_TARGET_ARCH", "x86_64"),
            ("CARGO_CFG_TARGET_FEATURE", "cmpxchg16b,fxsr,sse,sse2,sse3,sse4.1"),
            ("CARGO_CFG_UNIX", "1"),
            ("CARGO_CRATE_NAME", "fixture"),
            ("CARGO_FEATURE_ROOT_FEATURE", "1"),
            ("CARGO_PKG_NAME", "fixture"),
            ("CARGO_PKG_VERSION", "0.1.0"),
            // A package with no prerelease selects this fact as empty; that is checked evidence, not a missing value.
            ("CARGO_PKG_VERSION_PRE", ""),
            ("DEBUG", "true"),
            ("HOST", "aarch64-apple-darwin"),
            ("NUM_JOBS", "8"),
            ("OPT_LEVEL", "0"),
            ("PROFILE", "dev"),
            ("TARGET", "x86_64-unknown-linux-gnu"),
        ];
        let environment = facts
            .iter()
            .map(|(name, value)| (*name, environment_text(value)))
            .collect::<Vec<_>>();
        let selected = graph_with_root_environment(&environment)?.validated()?;

        let encoded = selected.to_json_bytes()?;
        let rendered = String::from_utf8(encoded.clone())?;
        for (name, value) in facts {
            assert!(
                rendered.contains(name),
                "public fact `{name}` was dropped from the payload"
            );
            assert!(rendered.contains(value), "public fact `{name}` lost its retained value");
        }
        assert_eq!(
            OvenSelectedRustFacetGraph::decode_validated(&encoded)?.digest(),
            selected.digest()
        );
        Ok(())
    }

    #[test]
    fn selected_graph_environment_text_fails_closed_outside_the_public_registry() -> TestResult {
        let credential_uri = "postgres://user:password@host/db";
        let refused = [
            // Secrecy is a property of the value, and no name-shaped denylist predicts either of these.
            ("DATABASE_URL", environment_text(credential_uri)),
            ("PRIVATE_MATERIAL", environment_text("abc123")),
            // Machine-local locations, under both unregistered and registered names.
            ("FIXTURE_LOCATION", environment_text("/Users/alice/project")),
            ("PROFILE", environment_text("/Users/alice/project")),
            ("TARGET", environment_text("C:\\Users\\alice\\project")),
            ("HOST", environment_text("~/project")),
            // A feature flag carries exactly one meaning; anything else is not that fact.
            ("CARGO_FEATURE_ROOT_FEATURE", environment_text("0")),
            // Nothing outside the registry may claim a retaining representation, path included.
            (
                "DATABASE_URL",
                OvenSelectedRustFacetEnvironmentValue::Path {
                    value: OvenSelectedRustFacetPath {
                        owner: project_owner_identity(b"pub fn use_dependency() {}\n"),
                        path: "config".to_string(),
                    },
                },
            ),
            // A registered public fact is not a place to hide a keyed blob either.
            ("PROFILE", environment_sensitive_digest(b"dev")),
        ];
        for (name, value) in refused {
            let graph = graph_with_root_environment(&[(name, value)])?;
            let field = refusal_field(graph, &format!("environment `{name}`"))?;
            assert!(
                field.ends_with(&format!("environment.{name}")),
                "environment `{name}` was refused at the unrelated field `{field}`"
            );
        }

        // A refusal reaches logs, so it must never carry the material it refused.
        let leaked = graph_with_root_environment(&[("DATABASE_URL", environment_text(credential_uri))])?;
        let rendered = match leaked.validated() {
            Ok(_) => return Err("a credential-bearing URI was admitted as public text".into()),
            Err(error) => error.to_string(),
        };
        assert!(
            !rendered.contains("password"),
            "a refusal echoed the credential it refused"
        );
        assert!(!rendered.contains(credential_uri));
        Ok(())
    }

    #[test]
    fn selected_graph_binds_sensitive_environment_to_identity_without_retaining_it() -> TestResult {
        let first = graph_with_root_environment(&[("PRIVATE_MATERIAL", environment_sensitive_digest(b"abc123"))])?
            .validated()?;
        let second = graph_with_root_environment(&[("PRIVATE_MATERIAL", environment_sensitive_digest(b"xyz789"))])?
            .validated()?;
        let repeated = graph_with_root_environment(&[("PRIVATE_MATERIAL", environment_sensitive_digest(b"abc123"))])?
            .validated()?;

        // The value affects compilation identity, so the graph must not collapse two different values into one.
        assert_ne!(first.digest(), second.digest());
        assert_eq!(first.digest(), repeated.digest());

        let rendered = String::from_utf8(first.to_json_bytes()?)?;
        assert!(rendered.contains("PRIVATE_MATERIAL"));
        assert!(rendered.contains("hmac-sha256:"));
        assert!(
            !rendered.contains("abc123"),
            "sensitive material reached the graph payload"
        );
        Ok(())
    }

    #[test]
    fn selected_graph_rejects_host_specific_source_identity() -> TestResult {
        // Every spelling below must be refused on every validating host: a Windows drive or UNC identity looks
        // like an ordinary relative name to this macOS/Linux test runner, so the rule cannot lean on `std::path`.
        let refused = [
            "/Users/alice/project",
            "C:\\Users\\alice\\project",
            "D:/work/foo",
            "\\\\server\\share\\foo",
            "~/project",
            "path:/Users/alice/project",
            "path:C:\\Users\\alice\\project",
            "path:D:/work/foo",
            "path:\\\\server\\share\\foo",
            "path:~/project",
            "path:../escape",
            "path:./foo",
            "path:.",
            "path:foo/../bar",
            "path:",
            // A `file:` URI is an absolute machine-local path wearing URI clothing; its coordinate opens with a
            // scheme letter, so every separator-shaped check above walks straight past it.
            "path:file:///tmp/project",
            "path:file:///Users/alice/project",
            "path:file:/tmp/project",
            "path:file://localhost/tmp/project",
            "path:file:///C:/Users/alice/project",
            "path:FILE:///tmp/project",
            "path:jar:file:/tmp/project",
            "path:git+file:///tmp/project",
            "file:///tmp/project",
            // Percent-encoding decodes to the same absolute path; the leading position must not be one escape away.
            "path:%2Ftmp/project",
            "path:%2FUsers%2Falice%2Fproject",
            "path:%5CUsers%5Calice",
            "path:c%3A/Users/alice",
            // A portable identity still has to say which vocabulary it is written in.
            "registry:fixture@0.1.0",
            "foo",
        ];
        for identity in refused {
            let graph = path_source_graph(identity, b"pub fn fixture() {}\n")?;
            let field = refusal_field(graph, &format!("source identity `{identity}`"))?;
            assert!(
                field.ends_with(".source.identity"),
                "source identity `{identity}` was refused at the unrelated field `{field}`"
            );
        }

        for identity in ["path:fixture", "path:workspaces/fixture"] {
            path_source_graph(identity, b"pub fn fixture() {}\n")?.validated()?;
        }

        // A Git coordinate is URL-shaped by nature, so it needs the same rule proved separately: `file:` is a local
        // path however it is dressed, while a remote scheme resolves the same way on every machine.
        for identity in [
            "git:file:///tmp/project",
            "git:file:/home/alice/repo",
            "git:FILE:///tmp/project",
            "git:git+file:///tmp/project",
        ] {
            let graph = kind_source_graph(OvenSelectedRustFacetSourceKind::Git, identity, b"pub fn fixture() {}\n")?;
            let field = refusal_field(graph, &format!("source identity `{identity}`"))?;
            assert!(
                field.ends_with(".source.identity"),
                "source identity `{identity}` was refused at the unrelated field `{field}`"
            );
        }
        for identity in [
            "git:https://github.com/encero-systems/incan#0a8395835",
            "git:ssh://git@github.com/encero-systems/incan#0a8395835",
            // Encoding deeper in a remote coordinate is ordinary URL syntax, not a local location.
            "git:https://gitlab.example.com/group%2Fsubgroup/repo#0a8395835",
        ] {
            kind_source_graph(OvenSelectedRustFacetSourceKind::Git, identity, b"pub fn fixture() {}\n")?.validated()?;
        }
        Ok(())
    }

    /// Retarget a one-unit graph at a Windows host and target, re-deriving the identities that depend on them.
    fn windows_selection_graph(
        environment: &[(&str, OvenSelectedRustFacetEnvironmentValue)],
    ) -> Result<OvenSelectedRustFacetGraph, Box<dyn std::error::Error>> {
        let mut graph = path_source_graph("path:fixture", b"pub fn fixture() {}\n")?;
        graph.selection.host = "x86_64-pc-windows-msvc".to_string();
        graph.selection.intent.target = "x86_64-pc-windows-msvc".to_string();
        graph.selection.target_spec.source.path = "target-specs/x86_64-pc-windows-msvc.json".to_string();
        for (name, value) in environment {
            graph.units[0].environment.insert((*name).to_string(), value.clone());
        }
        reidentify_unit(&mut graph, 0)?;
        Ok(graph)
    }

    #[test]
    fn selected_graph_admits_a_windows_hosted_and_windows_targeted_selection() -> TestResult {
        // Windows *facts* are first-class compiler inputs; only Windows *locations* are refused. A developer on
        // Windows must be able to describe their build, or the portability rule would just be an exclusion.
        let facts = [
            ("CARGO_CFG_TARGET_ENV", "msvc"),
            ("CARGO_CFG_TARGET_FAMILY", "windows"),
            ("CARGO_CFG_TARGET_OS", "windows"),
            ("CARGO_CFG_WINDOWS", "1"),
            ("HOST", "x86_64-pc-windows-msvc"),
            ("TARGET", "x86_64-pc-windows-msvc"),
        ];
        let mut environment = facts
            .iter()
            .map(|(name, value)| (*name, environment_text(value)))
            .collect::<Vec<_>>();
        // The one genuinely machine-local fact a Windows build has still travels as an owner-relative path.
        environment.push((
            "OUT_DIR",
            OvenSelectedRustFacetEnvironmentValue::Path {
                value: OvenSelectedRustFacetPath {
                    owner: project_owner_identity(b"pub fn fixture() {}\n"),
                    path: "generated/out".to_string(),
                },
            },
        ));

        let selected = windows_selection_graph(&environment)?.validated()?;
        let rendered = String::from_utf8(selected.to_json_bytes()?)?;
        for (name, value) in facts {
            assert!(rendered.contains(name));
            assert!(
                rendered.contains(value),
                "Windows fact `{name}` lost its retained value"
            );
        }

        // No Windows separator reaches the payload: a literal backslash encodes as `\\` in JSON, whereas the
        // escaped quotes in the fixture's cfg encode as `\"` and must not be mistaken for one.
        assert!(
            !rendered.contains("\\\\"),
            "a Windows path separator reached the portable graph"
        );

        // The host is a real compiler input, so the Windows graph must not collapse into the POSIX one.
        let posix = path_source_graph("path:fixture", b"pub fn fixture() {}\n")?.validated()?;
        assert_ne!(selected.digest(), posix.digest());
        Ok(())
    }

    fn path_source_graph_from_root(root: &Path) -> Result<OvenSelectedRustFacetGraph, Box<dyn std::error::Error>> {
        let bytes = fs::read(root.join("src/lib.rs"))?;
        Ok(path_source_graph("path:fixture", &bytes)?)
    }

    #[test]
    fn selected_graph_path_source_relocates_without_changing_identity_or_digest() -> TestResult {
        // Alice's and Bob's checkouts of the same source differ only in where they physically live.
        let alice = tempfile::tempdir()?;
        let bob = tempfile::tempdir()?;
        assert_ne!(alice.path(), bob.path());
        for root in [alice.path(), bob.path()] {
            fs::create_dir(root.join("src"))?;
            fs::write(root.join("src/lib.rs"), b"pub fn relocatable() {}\n")?;
        }

        let from_alice = path_source_graph_from_root(alice.path())?.validated()?;
        let from_bob = path_source_graph_from_root(bob.path())?.validated()?;

        assert_eq!(from_alice.digest(), from_bob.digest());
        assert_eq!(from_alice.to_json_bytes()?, from_bob.to_json_bytes()?);

        let rendered = String::from_utf8(from_alice.to_json_bytes()?)?;
        for root in [alice.path(), bob.path()] {
            assert!(
                !rendered.contains(root.to_string_lossy().as_ref()),
                "a physical checkout root reached the portable graph"
            );
        }
        assert!(rendered.contains("path:fixture"));

        // The digest must still answer to the logical identity, or equality above would be worth nothing.
        let renamed = path_source_graph("path:renamed", b"pub fn relocatable() {}\n")?.validated()?;
        assert_ne!(from_alice.digest(), renamed.digest());
        Ok(())
    }
}
