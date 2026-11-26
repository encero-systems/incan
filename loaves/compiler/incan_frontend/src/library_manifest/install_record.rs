//! What installing a compiled Loaf reads of it (DD-0015 §9, step 2), as one compact record of the Loaf.
//!
//! A consumer's checker installs a compiled Loaf a dependency interface names. Installing reads the Loaf's nominal
//! public types with their declarations' identities, its root exports by kind and the foreign type origins its
//! signatures carry; every layout is built later, by name. The library compile writes this record beside the Loaf's
//! declaration records so that installing reads it instead of decoding the whole manifest. The record is in the
//! manifest's own published forms: names, paths, canonical identities and type origins.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::{fs, path::Path};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::compiler_records::{CompilerRecord, RecordFamily};
use super::{
    AliasExport, CanonicalIdentityExport, LibraryIdentityGraph, LibraryManifest, LibraryManifestError,
    NominalTypeOriginExport, TypeParamExport, TypeRef,
};

/// Version of the Oven library-facts sidecar, including optional independently addressed values.
pub const LOAF_FACTS_SCHEMA: &str = "incan.loaf-facts/1";

/// The install value's independent address and exact encoded bytes in the existing facts sidecar.
#[derive(Deserialize, Serialize)]
pub struct PublishedInstallRecord {
    key: String,
    revision: String,
    value: String,
}

impl PublishedInstallRecord {
    /// Transport the same key and byte identity as the compiler record set.
    pub fn new(record: &CompilerRecord) -> Self {
        Self {
            key: record.key(),
            revision: format!("{:x}", Sha256::digest(&record.value)),
            value: hex::encode(&record.value),
        }
    }
}

/// Open only the install record selected by this exact manifest, and authenticate its bytes and independent name.
/// Callers preserve legacy behavior only when the admitted manifest has no install descriptor.
pub fn read_published_install_record(
    manifest_path: &Path,
    name: &str,
    manifest_digest: &str,
    expected_digest: &str,
) -> Result<CompilerRecord, LibraryManifestError> {
    read_facts(manifest_path, name, manifest_digest, expected_digest).map(|(record, _)| record)
}

/// Check the install binding once and return the optional named API payload beside it.
fn read_facts(
    manifest_path: &Path,
    name: &str,
    manifest_digest: &str,
    expected_digest: &str,
) -> Result<(CompilerRecord, Option<String>), LibraryManifestError> {
    #[derive(Deserialize)]
    struct Facts {
        schema: String,
        manifest_digest: String,
        install_record: PublishedInstallRecord,
        path_named_records: Option<String>,
    }
    let path = manifest_path.with_extension("incnfacts");
    let content = fs::read(&path).map_err(|source| LibraryManifestError::Read {
        path: path.clone(),
        source,
    })?;
    let invalid =
        |reason: &str| LibraryManifestError::Invalid(format!("install record sidecar {}: {reason}", path.display()));
    let facts: Facts = serde_json::from_slice(&content).map_err(|error| invalid(&error.to_string()))?;
    let manifest_digest = manifest_digest.strip_prefix("sha256:").unwrap_or(manifest_digest);
    if facts.schema != LOAF_FACTS_SCHEMA || facts.manifest_digest != manifest_digest {
        return Err(invalid("sidecar names a different manifest or schema"));
    }
    let stored = facts.install_record;
    let value = hex::decode(stored.value).map_err(|error| invalid(&error.to_string()))?;
    let record = CompilerRecord {
        family: RecordFamily::InstallRecord,
        index: 0,
        name: name.to_owned(),
        value,
    };
    let revision = format!("{:x}", Sha256::digest(&record.value));
    if stored.key != record.key() || stored.revision != revision || revision != expected_digest {
        return Err(invalid("named value disagrees with its selected content identity"));
    }
    Ok((record, facts.path_named_records))
}

/// Exact compiler-record values grouped by family and name in publisher order for overloads.
type PathLookup = BTreeMap<String, BTreeMap<String, Vec<String>>>;

/// Prepare the path Loaf's named API lookup from the same encoded values the installed record set uses.
pub(super) fn encode_path_lookup(records: &[CompilerRecord]) -> Result<String, LibraryManifestError> {
    let mut lookup = PathLookup::new();
    for record in records {
        lookup
            .entry(record.family.as_str().to_owned())
            .or_default()
            .entry(record.name.clone())
            .or_default()
            .push(hex::encode(&record.value));
    }
    serde_json::to_string(&lookup).map_err(|error| LibraryManifestError::Serialize(error.to_string()))
}

/// A path Loaf's checked install record and the exact named API lookups it authorizes.
#[derive(Debug)]
pub(crate) struct PathInstallFacts {
    pub(crate) record: Arc<LoafInstallRecord>,
    /// The producer's authenticated ABI index also answers explicit Rust re-exports.
    pub(crate) indexed_rust_reexports: bool,
    /// The manifest format covered by the Rust re-export index and a complete public namespace directory.
    pub(crate) rust_reexport_manifest_format: Option<u32>,
    lookup: Option<PathLookup>,
}

impl PathInstallFacts {
    /// Open both projections from the one sidecar. A missing or altered declared lookup is a refusal.
    pub(crate) fn read(
        manifest_path: &Path,
        name: &str,
        manifest_digest: &str,
        expected_digest: &str,
    ) -> Result<Self, LibraryManifestError> {
        #[derive(Deserialize)]
        struct InstallValue {
            #[serde(flatten)]
            record: LoafInstallRecord,
            path_lookup_digest: Option<String>,
            #[serde(default)]
            indexed_rust_reexports: bool,
            rust_reexport_manifest_format: Option<u32>,
        }
        let (value, lookup) = read_facts(manifest_path, name, manifest_digest, expected_digest)?;
        let install: InstallValue = LibraryManifest::decode_compiler_record_value(&value.value)?;
        let lookup = match (install.path_lookup_digest, lookup) {
            (Some(expected), Some(bytes)) if format!("{:x}", Sha256::digest(bytes.as_bytes())) == expected => {
                Some(serde_json::from_str(&bytes).map_err(|error| LibraryManifestError::Invalid(error.to_string()))?)
            }
            (None, None) => None,
            _ => {
                return Err(LibraryManifestError::Invalid(format!(
                    "named API records of `{name}` disagree with their selected content identity"
                )));
            }
        };
        tracing::trace!(target: "incan::invocation", phase = "path_install_record",
            loaf = name, key = %value.key(), revision = expected_digest, bytes = value.value.len());
        Ok(Self {
            record: Arc::new(install.record),
            indexed_rust_reexports: install.indexed_rust_reexports,
            rust_reexport_manifest_format: install.rust_reexport_manifest_format,
            lookup,
        })
    }

    /// Decode only the named values requested by this checker.
    pub(crate) fn lookup(
        &self,
        family: &str,
        name: &str,
    ) -> Result<Option<Vec<serde_json::Value>>, LibraryManifestError> {
        let Some(values) = self
            .lookup
            .as_ref()
            .and_then(|lookup| lookup.get(family))
            .and_then(|family| family.get(name))
        else {
            return Ok(None);
        };
        values
            .iter()
            .map(|value| {
                let encoded = hex::decode(value).map_err(|error| LibraryManifestError::Invalid(error.to_string()))?;
                LibraryManifest::decode_compiler_record_value(&encoded)
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }

    /// Check a complete named API family without opening any value. Other families still use their prior authority.
    pub(crate) fn names(&self, families: &[&str], name: Option<&str>) -> Option<bool> {
        let lookup = self.lookup.as_ref()?;
        if !families.iter().all(|family| Self::supports(family)) {
            return None;
        }
        Some(families.iter().any(|family| {
            lookup
                .get(*family)
                .is_some_and(|records| name.map_or(!records.is_empty(), |name| records.contains_key(name)))
        }))
    }

    /// Whether this producer publishes an exhaustive index for the named API family.
    pub(crate) fn supports(family: &str) -> bool {
        matches!(
            family,
            "public-namespace" | "declaration-signature" | "layout" | "default" | "type-scope"
        )
    }
}

/// Schema of the install record.
pub const INSTALL_RECORD_SCHEMA: u32 = 1;

/// What installing one compiled Loaf reads of it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LoafInstallRecord {
    pub schema: u32,
    /// Each unique top-level public name of the Loaf that names a nominal type, in identity-graph order: the public
    /// name, the source path of the declaration it names and that declaration's canonical identity. A consumer binds
    /// each under its own canonical spelling and provider.
    pub canonical_types: Vec<InstallCanonicalType>,
    /// The Loaf's root exports by kind, in manifest order.
    pub root_exports: InstallRootExports,
    /// The foreign nominal type origins the Loaf's published signatures carry, in binding-key order.
    pub type_origins: Vec<NominalTypeOriginExport>,
    /// Each checked API module with its model, class, enum, newtype, type alias and trait declarations, in module
    /// and declaration order. A consumer registers them under its declaration keys and replays their lexical scopes.
    pub modules: Vec<InstallModule>,
    /// The Loaf's identity graph, as its manifest publishes it. Install lays out each nominal export under its
    /// canonical spelling, and the foreign preparation of a Loaf that reaches this one reads it.
    pub identity_graph: LibraryIdentityGraph,
    /// For each public name the identity graph publishes, in first-occurrence order, the kind of root export the
    /// manifest gives that name first, in the order install searches the root exports.
    pub export_kinds: Vec<InstallNamedExport>,
    /// The Loaf's root alias exports, which install resolves when it lays them out.
    pub aliases: Vec<AliasExport>,
    /// Nested identity-graph sources that describe a type layout or a type alias.
    pub nested_layouts: Vec<InstallNestedLayout>,
}

impl LoafInstallRecord {
    /// The kind of root export each public name the identity graph publishes finds first, by name.
    pub fn export_kinds_by_name(&self) -> std::collections::HashMap<&str, Option<InstallExportKind>> {
        self.export_kinds
            .iter()
            .map(|named| (named.public_name.as_str(), named.kind))
            .collect()
    }
}

/// Whether a root export of this kind is a model, class, enum or newtype.
pub fn is_nominal_export(kind: Option<InstallExportKind>) -> bool {
    matches!(
        kind,
        Some(
            InstallExportKind::Model | InstallExportKind::Class | InstallExportKind::Enum | InstallExportKind::Newtype
        )
    )
}

/// The kind of root export a public name finds first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallNamedExport {
    pub public_name: String,
    /// `None` when no root export has the name.
    pub kind: Option<InstallExportKind>,
}

/// The kinds of a manifest's root exports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstallExportKind {
    Alias,
    Model,
    Class,
    Function,
    Partial,
    Trait,
    Enum,
    EnumVariant,
    TypeAlias,
    Newtype,
    Const,
    Static,
}

/// One checked API module of a compiled Loaf and the nominal declarations install registers from it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InstallModule {
    pub module_path: Vec<String>,
    pub declarations: Vec<InstallDeclaration>,
    /// Spellings in this module that resolve through a declaration rather than a root export.
    pub type_scope: BTreeMap<String, InstallScopeTarget>,
}

/// A declaration reached by a spelling in a checked module's type scope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallScopeTarget {
    /// The declaring module's authored path within the package.
    pub module: Vec<String>,
    /// The declaration's name in its module.
    pub name: String,
}

/// The layout recipe available at one nested identity export's source path.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InstallNestedLayout {
    /// The checked declaration path named by the identity graph.
    pub source_path: Vec<String>,
    /// How this declaration yields a type layout.
    pub kind: InstallNestedLayoutKind,
}

/// How a nested export describes a type.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum InstallNestedLayoutKind {
    /// A model, class, enum or newtype whose fields and methods are read on demand.
    Nominal,
    /// A checked type alias.
    TypeAlias,
    /// A forwarded type whose projection identifies the type it names.
    Forwarded(TypeRef),
}

/// One model, class, enum, newtype, type alias or trait of a checked API module.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InstallDeclaration {
    /// Its position among the module's checked declarations, where its layout is built from.
    pub position: usize,
    pub name: String,
    pub kind: InstallDeclarationKind,
    /// The canonical identity the Loaf's identity graph publishes at the declaration's public path.
    pub canonical: Option<CanonicalIdentityExport>,
    /// A type alias's target.
    pub alias_target: Option<TypeRef>,
    /// The type parameters of a type alias, model or class.
    pub type_params: Vec<TypeParamExport>,
}

/// The kinds of declaration install registers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstallDeclarationKind {
    Model,
    Class,
    Enum,
    Newtype,
    TypeAlias,
    Trait,
}

/// One nominal public type of a compiled Loaf.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallCanonicalType {
    pub public_name: String,
    pub source_path: Vec<String>,
    pub canonical: Option<CanonicalIdentityExport>,
}

/// The names of a compiled Loaf's root exports by kind, in manifest order.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallRootExports {
    pub models: Vec<String>,
    pub classes: Vec<String>,
    pub enums: Vec<String>,
    pub newtypes: Vec<String>,
    pub traits: Vec<String>,
}
