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
use super::{
    COMPILED_PROVIDER_METADATA_SCHEMA_VERSION, CompiledProviderMetadata, EnumExport, EnumValueExport,
    EnumValueTypeExport, FieldVisibilityExport, LIBRARY_MANIFEST_FORMAT, LibraryManifestError, ParamExport,
    ParamKindExport, PartialExport, ProviderCargoDependencySource, RUST_ABI_SCHEMA_VERSION, VocabProviderManifest,
};
use crate::frontend::api_metadata::{ApiDeclaration, CHECKED_API_METADATA_SCHEMA_VERSION};
use crate::frontend::contract_metadata::CONTRACT_METADATA_SCHEMA_VERSION;
use crate::frontend::registry_metadata::CHECKED_REGISTRY_METADATA_SCHEMA_VERSION;

/// Validate one raw manifest payload before it is written or decoded into the semantic model.
pub(super) fn validate_raw_manifest(raw: &RawLibraryManifest) -> Result<(), LibraryManifestError> {
    validate_manifest_version(raw)?;
    validate_field_visibilities(raw)?;
    validate_callable_param_exports(&raw.exports)?;
    validate_value_enum_exports(&raw.exports)?;
    validate_contract_metadata(raw)?;
    validate_rust_abi(raw)?;
    validate_vocab_payload(raw)?;
    validate_soft_keyword_activations(raw)?;
    Ok(())
}

/// Reject field-visibility states that the current source language cannot author or enforce.
fn validate_field_visibilities(raw: &RawLibraryManifest) -> Result<(), LibraryManifestError> {
    for model in &raw.exports.models {
        reject_private_fields(&format!("model `{}`", model.name), &model.fields)?;
    }
    if let Some(api) = &raw.contract_metadata.api {
        for module in &api.modules {
            for declaration in &module.declarations {
                match declaration {
                    ApiDeclaration::Model(model) => {
                        reject_private_fields(&format!("API model `{}`", model.name), &model.fields)?;
                    }
                    ApiDeclaration::Trait(trait_decl) => {
                        reject_private_fields(
                            &format!("API trait `{}` required", trait_decl.name),
                            &trait_decl.requires,
                        )?;
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

/// Reject private fields on one manifest surface that currently supports public fields only.
fn reject_private_fields(owner: &str, fields: &[super::FieldExport]) -> Result<(), LibraryManifestError> {
    for field in fields {
        if matches!(field.visibility, FieldVisibilityExport::Private) {
            return Err(LibraryManifestError::Invalid(format!(
                "{owner} field `{}` cannot be private in a library manifest",
                field.name
            )));
        }
    }
    Ok(())
}

/// Validate embedded Rust ABI metadata before consumers use it as a hot-path lookup source.
fn validate_rust_abi(raw: &RawLibraryManifest) -> Result<(), LibraryManifestError> {
    let Some(abi) = &raw.rust_abi else {
        return Ok(());
    };
    if abi.schema_version != RUST_ABI_SCHEMA_VERSION {
        return Err(LibraryManifestError::Invalid(format!(
            "rust_abi.schema_version {} is unsupported (expected {})",
            abi.schema_version, RUST_ABI_SCHEMA_VERSION
        )));
    }
    let mut paths = HashSet::new();
    for item in &abi.items {
        if item.canonical_path.trim().is_empty() {
            return Err(LibraryManifestError::Invalid(
                "rust_abi.items canonical_path cannot be empty".to_string(),
            ));
        }
        if !paths.insert(item.canonical_path.as_str()) {
            return Err(LibraryManifestError::Invalid(format!(
                "rust_abi.items contains duplicate canonical path `{}`",
                item.canonical_path
            )));
        }
    }
    Ok(())
}

/// Validate RFC 048 metadata embedded in a manifest before consumers trust it.
fn validate_contract_metadata(raw: &RawLibraryManifest) -> Result<(), LibraryManifestError> {
    let metadata = &raw.contract_metadata.models;
    if metadata.schema_version != CONTRACT_METADATA_SCHEMA_VERSION {
        return Err(LibraryManifestError::Invalid(format!(
            "contract_metadata.models.schema_version {} is unsupported (expected {})",
            metadata.schema_version, CONTRACT_METADATA_SCHEMA_VERSION
        )));
    }
    metadata
        .validate()
        .map_err(|error| LibraryManifestError::Invalid(error.to_string()))?;

    if let Some(api) = &raw.contract_metadata.api {
        if api.schema_version != CHECKED_API_METADATA_SCHEMA_VERSION {
            return Err(LibraryManifestError::Invalid(format!(
                "contract_metadata.api.schema_version {} is unsupported (expected {})",
                api.schema_version, CHECKED_API_METADATA_SCHEMA_VERSION
            )));
        }
        for module in &api.modules {
            if module.schema_version != CHECKED_API_METADATA_SCHEMA_VERSION {
                return Err(LibraryManifestError::Invalid(format!(
                    "contract_metadata.api.modules schema_version {} is unsupported (expected {})",
                    module.schema_version, CHECKED_API_METADATA_SCHEMA_VERSION
                )));
            }
        }
    }
    validate_compiled_provider_metadata(&raw.contract_metadata.provider)?;
    if let Some(registry) = &raw.contract_metadata.registry
        && registry.schema_version != CHECKED_REGISTRY_METADATA_SCHEMA_VERSION
    {
        return Err(LibraryManifestError::Invalid(format!(
            "contract_metadata.registry.schema_version {} is unsupported (expected {})",
            registry.schema_version, CHECKED_REGISTRY_METADATA_SCHEMA_VERSION
        )));
    }
    Ok(())
}

/// Validate generic compiled-provider metadata before any compiler stage trusts its feature projection or backend map.
fn validate_compiled_provider_metadata(metadata: &CompiledProviderMetadata) -> Result<(), LibraryManifestError> {
    if metadata.schema_version != COMPILED_PROVIDER_METADATA_SCHEMA_VERSION {
        return Err(LibraryManifestError::Invalid(format!(
            "contract_metadata.provider.schema_version {} is unsupported (expected {})",
            metadata.schema_version, COMPILED_PROVIDER_METADATA_SCHEMA_VERSION
        )));
    }
    if let Some(digest) = &metadata.semantic_source_digest {
        validate_sha256_digest("provider semantic source", digest)?;
    }
    for feature in metadata.public_features.keys() {
        validate_provider_identifier("public feature", feature)?;
    }
    for active in &metadata.active_features {
        if !metadata.public_features.contains_key(active) {
            return Err(LibraryManifestError::Invalid(format!(
                "contract_metadata.provider.active_features contains undeclared feature `{active}`"
            )));
        }
    }
    let mut provider_dependencies = HashSet::new();
    for dependency in &metadata.provider_dependencies {
        validate_provider_identifier("provider dependency", &dependency.dependency_key)?;
        if !provider_dependencies.insert(dependency.dependency_key.as_str()) {
            return Err(LibraryManifestError::Invalid(format!(
                "contract_metadata.provider.provider_dependencies contains duplicate dependency key `{}`",
                dependency.dependency_key
            )));
        }
        if dependency.provider_name.trim().is_empty() {
            return Err(LibraryManifestError::Invalid(format!(
                "provider dependency `{}` has an empty provider name",
                dependency.dependency_key
            )));
        }
        if dependency.provider_version.trim().is_empty() {
            return Err(LibraryManifestError::Invalid(format!(
                "provider dependency `{}` has an empty provider version",
                dependency.dependency_key
            )));
        }
        validate_provider_artifact_digest(&dependency.dependency_key, &dependency.artifact_digest)?;
        validate_provider_dependency_artifact_path(&dependency.dependency_key, &dependency.relative_artifact_path)?;
        for feature in &dependency.requested_features {
            validate_provider_identifier("dependency feature", feature)?;
        }
    }
    for (feature, declaration) in &metadata.public_features {
        for included in &declaration.includes {
            if !metadata.public_features.contains_key(included) {
                return Err(LibraryManifestError::Invalid(format!(
                    "provider feature `{feature}` includes undeclared feature `{included}`"
                )));
            }
        }
        for dependency in &declaration.optional_dependencies {
            validate_provider_identifier("optional dependency", dependency)?;
        }
        for (dependency, features) in &declaration.dependency_features {
            validate_provider_identifier("dependency", dependency)?;
            for dependency_feature in features {
                validate_provider_identifier("dependency feature", dependency_feature)?;
            }
        }
        for component in &declaration.required_sdk_components {
            validate_provider_identifier("SDK component", component)?;
        }
    }

    let mut claims = HashSet::new();
    for claim in &metadata.namespace_claims {
        for segment in &claim.module_path {
            validate_provider_identifier("module segment", segment)?;
        }
        validate_required_features(metadata, &claim.required_features, "namespace claim")?;
        if !claims.insert((claim.module_path.clone(), claim.required_features.clone())) {
            return Err(LibraryManifestError::Invalid(format!(
                "contract_metadata.provider.namespace_claims contains duplicate module `{}`",
                claim.module_path.join(".")
            )));
        }
    }
    for requirement in &metadata.fact_requirements {
        if requirement.identity.trim().is_empty() {
            return Err(LibraryManifestError::Invalid(
                "contract_metadata.provider.fact_requirements contains an empty identity".to_string(),
            ));
        }
        validate_required_features(metadata, &requirement.required_features, &requirement.identity)?;
    }
    for component in &metadata.required_sdk_components {
        validate_provider_identifier("SDK component", component)?;
    }

    let mut facet_ids = HashSet::new();
    for facet in &metadata.implementation_facets {
        validate_provider_identifier("implementation facet", &facet.id)?;
        if !facet_ids.insert(facet.id.as_str()) {
            return Err(LibraryManifestError::Invalid(format!(
                "contract_metadata.provider.implementation_facets contains duplicate id `{}`",
                facet.id
            )));
        }
        validate_required_features(metadata, &facet.required_features, &facet.id)?;
        for module in &facet.required_modules {
            for segment in module {
                validate_provider_identifier("module segment", segment)?;
            }
        }
        for (crate_name, features) in &facet.cargo_features {
            validate_provider_identifier("Cargo dependency", crate_name)?;
            for feature in features {
                validate_provider_identifier("Cargo feature", feature)?;
            }
        }
        for dependency in &facet.cargo_dependencies {
            validate_provider_identifier("Cargo dependency", &dependency.crate_name)?;
            if dependency.version.as_deref().is_some_and(str::is_empty) {
                return Err(LibraryManifestError::Invalid(format!(
                    "implementation facet `{}` has an empty Cargo version for `{}`",
                    facet.id, dependency.crate_name
                )));
            }
            for feature in &dependency.features {
                validate_provider_identifier("Cargo feature", feature)?;
            }
            match &dependency.source {
                ProviderCargoDependencySource::Registry if dependency.version.is_none() => {
                    return Err(LibraryManifestError::Invalid(format!(
                        "implementation facet `{}` registry dependency `{}` has no version",
                        facet.id, dependency.crate_name
                    )));
                }
                ProviderCargoDependencySource::Toolchain { relative_path } => {
                    let path = Path::new(relative_path);
                    if path.is_absolute() || path.components().any(|component| component == Component::ParentDir) {
                        return Err(LibraryManifestError::Invalid(format!(
                            "implementation facet `{}` toolchain dependency `{}` has non-relocatable path `{relative_path}`",
                            facet.id, dependency.crate_name
                        )));
                    }
                }
                ProviderCargoDependencySource::Registry => {}
            }
        }
    }
    Ok(())
}

/// Validate the exact SHA-256 identity emitted by the compiled-provider artifact hasher.
fn validate_provider_artifact_digest(dependency_key: &str, digest: &str) -> Result<(), LibraryManifestError> {
    validate_sha256_digest(&format!("provider dependency `{dependency_key}` artifact"), digest)
}

/// Validate one exact SHA-256 identity with a caller-owned diagnostic label.
fn validate_sha256_digest(label: &str, digest: &str) -> Result<(), LibraryManifestError> {
    let Some(hex) = digest.strip_prefix("sha256:") else {
        return Err(LibraryManifestError::Invalid(format!(
            "{label} digest must start with `sha256:`"
        )));
    };
    if hex.len() != 64 || !hex.chars().all(|character| character.is_ascii_hexdigit()) {
        return Err(LibraryManifestError::Invalid(format!(
            "{label} digest must contain 64 hexadecimal characters"
        )));
    }
    Ok(())
}

/// Validate one portable artifact-root-relative path while allowing a normalized leading parent traversal.
///
/// Ordinary path dependencies can be sibling providers inside one relocatable distribution tree. Their artifact path
/// therefore needs leading `..` components, unlike provider-owned files that must remain inside one artifact root.
fn validate_provider_dependency_artifact_path(
    dependency_key: &str,
    relative_path: &str,
) -> Result<(), LibraryManifestError> {
    let path = Path::new(relative_path);
    if relative_path.trim().is_empty() || path.is_absolute() || relative_path.contains('\\') {
        return Err(LibraryManifestError::Invalid(format!(
            "provider dependency `{dependency_key}` artifact path `{relative_path}` must be a portable relative path"
        )));
    }
    let mut saw_normal = false;
    for component in path.components() {
        match component {
            Component::ParentDir if !saw_normal => {}
            Component::Normal(_) => saw_normal = true,
            Component::ParentDir | Component::CurDir | Component::RootDir | Component::Prefix(_) => {
                return Err(LibraryManifestError::Invalid(format!(
                    "provider dependency `{dependency_key}` artifact path `{relative_path}` must be normalized"
                )));
            }
        }
    }
    if !saw_normal {
        return Err(LibraryManifestError::Invalid(format!(
            "provider dependency `{dependency_key}` artifact path `{relative_path}` must name an artifact directory"
        )));
    }
    Ok(())
}

/// Validate that every positive predicate references a feature declared by this provider.
fn validate_required_features(
    metadata: &CompiledProviderMetadata,
    required_features: &std::collections::BTreeSet<String>,
    owner: &str,
) -> Result<(), LibraryManifestError> {
    for feature in required_features {
        if !metadata.public_features.contains_key(feature) {
            return Err(LibraryManifestError::Invalid(format!(
                "provider fact `{owner}` requires undeclared feature `{feature}`"
            )));
        }
    }
    Ok(())
}

/// Validate identifiers persisted in provider metadata independently of authoring syntax.
fn validate_provider_identifier(kind: &str, value: &str) -> Result<(), LibraryManifestError> {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return Err(LibraryManifestError::Invalid(format!(
            "provider {kind} cannot be empty"
        )));
    };
    if !(first.is_ascii_alphabetic() || first == '_')
        || chars.any(|ch| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'))
    {
        return Err(LibraryManifestError::Invalid(format!(
            "provider {kind} `{value}` must use ASCII letters, digits, underscores, or hyphens"
        )));
    }
    Ok(())
}

/// Validate exported callable parameter metadata before import code trusts it as a semantic signature.
fn validate_callable_param_exports(exports: &RawLibraryExports) -> Result<(), LibraryManifestError> {
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
fn validate_partial_export(partial: &PartialExport) -> Result<(), LibraryManifestError> {
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
fn validate_callable_params(owner: &str, params: &[ParamExport]) -> Result<(), LibraryManifestError> {
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
fn validate_rest_param_has_no_default(owner: &str, param: &ParamExport) -> Result<(), LibraryManifestError> {
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
    validate_scoped_surface_descriptors(raw)?;
    validate_scoped_symbol_descriptors(raw)?;

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

/// Validate RFC 045 scoped-symbol descriptors before they become compiler-facing manifest data.
fn validate_scoped_symbol_descriptors(raw: &RawLibraryManifest) -> Result<(), LibraryManifestError> {
    let Some(vocab) = &raw.vocab else {
        return Ok(());
    };

    let mut seen_descriptor_keys = HashSet::new();
    let mut seen_positive_positions = HashSet::new();

    for surface in &vocab.dsl_surfaces {
        let activation_key = scoped_surface_activation_key(&surface.activation);
        let declarations: HashSet<&str> = surface
            .declarations
            .iter()
            .map(|declaration| declaration.keyword.as_str())
            .collect();
        let clauses: HashSet<(&str, &str)> = surface
            .declarations
            .iter()
            .flat_map(|declaration| {
                declaration
                    .clauses
                    .iter()
                    .map(|clause| (declaration.keyword.as_str(), clause.keyword.as_str()))
            })
            .collect();

        for descriptor in &surface.scoped_symbols {
            validate_scoped_symbol_descriptor_shape(descriptor)?;
            if !seen_descriptor_keys.insert(format!("{activation_key}:{}", descriptor.key)) {
                return Err(LibraryManifestError::Invalid(format!(
                    "duplicate scoped symbol descriptor key `{}` for activation `{activation_key}`",
                    descriptor.key
                )));
            }
            validate_scoped_symbol_role(descriptor)?;
            validate_scoped_symbol_diagnostics(descriptor)?;

            if descriptor.eligible_in.is_empty() {
                return Err(LibraryManifestError::Invalid(format!(
                    "scoped symbol descriptor `{}` must declare at least one eligible position",
                    descriptor.key
                )));
            }

            for eligibility in &descriptor.eligible_in {
                validate_scoped_symbol_eligibility(&descriptor.key, eligibility, &declarations, &clauses)?;
                let position_key = format!(
                    "{}:{}:{}:{}:{}:{:?}",
                    activation_key,
                    descriptor.symbol,
                    eligibility.declaration,
                    eligibility.clause.as_deref().unwrap_or(""),
                    eligibility.call.as_deref().unwrap_or(""),
                    eligibility.position
                );
                if !seen_positive_positions.insert(position_key) {
                    return Err(LibraryManifestError::Invalid(format!(
                        "ambiguous scoped symbol descriptor `{}` conflicts with another descriptor for the same activation, symbol, and eligible position",
                        descriptor.key
                    )));
                }
            }
        }
    }

    Ok(())
}

/// Validate scoped-symbol descriptor identity and identifier spelling.
fn validate_scoped_symbol_descriptor_shape(
    descriptor: &incan_vocab::ScopedSymbolDescriptor,
) -> Result<(), LibraryManifestError> {
    if descriptor.key.trim().is_empty() {
        return Err(LibraryManifestError::Invalid(
            "vocab scoped symbol descriptor key cannot be empty".to_string(),
        ));
    }
    if descriptor.symbol.trim().is_empty() {
        return Err(LibraryManifestError::Invalid(format!(
            "scoped symbol descriptor `{}` symbol cannot be empty",
            descriptor.key
        )));
    }
    if !is_identifier_spelling(&descriptor.symbol) {
        return Err(LibraryManifestError::Invalid(format!(
            "scoped symbol descriptor `{}` symbol `{}` is not a valid identifier",
            descriptor.key, descriptor.symbol
        )));
    }
    if incan_core::lang::keywords::from_str_hard_only(&descriptor.symbol).is_some() {
        return Err(LibraryManifestError::Invalid(format!(
            "scoped symbol descriptor `{}` symbol `{}` cannot be a hard keyword",
            descriptor.key, descriptor.symbol
        )));
    }
    Ok(())
}

/// Validate optional DSL-authored role metadata.
fn validate_scoped_symbol_role(descriptor: &incan_vocab::ScopedSymbolDescriptor) -> Result<(), LibraryManifestError> {
    let Some(role) = &descriptor.role else {
        return Ok(());
    };

    if role.key.trim().is_empty() {
        return Err(LibraryManifestError::Invalid(format!(
            "scoped symbol descriptor `{}` role key cannot be empty",
            descriptor.key
        )));
    }
    if role.label.as_ref().is_some_and(|label| label.trim().is_empty()) {
        return Err(LibraryManifestError::Invalid(format!(
            "scoped symbol descriptor `{}` role label cannot be empty",
            descriptor.key
        )));
    }
    if role
        .description
        .as_ref()
        .is_some_and(|description| description.trim().is_empty())
    {
        return Err(LibraryManifestError::Invalid(format!(
            "scoped symbol descriptor `{}` role description cannot be empty",
            descriptor.key
        )));
    }
    Ok(())
}

/// Validate author-provided diagnostic templates for one scoped-symbol descriptor.
fn validate_scoped_symbol_diagnostics(
    descriptor: &incan_vocab::ScopedSymbolDescriptor,
) -> Result<(), LibraryManifestError> {
    let mut seen_codes = HashSet::new();
    for diagnostic in &descriptor.diagnostics {
        if diagnostic.code.trim().is_empty() {
            return Err(LibraryManifestError::Invalid(format!(
                "scoped symbol descriptor `{}` diagnostic code cannot be empty",
                descriptor.key
            )));
        }
        if diagnostic.message.trim().is_empty() {
            return Err(LibraryManifestError::Invalid(format!(
                "scoped symbol descriptor `{}` diagnostic `{}` message cannot be empty",
                descriptor.key, diagnostic.code
            )));
        }
        if !seen_codes.insert(diagnostic.code.as_str()) {
            return Err(LibraryManifestError::Invalid(format!(
                "scoped symbol descriptor `{}` contains duplicate diagnostic code `{}`",
                descriptor.key, diagnostic.code
            )));
        }
    }
    Ok(())
}

/// Validate that a scoped-symbol positive eligibility rule references a known declaration or clause.
fn validate_scoped_symbol_eligibility(
    descriptor_key: &str,
    eligibility: &incan_vocab::ScopedSymbolEligibility,
    declarations: &HashSet<&str>,
    clauses: &HashSet<(&str, &str)>,
) -> Result<(), LibraryManifestError> {
    if eligibility.declaration.trim().is_empty() {
        return Err(LibraryManifestError::Invalid(format!(
            "scoped symbol descriptor `{descriptor_key}` eligibility declaration cannot be empty"
        )));
    }
    if !declarations.contains(eligibility.declaration.as_str()) {
        return Err(LibraryManifestError::Invalid(format!(
            "scoped symbol descriptor `{descriptor_key}` references unknown declaration `{}`",
            eligibility.declaration
        )));
    }

    match eligibility.position {
        incan_vocab::ScopedSymbolPosition::ClauseBody => match &eligibility.clause {
            Some(clause) if !clause.trim().is_empty() => {
                if eligibility.call.is_some() {
                    return Err(LibraryManifestError::Invalid(format!(
                        "scoped symbol descriptor `{descriptor_key}` clause-body eligibility cannot declare a call"
                    )));
                }
                if !clauses.contains(&(eligibility.declaration.as_str(), clause.as_str())) {
                    return Err(LibraryManifestError::Invalid(format!(
                        "scoped symbol descriptor `{descriptor_key}` references unknown clause `{}` in declaration `{}`",
                        clause, eligibility.declaration
                    )));
                }
                Ok(())
            }
            _ => Err(LibraryManifestError::Invalid(format!(
                "scoped symbol descriptor `{descriptor_key}` clause-body eligibility must declare a clause"
            ))),
        },
        incan_vocab::ScopedSymbolPosition::DeclarationBody => {
            if eligibility.clause.is_some() || eligibility.call.is_some() {
                return Err(LibraryManifestError::Invalid(format!(
                    "scoped symbol descriptor `{descriptor_key}` declaration eligibility cannot declare a clause or call"
                )));
            }
            Ok(())
        }
        incan_vocab::ScopedSymbolPosition::CallArgument => {
            if eligibility.clause.is_some() {
                return Err(LibraryManifestError::Invalid(format!(
                    "scoped symbol descriptor `{descriptor_key}` call-argument eligibility cannot declare a clause"
                )));
            }
            match eligibility.call.as_deref() {
                Some(call) if !call.trim().is_empty() => Ok(()),
                _ => Err(LibraryManifestError::Invalid(format!(
                    "scoped symbol descriptor `{descriptor_key}` call-argument eligibility must declare a call"
                ))),
            }
        }
        _ => Err(LibraryManifestError::Invalid(format!(
            "scoped symbol descriptor `{descriptor_key}` uses an unsupported eligibility position"
        ))),
    }
}

/// Return whether a scoped symbol spelling is compatible with ordinary identifier call syntax.
fn is_identifier_spelling(symbol: &str) -> bool {
    let mut chars = symbol.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic()) && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

/// Validate RFC 040 scoped-surface descriptors before they become compiler-facing manifest data.
fn validate_scoped_surface_descriptors(raw: &RawLibraryManifest) -> Result<(), LibraryManifestError> {
    let Some(vocab) = &raw.vocab else {
        return Ok(());
    };

    let mut seen_descriptor_keys = HashSet::new();
    let mut seen_positive_positions = HashSet::new();

    for surface in &vocab.dsl_surfaces {
        let activation_key = scoped_surface_activation_key(&surface.activation);
        let declarations: HashSet<&str> = surface
            .declarations
            .iter()
            .map(|declaration| declaration.keyword.as_str())
            .collect();
        let clauses: HashSet<(&str, &str)> = surface
            .declarations
            .iter()
            .flat_map(|declaration| {
                declaration
                    .clauses
                    .iter()
                    .map(|clause| (declaration.keyword.as_str(), clause.keyword.as_str()))
            })
            .collect();

        for descriptor in &surface.scoped_surfaces {
            if descriptor.key.trim().is_empty() {
                return Err(LibraryManifestError::Invalid(
                    "vocab scoped surface descriptor key cannot be empty".to_string(),
                ));
            }
            if !seen_descriptor_keys.insert(format!("{activation_key}:{}", descriptor.key)) {
                return Err(LibraryManifestError::Invalid(format!(
                    "duplicate scoped surface descriptor key `{}` for activation `{activation_key}`",
                    descriptor.key
                )));
            }
            validate_scoped_surface_syntax(descriptor)?;
            validate_scoped_surface_receiver(descriptor)?;
            validate_scoped_surface_diagnostics(descriptor)?;

            if descriptor.eligible_in.is_empty() {
                return Err(LibraryManifestError::Invalid(format!(
                    "scoped surface descriptor `{}` must declare at least one eligible position",
                    descriptor.key
                )));
            }

            for eligibility in &descriptor.eligible_in {
                validate_scoped_surface_eligibility(&descriptor.key, eligibility, &declarations, &clauses)?;
                let position_key = format!(
                    "{}:{}:{}:{}:{}:{:?}",
                    activation_key,
                    scoped_surface_syntax_key(&descriptor.syntax),
                    eligibility.declaration,
                    eligibility.clause.as_deref().unwrap_or(""),
                    eligibility.call.as_deref().unwrap_or(""),
                    eligibility.position
                );
                if !seen_positive_positions.insert(position_key) {
                    return Err(LibraryManifestError::Invalid(format!(
                        "ambiguous scoped surface descriptor `{}` conflicts with another descriptor for the same activation, syntax, and eligible position",
                        descriptor.key
                    )));
                }
            }
        }
    }

    Ok(())
}

/// Validate that descriptor syntax is well-formed and matches the declared family.
fn validate_scoped_surface_syntax(
    descriptor: &incan_vocab::ScopedSurfaceDescriptor,
) -> Result<(), LibraryManifestError> {
    match (&descriptor.family, &descriptor.syntax) {
        (
            incan_vocab::ScopedSurfaceFamily::OperatorLike | incan_vocab::ScopedSurfaceFamily::BindingLike,
            incan_vocab::ScopedSurfaceSyntax::Glyph { spelling },
        ) => {
            if spelling.trim().is_empty() {
                return Err(LibraryManifestError::Invalid(format!(
                    "scoped surface descriptor `{}` glyph spelling cannot be empty",
                    descriptor.key
                )));
            }
        }
        (
            incan_vocab::ScopedSurfaceFamily::ExpressionForm,
            incan_vocab::ScopedSurfaceSyntax::LeadingDotPath {
                min_segments,
                max_segments,
            },
        ) => {
            if *min_segments == 0 {
                return Err(LibraryManifestError::Invalid(format!(
                    "scoped surface descriptor `{}` leading-dot path must accept at least one segment",
                    descriptor.key
                )));
            }
            if max_segments.is_some_and(|max_segments| max_segments < *min_segments) {
                return Err(LibraryManifestError::Invalid(format!(
                    "scoped surface descriptor `{}` leading-dot max_segments cannot be less than min_segments",
                    descriptor.key
                )));
            }
        }
        _ => {
            return Err(LibraryManifestError::Invalid(format!(
                "scoped surface descriptor `{}` uses a syntax shape that does not match its family",
                descriptor.key
            )));
        }
    }

    Ok(())
}

/// Validate receiver metadata for expression-form descriptors.
fn validate_scoped_surface_receiver(
    descriptor: &incan_vocab::ScopedSurfaceDescriptor,
) -> Result<(), LibraryManifestError> {
    if descriptor.family == incan_vocab::ScopedSurfaceFamily::ExpressionForm && descriptor.receiver.is_none() {
        return Err(LibraryManifestError::Invalid(format!(
            "expression-form scoped surface descriptor `{}` must declare receiver derivation",
            descriptor.key
        )));
    }
    if descriptor.family != incan_vocab::ScopedSurfaceFamily::ExpressionForm && descriptor.receiver.is_some() {
        return Err(LibraryManifestError::Invalid(format!(
            "non-expression scoped surface descriptor `{}` cannot declare receiver derivation",
            descriptor.key
        )));
    }

    match &descriptor.receiver {
        Some(incan_vocab::ScopedSurfaceReceiver::Clause { clause }) if clause.trim().is_empty() => {
            Err(LibraryManifestError::Invalid(format!(
                "scoped surface descriptor `{}` receiver clause cannot be empty",
                descriptor.key
            )))
        }
        Some(incan_vocab::ScopedSurfaceReceiver::Custom { key }) if key.trim().is_empty() => {
            Err(LibraryManifestError::Invalid(format!(
                "scoped surface descriptor `{}` receiver custom key cannot be empty",
                descriptor.key
            )))
        }
        _ => Ok(()),
    }
}

/// Validate author-provided diagnostic templates for one scoped-surface descriptor.
fn validate_scoped_surface_diagnostics(
    descriptor: &incan_vocab::ScopedSurfaceDescriptor,
) -> Result<(), LibraryManifestError> {
    let mut seen_codes = HashSet::new();
    for diagnostic in &descriptor.diagnostics {
        if diagnostic.code.trim().is_empty() {
            return Err(LibraryManifestError::Invalid(format!(
                "scoped surface descriptor `{}` diagnostic code cannot be empty",
                descriptor.key
            )));
        }
        if diagnostic.message.trim().is_empty() {
            return Err(LibraryManifestError::Invalid(format!(
                "scoped surface descriptor `{}` diagnostic `{}` message cannot be empty",
                descriptor.key, diagnostic.code
            )));
        }
        if !seen_codes.insert(diagnostic.code.as_str()) {
            return Err(LibraryManifestError::Invalid(format!(
                "scoped surface descriptor `{}` contains duplicate diagnostic code `{}`",
                descriptor.key, diagnostic.code
            )));
        }
    }
    Ok(())
}

/// Validate that a positive eligibility rule references a known declaration or clause.
fn validate_scoped_surface_eligibility(
    descriptor_key: &str,
    eligibility: &incan_vocab::ScopedSurfaceEligibility,
    declarations: &HashSet<&str>,
    clauses: &HashSet<(&str, &str)>,
) -> Result<(), LibraryManifestError> {
    if eligibility.declaration.trim().is_empty() {
        return Err(LibraryManifestError::Invalid(format!(
            "scoped surface descriptor `{descriptor_key}` eligibility declaration cannot be empty"
        )));
    }
    if !declarations.contains(eligibility.declaration.as_str()) {
        return Err(LibraryManifestError::Invalid(format!(
            "scoped surface descriptor `{descriptor_key}` references unknown declaration `{}`",
            eligibility.declaration
        )));
    }

    match eligibility.position {
        incan_vocab::ScopedSurfacePosition::ClauseBody => match &eligibility.clause {
            Some(clause) if !clause.trim().is_empty() => {
                if eligibility.call.is_some() {
                    return Err(LibraryManifestError::Invalid(format!(
                        "scoped surface descriptor `{descriptor_key}` clause-body eligibility cannot declare a call"
                    )));
                }
                if !clauses.contains(&(eligibility.declaration.as_str(), clause.as_str())) {
                    return Err(LibraryManifestError::Invalid(format!(
                        "scoped surface descriptor `{descriptor_key}` references unknown clause `{}` in declaration `{}`",
                        clause, eligibility.declaration
                    )));
                }
                Ok(())
            }
            _ => Err(LibraryManifestError::Invalid(format!(
                "scoped surface descriptor `{descriptor_key}` clause-body eligibility must declare a clause"
            ))),
        },
        incan_vocab::ScopedSurfacePosition::DeclarationHead => Err(LibraryManifestError::Invalid(format!(
            "scoped surface descriptor `{descriptor_key}` declaration-head eligibility is not supported yet"
        ))),
        incan_vocab::ScopedSurfacePosition::DeclarationBody => {
            if eligibility.clause.is_some() || eligibility.call.is_some() {
                return Err(LibraryManifestError::Invalid(format!(
                    "scoped surface descriptor `{descriptor_key}` declaration eligibility cannot declare a clause or call"
                )));
            }
            Ok(())
        }
        incan_vocab::ScopedSurfacePosition::CallArgument => {
            if eligibility.clause.is_some() {
                return Err(LibraryManifestError::Invalid(format!(
                    "scoped surface descriptor `{descriptor_key}` call-argument eligibility cannot declare a clause"
                )));
            }
            match eligibility.call.as_deref() {
                Some(call) if !call.trim().is_empty() => Ok(()),
                _ => Err(LibraryManifestError::Invalid(format!(
                    "scoped surface descriptor `{descriptor_key}` call-argument eligibility must declare a call"
                ))),
            }
        }
        _ => Err(LibraryManifestError::Invalid(format!(
            "scoped surface descriptor `{descriptor_key}` uses an unsupported eligibility position"
        ))),
    }
}

/// Build a stable validation key for a descriptor activation rule.
fn scoped_surface_activation_key(activation: &incan_vocab::KeywordActivation) -> String {
    match activation {
        incan_vocab::KeywordActivation::Always => "always".to_string(),
        incan_vocab::KeywordActivation::OnImport { namespace } => format!("import:{namespace}"),
        _ => "unknown".to_string(),
    }
}

/// Build a stable validation key for a descriptor syntax shape.
fn scoped_surface_syntax_key(syntax: &incan_vocab::ScopedSurfaceSyntax) -> String {
    match syntax {
        incan_vocab::ScopedSurfaceSyntax::Glyph { spelling } => format!("glyph:{spelling}"),
        incan_vocab::ScopedSurfaceSyntax::LeadingDotPath {
            min_segments,
            max_segments,
        } => format!("leading-dot:{min_segments}:{max_segments:?}"),
        _ => "unsupported".to_string(),
    }
}

/// Validate RFC 032 value-enum metadata before import code trusts the manifest enum surface.
fn validate_value_enum_exports(exports: &RawLibraryExports) -> Result<(), LibraryManifestError> {
    for enum_export in &exports.enums {
        validate_value_enum_export(enum_export)?;
    }
    Ok(())
}

/// Validate one exported enum's value metadata.
fn validate_value_enum_export(enum_export: &EnumExport) -> Result<(), LibraryManifestError> {
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
fn value_matches_enum_type(value: &EnumValueExport, value_type: EnumValueTypeExport) -> bool {
    matches!(
        (value_type, value),
        (EnumValueTypeExport::Str, EnumValueExport::Str(_)) | (EnumValueTypeExport::Int, EnumValueExport::Int(_))
    )
}

/// Display name for a manifest value-enum backing type.
fn enum_value_type_name(value_type: EnumValueTypeExport) -> &'static str {
    match value_type {
        EnumValueTypeExport::Str => "str",
        EnumValueTypeExport::Int => "int",
    }
}

/// User-facing display for duplicate manifest value-enum values.
fn enum_value_display(value: &EnumValueExport) -> String {
    match value {
        EnumValueExport::Str(value) => value.clone(),
        EnumValueExport::Int(value) => value.to_string(),
    }
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
    names.extend(exports.aliases.iter().map(|item| item.name.as_str()));
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
    names.extend(exports.statics.iter().map(|item| item.name.as_str()));
    names
}
