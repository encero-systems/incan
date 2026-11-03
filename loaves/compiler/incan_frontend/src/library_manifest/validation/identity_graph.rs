//! Identity graph coverage, nominal members, root and nested backing, and export identity projections.

use super::*;

/// Validate the versioned canonical identity boundary before any consumer hydrates frontend facts from it.
pub(super) fn validate_identity_graph(raw: &RawLibraryManifest) -> Result<(), LibraryManifestError> {
    let graph = &raw.contract_metadata.identity_graph;
    if !matches!(
        graph.schema_version,
        LEGACY_LIBRARY_IDENTITY_GRAPH_SCHEMA_VERSION | LIBRARY_IDENTITY_GRAPH_SCHEMA_VERSION
    ) {
        return Err(LibraryManifestError::Invalid(format!(
            "contract_metadata.identity_graph.schema_version {} is unsupported (expected {} or {})",
            graph.schema_version, LEGACY_LIBRARY_IDENTITY_GRAPH_SCHEMA_VERSION, LIBRARY_IDENTITY_GRAPH_SCHEMA_VERSION
        )));
    }

    let mut seen = BTreeSet::new();
    for entry in &graph.exports {
        if entry.public_name.trim().is_empty() || entry.source_path.is_empty() {
            return Err(LibraryManifestError::Invalid(
                "contract_metadata.identity_graph contains an empty public name or source path".to_string(),
            ));
        }
        if entry.public_path.len() < 2
            || entry.public_path.first() != Some(&raw.name)
            || entry.public_path.last() != Some(&entry.public_name)
        {
            return Err(LibraryManifestError::Invalid(format!(
                "identity graph entry `{}` has a public path inconsistent with package `{}`",
                entry.public_name, raw.name
            )));
        }
        match (graph.schema_version, entry.canonical.as_ref()) {
            (LEGACY_LIBRARY_IDENTITY_GRAPH_SCHEMA_VERSION, Some(_)) => {
                return Err(LibraryManifestError::Invalid(format!(
                    "schema-v1 identity graph entry `{}` cannot publish canonical identity metadata",
                    entry.public_name
                )));
            }
            (LIBRARY_IDENTITY_GRAPH_SCHEMA_VERSION, None) => {
                return Err(LibraryManifestError::Invalid(format!(
                    "schema-v2 identity graph entry `{}` is missing its canonical identity",
                    entry.public_name
                )));
            }
            _ => {}
        }
        if let Some(identity) = &entry.canonical {
            validate_canonical_identity(identity, &format!("export `{}`", entry.public_name), false)?;
            validate_export_identity_binding(raw, &graph.exports, entry, identity)?;
            if graph.schema_version == LIBRARY_IDENTITY_GRAPH_SCHEMA_VERSION {
                if entry.public_path.len() == 2 {
                    validate_root_identity_graph_backing(raw, entry, identity)?;
                } else {
                    validate_nested_identity_graph_backing(raw, entry, identity)?;
                }
            }
            if !seen.insert((entry.public_path.as_slice(), identity)) {
                return Err(LibraryManifestError::Invalid(format!(
                    "contract_metadata.identity_graph contains duplicate canonical export `{}`",
                    entry.public_name
                )));
            }
        }
    }

    let require_members = graph.schema_version == LIBRARY_IDENTITY_GRAPH_SCHEMA_VERSION;
    if require_members {
        validate_current_identity_graph_coverage(raw)?;
    }
    validate_nominal_identity_graph_members(raw, require_members)
}

/// Validate nominal member identities after the graph's export entries and current-schema coverage have passed.
/// Keep owner-kind order and each enum's variant/method duplicate inventory at this transport trust boundary.
pub(super) fn validate_nominal_identity_graph_members(
    raw: &RawLibraryManifest,
    require_members: bool,
) -> Result<(), LibraryManifestError> {
    for model in &raw.exports.models {
        let owner_identity = root_canonical_identity(raw, ExportIdentityKind::Model, &model.name);
        validate_nominal_member_identities(
            &format!("model `{}`", model.name),
            owner_identity,
            &model.fields,
            &model.properties,
            &model.methods,
            require_members,
            false,
        )?;
    }
    for class in &raw.exports.classes {
        let owner_identity = root_canonical_identity(raw, ExportIdentityKind::Class, &class.name);
        validate_nominal_member_identities(
            &format!("class `{}`", class.name),
            owner_identity,
            &class.fields,
            &class.properties,
            &class.methods,
            require_members,
            class.extends.is_some(),
        )?;
    }
    for trait_export in &raw.exports.traits {
        let owner_identity = root_canonical_identity(raw, ExportIdentityKind::Trait, &trait_export.name);
        validate_method_identities(
            &format!("trait `{}`", trait_export.name),
            owner_identity,
            &trait_export.methods,
            require_members,
            None,
            false,
        )?;
    }
    for newtype in &raw.exports.newtypes {
        let owner_identity = root_canonical_identity(raw, ExportIdentityKind::Newtype, &newtype.name);
        validate_method_identities(
            &format!("newtype `{}`", newtype.name),
            owner_identity,
            &newtype.methods,
            require_members,
            None,
            false,
        )?;
    }
    for enum_export in &raw.exports.enums {
        let owner_identity = root_canonical_identity(raw, ExportIdentityKind::Enum, &enum_export.name);
        let mut seen_members = BTreeSet::new();
        for variant in &enum_export.variants {
            validate_member_identity(
                &format!("enum `{}` variant `{}`", enum_export.name, variant.name),
                &variant.name,
                variant.canonical.as_ref(),
                "variant",
                owner_identity,
                require_members,
                false,
            )?;
            record_member_identity(
                &format!("enum `{}`", enum_export.name),
                &variant.name,
                variant.canonical.as_ref(),
                &mut seen_members,
            )?;
        }
        validate_method_identities(
            &format!("enum `{}`", enum_export.name),
            owner_identity,
            &enum_export.methods,
            require_members,
            Some(&mut seen_members),
            false,
        )?;
    }
    Ok(())
}

/// Bind a package-root v2 identity to its exact checked API declaration whenever API metadata is present.
///
/// Current-schema manifests without embedded API metadata remain a supported compact artifact form. Once a producer
/// publishes both representations, however, the identity graph may not advertise a name/kind/span absent from the
/// checked API snapshot.
pub(super) fn validate_root_identity_graph_backing(
    raw: &RawLibraryManifest,
    entry: &super::ExportIdentity,
    identity: &CanonicalIdentityExport,
) -> Result<(), LibraryManifestError> {
    let Some(api) = raw.contract_metadata.api.as_ref() else {
        return Ok(());
    };
    if let ExportIdentityProjection::Reexport { target_path } = &entry.projection {
        let binding_exists = api.modules.iter().any(|module| {
            matches!(module.module_path.as_slice(), [root] if root == "lib" || root == "main")
                && module.declarations.iter().any(|declaration| {
                    matches!(
                        declaration,
                        ApiDeclaration::Alias(alias)
                            if alias.is_public
                                && alias.name == entry.public_name
                                && alias.target_path == *target_path
                    )
                })
        });
        if binding_exists && package_local_identity_target_matches(raw, api, identity) {
            return Ok(());
        }
        return Err(LibraryManifestError::Invalid(format!(
            "package-root identity graph entry `{}` is not backed by its checked API declaration",
            entry.public_name
        )));
    }
    let backing_path = &entry.source_path;
    let Some((declaration_name, module_path)) = backing_path.split_last() else {
        return Err(LibraryManifestError::Invalid(format!(
            "package-root identity graph entry `{}` has no checked API declaration backing",
            entry.public_name
        )));
    };
    let backed = api
        .modules
        .iter()
        .find(|module| module.module_path == module_path)
        .is_some_and(|module| {
            module.declarations.iter().any(|declaration| {
                api_declaration_backs_identity_entry(
                    raw,
                    api,
                    &module.module_path,
                    declaration,
                    declaration_name,
                    entry,
                    identity,
                )
            })
        });
    if !backed {
        return Err(LibraryManifestError::Invalid(format!(
            "package-root identity graph entry `{}` is not backed by its checked API declaration",
            entry.public_name
        )));
    }
    Ok(())
}

/// Require exact multiset coverage between current-schema raw exports and package-root identity entries.
///
/// Deeper public namespace entries are an additional API projection and remain valid. At the package root, however,
/// every raw declaration must have exactly one graph entry and every graph entry must describe a raw declaration.
/// Counting by `(kind, public name)` preserves overload sets while rejecting missing, duplicate, and fabricated
/// roots.
pub(super) fn validate_current_identity_graph_coverage(raw: &RawLibraryManifest) -> Result<(), LibraryManifestError> {
    let mut required = Vec::<(ExportIdentityKind, String)>::new();
    let mut add = |kind, name: &str| required.push((kind, name.to_string()));
    for export in &raw.exports.aliases {
        add(ExportIdentityKind::Alias, &export.name);
    }
    for export in &raw.exports.partials {
        add(ExportIdentityKind::Partial, &export.name);
    }
    for export in &raw.exports.models {
        add(ExportIdentityKind::Model, &export.name);
    }
    for export in &raw.exports.classes {
        add(ExportIdentityKind::Class, &export.name);
    }
    for export in &raw.exports.functions {
        add(ExportIdentityKind::Function, &export.name);
    }
    for export in &raw.exports.traits {
        add(ExportIdentityKind::Trait, &export.name);
    }
    for export in &raw.exports.enums {
        add(ExportIdentityKind::Enum, &export.name);
    }
    for export in &raw.exports.type_aliases {
        add(ExportIdentityKind::TypeAlias, &export.name);
    }
    for export in &raw.exports.newtypes {
        add(ExportIdentityKind::Newtype, &export.name);
    }
    for export in &raw.exports.consts {
        add(ExportIdentityKind::Const, &export.name);
    }
    for export in &raw.exports.statics {
        add(ExportIdentityKind::Static, &export.name);
    }

    let required = required
        .into_iter()
        .fold(std::collections::BTreeMap::new(), |mut counts, key| {
            *counts.entry(key).or_insert(0usize) += 1;
            counts
        });
    let published = raw
        .contract_metadata
        .identity_graph
        .exports
        .iter()
        .filter(|entry| entry.public_path.len() == 2)
        .fold(std::collections::BTreeMap::new(), |mut counts, entry| {
            *counts.entry((entry.kind, entry.public_name.clone())).or_insert(0usize) += 1;
            counts
        });
    for (key, required_count) in &required {
        let published_count = published.get(key).copied().unwrap_or(0);
        if published_count != *required_count {
            return Err(LibraryManifestError::Invalid(format!(
                "schema-v2 identity graph publishes {published_count} root {:?} identities named `{}` for {required_count} raw declarations",
                key.0, key.1
            )));
        }
    }
    if let Some(((kind, name), count)) = published.iter().find(|(key, _)| !required.contains_key(*key)) {
        return Err(LibraryManifestError::Invalid(format!(
            "schema-v2 identity graph publishes {count} unbacked root {kind:?} identities named `{name}`"
        )));
    }
    Ok(())
}

/// Bind one graph entry's canonical identity to the exact source/projection path the entry publishes.
pub(super) fn validate_export_identity_binding(
    raw: &RawLibraryManifest,
    siblings: &[super::ExportIdentity],
    entry: &super::ExportIdentity,
    identity: &CanonicalIdentityExport,
) -> Result<(), LibraryManifestError> {
    validate_export_canonical_kind(raw, entry, identity)?;
    let authoritative_path = validate_export_projection_path(raw, entry, identity)?;
    if authoritative_path.is_empty() || !canonical_identity_matches_path(siblings, identity, authoritative_path) {
        // A path can end on an intermediate hop, so report both the path and every public carrier of the identity.
        let carriers = siblings
            .iter()
            .filter(|sibling| sibling.canonical.as_ref() == Some(identity))
            .map(|sibling| sibling.public_path.join("."))
            .collect::<Vec<_>>();
        return Err(LibraryManifestError::Invalid(format!(
            "identity graph entry `{}` canonical identity disagrees with its authoritative source/projection path: \
             the path is `{}` while the identity names `{}`, and this package publishes that identity at {}",
            entry.public_name,
            authoritative_path.join("."),
            identity.declaration_name,
            if carriers.is_empty() {
                "no other public path".to_string()
            } else {
                format!("`{}`", carriers.join("`, `"))
            }
        )));
    }
    validate_root_export_projection(raw, entry, identity)
}

/// Require a graph export's canonical kind to match its raw declaration kind. Alias targets retain their own kind;
/// a newtype's checked raw declaration distinguishes newtype and rusttype identities.
pub(super) fn validate_export_canonical_kind(
    raw: &RawLibraryManifest,
    entry: &super::ExportIdentity,
    identity: &CanonicalIdentityExport,
) -> Result<(), LibraryManifestError> {
    let expected_kind = match entry.kind {
        ExportIdentityKind::Function => Some("function"),
        ExportIdentityKind::Partial => Some("partial"),
        ExportIdentityKind::Alias => None,
        ExportIdentityKind::TypeAlias => Some("type_alias"),
        ExportIdentityKind::Model => Some("model"),
        ExportIdentityKind::Class => Some("class"),
        ExportIdentityKind::Trait => Some("trait"),
        ExportIdentityKind::Enum => Some("enum"),
        ExportIdentityKind::Newtype => raw
            .exports
            .newtypes
            .iter()
            .find(|newtype| newtype.name == entry.public_name)
            .map(|newtype| if newtype.is_rusttype { "rusttype" } else { "newtype" }),
        ExportIdentityKind::Const => Some("const"),
        ExportIdentityKind::Static => Some("static"),
    };
    if let Some(expected_kind) = expected_kind
        && identity.kind != expected_kind
    {
        return Err(LibraryManifestError::Invalid(format!(
            "identity graph entry `{}` publishes canonical kind `{}` instead of `{expected_kind}`",
            entry.public_name, identity.kind
        )));
    }

    Ok(())
}

/// Validate the selected projection's shape and return its authoritative source path without inventing a target.
/// Direct and partial exports name their own canonical declaration; aliases and reexports retain their target hop.
pub(super) fn validate_export_projection_path<'entry>(
    raw: &RawLibraryManifest,
    entry: &'entry super::ExportIdentity,
    identity: &CanonicalIdentityExport,
) -> Result<&'entry [String], LibraryManifestError> {
    let authoritative_path = match &entry.projection {
        ExportIdentityProjection::Direct => {
            if matches!(entry.kind, ExportIdentityKind::Alias | ExportIdentityKind::Partial) {
                return Err(LibraryManifestError::Invalid(format!(
                    "identity graph entry `{}` uses a direct projection for {:?}",
                    entry.public_name, entry.kind
                )));
            }
            if !matches!(
                &identity.origin,
                CanonicalIdentityOriginExport::Package { library, .. } if library == &raw.name
            ) {
                return Err(LibraryManifestError::Invalid(format!(
                    "identity graph direct export `{}` has a canonical origin outside manifest package `{}`",
                    entry.public_name, raw.name
                )));
            }
            if entry.public_name != identity.declaration_name
                || entry.source_path.last() != Some(&identity.declaration_name)
            {
                return Err(LibraryManifestError::Invalid(format!(
                    "identity graph direct export `{}` does not name its canonical declaration",
                    entry.public_name
                )));
            }
            &entry.source_path
        }
        ExportIdentityProjection::Alias { target_path } => {
            if !matches!(entry.kind, ExportIdentityKind::Alias | ExportIdentityKind::Function) {
                return Err(LibraryManifestError::Invalid(format!(
                    "identity graph entry `{}` uses an alias projection for {:?}",
                    entry.public_name, entry.kind
                )));
            }
            if entry.source_path.last() != Some(&entry.public_name) || target_path.is_empty() {
                return Err(LibraryManifestError::Invalid(format!(
                    "identity graph alias `{}` has an invalid source or target path",
                    entry.public_name
                )));
            }
            target_path
        }
        ExportIdentityProjection::Reexport { target_path } => {
            // Reexports accept every public declaration kind. Their invariant is path identity: the projection
            // republishes its resolved target rather than renaming it, regardless of the target's declaration kind.
            if entry.source_path != *target_path {
                return Err(LibraryManifestError::Invalid(format!(
                    "identity graph reexport `{}` has different source and target paths",
                    entry.public_name
                )));
            }
            target_path
        }
        ExportIdentityProjection::Partial {
            target_path,
            target_kind,
        } => {
            if entry.kind != ExportIdentityKind::Partial
                || matches!(target_kind, super::PartialTargetKindExport::Unknown)
                || target_path.is_empty()
            {
                return Err(LibraryManifestError::Invalid(format!(
                    "identity graph partial `{}` has an invalid target projection",
                    entry.public_name
                )));
            }
            if entry.public_name != identity.declaration_name
                || entry.source_path.last() != Some(&identity.declaration_name)
            {
                return Err(LibraryManifestError::Invalid(format!(
                    "identity graph partial `{}` does not name its canonical declaration",
                    entry.public_name
                )));
            }
            &entry.source_path
        }
    };
    Ok(authoritative_path)
}

/// Bind package-root alias and partial projections to their raw exports after the canonical path proof. Nested
/// projections retain their existing graph-level validation and do not require a package-root raw declaration.
pub(super) fn validate_root_export_projection(
    raw: &RawLibraryManifest,
    entry: &super::ExportIdentity,
    identity: &CanonicalIdentityExport,
) -> Result<(), LibraryManifestError> {
    if entry.public_path.len() == 2 {
        match (&entry.kind, &entry.projection) {
            (ExportIdentityKind::Alias, ExportIdentityProjection::Alias { .. })
            | (ExportIdentityKind::Alias, ExportIdentityProjection::Reexport { .. }) => {
                // Public names are unique at the package root. A facade's graph and raw export can name different
                // hops in the same reexport chain, so their canonical target binds them rather than path equality.
                let raw_alias = raw.exports.aliases.iter().find(|alias| alias.name == entry.public_name);
                let Some(raw_alias) = raw_alias else {
                    return Err(LibraryManifestError::Invalid(format!(
                        "identity graph alias `{}` has no matching raw alias export",
                        entry.public_name
                    )));
                };
                // The prefixes may differ, but both sides must still name the declaration the identity names -- or,
                // for an alias of an alias, reach it. A chain records each link on the hop it was written on, so the
                // raw export names the intermediate alias while the identity carries the terminal declaration.
                let reaches_identity_declaration = raw_alias.target_path.last() == Some(&identity.declaration_name)
                    || raw.contract_metadata.api.as_ref().is_some_and(|api| {
                        checked_api_alias_chain_reaches(api, &[], &raw_alias.target_path, &identity.declaration_name)
                    });
                if !reaches_identity_declaration {
                    return Err(LibraryManifestError::Invalid(format!(
                        "identity graph alias `{}` projection disagrees with its raw export: the raw export targets \
                         `{}` and the canonical identity names `{}`",
                        entry.public_name,
                        raw_alias.target_path.join("."),
                        identity.declaration_name
                    )));
                }
                validate_alias_callable_metadata(
                    &format!("identity graph alias `{}`", entry.public_name),
                    &entry.public_name,
                    identity,
                    raw_alias
                        .projected_function
                        .as_ref()
                        .map(|function| function.name.as_str()),
                )?;
            }
            (
                ExportIdentityKind::Partial,
                ExportIdentityProjection::Partial {
                    target_path,
                    target_kind,
                },
            ) => {
                let matches_raw = raw.exports.partials.iter().any(|partial| {
                    partial.name == entry.public_name
                        && partial.target_path == *target_path
                        && partial.target_kind == *target_kind
                });
                if !matches_raw {
                    return Err(LibraryManifestError::Invalid(format!(
                        "identity graph partial `{}` projection disagrees with its raw export",
                        entry.public_name
                    )));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Require a callable alias projection to agree with the canonical target it advertises.
pub(super) fn validate_alias_callable_metadata(
    owner: &str,
    public_name: &str,
    identity: &CanonicalIdentityExport,
    projected_function_name: Option<&str>,
) -> Result<(), LibraryManifestError> {
    let projected_function_name = match (identity.kind.as_str(), projected_function_name) {
        ("function" | "partial" | "builtin", Some(projected_function_name)) => projected_function_name,
        ("function" | "partial" | "builtin", None) => {
            return Err(LibraryManifestError::Invalid(format!(
                "{owner} has a canonical callable target without callable metadata"
            )));
        }
        (_, Some(_)) => {
            return Err(LibraryManifestError::Invalid(format!(
                "{owner} publishes callable metadata for non-callable canonical kind `{}`",
                identity.kind
            )));
        }
        (_, None) => return Ok(()),
    };
    if projected_function_name != public_name {
        return Err(LibraryManifestError::Invalid(format!(
            "{owner} callable projection is named `{}` instead of `{public_name}`",
            projected_function_name
        )));
    }
    Ok(())
}

/// Bind a deeper v2 graph entry to the checked API namespace member and declaration that actually exports it.
pub(super) fn validate_nested_identity_graph_backing(
    raw: &RawLibraryManifest,
    entry: &super::ExportIdentity,
    identity: &CanonicalIdentityExport,
) -> Result<(), LibraryManifestError> {
    if entry.public_path.len() == 2 {
        return Ok(());
    }
    let Some(api) = raw.contract_metadata.api.as_ref() else {
        return Err(LibraryManifestError::Invalid(format!(
            "identity graph entry `{}` has no checked API namespace backing",
            entry.public_name
        )));
    };
    let namespace_path = &entry.public_path[1..entry.public_path.len() - 1];
    let Some(namespace) = api
        .public_namespaces
        .iter()
        .find(|namespace| namespace.path == namespace_path)
    else {
        return Err(LibraryManifestError::Invalid(format!(
            "identity graph entry `{}` has no checked API namespace backing",
            entry.public_name
        )));
    };
    let backed = namespace
        .members
        .iter()
        .filter(|member| member.name == entry.public_name)
        .any(|member| {
            let Some((declaration_name, module_path)) = member.source_path.split_last() else {
                return false;
            };
            api.modules
                .iter()
                .find(|module| module.module_path == module_path)
                .is_some_and(|module| {
                    module.declarations.iter().any(|declaration| {
                        api_declaration_backs_identity_entry(
                            raw,
                            api,
                            &module.module_path,
                            declaration,
                            declaration_name,
                            entry,
                            identity,
                        )
                    })
                })
        });
    if !backed {
        // Name what the two records disagreed on. The graph entry and the checked API each describe the same export
        // from a different side, so a bare rejection leaves no way to tell a wrong kind from a wrong projection or a
        // member whose source path points somewhere the API never declared.
        let offered = namespace
            .members
            .iter()
            .filter(|member| member.name == entry.public_name)
            .map(|member| {
                // An alias's own target is the field these two records disagree on most often, so name it here rather
                // than leaving the reader to guess which spelling the checked API kept.
                let target = member
                    .source_path
                    .split_last()
                    .and_then(|(declaration_name, module_path)| {
                        api.modules
                            .iter()
                            .find(|module| module.module_path == module_path)
                            .and_then(|module| {
                                module.declarations.iter().find_map(|declaration| match declaration {
                                    ApiDeclaration::Alias(alias) if alias.name == *declaration_name => {
                                        Some(format!(" aliasing `{}`", alias.target_path.join(".")))
                                    }
                                    _ => None,
                                })
                            })
                    });
                format!("{}{}", member.source_path.join("."), target.unwrap_or_default())
            })
            .collect::<Vec<_>>();
        return Err(LibraryManifestError::Invalid(format!(
            "identity graph entry `{}` is not backed by a checked API namespace declaration: \
             public path `{}` carries kind {:?} and projection {:?}, and namespace `{}` offers {}",
            entry.public_name,
            entry.public_path.join("."),
            entry.kind,
            entry.projection,
            namespace_path.join("."),
            if offered.is_empty() {
                "no member of that name".to_string()
            } else {
                format!("`{}`", offered.join("`, `"))
            }
        )));
    }
    Ok(())
}
