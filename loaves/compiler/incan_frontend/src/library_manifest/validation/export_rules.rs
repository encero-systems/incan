//! Callable parameter, partial, value-enum, artifact path and ownership export rules, and manifest versions.

use super::*;

/// Validate exported callable parameter metadata before import code trusts it as a semantic signature.
pub(super) fn validate_callable_param_exports(exports: &RawLibraryExports) -> Result<(), LibraryManifestError> {
    for function in &exports.functions {
        validate_callable_params(&format!("function `{}`", function.name), &function.params)?;
    }
    for partial in &exports.partials {
        validate_partial_export(partial)?;
        validate_callable_params(&format!("partial `{}`", partial.name), &partial.params)?;
    }
    for model in &exports.models {
        for method in &model.methods {
            validate_callable_params(
                &format!("model `{}` method `{}`", model.name, method.name),
                &method.params,
            )?;
        }
    }
    for class in &exports.classes {
        for method in &class.methods {
            validate_callable_params(
                &format!("class `{}` method `{}`", class.name, method.name),
                &method.params,
            )?;
        }
    }
    for trait_export in &exports.traits {
        for method in &trait_export.methods {
            validate_callable_params(
                &format!("trait `{}` method `{}`", trait_export.name, method.name),
                &method.params,
            )?;
        }
    }
    for enum_export in &exports.enums {
        for method in &enum_export.methods {
            validate_callable_params(
                &format!("enum `{}` method `{}`", enum_export.name, method.name),
                &method.params,
            )?;
        }
    }
    for newtype in &exports.newtypes {
        for method in &newtype.methods {
            validate_callable_params(
                &format!("newtype `{}` method `{}`", newtype.name, method.name),
                &method.params,
            )?;
        }
    }
    Ok(())
}

/// Validate one exported partial's provenance payload.
pub(super) fn validate_partial_export(partial: &PartialExport) -> Result<(), LibraryManifestError> {
    if partial.target_path.is_empty() {
        return Err(LibraryManifestError::Invalid(format!(
            "partial `{}` must declare a non-empty target path",
            partial.name
        )));
    }
    if partial.presets.is_empty() {
        return Err(LibraryManifestError::Invalid(format!(
            "partial `{}` must declare at least one preset",
            partial.name
        )));
    }
    let mut seen = HashSet::new();
    for preset in &partial.presets {
        if !seen.insert(preset.name.as_str()) {
            return Err(LibraryManifestError::Invalid(format!(
                "partial `{}` repeats preset `{}`",
                partial.name, preset.name
            )));
        }
    }
    Ok(())
}

/// Validate one exported callable signature's rest-parameter metadata.
pub(super) fn validate_callable_params(owner: &str, params: &[ParamExport]) -> Result<(), LibraryManifestError> {
    let mut saw_rest_positional = false;
    let mut saw_rest_keyword = false;
    let mut saw_rest = false;

    for param in params {
        match param.kind {
            ParamKindExport::Normal => {
                if saw_rest_keyword {
                    return Err(LibraryManifestError::Invalid(format!(
                        "{owner} parameter `{}` cannot appear after a `**kwargs` rest parameter",
                        param.name
                    )));
                }
                if saw_rest {
                    return Err(LibraryManifestError::Invalid(format!(
                        "{owner} parameter `{}` cannot appear after a rest parameter",
                        param.name
                    )));
                }
            }
            ParamKindExport::RestPositional => {
                if saw_rest_positional {
                    return Err(LibraryManifestError::Invalid(format!(
                        "{owner} declares more than one `*args` rest parameter"
                    )));
                }
                if saw_rest_keyword {
                    return Err(LibraryManifestError::Invalid(format!(
                        "{owner} `*args` rest parameter must appear before `**kwargs`"
                    )));
                }
                validate_rest_param_has_no_default(owner, param)?;
                saw_rest_positional = true;
                saw_rest = true;
            }
            ParamKindExport::RestKeyword => {
                if saw_rest_keyword {
                    return Err(LibraryManifestError::Invalid(format!(
                        "{owner} declares more than one `**kwargs` rest parameter"
                    )));
                }
                validate_rest_param_has_no_default(owner, param)?;
                saw_rest_keyword = true;
                saw_rest = true;
            }
        }
    }

    Ok(())
}

/// Reject rest parameters that claim default values across the manifest boundary.
pub(super) fn validate_rest_param_has_no_default(owner: &str, param: &ParamExport) -> Result<(), LibraryManifestError> {
    if param.has_default {
        return Err(LibraryManifestError::Invalid(format!(
            "{owner} rest parameter `{}` cannot declare a default value",
            param.name
        )));
    }
    Ok(())
}

/// Validate top-level manifest format and compiler-version compatibility.
///
/// This is the first gate because downstream validation rules only make sense once the compiler knows it understands
/// the payload shape and that the manifest does not require a newer Incan version than the current binary provides.
pub(super) fn validate_manifest_version(raw: &RawLibraryManifest) -> Result<(), LibraryManifestError> {
    if !matches!(
        raw.manifest_format,
        LIBRARY_MANIFEST_FORMAT | super::OWNERSHIP_MANIFEST_FORMAT
    ) {
        return Err(LibraryManifestError::Invalid(format!(
            "unsupported manifest_format {} (expected {})",
            raw.manifest_format, LIBRARY_MANIFEST_FORMAT
        )));
    }

    let manifest_version = Version::parse(&raw.incan_version).map_err(|err| {
        LibraryManifestError::Invalid(format!("invalid `incan_version` value `{}`: {err}", raw.incan_version))
    })?;
    let compiler_version = Version::parse(incan_lang::version::INCAN_VERSION).map_err(|err| {
        LibraryManifestError::Invalid(format!(
            "invalid compiler version `{}`: {err}",
            incan_lang::version::INCAN_VERSION
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

/// Validate RFC 032 value-enum metadata before import code trusts the manifest enum surface.
pub(super) fn validate_value_enum_exports(exports: &RawLibraryExports) -> Result<(), LibraryManifestError> {
    for enum_export in &exports.enums {
        validate_value_enum_export(enum_export)?;
    }
    Ok(())
}

/// Validate one exported enum's value metadata.
pub(super) fn validate_value_enum_export(enum_export: &EnumExport) -> Result<(), LibraryManifestError> {
    let variant_names = enum_export
        .variants
        .iter()
        .map(|variant| variant.name.as_str())
        .collect::<HashSet<_>>();
    let mut alias_names = HashSet::new();
    for alias in &enum_export.variant_aliases {
        if variant_names.contains(alias.name.as_str()) || !alias_names.insert(alias.name.as_str()) {
            return Err(LibraryManifestError::Invalid(format!(
                "enum `{}` has duplicate variant alias `{}`",
                enum_export.name, alias.name
            )));
        }
        if !variant_names.contains(alias.target.as_str()) {
            return Err(LibraryManifestError::Invalid(format!(
                "enum `{}.{}` aliases unknown variant `{}`",
                enum_export.name, alias.name, alias.target
            )));
        }
    }

    let Some(value_type) = enum_export.value_type else {
        if enum_export.ordinal_type_identity.is_some() {
            return Err(LibraryManifestError::Invalid(format!(
                "enum `{}` has ordinal_type_identity but no enum value_type",
                enum_export.name
            )));
        }
        for variant in &enum_export.variants {
            if variant.value.is_some() {
                return Err(LibraryManifestError::Invalid(format!(
                    "enum `{}` variant `{}` has a value but no enum value_type",
                    enum_export.name, variant.name
                )));
            }
        }
        return Ok(());
    };

    if !enum_export.type_params.is_empty() {
        return Err(LibraryManifestError::Invalid(format!(
            "value enum `{}` cannot have type parameters",
            enum_export.name
        )));
    }
    if enum_export.ordinal_type_identity.as_deref().is_some_and(str::is_empty) {
        return Err(LibraryManifestError::Invalid(format!(
            "value enum `{}` has an empty ordinal_type_identity",
            enum_export.name
        )));
    }

    let mut seen_values = HashSet::new();
    for variant in &enum_export.variants {
        if !variant.fields.is_empty() {
            return Err(LibraryManifestError::Invalid(format!(
                "value enum `{}.{}` cannot carry payload fields",
                enum_export.name, variant.name
            )));
        }

        let Some(value) = &variant.value else {
            return Err(LibraryManifestError::Invalid(format!(
                "value enum `{}.{}` is missing a raw value",
                enum_export.name, variant.name
            )));
        };

        if !value_matches_enum_type(value, value_type) {
            return Err(LibraryManifestError::Invalid(format!(
                "value enum `{}.{}` has a raw value that does not match backing type `{}`",
                enum_export.name,
                variant.name,
                enum_value_type_name(value_type)
            )));
        }

        if !seen_values.insert(value.clone()) {
            return Err(LibraryManifestError::Invalid(format!(
                "value enum `{}` has duplicate raw value `{}`",
                enum_export.name,
                enum_value_display(value)
            )));
        }
    }

    Ok(())
}

/// Return whether a raw variant value matches its enum's declared backing type.
pub(super) fn value_matches_enum_type(value: &EnumValueExport, value_type: EnumValueTypeExport) -> bool {
    matches!(
        (value_type, value),
        (EnumValueTypeExport::Str, EnumValueExport::Str(_)) | (EnumValueTypeExport::Int, EnumValueExport::Int(_))
    )
}

/// Display name for a manifest value-enum backing type.
pub(super) fn enum_value_type_name(value_type: EnumValueTypeExport) -> &'static str {
    match value_type {
        EnumValueTypeExport::Str => "str",
        EnumValueTypeExport::Int => "int",
    }
}

/// User-facing display for duplicate manifest value-enum values.
pub(super) fn enum_value_display(value: &EnumValueExport) -> String {
    match value {
        EnumValueExport::Str(value) => value.clone(),
        EnumValueExport::Int(value) => value.to_string(),
    }
}

/// Validate soft-keyword activation declarations exported by the library.
///
/// Each activation must name a known soft keyword and a non-empty namespace so import-time keyword activation remains
/// deterministic and cannot accidentally claim hard keywords.
pub(super) fn validate_soft_keyword_activations(raw: &RawLibraryManifest) -> Result<(), LibraryManifestError> {
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
        if let Some(id) = incan_lang::lang::keywords::from_str(&activation.keyword) {
            if !incan_lang::lang::keywords::is_soft(id) {
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
pub(super) fn validate_relative_artifact_path(relative_path: &str) -> Result<(), LibraryManifestError> {
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

/// Validate explicit contracts before a consumer can hydrate any callable shape.
pub(super) fn validate_ownership_exports(exports: &RawLibraryExports, format: u32) -> Result<(), LibraryManifestError> {
    for function in exports.functions.iter().chain(
        exports
            .aliases
            .iter()
            .filter_map(|alias| alias.projected_function.as_ref()),
    ) {
        super::ownership::validate_format(format, function.ownership.as_ref())?;
        super::ownership::validate(
            function.ownership.as_ref(),
            &function.params,
            &function.return_type,
            false,
        )?;
    }
    let methods = exports
        .models
        .iter()
        .flat_map(|owner| &owner.methods)
        .chain(exports.classes.iter().flat_map(|owner| &owner.methods))
        .chain(exports.traits.iter().flat_map(|owner| &owner.methods))
        .chain(exports.enums.iter().flat_map(|owner| &owner.methods))
        .chain(exports.newtypes.iter().flat_map(|owner| &owner.methods));
    for method in methods {
        super::ownership::validate_format(format, method.ownership.as_ref())?;
        super::ownership::validate(
            method.ownership.as_ref(),
            &method.params,
            &method.return_type,
            method.receiver.is_some(),
        )?;
    }
    Ok(())
}
