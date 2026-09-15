//! Checked public-capability descriptor decoding.
//!
//! This is the compiler-owned bridge from RFC 113 structural registry metadata to consumers that need the stable
//! public capability contract. It deliberately accepts only [`CheckedRegistryMetadataPackage`] values produced by
//! type checking; callers must not reparse authored source, scrape generated documentation, or inspect runtime
//! registry state.

use std::collections::BTreeMap;

use crate::registry_metadata::{CheckedRegistryEntry, CheckedRegistryMetadataPackage, CheckedRegistryValue};

/// One public `std.features` descriptor decoded from checked registry metadata.
///
/// The fields mirror the Incan-authored `FeatureDescriptor` contract. This Rust value is a read-only projection:
/// it must not become a second authority for public capability descriptions or transient backend status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicFeatureDescriptor {
    /// Stable Incan-authored `FeatureId` string.
    pub id: String,
    /// Human-readable public capability name.
    pub name: String,
    /// Public inventory category selected by the standard library.
    pub category: String,
    /// First release that publicly advertised this capability.
    pub since: String,
    /// Linked RFC identifier retained by the public descriptor.
    pub rfc: String,
    /// Public stability classification retained by the public descriptor.
    pub stability: String,
    /// User-facing activation contract.
    pub activation: String,
    /// User-facing capability summary.
    pub summary: String,
    /// Canonical source forms supplied by the public descriptor.
    pub canonical_forms: Vec<String>,
    /// Preferred public surface over older or less direct alternatives.
    pub prefer_over: String,
    /// Public documentation references owned by the descriptor.
    pub references: Vec<(String, String)>,
}

/// Return whether a registry identity names the standard-library feature catalogue, under either spelling.
///
/// The catalogue was renamed from `capabilities` to `features` so that "capability" means one thing (see #1228). A
/// registry identity is `module::static`, so that rename is *observable in checked metadata* -- and the frozen v0.5.0
/// migration baseline legitimately still carries the old spelling, because a baseline records what that release
/// actually was. Both are accepted here rather than rewriting the baseline, which would falsify it.
fn is_feature_registry_identity(identity: &str) -> bool {
    identity == "features::features" || identity == "capabilities::capabilities"
}

/// Decode every descriptor in the checked `std.features` registry.
///
/// The checked package may include other modules and registries. Only the canonical
/// `features::features` registry is selected, preserving the checked entry order so existing public
/// projections do not acquire an unrelated presentation order.
pub fn public_feature_descriptors(
    package: &CheckedRegistryMetadataPackage,
) -> Result<Vec<PublicFeatureDescriptor>, String> {
    let Some(module) = package.modules.iter().find(|module| {
        module
            .registries
            .iter()
            .any(|registry| is_feature_registry_identity(&registry.identity))
    }) else {
        return Err("checked std.features registry was not found".to_string());
    };
    let entries = module
        .entries
        .iter()
        .filter(|entry| is_feature_registry_identity(&entry.registry_identity))
        .map(public_capability_descriptor)
        .collect::<Result<Vec<_>, _>>()?;
    if entries.is_empty() {
        return Err("checked std.features inventory must contain at least one entry".to_string());
    }
    Ok(entries)
}

/// Decode one checked `FeatureDescriptor` without accepting a runtime-shaped substitute.
fn public_capability_descriptor(entry: &CheckedRegistryEntry) -> Result<PublicFeatureDescriptor, String> {
    let fields = checked_model_fields_either(&entry.descriptor, "FeatureDescriptor", "CapabilityDescriptor")?;
    let id = checked_newtype_string_either(checked_required_field(&fields, "id")?, "FeatureId", "CapabilityId")?;
    let name = checked_string(checked_required_field(&fields, "name")?)?;
    let category = checked_enum_variant_either(
        checked_required_field(&fields, "category")?,
        "FeatureCategory",
        "CapabilityCategory",
    )?;
    let since = checked_string(checked_required_field(&fields, "since")?)?;
    let rfc = checked_string(checked_required_field(&fields, "rfc")?)?;
    let stability = checked_enum_variant_either(
        checked_required_field(&fields, "stability")?,
        "FeatureStability",
        "CapabilityStability",
    )?;
    let activation = checked_string(checked_required_field(&fields, "activation")?)?;
    let summary = checked_string(checked_required_field(&fields, "summary")?)?;
    let canonical_forms = checked_list(checked_required_field(&fields, "canonical_forms")?)?
        .iter()
        .map(checked_string)
        .collect::<Result<Vec<_>, _>>()?;
    let prefer_over = checked_string(checked_required_field(&fields, "prefer_over")?)?;
    let references = checked_list(checked_required_field(&fields, "references")?)?
        .iter()
        .map(|reference| {
            let fields = checked_model_fields(reference, "CapabilityReference")?;
            Ok((
                checked_string(checked_required_field(&fields, "label")?)?,
                checked_string(checked_required_field(&fields, "path")?)?,
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(PublicFeatureDescriptor {
        id,
        name,
        category,
        since,
        rfc,
        stability,
        activation,
        summary,
        canonical_forms,
        prefer_over,
        references,
    })
}

/// Return the named fields of a checked model after verifying the descriptor type.
/// Decode a checked model's fields, accepting either the current or the pre-#1228 model name.
///
/// See [`is_feature_registry_identity`]: the frozen v0.5.0 baseline carries the old spelling and must stay readable.
fn checked_model_fields_either(
    value: &CheckedRegistryValue,
    expected_name: &str,
    legacy_name: &str,
) -> Result<BTreeMap<String, CheckedRegistryValue>, String> {
    checked_model_fields(value, expected_name).or_else(|_| checked_model_fields(value, legacy_name))
}

/// Decode a checked enum variant, accepting either the current or the pre-#1228 enum name.
fn checked_enum_variant_either(
    value: &CheckedRegistryValue,
    expected_name: &str,
    legacy_name: &str,
) -> Result<String, String> {
    checked_enum_variant(value, expected_name).or_else(|_| checked_enum_variant(value, legacy_name))
}

/// Decode a checked newtype string, accepting either the current or the pre-#1228 newtype name.
fn checked_newtype_string_either(
    value: &CheckedRegistryValue,
    expected_name: &str,
    legacy_name: &str,
) -> Result<String, String> {
    checked_newtype_string(value, expected_name).or_else(|_| checked_newtype_string(value, legacy_name))
}

fn checked_model_fields(
    value: &CheckedRegistryValue,
    expected_name: &str,
) -> Result<BTreeMap<String, CheckedRegistryValue>, String> {
    let CheckedRegistryValue::Model { name, fields } = value else {
        return Err(format!("expected {expected_name} descriptor model"));
    };
    if name != expected_name {
        return Err(format!("expected {expected_name} descriptor model, found {name}"));
    }
    Ok(fields
        .iter()
        .map(|field| (field.name.clone(), field.value.clone()))
        .collect())
}

/// Return one required checked descriptor field.
fn checked_required_field<'a>(
    fields: &'a BTreeMap<String, CheckedRegistryValue>,
    name: &str,
) -> Result<&'a CheckedRegistryValue, String> {
    fields
        .get(name)
        .ok_or_else(|| format!("FeatureDescriptor is missing `{name}`"))
}

/// Extract an exact checked string value.
fn checked_string(value: &CheckedRegistryValue) -> Result<String, String> {
    match value {
        CheckedRegistryValue::String(value) => Ok(value.clone()),
        _ => Err("expected checked string descriptor value".to_string()),
    }
}

/// Extract the string payload of one checked newtype with the expected domain identity.
fn checked_newtype_string(value: &CheckedRegistryValue, expected_name: &str) -> Result<String, String> {
    let CheckedRegistryValue::Newtype { name, value } = value else {
        return Err(format!("expected {expected_name} newtype value"));
    };
    if name != expected_name {
        return Err(format!("expected {expected_name} newtype value, found {name}"));
    }
    checked_string(value)
}

/// Extract an exact checked enum variant from the expected enum.
fn checked_enum_variant(value: &CheckedRegistryValue, expected_enum: &str) -> Result<String, String> {
    let CheckedRegistryValue::ConstRef(path) = value else {
        return Err(format!("expected {expected_enum} enum value"));
    };
    if path.first().map(String::as_str) != Some(expected_enum) {
        return Err(format!("expected {expected_enum} enum value"));
    }
    path.last()
        .cloned()
        .ok_or_else(|| format!("expected {expected_enum} enum variant"))
}

/// Borrow a checked structural list without coercing any other value shape.
fn checked_list(value: &CheckedRegistryValue) -> Result<&[CheckedRegistryValue], String> {
    match value {
        CheckedRegistryValue::List(values) => Ok(values),
        _ => Err("expected checked descriptor list value".to_string()),
    }
}
