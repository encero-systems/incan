//! The installed compiler records of one Loaf and their two digests (RFC 179).
//!
//! The toolchain's driver exports these records once at installation: one record per exported declaration and
//! per made-private type, vocabulary and soft-keyword activation, the Loaf's contract metadata, its documentation, its
//! Rust ABI, its executable descriptor and public executable identities (all executable resolution reads besides the
//! metadata), its runtime callable bindings (all native lowering reads of it), each checked declaration's signature,
//! layout and defaults (what a consumer reads of one declaration), each public namespace of its checked API (what a
//! consumer reads to resolve an import of it), and the Loaf's metadata record, which carries the digests.
//!
//! - The **record-content digest** covers every record, documentation included: each record's key, model description
//!   hash and object hash (the SHA-256 of its bytes), in key order.
//! - The **semantic facts digest** covers every record except documentation: each record's key, kind, model description
//!   hash and canonical hash, in key order. A canonical hash leaves out source positions: anchor spans, the spans
//!   inside canonical identities, and the executable descriptor's digest of the governed bodies' bytes, whose Body IR
//!   carries spans. The metadata record's canonical value also leaves out its publication fields: the Loaf's version,
//!   the version of the toolchain that checked it and the digest of the manifest file, which covers docstrings.
//!   Republishing a Loaf under another version, or changing only its documentation, whatever its length, keeps its
//!   semantic facts digest.
//!
//! The digest fields themselves are left out of the metadata value both digests cover.

use serde::Serialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::declaration_records::{DeclarationFacet, declaration_facts};
use super::executable_surface::{own_public_identities, runtime_bindings};
use super::{LibraryManifest, LibraryManifestError};

/// The record identity and value schema every installed compiler record follows.
pub const COMPILER_RECORD_PROFILE: &str = "incan.compiler-records/2";

/// The metadata fields that describe a publication rather than what consumers compile against.
const PUBLICATION_FIELDS: [&str; 3] = ["version", "incan_version", "manifest_digest"];

/// The fields that hold source positions: an anchor's span and a canonical identity's declaration span.
const POSITION_FIELDS: [&str; 2] = ["span", "declaration_span"];

/// The executable descriptor's field naming the governed bodies by the digest of their bytes, which carry positions.
const BODIES_DIGEST_FIELD: &str = "content_digest";

/// The closed record families of one Loaf's installed compiler records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordFamily {
    Metadata,
    Alias,
    Partial,
    Model,
    Class,
    Function,
    Trait,
    Enum,
    TypeAlias,
    Newtype,
    Const,
    Static,
    MadePrivateType,
    Vocabulary,
    SoftKeyword,
    ContractMetadata,
    Documentation,
    /// The executable descriptor executable resolution checks a reached Loaf against.
    ExecutableDescriptor,
    /// The public executable identities a Loaf declares itself.
    PublicIdentities,
    /// The runtime callables native lowering binds by their public path.
    RuntimeBindings,
    /// One declaration's signature: what a caller checks a use against.
    DeclarationSignature,
    /// One nominal declaration's fields or variants.
    Layout,
    /// Which parameters and fields of one declaration have a default, and the defaults.
    Default,
    /// One public namespace of the checked API: its members, child modules and the identities of its public paths, and
    /// what its parent namespace publishes under its name.
    PublicNamespace,
    RustAbi,
}

impl RecordFamily {
    /// The family's spelling in record keys.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Metadata => "metadata",
            Self::Alias => "alias",
            Self::Partial => "partial",
            Self::Model => "model",
            Self::Class => "class",
            Self::Function => "function",
            Self::Trait => "trait",
            Self::Enum => "enum",
            Self::TypeAlias => "type-alias",
            Self::Newtype => "newtype",
            Self::Const => "const",
            Self::Static => "static",
            Self::MadePrivateType => "made-private-type",
            Self::Vocabulary => "vocabulary",
            Self::SoftKeyword => "soft-keyword",
            Self::ContractMetadata => "contract-metadata",
            Self::Documentation => "documentation",
            Self::ExecutableDescriptor => "executable-descriptor",
            Self::PublicIdentities => "public-identities",
            Self::RuntimeBindings => "runtime-bindings",
            Self::DeclarationSignature => "declaration-signature",
            Self::Layout => "layout",
            Self::Default => "default",
            Self::PublicNamespace => "public-namespace",
            Self::RustAbi => "rust-abi",
        }
    }

    /// The family holding one declaration fact.
    pub fn of_declaration(facet: DeclarationFacet) -> Self {
        match facet {
            DeclarationFacet::Signature => Self::DeclarationSignature,
            DeclarationFacet::Layout => Self::Layout,
            DeclarationFacet::Defaults => Self::Default,
        }
    }

    /// The hash of the family's value model: its schema within the record profile.
    fn model_description_hash(self) -> String {
        sha256_hex(format!("{COMPILER_RECORD_PROFILE}/{}", self.as_str()).as_bytes())
    }
}

/// One installed compiler record of a Loaf.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompilerRecord {
    pub family: RecordFamily,
    /// The record's position among its family's records.
    pub index: usize,
    /// The declaration or Loaf name the record is addressed by.
    pub name: String,
    /// The record's encoded value.
    pub value: Vec<u8>,
}

impl CompilerRecord {
    /// The record's key below its Loaf: family, ordinal and the digest of its name.
    pub fn key(&self) -> String {
        format!(
            "{}/{:08}/{}",
            self.family.as_str(),
            self.index,
            sha256_hex(self.name.as_bytes())
        )
    }
}

/// The two digests of one Loaf's compiler records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FactDigests {
    /// Every record's key, model description hash and object hash.
    pub record_content: String,
    /// Every record's key, kind, model description hash and canonical hash, without documentation and publication
    /// fields.
    pub semantic_facts: String,
}

/// One Loaf's installed compiler records, the metadata record first, and their digests.
#[derive(Clone, Debug)]
pub struct CompilerRecordSet {
    pub records: Vec<CompilerRecord>,
    pub digests: FactDigests,
}

impl LibraryManifest {
    /// Split this validated manifest into its installed compiler records; `manifest_digest` names the manifest file.
    pub fn compiler_records(&self, manifest_digest: &str) -> Result<CompilerRecordSet, LibraryManifestError> {
        let mut records = self.declaration_records()?;
        let (contract_facts, documentation) = self.contract_metadata.split_documentation();
        records.push(record(RecordFamily::ContractMetadata, 0, &self.name, &contract_facts)?);
        // A consumer reads one declaration's signature, layout or defaults by name, never the whole checked API.
        if let Some(api) = &contract_facts.api {
            records.extend(declaration_fact_records(api)?);
            records.extend(namespace_records(api, &contract_facts.identity_graph, &self.name)?);
        }
        if !documentation.is_empty() {
            records.push(record(RecordFamily::Documentation, 0, &self.name, &documentation)?);
        }
        // Executable resolution reads these two, never the whole contract metadata (RFC 179).
        if let Some(descriptor) = &self.contract_metadata.executable_representation {
            records.push(record(RecordFamily::ExecutableDescriptor, 0, &self.name, descriptor)?);
        }
        let identities = own_public_identities(self);
        records.push(record(RecordFamily::PublicIdentities, 0, &self.name, &identities)?);
        // Native lowering reads this one by name, never the contract metadata it is derived from.
        records.push(record(
            RecordFamily::RuntimeBindings,
            0,
            &self.name,
            &runtime_bindings(self),
        )?);
        if let Some(rust_abi) = &self.rust_abi {
            records.push(record(RecordFamily::RustAbi, 0, &self.name, rust_abi)?);
        }
        let metadata = self.metadata_value(manifest_digest);
        let digests = digests(&records, &self.name, &metadata)?;
        let mut stored = metadata;
        stored.insert("record_content_digest".to_owned(), json!(digests.record_content));
        stored.insert("semantic_facts_digest".to_owned(), json!(digests.semantic_facts));
        records.insert(0, record(RecordFamily::Metadata, 0, &self.name, &stored)?);
        Ok(CompilerRecordSet { records, digests })
    }

    /// One record per exported declaration, made-private type, vocabulary and soft-keyword activation.
    fn declaration_records(&self) -> Result<Vec<CompilerRecord>, LibraryManifestError> {
        let exports = &self.exports;
        let mut records = Vec::new();
        named(&mut records, RecordFamily::Alias, &exports.aliases, |item| &item.name)?;
        named(&mut records, RecordFamily::Partial, &exports.partials, |item| {
            &item.name
        })?;
        named(&mut records, RecordFamily::Model, &exports.models, |item| &item.name)?;
        named(&mut records, RecordFamily::Class, &exports.classes, |item| &item.name)?;
        named(&mut records, RecordFamily::Function, &exports.functions, |item| {
            &item.name
        })?;
        named(&mut records, RecordFamily::Trait, &exports.traits, |item| &item.name)?;
        named(&mut records, RecordFamily::Enum, &exports.enums, |item| &item.name)?;
        named(&mut records, RecordFamily::TypeAlias, &exports.type_aliases, |item| {
            &item.name
        })?;
        named(&mut records, RecordFamily::Newtype, &exports.newtypes, |item| {
            &item.name
        })?;
        named(&mut records, RecordFamily::Const, &exports.consts, |item| &item.name)?;
        named(&mut records, RecordFamily::Static, &exports.statics, |item| &item.name)?;
        named(
            &mut records,
            RecordFamily::MadePrivateType,
            &self.made_private_types,
            |item| &item.name,
        )?;
        if let Some(vocab) = &self.vocab {
            records.push(CompilerRecord {
                family: RecordFamily::Vocabulary,
                index: 0,
                name: vocab.package_name.clone(),
                value: vocab.encode_compiler_record_value()?,
            });
        }
        named(
            &mut records,
            RecordFamily::SoftKeyword,
            &self.soft_keywords.activations,
            |item| &item.keyword,
        )?;
        Ok(records)
    }

    /// The Loaf's metadata record value, before its digests are added.
    fn metadata_value(&self, manifest_digest: &str) -> Map<String, Value> {
        let rust_paths = self
            .rust_abi
            .as_ref()
            .map(|abi| {
                abi.items
                    .iter()
                    .map(|item| item.canonical_path.as_str())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let value = json!({
            "name": self.name.as_str(),
            "version": self.version.as_str(),
            "incan_version": self.incan_version.as_str(),
            "manifest_format": self.manifest_format,
            "manifest_digest": manifest_digest,
            "vocabulary": self.vocab.is_some(),
            "provider_operations": !self.contract_metadata.provider.operation_descriptors.is_empty(),
            "native_unions": !self.contract_metadata.native_unions.is_empty(),
            "rust_paths": rust_paths,
            "rust_abi_cache_declared": self
                .rust_abi
                .as_ref()
                .and_then(|abi| abi.cache_digest.as_ref())
                .is_some(),
        });
        match value {
            Value::Object(fields) => fields,
            _ => Map::new(),
        }
    }
}

/// Add one record per value of a repeatable family, in declaration order.
fn named<T: Serialize>(
    records: &mut Vec<CompilerRecord>,
    family: RecordFamily,
    values: &[T],
    name: impl Fn(&T) -> &str,
) -> Result<(), LibraryManifestError> {
    for (index, value) in values.iter().enumerate() {
        records.push(record(family, index, name(value), value)?);
    }
    Ok(())
}

/// One record per declaration fact of the documentation-free `api`, numbered within its family in declaration order.
fn declaration_fact_records(
    api: &crate::api_metadata::CheckedApiMetadataPackage,
) -> Result<Vec<CompilerRecord>, LibraryManifestError> {
    let facts = declaration_facts(api).map_err(|error| LibraryManifestError::Serialize(error.to_string()))?;
    let mut ordinals = std::collections::BTreeMap::new();
    let mut records = Vec::with_capacity(facts.len());
    for fact in facts {
        let family = RecordFamily::of_declaration(fact.facet);
        let ordinal = ordinals.entry(family.as_str()).or_insert(0usize);
        records.push(record(family, *ordinal, &fact.name, &fact.value)?);
        *ordinal += 1;
    }
    Ok(records)
}

/// One record per public namespace of the documentation-free `api`, named by its path: its members, its child modules
/// and the identities of the public paths directly under it, and what its parent publishes under its name, so a
/// consumer resolves an import of it from that record.
fn namespace_records(
    api: &crate::api_metadata::CheckedApiMetadataPackage,
    graph: &super::LibraryIdentityGraph,
    package: &str,
) -> Result<Vec<CompilerRecord>, LibraryManifestError> {
    let mut records = Vec::with_capacity(api.public_namespaces.len());
    for (index, namespace) in api.public_namespaces.iter().enumerate() {
        let prefix = std::iter::once(package.to_owned())
            .chain(namespace.path.iter().cloned())
            .collect::<Vec<_>>();
        let identities = graph
            .exports
            .iter()
            .filter(|export| export.public_path.len() == prefix.len() + 1 && export.public_path.starts_with(&prefix))
            .collect::<Vec<_>>();
        let mut value = json!({
            "path": namespace.path,
            "members": namespace.members,
            "child_modules": namespace.child_modules,
            "identities": identities,
        });
        // What the parent namespace publishes under this namespace's name, so a consumer importing this namespace from
        // its parent reads this record alone, never the parent's, which carries every member of its child modules.
        if let Some((name, parent)) = namespace.path.split_last()
            && !parent.is_empty()
            && let Some(parent) = api
                .public_namespaces
                .iter()
                .find(|candidate| candidate.path.as_slice() == parent)
        {
            let named = parent
                .members
                .iter()
                .filter(|member| member.name == *name)
                .collect::<Vec<_>>();
            value["parent_members"] = json!(named);
        }
        records.push(record(
            RecordFamily::PublicNamespace,
            index,
            &namespace.path.join("::"),
            &value,
        )?);
    }
    Ok(records)
}

/// Encode one record value with the compiler-record codec.
fn record<T: Serialize>(
    family: RecordFamily,
    index: usize,
    name: &str,
    value: &T,
) -> Result<CompilerRecord, LibraryManifestError> {
    Ok(CompilerRecord {
        family,
        index,
        name: name.to_owned(),
        value: LibraryManifest::encode_compiler_record_value(value)?,
    })
}

/// The record-content and semantic facts digests of `records` and the Loaf's metadata value.
fn digests(
    records: &[CompilerRecord],
    name: &str,
    metadata: &Map<String, Value>,
) -> Result<FactDigests, LibraryManifestError> {
    let mut canonical_metadata = metadata.clone();
    for field in PUBLICATION_FIELDS {
        canonical_metadata.remove(field);
    }
    let metadata_record = record(RecordFamily::Metadata, 0, name, metadata)?;
    let metadata_canonical = LibraryManifest::encode_compiler_record_value(&canonical_metadata)?;
    let mut content = records
        .iter()
        .chain(std::iter::once(&metadata_record))
        .map(|record| {
            [
                record.key(),
                record.family.model_description_hash(),
                sha256_hex(&record.value),
            ]
        })
        .collect::<Vec<_>>();
    let mut semantic = Vec::new();
    for record in records
        .iter()
        .filter(|record| record.family != RecordFamily::Documentation)
    {
        semantic.push(semantic_entry(record, &canonical_value(record.family, &record.value)?));
    }
    semantic.push(semantic_entry(&metadata_record, &metadata_canonical));
    content.sort();
    semantic.sort();
    Ok(FactDigests {
        record_content: framed_digest(&content)?,
        semantic_facts: framed_digest(&semantic)?,
    })
}

/// The bytes a record's canonical hash covers: its value without source positions.
///
/// Spans locate a declaration in its producer's source, and a declaration's canonical identity carries its span; a
/// docstring that changes length moves every later one. What a declaration is stays in its identity's namespace,
/// origin and name, the parts `StableDeclarationId` keeps, and in the rest of its record.
///
/// The executable descriptor names the governed bodies by the digest of their `.incnbody` bytes, and their Body IR
/// carries spans and node identities derived from spans, so a longer docstring changes that digest while every body
/// stays what it was. The descriptor's representation version stays a compile fact; what the bodies execute is
/// identified by the content of the native library compiled from the same checked bodies, which a consumer's unit
/// identity folds beside these facts.
fn canonical_value(family: RecordFamily, value: &[u8]) -> Result<Vec<u8>, LibraryManifestError> {
    let mut decoded: Value = LibraryManifest::decode_compiler_record_value(value)?;
    without_positions(&mut decoded);
    let descriptor = match family {
        RecordFamily::ExecutableDescriptor => Some(&mut decoded),
        RecordFamily::ContractMetadata => decoded.get_mut("executable_representation"),
        _ => None,
    };
    if let Some(Value::Object(descriptor)) = descriptor {
        descriptor.remove(BODIES_DIGEST_FIELD);
    }
    LibraryManifest::encode_compiler_record_value(&decoded)
}

/// Remove every source position from a decoded record value: the `span` and `declaration_span` fields, and any field
/// whose value is itself a source span, such as a decorator's `anchor`.
fn without_positions(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            let positions = fields
                .iter()
                .filter(|(name, field)| POSITION_FIELDS.contains(&name.as_str()) || is_span(field))
                .map(|(name, _)| name.clone())
                .collect::<Vec<_>>();
            for name in positions {
                fields.remove(&name);
            }
            fields.values_mut().for_each(without_positions);
        }
        Value::Array(items) => items.iter_mut().for_each(without_positions),
        _ => {}
    }
}

/// Whether a decoded value is a source span: an object of exactly `start`, `end` and `expansion`.
fn is_span(value: &Value) -> bool {
    matches!(value, Value::Object(fields)
        if fields.len() == 3 && ["start", "end", "expansion"].iter().all(|key| fields.contains_key(*key)))
}

/// One record's semantic facts entry: key, kind, model description hash and canonical hash.
fn semantic_entry(record: &CompilerRecord, canonical: &[u8]) -> [String; 4] {
    [
        record.key(),
        record.family.as_str().to_owned(),
        record.family.model_description_hash(),
        sha256_hex(canonical),
    ]
}

/// The SHA-256 of the JSON framing of `entries`, so no two entry lists alias each other.
fn framed_digest<T: Serialize>(entries: &[T]) -> Result<String, LibraryManifestError> {
    let framed = serde_json::to_vec(entries).map_err(|error| LibraryManifestError::Serialize(error.to_string()))?;
    Ok(sha256_hex(&framed))
}

/// The lowercase hexadecimal SHA-256 of `bytes`.
fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests;
