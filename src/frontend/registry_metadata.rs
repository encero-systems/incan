//! Checked typed-registry metadata shared by inspection and package artifacts.
//!
//! This module deliberately converts the typechecker's RFC 113 artifacts rather than parsing decorators, source
//! comments, generated Rust, or runtime registry values. Runtime entries remain process-local; these records are the
//! complete checked projection used by package tooling.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::frontend::api_metadata::{ApiDeclaration, CheckedApiMetadata};
use crate::frontend::typechecker::{RegistryExplicitEntryInfo, TypeCheckInfo};
use incan_semantics_core::{SemanticRegistrySubjectKind, SemanticRegistryValue};

/// Wire-schema version for the checked-registry package projection.
pub const CHECKED_REGISTRY_METADATA_SCHEMA_VERSION: u32 = 1;

/// Checked registry metadata embedded in a source inspection result or `.incnlib` artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckedRegistryMetadataPackage {
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<CheckedRegistryPackageIdentity>,
    pub modules: Vec<CheckedRegistryMetadataModule>,
}

/// Package identity known at the collection boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckedRegistryPackageIdentity {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// Checked registry definitions and entries originating from one source compilation unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckedRegistryMetadataModule {
    pub module_path: Vec<String>,
    pub registries: Vec<CheckedRegistryDefinition>,
    pub entries: Vec<CheckedRegistryEntry>,
}

/// Public or local typed registry definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckedRegistryDefinition {
    /// Canonical compiler identity: `<module>::<binding>`.
    pub identity: String,
    pub binding: String,
    pub public: bool,
    pub key_type: String,
    pub descriptor_type: String,
    pub subjects: Vec<CheckedRegistrySubjectKind>,
    /// Public facade import paths that resolve to this source-owned registry binding.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reexport_paths: Vec<CheckedRegistryReexport>,
}

/// One complete compiler-checked registry entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckedRegistryEntry {
    pub registry_identity: String,
    pub registry_public: bool,
    pub key: CheckedRegistryValue,
    pub descriptor: CheckedRegistryValue,
    pub subject_kind: CheckedRegistrySubjectKind,
    pub subject_identity: String,
    pub registration_anchor: CheckedRegistrySourceAnchor,
    pub subject_anchor: CheckedRegistrySourceAnchor,
    pub provenance: CheckedRegistryProvenance,
    /// Public facade import paths that resolve to this source-owned subject.
    ///
    /// A reexport is a projection only: it never makes a second registry entry or changes the subject identity.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reexport_paths: Vec<CheckedRegistryReexport>,
}

/// One public facade path for a source-owned registry binding or described subject.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckedRegistryReexport {
    /// Fully-qualified public import path, split into Incan path segments.
    pub path: Vec<String>,
    /// Source anchor of the facade's public import declaration.
    pub anchor: CheckedRegistrySourceAnchor,
}

/// Provenance remains explicit so a future runtime observation cannot be mistaken for a checked declaration fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckedRegistryProvenance {
    CheckedDeclaration,
    CheckedCompilationUnitEntry,
    CheckedPackageEntry,
}

/// Source anchor with a stable compiler-facing identity and byte range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckedRegistrySourceAnchor {
    pub id: String,
    pub start: usize,
    pub end: usize,
}

/// Stable checked registry subject kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckedRegistrySubjectKind {
    Function,
    Method,
    CompilationUnit,
    Package,
}

/// Structural descriptor or key value retained without user-code evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum CheckedRegistryValue {
    Int(i64),
    Float(String),
    Bool(bool),
    String(String),
    Bytes(Vec<u8>),
    None,
    Type(String),
    Option(Box<CheckedRegistryValue>),
    List(Vec<CheckedRegistryValue>),
    Dict(Vec<CheckedRegistryDictEntry>),
    ConstRef(Vec<String>),
    Newtype {
        name: String,
        value: Box<CheckedRegistryValue>,
    },
    Model {
        name: String,
        fields: Vec<CheckedRegistryModelField>,
    },
}

/// One deterministic dictionary entry in a checked structural value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckedRegistryDictEntry {
    pub key: CheckedRegistryValue,
    pub value: CheckedRegistryValue,
}

/// One deterministic named model field in a checked structural value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckedRegistryModelField {
    pub name: String,
    pub value: CheckedRegistryValue,
}

/// Convert the registry artifacts from one typechecked module into the package-facing checked projection.
///
/// `package_identity` comes from the command's manifest-or-entrypoint boundary. It is deliberately passed in rather
/// than inferred from a module path so explicit package subjects agree with the runtime value produced by the same
/// compilation and never pretend that a module name is a package name.
pub fn collect_checked_registry_metadata(
    type_info: &TypeCheckInfo,
    module_path: Vec<String>,
    package_identity: &str,
) -> CheckedRegistryMetadataModule {
    let module_identity = module_path.join("::");
    let mut registries = type_info
        .registry
        .definitions
        .iter()
        .map(|(binding, definition)| CheckedRegistryDefinition {
            identity: format!("{module_identity}::{binding}"),
            binding: binding.clone(),
            public: definition.is_public,
            key_type: definition.key_type.to_string(),
            descriptor_type: definition.descriptor_type.to_string(),
            subjects: sorted_subjects(&definition.subjects),
            reexport_paths: Vec::new(),
        })
        .collect::<Vec<_>>();
    registries.sort_by(|left, right| left.identity.cmp(&right.identity));

    let mut entries = Vec::new();
    for description in &type_info.registry.descriptions {
        let Some(definition) = type_info.registry.definitions.get(&description.registry_name) else {
            continue;
        };
        let subject_identity = format!("{module_identity}.{}", description.declaration_name);
        entries.push(CheckedRegistryEntry {
            registry_identity: format!("{module_identity}::{}", description.registry_name),
            registry_public: definition.is_public,
            key: checked_registry_value(&description.key),
            descriptor: checked_registry_value(&description.descriptor),
            subject_kind: description.subject_kind.into(),
            subject_identity,
            registration_anchor: CheckedRegistrySourceAnchor {
                id: format!(
                    "{module_identity}#describe.{}..{}",
                    description.decorator_span.0, description.decorator_span.1
                ),
                start: description.decorator_span.0,
                end: description.decorator_span.1,
            },
            subject_anchor: CheckedRegistrySourceAnchor {
                id: format!(
                    "{module_identity}#declaration.{}..{}",
                    description.declaration_span.0, description.declaration_span.1
                ),
                start: description.declaration_span.0,
                end: description.declaration_span.1,
            },
            provenance: CheckedRegistryProvenance::CheckedDeclaration,
            reexport_paths: Vec::new(),
        });
    }
    for entry in &type_info.registry.explicit_entries {
        let Some(definition) = type_info.registry.definitions.get(&entry.registry_name) else {
            continue;
        };
        entries.push(checked_explicit_entry(
            &module_identity,
            package_identity,
            definition.is_public,
            entry,
        ));
    }
    entries.sort_by(|left, right| {
        (
            &left.registry_identity,
            &left.subject_identity,
            left.registration_anchor.start,
            left.registration_anchor.end,
        )
            .cmp(&(
                &right.registry_identity,
                &right.subject_identity,
                right.registration_anchor.start,
                right.registration_anchor.end,
            ))
    });

    CheckedRegistryMetadataModule {
        module_path,
        registries,
        entries,
    }
}

/// Convert one checked compilation-unit or package registration into the portable metadata projection.
fn checked_explicit_entry(
    module_identity: &str,
    package_identity: &str,
    registry_public: bool,
    entry: &RegistryExplicitEntryInfo,
) -> CheckedRegistryEntry {
    let (subject_identity, provenance) = match entry.subject_kind {
        SemanticRegistrySubjectKind::CompilationUnit => (
            module_identity.to_string(),
            CheckedRegistryProvenance::CheckedCompilationUnitEntry,
        ),
        SemanticRegistrySubjectKind::Package => (
            package_identity.to_string(),
            CheckedRegistryProvenance::CheckedPackageEntry,
        ),
        SemanticRegistrySubjectKind::Function | SemanticRegistrySubjectKind::Method => {
            unreachable!("explicit registry entries are limited to unit and package subjects")
        }
    };
    CheckedRegistryEntry {
        registry_identity: format!("{module_identity}::{}", entry.registry_name),
        registry_public,
        key: checked_registry_value(&entry.key),
        descriptor: checked_registry_value(&entry.descriptor),
        subject_kind: entry.subject_kind.into(),
        subject_identity: subject_identity.clone(),
        registration_anchor: CheckedRegistrySourceAnchor {
            id: format!(
                "{module_identity}#static.{}..{}",
                entry.declaration_span.0, entry.declaration_span.1
            ),
            start: entry.declaration_span.0,
            end: entry.declaration_span.1,
        },
        subject_anchor: CheckedRegistrySourceAnchor {
            id: subject_identity,
            start: entry.subject_span.0,
            end: entry.subject_span.1,
        },
        provenance,
        reexport_paths: Vec::new(),
    }
}

/// Attach public import-alias paths to source-owned registry facts.
///
/// The checked API projection is the package's existing authority for public aliases. Registry metadata uses it only
/// to describe an additional consumer path; registry identity, descriptor, source anchors, and provenance continue to
/// come from the module that declared the registry entry. This keeps a chain of facade reexports from becoming a
/// chain of duplicate semantic entries.
pub fn materialize_registry_reexport_projections(
    registry_modules: &mut [CheckedRegistryMetadataModule],
    api_modules: &[CheckedApiMetadata],
) {
    let mut aliases = BTreeMap::new();
    for module in api_modules {
        for declaration in &module.declarations {
            let ApiDeclaration::Alias(alias) = declaration else {
                continue;
            };
            let mut path = module.module_path.clone();
            path.push(alias.name.clone());
            aliases.insert(
                path,
                (
                    normalized_alias_target(&alias.target_path),
                    CheckedRegistrySourceAnchor {
                        id: alias.anchor.id.clone(),
                        start: alias.anchor.span.start,
                        end: alias.anchor.span.end,
                    },
                ),
            );
        }
    }

    let projections = aliases
        .keys()
        .filter_map(|path| {
            let (_, anchor) = aliases.get(path)?;
            let target = resolve_alias_target(path, &aliases)?;
            Some((
                target,
                CheckedRegistryReexport {
                    path: path.clone(),
                    anchor: anchor.clone(),
                },
            ))
        })
        .collect::<Vec<_>>();

    for module in registry_modules {
        for registry in &mut module.registries {
            let canonical = registry_identity_path(&registry.identity);
            registry.reexport_paths = projections_for_target(&projections, &canonical);
        }
        for entry in &mut module.entries {
            let Some(canonical) = declaration_subject_path(&entry.subject_identity) else {
                continue;
            };
            entry.reexport_paths = projections_for_target(&projections, &canonical);
        }
    }
}

/// Remove the API metadata spelling's optional `crate` root marker.
fn normalized_alias_target(path: &[String]) -> Vec<String> {
    path.strip_prefix(&["crate".to_string()]).unwrap_or(path).to_vec()
}

/// Follow an alias chain to its canonical source path. Cycles are omitted rather than represented as an invented
/// source-owned registry projection; normal import resolution reports those invalid source programs separately.
fn resolve_alias_target(
    path: &[String],
    aliases: &BTreeMap<Vec<String>, (Vec<String>, CheckedRegistrySourceAnchor)>,
) -> Option<Vec<String>> {
    let mut current = aliases.get(path)?.0.clone();
    let mut visited = BTreeSet::new();
    visited.insert(path.to_vec());
    while let Some((next, _)) = aliases.get(&current) {
        if !visited.insert(current.clone()) {
            return None;
        }
        current = next.clone();
    }
    Some(current)
}

/// Convert `<module>::<binding>` into the API projection path vocabulary.
fn registry_identity_path(identity: &str) -> Vec<String> {
    identity.split("::").map(str::to_string).collect()
}

/// Convert a function subject identity such as `pkg::text.normalize` into an API path.
///
/// Methods intentionally have no facade alias projection today: importing a model does not introduce an alias for an
/// individual method. Their canonical source subject remains discoverable in the same checked entry.
fn declaration_subject_path(identity: &str) -> Option<Vec<String>> {
    let (module, declaration) = identity.rsplit_once('.')?;
    if declaration.contains('.') {
        return None;
    }
    let mut path = module.split("::").map(str::to_string).collect::<Vec<_>>();
    path.push(declaration.to_string());
    Some(path)
}

/// Return deterministic, deduplicated public reexport projections for one canonical declaration target.
fn projections_for_target(
    projections: &[(Vec<String>, CheckedRegistryReexport)],
    canonical: &[String],
) -> Vec<CheckedRegistryReexport> {
    let mut matches = projections
        .iter()
        .filter(|(target, _)| target == canonical)
        .map(|(_, projection)| projection.clone())
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| {
        (&left.path, &left.anchor.id, left.anchor.start, left.anchor.end).cmp(&(
            &right.path,
            &right.anchor.id,
            right.anchor.start,
            right.anchor.end,
        ))
    });
    matches.dedup();
    matches
}

/// Convert semantic subject kinds to their stable metadata representation in deterministic order.
fn sorted_subjects(subjects: &[SemanticRegistrySubjectKind]) -> Vec<CheckedRegistrySubjectKind> {
    let mut exported = subjects.iter().copied().map(Into::into).collect::<Vec<_>>();
    exported.sort();
    exported
}

/// Convert one compiler-owned structural registry value to its portable checked metadata representation.
fn checked_registry_value(value: &SemanticRegistryValue) -> CheckedRegistryValue {
    match value {
        SemanticRegistryValue::Int(value) => CheckedRegistryValue::Int(*value),
        SemanticRegistryValue::Float(value) => CheckedRegistryValue::Float(value.clone()),
        SemanticRegistryValue::Bool(value) => CheckedRegistryValue::Bool(*value),
        SemanticRegistryValue::String(value) => CheckedRegistryValue::String(value.clone()),
        SemanticRegistryValue::Bytes(value) => CheckedRegistryValue::Bytes(value.clone()),
        SemanticRegistryValue::None => CheckedRegistryValue::None,
        SemanticRegistryValue::Type(value) => CheckedRegistryValue::Type(value.clone()),
        SemanticRegistryValue::Option(value) => CheckedRegistryValue::Option(Box::new(checked_registry_value(value))),
        SemanticRegistryValue::List(values) => {
            CheckedRegistryValue::List(values.iter().map(checked_registry_value).collect())
        }
        SemanticRegistryValue::Dict(entries) => CheckedRegistryValue::Dict(
            entries
                .iter()
                .map(|(key, value)| CheckedRegistryDictEntry {
                    key: checked_registry_value(key),
                    value: checked_registry_value(value),
                })
                .collect(),
        ),
        SemanticRegistryValue::ConstRef(path) => CheckedRegistryValue::ConstRef(path.clone()),
        SemanticRegistryValue::Newtype { name, value } => CheckedRegistryValue::Newtype {
            name: name.clone(),
            value: Box::new(checked_registry_value(value)),
        },
        SemanticRegistryValue::Model { name, fields } => CheckedRegistryValue::Model {
            name: name.clone(),
            fields: fields
                .iter()
                .map(|(name, value)| CheckedRegistryModelField {
                    name: name.clone(),
                    value: checked_registry_value(value),
                })
                .collect(),
        },
    }
}

impl From<SemanticRegistrySubjectKind> for CheckedRegistrySubjectKind {
    fn from(value: SemanticRegistrySubjectKind) -> Self {
        match value {
            SemanticRegistrySubjectKind::Function => Self::Function,
            SemanticRegistrySubjectKind::Method => Self::Method,
            SemanticRegistrySubjectKind::CompilationUnit => Self::CompilationUnit,
            SemanticRegistrySubjectKind::Package => Self::Package,
        }
    }
}
