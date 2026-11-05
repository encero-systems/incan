//! Field visibility, Rust ABI, contract metadata, and compiled provider metadata validation.

use super::*;

/// Reject field-visibility states that are not valid on their decoded manifest surface.
pub(super) fn validate_field_visibilities(raw: &RawLibraryManifest) -> Result<(), LibraryManifestError> {
    if let Some(api) = &raw.contract_metadata.api {
        for module in &api.modules {
            for declaration in &module.declarations {
                if let ApiDeclaration::Trait(trait_decl) = declaration {
                    reject_private_fields(
                        &format!("API trait `{}` required", trait_decl.name),
                        &trait_decl.requires,
                    )?;
                }
            }
        }
        validate_checked_api_public_namespaces(api)
            .map_err(|error| LibraryManifestError::Invalid(error.to_string()))?;
    }
    Ok(())
}

/// Reject private fields on one manifest surface that only represents public requirements.
pub(super) fn reject_private_fields(owner: &str, fields: &[super::FieldExport]) -> Result<(), LibraryManifestError> {
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
pub(super) fn validate_rust_abi(raw: &RawLibraryManifest) -> Result<(), LibraryManifestError> {
    let Some(abi) = &raw.rust_abi else {
        return Ok(());
    };
    if abi.schema_version != RUST_ABI_SCHEMA_VERSION {
        return Err(LibraryManifestError::Invalid(format!(
            "rust_abi.schema_version {} is unsupported (expected {})",
            abi.schema_version, RUST_ABI_SCHEMA_VERSION
        )));
    }
    if let Some(digest) = &abi.cache_digest {
        sha256_digest("rust_abi.cache_digest", digest, true).map_err(LibraryManifestError::Invalid)?;
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
pub(super) fn validate_contract_metadata(raw: &RawLibraryManifest) -> Result<(), LibraryManifestError> {
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
pub(super) fn validate_compiled_provider_metadata(
    metadata: &CompiledProviderMetadata,
) -> Result<(), LibraryManifestError> {
    if metadata.schema_version != COMPILED_PROVIDER_METADATA_SCHEMA_VERSION {
        return Err(LibraryManifestError::Invalid(format!(
            "contract_metadata.provider.schema_version {} is unsupported (expected {})",
            metadata.schema_version, COMPILED_PROVIDER_METADATA_SCHEMA_VERSION
        )));
    }
    if let Some(digest) = &metadata.semantic_source_digest {
        sha256_digest("provider semantic source digest", digest, true).map_err(LibraryManifestError::Invalid)?;
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
        sha256_digest(
            &format!("provider dependency `{}` artifact digest", dependency.dependency_key),
            &dependency.artifact_digest,
            true,
        )
        .map_err(LibraryManifestError::Invalid)?;
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

    validate_provider_operation_descriptors(metadata)
}

/// Require distinct function operation identities whose requirements are checked capability identities.
pub(super) fn validate_provider_operation_descriptors(
    metadata: &CompiledProviderMetadata,
) -> Result<(), LibraryManifestError> {
    let mut operation_ids = HashSet::new();
    for descriptor in &metadata.operation_descriptors {
        if descriptor.operation.kind != SemanticSourceTargetKind::Function {
            return Err(LibraryManifestError::Invalid(
                "contract_metadata.provider.operation_descriptors contains a non-function operation identity"
                    .to_string(),
            ));
        }
        if descriptor.required_capability.kind != SemanticSourceTargetKind::Capability {
            return Err(LibraryManifestError::Invalid(
                "contract_metadata.provider.operation_descriptors contains a non-capability requirement identity"
                    .to_string(),
            ));
        }
        if !operation_ids.insert(&descriptor.operation) {
            return Err(LibraryManifestError::Invalid(format!(
                "contract_metadata.provider.operation_descriptors contains duplicate operation `{}`",
                descriptor.operation.declaration_name
            )));
        }
    }
    Ok(())
}

/// Validate one SHA-256 digest exactly as every writer emits it: 64 lowercase hexadecimal characters, after `sha256:`
/// when `prefixed`. `label` names the field in the refusal; every digest field is checked here and nowhere else.
pub(crate) fn sha256_digest(label: &str, value: &str, prefixed: bool) -> Result<(), String> {
    let hex = if prefixed {
        value.strip_prefix("sha256:")
    } else {
        Some(value)
    };
    let lowercase_hex = |hex: &str| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    };
    if hex.is_some_and(lowercase_hex) {
        return Ok(());
    }
    let form = if prefixed {
        "`sha256:` and 64 lowercase hexadecimal characters"
    } else {
        "64 lowercase hexadecimal characters"
    };
    Err(format!("{label} must be a SHA-256 digest: {form}"))
}

/// Validate one portable artifact-root-relative path while allowing a normalized leading parent traversal.
///
/// Ordinary path dependencies can be sibling providers inside one relocatable distribution tree. Their artifact path
/// therefore needs leading `..` components, unlike provider-owned files that must remain inside one artifact root.
pub(super) fn validate_provider_dependency_artifact_path(
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
pub(super) fn validate_required_features(
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
pub(super) fn validate_provider_identifier(kind: &str, value: &str) -> Result<(), LibraryManifestError> {
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
