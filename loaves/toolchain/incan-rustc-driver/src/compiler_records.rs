//! One-time conversion of installed Loaf manifests into validated compiler-record values (#2946).
//!
//! This command is an installation boundary, not a compilation fallback. It is the only records-mode path that
//! opens `.incnlib` artifacts: the candidate compiler validates the manifest with its owning semantic model, then
//! writes independently content-named values for Oven to admit through the sealed-set path.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use incan_frontend::library_manifest::LibraryManifest;
use incan_frontend::library_manifest::{FunctionExport, ParamKindExport, TypeRef};
use incan_semantics_core::stable_identity::StableDeclarationId;
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};

const EXPORT_SCHEMA: &str = "incan.compiler-record-export/3";
const LIBRARY_FACT_SCHEMA: &str = "incan.library-fact/1";
const FUNCTION_FACT_SCHEMA: &str = "incan.function-fact/1";

/// Closed families produced from one admitted Loaf manifest.
#[derive(Clone, Copy)]
enum ManifestRecordFamily {
    Library,
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
    RustAbi,
}

impl ManifestRecordFamily {
    /// Stable record-family spelling owned by the converter profile.
    fn as_str(self) -> &'static str {
        match self {
            Self::Library => "library",
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
            Self::RustAbi => "rust-abi",
        }
    }
}

/// Flat first-slice type graph accepted by the Incan-owned function-fact codec.
struct FunctionTypeGraph {
    tags: Vec<String>,
    names: Vec<String>,
    child_offsets: Vec<i64>,
    children: Vec<i64>,
}

impl FunctionTypeGraph {
    /// Start one canonical prefix-offset graph with no nodes or edges.
    fn new() -> Self {
        Self {
            tags: Vec::new(),
            names: Vec::new(),
            child_offsets: vec![0],
            children: Vec::new(),
        }
    }

    /// Add one supported manifest type recursively and return its node index.
    fn add(&mut self, ty: &TypeRef) -> Result<i64, Box<dyn std::error::Error>> {
        let (tag, name, nested): (&str, &str, Vec<&TypeRef>) = match ty {
            TypeRef::Named { name, origin: None } => ("named", name, Vec::new()),
            TypeRef::Applied {
                name,
                args,
                origin: None,
            } => ("applied", name, args.iter().collect()),
            TypeRef::Function { params, return_type } => {
                let mut nested = params.iter().collect::<Vec<_>>();
                nested.push(return_type);
                ("function", "", nested)
            }
            TypeRef::TypeToken { inner } => ("type-token", "", vec![inner]),
            TypeRef::Tuple { elements } => ("tuple", "", elements.iter().collect()),
            TypeRef::TypeParam { name } => ("type-param", name, Vec::new()),
            TypeRef::SelfType => ("self", "", Vec::new()),
            TypeRef::Ref { inner } => ("ref", "", vec![inner]),
            TypeRef::RustPath { path } => ("rust-path", path, Vec::new()),
            TypeRef::Unknown => ("unknown", "", Vec::new()),
            TypeRef::MutParam { inner } => ("mut-param", "", vec![inner]),
            TypeRef::Named { origin: Some(_), .. } | TypeRef::Applied { origin: Some(_), .. } => {
                return Err("function facts with foreign nominal origins require the next schema slice".into());
            }
            TypeRef::NativeUnion(_) | TypeRef::Ownership { .. } | TypeRef::Trait { .. } => {
                return Err("function fact contains a type outside the first schema slice".into());
            }
        };
        let mut child_indices = Vec::with_capacity(nested.len());
        for child in nested {
            child_indices.push(self.add(child)?);
        }
        let index = host_i64(self.tags.len(), "function type node")?;
        self.tags.push(tag.to_owned());
        self.names.push(name.to_owned());
        self.children.extend(child_indices);
        self.child_offsets
            .push(host_i64(self.children.len(), "function type edge")?);
        Ok(index)
    }
}

/// One value the compiler validated and Oven will place in the sealed set.
#[derive(Serialize)]
struct ExportedRecord {
    key: String,
    revision: String,
    object: String,
}

/// Convert every installed standard Loaf manifest below `loaves` into independently addressed record values.
pub(crate) fn export(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let [installed, output] = arguments else {
        return Err("usage: incan-rustc-driver --compiler-records INSTALLED OUTPUT".into());
    };
    let installed = Path::new(installed);
    let output = Path::new(output);
    if output.exists() {
        return Err(format!("compiler-record export already exists: {}", output.display()).into());
    }
    fs::create_dir_all(output.join("objects"))?;
    let mut manifests = manifest_paths(installed)?;
    manifests.sort();
    let mut records = Vec::new();
    for path in manifests {
        let manifest_digest = format!("{:x}", Sha256::digest(fs::read(&path)?));
        let manifest = LibraryManifest::read_from_path(&path)?;
        let profile = path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("manifest has no UTF-8 profile directory: {}", path.display()))?;
        export_manifest(output, profile, &manifest_digest, &manifest, &mut records)?;
    }
    records.sort_by(|left, right| left.key.cmp(&right.key));
    let index = json!({"schema": EXPORT_SCHEMA, "records": records});
    fs::write(output.join("index.json"), serde_json::to_vec(&index)?)?;
    Ok(())
}

/// Inventory ordinary manifest files without following directory symlinks or accepting another artifact suffix.
fn manifest_paths(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut pending = vec![root.to_path_buf()];
    let mut manifests = Vec::new();
    while let Some(directory) = pending.pop() {
        let mut entries = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() && entry.path().extension().is_some_and(|extension| extension == "incnlib") {
                manifests.push(entry.path());
            }
        }
    }
    Ok(manifests)
}

/// Split one validated manifest at the declaration and metadata-family boundaries consumers select.
fn export_manifest(
    output: &Path,
    profile: &str,
    manifest_digest: &str,
    manifest: &LibraryManifest,
    records: &mut Vec<ExportedRecord>,
) -> Result<(), Box<dyn std::error::Error>> {
    let prefix = format!("loaf/{profile}/{}", manifest.name);
    let rust_paths = manifest
        .rust_abi
        .as_ref()
        .map(|abi| {
            abi.items
                .iter()
                .map(|item| item.canonical_path.as_str())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let library_fact = internal_models::LibraryFact {
        schema: LIBRARY_FACT_SCHEMA.to_owned(),
        kind: "library".to_owned(),
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        incan_version: manifest.incan_version.clone(),
        manifest_format: i64::from(manifest.manifest_format),
        manifest_digest: manifest_digest.to_owned(),
        vocabulary: manifest.vocab.is_some(),
        provider_operations: !manifest.contract_metadata.provider.operation_descriptors.is_empty(),
        native_unions: !manifest.contract_metadata.native_unions.is_empty(),
        rust_paths: rust_paths.into_iter().map(str::to_owned).collect(),
        rust_abi_declared: manifest.rust_abi.is_some(),
    };
    add_encoded_value(
        output,
        &prefix,
        ManifestRecordFamily::Library,
        0,
        &manifest.name,
        internal_models::encode_library_fact(library_fact)
            .map_err(|error| format!("library fact encoding refused: {error}"))?,
        records,
    )?;
    add_named_values(
        output,
        &prefix,
        ManifestRecordFamily::Alias,
        &manifest.exports.aliases,
        |item| &item.name,
        records,
    )?;
    add_named_values(
        output,
        &prefix,
        ManifestRecordFamily::Partial,
        &manifest.exports.partials,
        |item| &item.name,
        records,
    )?;
    add_named_values(
        output,
        &prefix,
        ManifestRecordFamily::Model,
        &manifest.exports.models,
        |item| &item.name,
        records,
    )?;
    add_named_values(
        output,
        &prefix,
        ManifestRecordFamily::Class,
        &manifest.exports.classes,
        |item| &item.name,
        records,
    )?;
    for (index, function) in manifest.exports.functions.iter().enumerate() {
        add_function_fact(output, &prefix, index, manifest, function, records)?;
    }
    add_named_values(
        output,
        &prefix,
        ManifestRecordFamily::Trait,
        &manifest.exports.traits,
        |item| &item.name,
        records,
    )?;
    add_named_values(
        output,
        &prefix,
        ManifestRecordFamily::Enum,
        &manifest.exports.enums,
        |item| &item.name,
        records,
    )?;
    add_named_values(
        output,
        &prefix,
        ManifestRecordFamily::TypeAlias,
        &manifest.exports.type_aliases,
        |item| &item.name,
        records,
    )?;
    add_named_values(
        output,
        &prefix,
        ManifestRecordFamily::Newtype,
        &manifest.exports.newtypes,
        |item| &item.name,
        records,
    )?;
    add_named_values(
        output,
        &prefix,
        ManifestRecordFamily::Const,
        &manifest.exports.consts,
        |item| &item.name,
        records,
    )?;
    add_named_values(
        output,
        &prefix,
        ManifestRecordFamily::Static,
        &manifest.exports.statics,
        |item| &item.name,
        records,
    )?;
    for (index, item) in manifest.made_private_types.iter().enumerate() {
        add_value(
            output,
            &prefix,
            ManifestRecordFamily::MadePrivateType,
            index,
            &item.name,
            item,
            records,
        )?;
    }
    if let Some(vocab) = &manifest.vocab {
        add_encoded_value(
            output,
            &prefix,
            ManifestRecordFamily::Vocabulary,
            0,
            &vocab.package_name,
            vocab.encode_compiler_record_value()?,
            records,
        )?;
    }
    for (index, item) in manifest.soft_keywords.activations.iter().enumerate() {
        add_value(
            output,
            &prefix,
            ManifestRecordFamily::SoftKeyword,
            index,
            &item.keyword,
            item,
            records,
        )?;
    }
    add_value(
        output,
        &prefix,
        ManifestRecordFamily::ContractMetadata,
        0,
        &manifest.name,
        &manifest.contract_metadata,
        records,
    )?;
    if let Some(rust_abi) = &manifest.rust_abi {
        add_value(
            output,
            &prefix,
            ManifestRecordFamily::RustAbi,
            0,
            &manifest.name,
            rust_abi,
            records,
        )?;
    }
    Ok(())
}

/// Convert one supported function export into the Incan-owned fact model and address it by stable declaration id.
fn add_function_fact(
    output: &Path,
    prefix: &str,
    index: usize,
    manifest: &LibraryManifest,
    function: &FunctionExport,
    records: &mut Vec<ExportedRecord>,
) -> Result<(), Box<dyn std::error::Error>> {
    if function.ownership.is_some() || function.params.iter().any(|parameter| parameter.default.is_some()) {
        return Err(format!(
            "function `{}` uses ownership or default facts outside the first function-fact slice",
            function.name
        )
        .into());
    }
    if function
        .type_params
        .iter()
        .flat_map(|parameter| &parameter.bounds)
        .any(|bound| !bound.implementation_type_params.is_empty())
    {
        return Err(format!(
            "function `{}` uses implementation bounds outside the first function-fact slice",
            function.name
        )
        .into());
    }
    let stable_identity = stable_function_identity(manifest, function)?;
    let mut graph = FunctionTypeGraph::new();
    let mut type_parameter_names = Vec::new();
    let mut type_parameter_bound_offsets = vec![0];
    let mut bound_names = Vec::new();
    let mut bound_source_names = Vec::new();
    let mut bound_module_paths = Vec::new();
    let mut bound_type_argument_offsets = vec![0];
    let mut bound_type_argument_roots = Vec::new();
    let mut bound_inferred = Vec::new();
    for parameter in &function.type_params {
        type_parameter_names.push(parameter.name.clone());
        for bound in &parameter.bounds {
            bound_names.push(bound.name.clone());
            bound_source_names.push(bound.source_name.clone());
            bound_module_paths.push(bound.module_path.clone().unwrap_or_default());
            for argument in &bound.type_args {
                bound_type_argument_roots.push(graph.add(argument)?);
            }
            bound_type_argument_offsets.push(host_i64(
                bound_type_argument_roots.len(),
                "function bound type argument",
            )?);
            bound_inferred.push(bound.inferred);
        }
        type_parameter_bound_offsets.push(host_i64(bound_names.len(), "function type-parameter bound")?);
    }
    let mut parameter_type_roots = Vec::new();
    for parameter in &function.params {
        parameter_type_roots.push(graph.add(&parameter.ty)?);
    }
    let return_type_root = graph.add(&function.return_type)?;
    let fact = internal_models::FunctionFact {
        schema: FUNCTION_FACT_SCHEMA.to_owned(),
        kind: "function".to_owned(),
        stable_identity: stable_identity.clone(),
        name: function.name.clone(),
        emitted_name: function.emitted_name.clone(),
        type_parameter_names,
        type_parameter_bound_offsets,
        bound_names,
        bound_source_names,
        bound_module_paths,
        bound_type_argument_offsets,
        bound_type_argument_roots,
        bound_inferred,
        parameter_names: function.params.iter().map(|parameter| parameter.name.clone()).collect(),
        parameter_type_roots,
        parameter_surface_types: function
            .params
            .iter()
            .map(|parameter| parameter.surface_type_name.clone())
            .collect(),
        parameter_kinds: function
            .params
            .iter()
            .map(|parameter| parameter_kind(parameter.kind))
            .collect(),
        parameter_has_default: function.params.iter().map(|parameter| parameter.has_default).collect(),
        parameter_is_mut: function.params.iter().map(|parameter| parameter.is_mut).collect(),
        type_tags: graph.tags,
        type_names: graph.names,
        type_child_offsets: graph.child_offsets,
        type_children: graph.children,
        return_type_root,
        is_async: function.is_async,
    };
    let bytes = internal_models::encode_function_fact(fact)
        .map_err(|error| format!("function fact encoding refused for `{}`: {error}", function.name))?;
    add_encoded_identity_value(
        output,
        prefix,
        ManifestRecordFamily::Function,
        index,
        &stable_identity,
        bytes,
        records,
    )
}

/// Resolve one non-overloaded public function to its span-free RFC 106 identity.
fn stable_function_identity(
    manifest: &LibraryManifest,
    function: &FunctionExport,
) -> Result<String, Box<dyn std::error::Error>> {
    if manifest
        .exports
        .functions
        .iter()
        .filter(|candidate| candidate.name == function.name)
        .count()
        != 1
    {
        return Err(format!(
            "function `{}` belongs to an overload set that requires checked signature identities",
            function.name
        )
        .into());
    }
    let mut identities = manifest
        .contract_metadata
        .identity_graph
        .function_identities_for_public_name(&function.name)
        .into_iter()
        .map(|canonical| {
            let canonical = canonical.ok_or("function identity graph contains an unhydrated identity")?;
            StableDeclarationId::from_canonical(&canonical, None, None)
                .map(|identity| identity.render_compact())
                .ok_or("function identity cannot be made edit-stable")
        })
        .collect::<Result<Vec<_>, _>>()?;
    identities.sort();
    identities.dedup();
    if identities.len() != 1 {
        return Err(format!(
            "function `{}` requires exactly one stable public identity, found {}",
            function.name,
            identities.len()
        )
        .into());
    }
    identities
        .pop()
        .ok_or_else(|| format!("function `{}` has no stable public identity", function.name).into())
}

/// Render the closed parameter-kind vocabulary stored in function facts.
fn parameter_kind(kind: ParamKindExport) -> String {
    match kind {
        ParamKindExport::Normal => "normal",
        ParamKindExport::RestPositional => "rest_positional",
        ParamKindExport::RestKeyword => "rest_keyword",
    }
    .to_owned()
}

/// Convert one bounded host collection length into the Incan `int` carrier without truncation.
fn host_i64(value: usize, label: &str) -> Result<i64, Box<dyn std::error::Error>> {
    i64::try_from(value).map_err(|error| format!("{label} count exceeds i64: {error}").into())
}

/// Add every declaration of one family without combining unrelated declaration payloads.
fn add_named_values<T: Serialize>(
    output: &Path,
    prefix: &str,
    family: ManifestRecordFamily,
    values: &[T],
    name: impl Fn(&T) -> &str,
    records: &mut Vec<ExportedRecord>,
) -> Result<(), Box<dyn std::error::Error>> {
    for (index, value) in values.iter().enumerate() {
        add_value(output, prefix, family, index, name(value), value, records)?;
    }
    Ok(())
}

/// Encode one semantic value canonically enough for content addressing and retain its logical record identity.
fn add_value<T: Serialize>(
    output: &Path,
    prefix: &str,
    family: ManifestRecordFamily,
    index: usize,
    name: &str,
    value: &T,
    records: &mut Vec<ExportedRecord>,
) -> Result<(), Box<dyn std::error::Error>> {
    add_encoded_value(
        output,
        prefix,
        family,
        index,
        name,
        LibraryManifest::encode_compiler_record_value(value)?,
        records,
    )
}

/// Address and write one already encoded family value without combining it with sibling records.
fn add_encoded_value(
    output: &Path,
    prefix: &str,
    family: ManifestRecordFamily,
    index: usize,
    name: &str,
    bytes: Vec<u8>,
    records: &mut Vec<ExportedRecord>,
) -> Result<(), Box<dyn std::error::Error>> {
    add_encoded_identity_value(output, prefix, family, index, name, bytes, records)
}

/// Address and write one encoded value using the digest of its logical declaration identity.
fn add_encoded_identity_value(
    output: &Path,
    prefix: &str,
    family: ManifestRecordFamily,
    index: usize,
    identity: &str,
    bytes: Vec<u8>,
    records: &mut Vec<ExportedRecord>,
) -> Result<(), Box<dyn std::error::Error>> {
    let revision = format!("{:x}", Sha256::digest(&bytes));
    let object = format!("objects/{revision}");
    let path = output.join(&object);
    if path.exists() {
        if fs::read(&path)? != bytes {
            return Err(format!("compiler-record object collision at {}", path.display()).into());
        }
    } else {
        fs::write(&path, &bytes)?;
    }
    let name_digest = format!("{:x}", Sha256::digest(identity.as_bytes()));
    records.push(ExportedRecord {
        key: format!("{prefix}/{}/{index:08}/{name_digest}", family.as_str()),
        revision: revision.clone(),
        object,
    });
    Ok(())
}
