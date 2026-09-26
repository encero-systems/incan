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

/// Prefix reserved for the names the compiler generates (#1769).
///
/// Generated items and locals (the original a decorator wraps, lowering temporaries, projected symbol names) are
/// spelled with this prefix, so the checker refuses it on every name a source program declares or binds; a source name
/// spelled the same way could collide with a generated one in the build.
pub const RESERVED_COMPILER_NAME_PREFIX: &str = "__incan_";

/// Static method the compiler reads as a type's source-defined constructor hook.
///
/// `Type(args)` on a type that declares it calls the hook instead of building the fields. It is the one source
/// declaration that may use [`RESERVED_COMPILER_NAME_PREFIX`], because the compiler looks it up by this exact name.
pub const TYPE_CONSTRUCTOR_HOOK: &str = "__incan_new";

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
