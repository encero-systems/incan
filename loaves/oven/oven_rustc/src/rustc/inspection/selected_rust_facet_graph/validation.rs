//! The checks a selected Rust facet graph passes field by field: portable coordinates, source identities, aliases,
//! environment names and values, canonical ordering, and the unit identity folded from all of them.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{
    OVEN_SELECTED_RUST_FACET_UNIT_DIGEST_DOMAIN, OvenSelectedRustFacetCrateKind, OvenSelectedRustFacetDependency,
    OvenSelectedRustFacetDomain, OvenSelectedRustFacetEnvironmentValue, OvenSelectedRustFacetGeneratedInput,
    OvenSelectedRustFacetGraph, OvenSelectedRustFacetGraphError, OvenSelectedRustFacetOwnerKind,
    OvenSelectedRustFacetPath, OvenSelectedRustFacetSelection, OvenSelectedRustFacetSource,
    OvenSelectedRustFacetSourceKind, OvenSelectedRustFacetSourceMember, OvenSelectedRustFacetTargetSpec,
    OvenSelectedRustFacetUnit, OvenSelectedRustFacetUnitRole,
};

/// Build the refusal for a required selected-graph field that was absent or empty.
pub(crate) fn selected_graph_missing(field: impl Into<String>) -> OvenSelectedRustFacetGraphError {
    OvenSelectedRustFacetGraphError::Missing { field: field.into() }
}

/// Build the refusal for a selected-graph field that is present but not admissible, naming both.
pub(crate) fn selected_graph_invalid(
    field: impl Into<String>,
    message: impl Into<String>,
) -> OvenSelectedRustFacetGraphError {
    OvenSelectedRustFacetGraphError::Invalid {
        field: field.into(),
        message: message.into(),
    }
}

/// Require one text field to be non-empty and already trimmed.
///
/// Trimming here instead of accepting and normalizing keeps the wire form canonical: two graphs that differ
/// only in surrounding whitespace would otherwise digest differently while meaning the same thing.
pub(crate) fn validate_selected_graph_text(value: &str, field: &str) -> Result<(), OvenSelectedRustFacetGraphError> {
    if value.is_empty() {
        return Err(selected_graph_missing(field));
    }
    if value.trim() != value {
        return Err(selected_graph_invalid(field, "has leading or trailing whitespace"));
    }
    Ok(())
}

/// Require one field to be a lowercase-hex `sha256:` identity of the right length.
pub(crate) fn validate_selected_graph_digest(value: &str, field: &str) -> Result<(), OvenSelectedRustFacetGraphError> {
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
pub(crate) fn validate_selected_graph_path(
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
pub(crate) fn validate_selected_graph_source_identity(
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
pub(crate) fn validate_selected_graph_alias(value: &str, field: &str) -> Result<(), OvenSelectedRustFacetGraphError> {
    validate_selected_graph_text(value, field)?;
    if value.contains("::") || value.chars().any(char::is_whitespace) {
        return Err(selected_graph_invalid(field, "is not one Rust-facing alias"));
    }
    Ok(())
}

/// Require a string list to be sorted and duplicate-free, so its digest does not depend on producer order.
pub(crate) fn validate_selected_graph_sorted_strings(
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
pub fn selected_graph_sha256(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

/// Digest one unit's declared source members into the single identity its selection carries.
///
/// An empty member set is refused rather than digested: a source tree with nothing in it is a declaration
/// error, and hashing it would mint a perfectly stable identity for no source at all.
pub fn selected_graph_source_digest(
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
pub(crate) fn canonicalize_selected_graph(graph: &mut OvenSelectedRustFacetGraph) {
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
pub fn selected_graph_unit_identity(
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
pub const fn selected_graph_unit_role_is_valid(
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
pub(crate) fn validate_selected_graph_owner_reference(
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
pub(crate) fn validate_selected_graph_path_reference(
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
pub(crate) fn validate_selected_graph_environment(
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
