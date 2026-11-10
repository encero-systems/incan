//! One-time conversion of installed Loaf manifests into validated compiler-record values (#2946).
//!
//! This command is an installation boundary, not a compilation fallback. It is the only records-mode path that
//! opens `.incnlib` artifacts: the candidate compiler validates the manifest with its owning semantic model, then
//! writes independently content-named values for Oven to admit through the sealed-set path.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use incan_frontend::library_manifest::LibraryManifest;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const EXPORT_SCHEMA: &str = "incan.compiler-record-export/1";

/// Closed families produced from one admitted Loaf manifest.
#[derive(Clone, Copy)]
enum ManifestRecordFamily {
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
    RustAbi,
}

impl ManifestRecordFamily {
    /// Stable record-family spelling owned by the converter profile.
    fn as_str(self) -> &'static str {
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
            Self::RustAbi => "rust-abi",
        }
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
    add_value(
        output,
        &prefix,
        ManifestRecordFamily::Metadata,
        0,
        &manifest.name,
        &json!({
            "name": manifest.name.as_str(),
            "version": manifest.version.as_str(),
            "incan_version": manifest.incan_version.as_str(),
            "manifest_format": manifest.manifest_format,
            "manifest_digest": manifest_digest,
            "vocabulary": manifest.vocab.is_some(),
            "provider_operations": !manifest.contract_metadata.provider.operation_descriptors.is_empty(),
            "native_unions": !manifest.contract_metadata.native_unions.is_empty(),
            "rust_paths": rust_paths,
            "rust_abi_cache_declared": manifest
                .rust_abi
                .as_ref()
                .and_then(|abi| abi.cache_digest.as_ref())
                .is_some(),
        }),
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
    add_named_values(
        output,
        &prefix,
        ManifestRecordFamily::Function,
        &manifest.exports.functions,
        |item| &item.name,
        records,
    )?;
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
        let value: Value = serde_json::from_str(&vocab.to_json_string()?)?;
        add_value(
            output,
            &prefix,
            ManifestRecordFamily::Vocabulary,
            0,
            &vocab.package_name,
            &value,
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
    let bytes = serde_json::to_vec(value)?;
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
    let name_digest = format!("{:x}", Sha256::digest(name.as_bytes()));
    records.push(ExportedRecord {
        key: format!("{prefix}/{}/{index:08}/{name_digest}", family.as_str()),
        revision: revision.clone(),
        object,
    });
    Ok(())
}
