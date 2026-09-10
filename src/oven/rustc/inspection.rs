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
        /// Stable graph-local reference used by unit inputs.
        pub id: String,
        /// Content-addressed Store, Loaf, toolchain, or provider-output identity.
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
        /// Compiler-owned support or sysroot source.
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
        /// Stable owner ID from the graph's complete owner table.
        pub owner: String,
        /// Portable owner-relative path; `.` denotes the owner root itself.
        pub path: String,
    }

    /// Environment value retained without embedding a machine-local delivery path.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
    pub(crate) enum OvenSelectedRustFacetEnvironmentValue {
        /// Explicit non-path value, including an explicitly selected empty string.
        Text { value: String },
        /// Path rebound through one retained owner by the physical projection adapter.
        Path { value: OvenSelectedRustFacetPath },
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

    /// Selection role controlling why a unit participates in this graph.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub(crate) enum OvenSelectedRustFacetUnitRole {
        /// Ordinary library dependency.
        Library,
        /// Executable root.
        Binary,
        /// Build-script host unit.
        BuildScript,
        /// Procedural-macro host unit.
        ProcMacro,
        /// Unit or integration test root.
        Test,
        /// Compiler support or explicit sysroot unit.
        CompilerSupport,
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
        /// Stable unit identity used by dependency and exposed-root references.
        pub id: String,
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

    /// Complete selection context shared by every unit in one graph.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub(crate) struct OvenSelectedRustFacetSelection {
        /// Existing target/toolchain/profile/root-feature build intent.
        pub intent: OvenBuildIntent,
        /// Exact build host triple; host and target remain distinct.
        pub host: String,
        /// Dependency-role activation purpose.
        pub purpose: OvenSelectedRustFacetPurpose,
        /// Whether the selected root activation includes default features.
        pub default_features: bool,
        /// Semver-only compiler version required by rust-analyzer.
        pub toolchain_version: String,
        /// Digest of the selected toolchain's exact target-spec JSON bytes.
        pub target_spec_digest: String,
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
        /// Sorted complete owner table referenced by sources and generated inputs.
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
        let records = members
            .iter()
            .map(|member| (member.path.as_str(), member.digest.as_str()))
            .collect::<BTreeMap<_, _>>();
        let bytes = serde_json::to_vec(&records)
            .map_err(|error| selected_graph_invalid("source_members", format!("cannot encode: {error}")))?;
        Ok(selected_graph_sha256(&bytes))
    }

    fn validate_selected_graph_path_reference(
        value: &OvenSelectedRustFacetPath,
        owners: &BTreeSet<&str>,
        field: &str,
    ) -> Result<(), OvenSelectedRustFacetGraphError> {
        validate_selected_graph_text(&value.owner, field)?;
        if !owners.contains(value.owner.as_str()) {
            return Err(selected_graph_missing(format!("{field} owner `{}`", value.owner)));
        }
        validate_selected_graph_path(&value.path, field, true)
    }

    impl OvenSelectedRustFacetGraph {
        /// Validate complete evidence and canonical ordering without selecting or repairing any graph fact.
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
            validate_selected_graph_digest(&self.selection.target_spec_digest, "selection.target_spec_digest")?;
            validate_selected_graph_sorted_strings(&self.selection.intent.features, "selection.intent.features")?;

            if self.owners.is_empty() {
                return Err(selected_graph_missing("owners"));
            }
            let mut owners = BTreeSet::new();
            let mut owner_identities = BTreeSet::new();
            for (index, owner) in self.owners.iter().enumerate() {
                let field = format!("owners[{index}]");
                validate_selected_graph_text(&owner.id, &format!("{field}.id"))?;
                validate_selected_graph_digest(&owner.identity, &format!("{field}.identity"))?;
                if index > 0 && self.owners[index - 1].id >= owner.id {
                    return Err(selected_graph_invalid("owners", "must be sorted by unique owner ID"));
                }
                if !owners.insert(owner.id.as_str()) {
                    return Err(selected_graph_invalid(&field, "repeats an owner ID"));
                }
                if !owner_identities.insert(owner.identity.as_str()) {
                    return Err(selected_graph_invalid(&field, "repeats an owner identity"));
                }
            }

            if self.units.is_empty() {
                return Err(selected_graph_missing("units"));
            }
            let unit_indexes = self
                .units
                .iter()
                .enumerate()
                .map(|(index, unit)| (unit.id.as_str(), index))
                .collect::<BTreeMap<_, _>>();
            if unit_indexes.len() != self.units.len() {
                return Err(selected_graph_invalid("units", "repeat a unit ID"));
            }
            let mut selected_roots = BTreeSet::new();
            for (index, unit) in self.units.iter().enumerate() {
                let field = format!("units[{index}]");
                for (name, value) in [
                    ("id", unit.id.as_str()),
                    ("package", unit.package.as_str()),
                    ("package_version", unit.package_version.as_str()),
                    ("crate_name", unit.crate_name.as_str()),
                    ("edition", unit.edition.as_str()),
                ] {
                    validate_selected_graph_text(value, &format!("{field}.{name}"))?;
                }
                if index > 0 && self.units[index - 1].id >= unit.id {
                    return Err(selected_graph_invalid("units", "must be sorted by unique unit ID"));
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
                validate_selected_graph_text(&unit.source.identity, &format!("{field}.source.identity"))?;
                validate_selected_graph_text(&unit.source.owner, &format!("{field}.source.owner"))?;
                if !owners.contains(unit.source.owner.as_str()) {
                    return Err(selected_graph_missing(format!(
                        "{field}.source.owner `{}`",
                        unit.source.owner
                    )));
                }
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
                        "must be sorted by unique path",
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
                if !selected_roots.insert((
                    unit.source.owner.as_str(),
                    unit.source.root.as_str(),
                    unit.root_module.as_str(),
                )) {
                    return Err(selected_graph_invalid(
                        format!("{field}.root_module"),
                        "repeats another selected unit root",
                    ));
                }
                validate_selected_graph_sorted_strings(&unit.features, &format!("{field}.features"))?;
                validate_selected_graph_sorted_strings(&unit.cfg, &format!("{field}.cfg"))?;
                for (name, value) in &unit.environment {
                    validate_selected_graph_text(name, &format!("{field}.environment key"))?;
                    if let OvenSelectedRustFacetEnvironmentValue::Path { value } = value {
                        validate_selected_graph_path_reference(value, &owners, &format!("{field}.environment.{name}"))?;
                    }
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
                            "must be sorted and unique",
                        ));
                    }
                    for path in paths {
                        validate_selected_graph_path_reference(path, &owners, &format!("{field}.{name}"))?;
                    }
                }
                for (dependency_index, dependency) in unit.dependencies.iter().enumerate() {
                    validate_selected_graph_alias(
                        &dependency.alias,
                        &format!("{field}.dependencies[{dependency_index}].alias"),
                    )?;
                    validate_selected_graph_text(
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
                        "must be sorted by unique alias",
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
                    )?;
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
                        "must be sorted by unique name",
                    ));
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

            if self.exposed_roots.is_empty() {
                return Err(selected_graph_missing("exposed_roots"));
            }
            for (alias, unit) in &self.exposed_roots {
                validate_selected_graph_alias(alias, "exposed_roots alias")?;
                if !unit_indexes.contains_key(unit.as_str()) {
                    return Err(selected_graph_missing(format!("exposed_roots.{alias} unit `{unit}`")));
                }
            }
            Ok(())
        }

        /// Validate and hash the canonical portable graph without consulting a filesystem or ambient toolchain.
        pub(crate) fn validated(self) -> Result<ValidatedOvenSelectedRustFacetGraph, OvenSelectedRustFacetGraphError> {
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

    fn leaf_unit() -> Result<OvenSelectedRustFacetUnit, OvenSelectedRustFacetGraphError> {
        let source_members = vec![member("src/lib.rs", b"pub struct Dependency;\n")];
        Ok(OvenSelectedRustFacetUnit {
            id: "unit-dependency".to_string(),
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
                "owner-dependency",
                &source_members,
            )?,
            root_module: "src/lib.rs".to_string(),
            source_members,
            features: Vec::new(),
            default_features: false,
            cfg: Vec::new(),
            environment: BTreeMap::new(),
            include_dirs: vec![OvenSelectedRustFacetPath {
                owner: "owner-dependency".to_string(),
                path: ".".to_string(),
            }],
            exclude_dirs: Vec::new(),
            dependencies: Vec::new(),
            generated_inputs: Vec::new(),
        })
    }

    fn root_unit(bytes: &[u8]) -> Result<OvenSelectedRustFacetUnit, OvenSelectedRustFacetGraphError> {
        let source_members = vec![member("src/lib.rs", bytes)];
        Ok(OvenSelectedRustFacetUnit {
            id: "unit-root".to_string(),
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
                "owner-project",
                &source_members,
            )?,
            root_module: "src/lib.rs".to_string(),
            source_members,
            features: vec!["root-feature".to_string()],
            default_features: true,
            cfg: vec!["feature=\"root-feature\"".to_string()],
            environment: BTreeMap::new(),
            include_dirs: vec![OvenSelectedRustFacetPath {
                owner: "owner-project".to_string(),
                path: ".".to_string(),
            }],
            exclude_dirs: Vec::new(),
            dependencies: vec![OvenSelectedRustFacetDependency {
                alias: "renamed_dep".to_string(),
                unit: "unit-dependency".to_string(),
            }],
            generated_inputs: Vec::new(),
        })
    }

    fn graph(root_bytes: &[u8]) -> Result<OvenSelectedRustFacetGraph, OvenSelectedRustFacetGraphError> {
        Ok(OvenSelectedRustFacetGraph {
            schema_version: OVEN_SELECTED_RUST_FACET_GRAPH_SCHEMA_VERSION,
            selection: OvenSelectedRustFacetSelection {
                intent: OvenBuildIntent {
                    target: "x86_64-unknown-linux-gnu".to_string(),
                    toolchain: "rustc 1.85.0 (fixture)".to_string(),
                    profile: "dev".to_string(),
                    features: vec!["root-feature".to_string()],
                },
                host: "x86_64-unknown-linux-gnu".to_string(),
                purpose: OvenSelectedRustFacetPurpose::Normal,
                default_features: true,
                toolchain_version: "1.85.0".to_string(),
                target_spec_digest: selected_graph_sha256(b"fixture target spec"),
            },
            owners: vec![
                OvenSelectedRustFacetOwner {
                    id: "owner-dependency".to_string(),
                    identity: selected_graph_sha256(b"dependency owner"),
                    kind: OvenSelectedRustFacetOwnerKind::Constituent,
                },
                OvenSelectedRustFacetOwner {
                    id: "owner-project".to_string(),
                    identity: selected_graph_sha256(root_bytes),
                    kind: OvenSelectedRustFacetOwnerKind::ProjectAuthority,
                },
            ],
            units: vec![leaf_unit()?, root_unit(root_bytes)?],
            exposed_roots: BTreeMap::from([("fixture".to_string(), "unit-root".to_string())]),
        })
    }

    #[test]
    fn selected_graph_preserves_renamed_dependency_alias() -> TestResult {
        let selected = graph(b"pub fn use_dependency() {}\n")?.validated()?;
        let root = selected
            .graph()
            .units
            .iter()
            .find(|unit| unit.id == "unit-root")
            .ok_or("validated graph lost its root unit")?;
        assert_eq!(
            root.dependencies,
            vec![OvenSelectedRustFacetDependency {
                alias: "renamed_dep".to_string(),
                unit: "unit-dependency".to_string(),
            }]
        );
        let encoded = selected.to_json_bytes()?;
        let decoded = serde_json::from_slice::<OvenSelectedRustFacetGraph>(&encoded)?;
        assert_eq!(decoded, *selected.graph());
        assert!(selected.digest().starts_with("sha256:"));
        Ok(())
    }

    #[test]
    fn selected_graph_retains_an_explicit_empty_leaf() -> TestResult {
        let selected = graph(b"pub fn use_dependency() {}\n")?.validated()?;
        let encoded = selected.to_json_bytes()?;
        let mut document = serde_json::from_slice::<serde_json::Value>(&encoded)?;
        let leaf = document["units"]
            .as_array()
            .ok_or("serialized graph has no unit array")?
            .iter()
            .find(|unit| unit["id"].as_str() == Some("unit-dependency"))
            .ok_or("serialized graph lost its leaf unit")?;
        assert_eq!(leaf["dependencies"].as_array().map(Vec::len), Some(0));
        assert_eq!(leaf["features"].as_array().map(Vec::len), Some(0));
        assert_eq!(leaf["cfg"].as_array().map(Vec::len), Some(0));
        assert_eq!(leaf["environment"].as_object().map(serde_json::Map::len), Some(0));
        assert_eq!(leaf["exclude_dirs"].as_array().map(Vec::len), Some(0));
        assert_eq!(leaf["generated_inputs"].as_array().map(Vec::len), Some(0));

        let missing_leaf = document["units"]
            .as_array_mut()
            .ok_or("serialized graph has no mutable unit array")?
            .iter_mut()
            .find(|unit| unit["id"].as_str() == Some("unit-dependency"))
            .and_then(serde_json::Value::as_object_mut)
            .ok_or("serialized graph lost its mutable leaf unit")?;
        assert!(missing_leaf.remove("dependencies").is_some());
        assert!(serde_json::from_value::<OvenSelectedRustFacetGraph>(document).is_err());
        Ok(())
    }

    #[test]
    fn selected_graph_refuses_missing_units_owners_digests_and_root_references() -> TestResult {
        let baseline = graph(b"pub fn use_dependency() {}\n")?;

        let mut missing_unit = baseline.clone();
        missing_unit.units[1].dependencies[0].unit = "unit-absent".to_string();
        assert!(matches!(
            missing_unit.validated(),
            Err(OvenSelectedRustFacetGraphError::Missing { .. })
        ));

        let mut missing_owner = baseline.clone();
        missing_owner.units[1].source.owner = "owner-absent".to_string();
        assert!(matches!(
            missing_owner.validated(),
            Err(OvenSelectedRustFacetGraphError::Missing { .. })
        ));

        let mut missing_digest = baseline.clone();
        missing_digest.units[1].source.digest.clear();
        assert!(matches!(
            missing_digest.validated(),
            Err(OvenSelectedRustFacetGraphError::Missing { .. })
        ));

        let mut missing_root = baseline;
        missing_root
            .exposed_roots
            .insert("fixture".to_string(), "unit-absent".to_string());
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
}
