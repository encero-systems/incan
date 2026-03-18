//! Validation policy for raw `.incnlib` payloads.
//!
//! This module stays on the transport-facing side of the manifest boundary: it validates decoded `RawLibraryManifest`
//! values before the rest of the compiler treats them as trustworthy semantic data. The checks here intentionally fail
//! early on producer mistakes such as unsupported manifest versions, malformed vocab artifacts, invalid soft-keyword
//! activations, or helper bindings that drift from the exported library surface.

use std::collections::HashSet;
use std::path::{Component, Path};

use semver::Version;

use super::wire::{RawLibraryExports, RawLibraryManifest};
use super::{LIBRARY_MANIFEST_FORMAT, LibraryManifestError, VocabProviderManifest};

/// Validate one raw manifest payload before it is written or decoded into the semantic model.
pub(super) fn validate_raw_manifest(raw: &RawLibraryManifest) -> Result<(), LibraryManifestError> {
    validate_manifest_version(raw)?;
    validate_vocab_payload(raw)?;
    validate_soft_keyword_activations(raw)?;
    Ok(())
}

/// Validate top-level manifest format and compiler-version compatibility.
///
/// This is the first gate because downstream validation rules only make sense once the compiler knows it understands
/// the payload shape and that the manifest does not require a newer Incan version than the current binary provides.
fn validate_manifest_version(raw: &RawLibraryManifest) -> Result<(), LibraryManifestError> {
    if raw.manifest_format != LIBRARY_MANIFEST_FORMAT {
        return Err(LibraryManifestError::Invalid(format!(
            "unsupported manifest_format {} (expected {})",
            raw.manifest_format, LIBRARY_MANIFEST_FORMAT
        )));
    }

    let manifest_version = Version::parse(&raw.incan_version).map_err(|err| {
        LibraryManifestError::Invalid(format!("invalid `incan_version` value `{}`: {err}", raw.incan_version))
    })?;
    let compiler_version = Version::parse(crate::version::INCAN_VERSION).map_err(|err| {
        LibraryManifestError::Invalid(format!(
            "invalid compiler version `{}`: {err}",
            crate::version::INCAN_VERSION
        ))
    })?;

    if manifest_version > compiler_version {
        return Err(LibraryManifestError::Invalid(format!(
            "manifest requires Incan {} but compiler is {}",
            manifest_version, compiler_version
        )));
    }

    Ok(())
}

/// Validate the optional vocab payload and its desugarer artifact metadata.
///
/// This keeps producer-facing vocab metadata internally consistent before the compiler tries to load any companion
/// artifact or resolve helper references against exported symbols.
fn validate_vocab_payload(raw: &RawLibraryManifest) -> Result<(), LibraryManifestError> {
    let Some(vocab) = &raw.vocab else {
        return Ok(());
    };

    if vocab.crate_path.trim().is_empty() {
        return Err(LibraryManifestError::Invalid(
            "vocab crate_path cannot be empty".to_string(),
        ));
    }
    if vocab.package_name.trim().is_empty() {
        return Err(LibraryManifestError::Invalid(
            "vocab package_name cannot be empty".to_string(),
        ));
    }

    validate_helper_bindings(&raw.exports, &vocab.provider_manifest)?;

    let Some(desugarer) = &vocab.desugarer_artifact else {
        return Ok(());
    };

    if desugarer.abi_version == 0 {
        return Err(LibraryManifestError::Invalid(
            "vocab desugarer_artifact.abi_version must be >= 1".to_string(),
        ));
    }
    if desugarer.abi_version > incan_vocab::WASM_DESUGAR_ABI_VERSION {
        return Err(LibraryManifestError::Invalid(format!(
            "vocab desugarer_artifact.abi_version {} is newer than compiler-supported version {}",
            desugarer.abi_version,
            incan_vocab::WASM_DESUGAR_ABI_VERSION
        )));
    }
    if desugarer.relative_path.trim().is_empty() {
        return Err(LibraryManifestError::Invalid(
            "vocab desugarer_artifact.relative_path cannot be empty".to_string(),
        ));
    }
    validate_relative_artifact_path(&desugarer.relative_path)?;
    if desugarer.target.trim().is_empty() {
        return Err(LibraryManifestError::Invalid(
            "vocab desugarer_artifact.target cannot be empty".to_string(),
        ));
    }
    if desugarer.profile.trim().is_empty() {
        return Err(LibraryManifestError::Invalid(
            "vocab desugarer_artifact.profile cannot be empty".to_string(),
        ));
    }
    if desugarer.entrypoint.trim().is_empty() {
        return Err(LibraryManifestError::Invalid(
            "vocab desugarer_artifact.entrypoint cannot be empty".to_string(),
        ));
    }
    if desugarer.sha256.trim().is_empty() {
        return Err(LibraryManifestError::Invalid(
            "vocab desugarer_artifact.sha256 cannot be empty".to_string(),
        ));
    }
    validate_sha256_hex(&desugarer.sha256)
}

/// Validate soft-keyword activation declarations exported by the library.
///
/// Each activation must name a known soft keyword and a non-empty namespace so import-time keyword activation remains
/// deterministic and cannot accidentally claim hard keywords.
fn validate_soft_keyword_activations(raw: &RawLibraryManifest) -> Result<(), LibraryManifestError> {
    for activation in &raw.soft_keywords.activations {
        if activation.keyword.trim().is_empty() {
            return Err(LibraryManifestError::Invalid(
                "soft keyword activation keyword cannot be empty".to_string(),
            ));
        }
        if activation.namespace.trim().is_empty() {
            return Err(LibraryManifestError::Invalid(
                "soft keyword activation namespace cannot be empty".to_string(),
            ));
        }
        if let Some(id) = incan_core::lang::keywords::from_str(&activation.keyword) {
            if !incan_core::lang::keywords::is_soft(id) {
                return Err(LibraryManifestError::Invalid(format!(
                    "keyword `{}` is not a soft keyword",
                    activation.keyword
                )));
            }
        } else {
            return Err(LibraryManifestError::Invalid(format!(
                "unknown soft keyword `{}`",
                activation.keyword
            )));
        }
    }

    Ok(())
}

/// Reject non-normalized desugarer artifact paths before they reach filesystem resolution.
///
/// Producer manifests must store a clean relative path so both producer-side validation and consumer-side artifact
/// loading apply the same traversal and normalization rules.
fn validate_relative_artifact_path(relative_path: &str) -> Result<(), LibraryManifestError> {
    let path = Path::new(relative_path);
    if path.is_absolute() {
        return Err(LibraryManifestError::Invalid(format!(
            "vocab desugarer_artifact.relative_path `{relative_path}` must be relative"
        )));
    }
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::CurDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(LibraryManifestError::Invalid(format!(
            "vocab desugarer_artifact.relative_path `{relative_path}` must be a normalized relative path"
        )));
    }
    Ok(())
}

/// Validate that a manifest-provided SHA-256 digest is a full hexadecimal string.
///
/// The compiler uses this value as an integrity check for packaged desugarer artifacts, so partial or malformed digests
/// are rejected up front instead of weakening the trust boundary.
fn validate_sha256_hex(sha256: &str) -> Result<(), LibraryManifestError> {
    if sha256.len() != 64 {
        return Err(LibraryManifestError::Invalid(format!(
            "vocab desugarer_artifact.sha256 must be 64 hex characters, got length {}",
            sha256.len()
        )));
    }
    if !sha256.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(LibraryManifestError::Invalid(
            "vocab desugarer_artifact.sha256 must contain only hex characters".to_string(),
        ));
    }
    Ok(())
}

/// Validate symbolic helper bindings exposed by a vocab provider manifest.
///
/// A helper binding is only valid when:
/// - the symbolic key is non-empty,
/// - the referenced exported symbol name is non-empty,
/// - the symbolic key is unique within the provider manifest, and
/// - the referenced export actually exists in the library's published surface.
fn validate_helper_bindings(
    exports: &RawLibraryExports,
    provider_manifest: &VocabProviderManifest,
) -> Result<(), LibraryManifestError> {
    let export_names = library_export_names(exports);
    let mut seen_keys = HashSet::new();

    for binding in &provider_manifest.helper_bindings {
        if binding.key.trim().is_empty() {
            return Err(LibraryManifestError::Invalid(
                "vocab provider_manifest.helper_bindings key cannot be empty".to_string(),
            ));
        }
        if binding.exported_name.trim().is_empty() {
            return Err(LibraryManifestError::Invalid(format!(
                "vocab helper binding `{}` must declare a non-empty exported_name",
                binding.key
            )));
        }
        if !seen_keys.insert(binding.key.as_str()) {
            return Err(LibraryManifestError::Invalid(format!(
                "vocab provider_manifest.helper_bindings contains duplicate key `{}`",
                binding.key
            )));
        }
        if !export_names.contains(binding.exported_name.as_str()) {
            return Err(LibraryManifestError::Invalid(format!(
                "vocab helper binding `{}` points to unknown exported symbol `{}`",
                binding.key, binding.exported_name
            )));
        }
    }

    Ok(())
}

/// Collect the set of exportable names that helper bindings are allowed to target.
///
/// This flattens the public surface into a simple membership check so helper binding validation can reject drift
/// without re-encoding export-shape logic in multiple places.
fn library_export_names(exports: &RawLibraryExports) -> HashSet<&str> {
    let mut names = HashSet::new();
    names.extend(exports.models.iter().map(|item| item.name.as_str()));
    names.extend(exports.classes.iter().map(|item| item.name.as_str()));
    names.extend(exports.functions.iter().map(|item| item.name.as_str()));
    names.extend(exports.traits.iter().map(|item| item.name.as_str()));
    names.extend(exports.enums.iter().map(|item| item.name.as_str()));
    names.extend(
        exports
            .enums
            .iter()
            .flat_map(|item| item.variants.iter().map(|variant| variant.name.as_str())),
    );
    names.extend(exports.type_aliases.iter().map(|item| item.name.as_str()));
    names.extend(exports.newtypes.iter().map(|item| item.name.as_str()));
    names.extend(exports.consts.iter().map(|item| item.name.as_str()));
    names
}
