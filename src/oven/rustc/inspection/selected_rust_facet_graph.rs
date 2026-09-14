//! The portable Rust facet graph selected before physical rust-analyzer projection: its wire schema, the
//! validation that admits it, and the relocation-independent identity a Store payload or inspection sidecar
//! carries for it.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::super::super::OvenBuildIntent;

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
pub(crate) struct ValidatedOvenSelectedRustFacetGraph {
    graph: OvenSelectedRustFacetGraph,
    #[allow(
        dead_code,
        reason = "read by the Store payload consumer of Gate 6 of RFC 119, which lands after this producer"
    )]
    digest: String,
}

/// Minimal header decoded before any version-specific graph fields.
#[derive(Deserialize)]
#[allow(
    dead_code,
    reason = "read by the Store payload consumer of Gate 6 of RFC 119, which lands after this producer"
)]
struct OvenSelectedRustFacetGraphHeader {
    schema_version: u32,
}

/// Build the refusal for a required selected-graph field that was absent or empty.
fn selected_graph_missing(field: impl Into<String>) -> OvenSelectedRustFacetGraphError {
    OvenSelectedRustFacetGraphError::Missing { field: field.into() }
}

/// Build the refusal for a selected-graph field that is present but not admissible, naming both.
fn selected_graph_invalid(field: impl Into<String>, message: impl Into<String>) -> OvenSelectedRustFacetGraphError {
    OvenSelectedRustFacetGraphError::Invalid {
        field: field.into(),
        message: message.into(),
    }
}

/// Require one text field to be non-empty and already trimmed.
///
/// Trimming here instead of accepting and normalizing keeps the wire form canonical: two graphs that differ
/// only in surrounding whitespace would otherwise digest differently while meaning the same thing.
fn validate_selected_graph_text(value: &str, field: &str) -> Result<(), OvenSelectedRustFacetGraphError> {
    if value.is_empty() {
        return Err(selected_graph_missing(field));
    }
    if value.trim() != value {
        return Err(selected_graph_invalid(field, "has leading or trailing whitespace"));
    }
    Ok(())
}

/// Require one field to be a lowercase-hex `sha256:` identity of the right length.
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

/// Require one declared path to be relative, portable and free of components that escape its owner.
///
/// `allow_owner_root` admits the empty path, which names the owner root itself; everywhere else an empty
/// path is a missing field rather than a reference to the root.
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

/// Require one dependency alias to be a single Rust-facing name rather than a path or a phrase.
fn validate_selected_graph_alias(value: &str, field: &str) -> Result<(), OvenSelectedRustFacetGraphError> {
    validate_selected_graph_text(value, field)?;
    if value.contains("::") || value.chars().any(char::is_whitespace) {
        return Err(selected_graph_invalid(field, "is not one Rust-facing alias"));
    }
    Ok(())
}

/// Require a string list to be sorted and duplicate-free, so its digest does not depend on producer order.
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

/// Render one SHA-256 digest in the `sha256:<hex>` form every selected-graph identity uses.
pub(crate) fn selected_graph_sha256(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

/// Digest one unit's declared source members into the single identity its selection carries.
///
/// An empty member set is refused rather than digested: a source tree with nothing in it is a declaration
/// error, and hashing it would mint a perfectly stable identity for no source at all.
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

/// Require one field to be a lowercase-hex `hmac-sha256:` value of the right length.
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

/// Require one declared environment variable name to be a plain name the compiler can be given.
fn validate_selected_graph_environment_name(value: &str, field: &str) -> Result<(), OvenSelectedRustFacetGraphError> {
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
            Self::Sensitive => "a keyed `sensitive_digest`, because the name is not a registered public compiler fact",
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

/// Whether one environment variable holds a search-path list rather than a single value.
///
/// A list is validated and relocated element by element, so treating one as a scalar would let an entry
/// outside the admitted roots through inside a longer string.
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

/// Put one selection into canonical order so equal selections digest equally.
fn canonicalize_selected_graph_selection(selection: &mut OvenSelectedRustFacetSelection) {
    selection.intent.features.sort();
}

/// Put one unit's unordered facts into canonical order before it contributes to an identity.
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

/// Put a whole graph into canonical order, so a producer's iteration order cannot reach an identity.
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

/// Resolve one owner reference against the owners the graph declares, returning its kind.
///
/// A reference to an owner the graph never declared is refused here rather than at materialization, so the
/// graph is internally consistent before anything physical is looked for.
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

/// Resolve one declared path against its owner, requiring both the owner and the path shape to be admissible.
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
        (OvenSelectedRustFacetEnvironmentClass::PublicText, OvenSelectedRustFacetEnvironmentValue::Text { value }) => {
            validate_selected_graph_public_environment_text(value, field)
        }
        (
            OvenSelectedRustFacetEnvironmentClass::FeatureActivation,
            OvenSelectedRustFacetEnvironmentValue::Text { value },
        ) if value == "1" => Ok(()),
        (OvenSelectedRustFacetEnvironmentClass::OwnerPath, OvenSelectedRustFacetEnvironmentValue::Path { value }) => {
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
    #[allow(
        dead_code,
        reason = "read by the Store payload consumer of Gate 6 of RFC 119, which lands after this producer"
    )]
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

            let source_owner_kind =
                validate_selected_graph_owner_reference(&unit.source.owner, &owners, &format!("{field}.source.owner"))?;
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
    pub(crate) fn validated(mut self) -> Result<ValidatedOvenSelectedRustFacetGraph, OvenSelectedRustFacetGraphError> {
        canonicalize_selected_graph(&mut self);
        self.validate_shape()?;
        let bytes = serde_json::to_vec(&(OVEN_SELECTED_RUST_FACET_GRAPH_DIGEST_DOMAIN, &self)).map_err(|error| {
            selected_graph_invalid("graph", format!("cannot encode deterministic digest input: {error}"))
        })?;
        Ok(ValidatedOvenSelectedRustFacetGraph {
            graph: self,
            digest: selected_graph_sha256(&bytes),
        })
    }
}

impl ValidatedOvenSelectedRustFacetGraph {
    /// Borrow the validated portable graph.
    pub(crate) fn graph(&self) -> &OvenSelectedRustFacetGraph {
        &self.graph
    }

    /// Return the graph's location-independent content identity.
    #[allow(
        dead_code,
        reason = "read by the Store payload consumer of Gate 6 of RFC 119, which lands after this producer"
    )]
    pub(crate) fn digest(&self) -> &str {
        &self.digest
    }

    /// Encode the admitted graph for a Store payload or inspection sidecar.
    #[allow(
        dead_code,
        reason = "read by the Store payload consumer of Gate 6 of RFC 119, which lands after this producer"
    )]
    pub(crate) fn to_json_bytes(&self) -> Result<Vec<u8>, OvenSelectedRustFacetGraphError> {
        serde_json::to_vec(&self.graph)
            .map_err(|error| selected_graph_invalid("graph", format!("cannot encode wire payload: {error}")))
    }

    /// Recover the validated graph for ownership transfer into a publisher.
    #[allow(
        dead_code,
        reason = "read by the Store payload consumer of Gate 6 of RFC 119, which lands after this producer"
    )]
    pub(crate) fn into_graph(self) -> OvenSelectedRustFacetGraph {
        self.graph
    }
}
