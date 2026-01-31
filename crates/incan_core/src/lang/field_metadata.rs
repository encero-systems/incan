//! Field metadata key registry.
//!
//! Centralizes the allowed keys for model/class field metadata to avoid stringly-typed
//! checks spread across the compiler and tooling.

/// Stable identifier for field metadata keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FieldMetadataKey {
    Alias,
    Description,
}

/// Ordered list of supported field metadata keys.
pub const FIELD_METADATA_KEYS: &[FieldMetadataKey] = &[FieldMetadataKey::Alias, FieldMetadataKey::Description];

/// Parse a field metadata key from its canonical string form.
pub fn from_str(s: &str) -> Option<FieldMetadataKey> {
    match s {
        "alias" => Some(FieldMetadataKey::Alias),
        "description" => Some(FieldMetadataKey::Description),
        _ => None,
    }
}

/// Return the canonical spelling for a field metadata key.
pub fn as_str(key: FieldMetadataKey) -> &'static str {
    match key {
        FieldMetadataKey::Alias => "alias",
        FieldMetadataKey::Description => "description",
    }
}
