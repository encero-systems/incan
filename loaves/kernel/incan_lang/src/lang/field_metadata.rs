//! Field metadata key registry.
//!
//! Centralizes the allowed keys for model/class field metadata to avoid stringly-typed checks spread across the
//! compiler and tooling.

/// Stable identifier for field metadata keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FieldMetadataKey {
    Alias,
    Description,
    RenamedFrom,
    Canonical,
    MadePrivate,
}

/// Ordered list of supported field metadata keys.
pub const FIELD_METADATA_KEYS: &[FieldMetadataKey] = &[
    FieldMetadataKey::Alias,
    FieldMetadataKey::Description,
    FieldMetadataKey::RenamedFrom,
    FieldMetadataKey::Canonical,
    FieldMetadataKey::MadePrivate,
];

/// Safe value shape accepted by one compiler-known field metadata key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldMetadataValueKind {
    String,
    StringOrStringList,
    Boolean,
    BooleanStringOrLifecycle,
}

/// Return the safe value shape accepted by a compiler-known field metadata key.
pub fn value_kind(key: FieldMetadataKey) -> FieldMetadataValueKind {
    match key {
        FieldMetadataKey::Alias | FieldMetadataKey::Description => FieldMetadataValueKind::String,
        FieldMetadataKey::RenamedFrom => FieldMetadataValueKind::StringOrStringList,
        FieldMetadataKey::Canonical => FieldMetadataValueKind::Boolean,
        FieldMetadataKey::MadePrivate => FieldMetadataValueKind::BooleanStringOrLifecycle,
    }
}

/// Describe a metadata value shape for source diagnostics.
pub fn value_kind_description(kind: FieldMetadataValueKind) -> &'static str {
    match kind {
        FieldMetadataValueKind::String => "a string literal",
        FieldMetadataValueKind::StringOrStringList => "a string literal or a list of string literals",
        FieldMetadataValueKind::Boolean => "a boolean literal",
        FieldMetadataValueKind::BooleanStringOrLifecycle => {
            "a boolean literal, a string literal, or a nested lifecycle descriptor"
        }
    }
}

/// Parse a field metadata key from its canonical string form.
pub fn from_str(s: &str) -> Option<FieldMetadataKey> {
    match s {
        "alias" => Some(FieldMetadataKey::Alias),
        "description" => Some(FieldMetadataKey::Description),
        "renamed_from" => Some(FieldMetadataKey::RenamedFrom),
        "canonical" => Some(FieldMetadataKey::Canonical),
        "made_private" => Some(FieldMetadataKey::MadePrivate),
        _ => None,
    }
}

/// Return the canonical spelling for a field metadata key.
pub fn as_str(key: FieldMetadataKey) -> &'static str {
    match key {
        FieldMetadataKey::Alias => "alias",
        FieldMetadataKey::Description => "description",
        FieldMetadataKey::RenamedFrom => "renamed_from",
        FieldMetadataKey::Canonical => "canonical",
        FieldMetadataKey::MadePrivate => "made_private",
    }
}
