//! Shared compiler conventions (well-known identifiers).

/// Entry point function name.
pub const ENTRYPOINT_NAME: &str = "main";

/// Tuple newtype field index used by codegen (`struct Newtype(T)` field name).
pub const NEWTYPE_TUPLE_FIELD: &str = "0";

/// Preferred validated-constructor method for newtypes.
pub const NEWTYPE_FROM_UNDERLYING_METHOD: &str = "from_underlying";

/// Convention: validation method name for `@derive(Validate)`.
pub const VALIDATE_METHOD: &str = "validate";

/// Convention: constructor method name for derived validation helpers.
pub const NEW_METHOD: &str = "new";

/// Type name alias for Unit.
pub const UNIT_TYPE_NAME: &str = "Unit";

/// Type name alias for None (treated as Unit in type position).
pub const NONE_TYPE_NAME: &str = "None";

/// Validate one RFC 114 package-feature identifier.
///
/// Package features deliberately use a smaller, backend-neutral spelling contract than arbitrary manifest strings:
/// an ASCII letter or underscore first, followed by ASCII letters, digits, underscores, or hyphens.
pub fn validate_package_feature_identifier(identifier: &str) -> Result<(), &'static str> {
    let mut chars = identifier.chars();
    let Some(first) = chars.next() else {
        return Err("identifier cannot be empty");
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return Err("identifier must start with an ASCII letter or underscore");
    }
    if chars.any(|ch| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')) {
        return Err("identifier may contain only ASCII letters, digits, underscores, and hyphens");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_package_feature_identifier;

    #[test]
    fn validates_package_feature_identifier_subset() {
        assert!(validate_package_feature_identifier("json").is_ok());
        assert!(validate_package_feature_identifier("_internal-v2").is_ok());
        assert!(validate_package_feature_identifier("").is_err());
        assert!(validate_package_feature_identifier("2d").is_err());
        assert!(validate_package_feature_identifier("dependency/name").is_err());
    }
}
