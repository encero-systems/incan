//! Checked API backing for identity entries, canonical identity matches, and member identities.

use super::*;

/// Return whether one checked API declaration authorizes the graph kind and projection at its public path.
pub(super) fn api_declaration_backs_identity_entry(
    raw: &RawLibraryManifest,
    api: &crate::api_metadata::CheckedApiMetadataPackage,
    module_path: &[String],
    declaration: &ApiDeclaration,
    declaration_name: &str,
    entry: &super::ExportIdentity,
    identity: &CanonicalIdentityExport,
) -> bool {
    match declaration {
        ApiDeclaration::Alias(alias) if alias.name == declaration_name && alias.is_public => {
            // The two records name the same target at different qualifications, and for a chain at different hops:
            // a same-module alias records `["helper"]` where the graph entry records `["provider", "helper"]`, and a
            // facade records the hop it was written on where the entry carries the entrypoint's. Compare what they
            // can honestly agree on -- the declaration each names -- and let the identity carry the rest.
            //
            // The graph resolves a target through the module's own import aliases while the checked API keeps the
            // spelling as written: `pub public_target = alias target_impl` over `from helper import target as
            // target_impl` records `["helper", "target"]` on one side and `["target_impl"]` on the other. Both are
            // true, so follow the alias chain when the two spellings do not already agree.
            let target_matches = match &entry.projection {
                ExportIdentityProjection::Alias { target_path }
                | ExportIdentityProjection::Reexport { target_path } => {
                    target_path.last() == alias.target_path.last()
                        || target_path.last().is_some_and(|declaration| {
                            checked_api_alias_chain_reaches(api, module_path, &alias.target_path, declaration)
                        })
                }
                ExportIdentityProjection::Direct | ExportIdentityProjection::Partial { .. } => false,
            };
            if !target_matches || !matches!(entry.kind, ExportIdentityKind::Alias | ExportIdentityKind::Function) {
                return false;
            }
            // `source_path` is the target's resolved declaration path; `target_path` is how the alias spelled it in its
            // own module. Resolve the spelling as the projection was resolved and require it to name the projection's
            // declaration: an unqualified `helper` inside `provider` names `["provider", "helper"]`, and
            // `crate.inner.greet` names `["inner", "greet"]`.
            //
            // A chain hop names a different declaration honestly: an alias of an alias carries the *terminal*
            // function's source path, while `target_path` names the hop right next to it, so `["registry", "scale"]`
            // is not `["registry", "scale_alias"]`. Accept the hop when it republishes this same projection.
            if let Some(projected) = &alias.projected_function
                && projected.source_path != resolve_api_alias_target(module_path, &alias.target_path)
                && !alias_target_republishes_projection(api, module_path, &alias.target_path, projected)
            {
                return false;
            }
            // `materialize_api_alias_projections` seeds callable projections from function declarations alone, so an
            // alias over a partial carries none here however the partial is reached. The raw export side does project
            // partials and the package-root entry checks that, so demanding it again from the checked API rejected
            // every facade hop over a partial rather than catching a real disagreement.
            let projected_callable = alias
                .projected_function
                .as_ref()
                .map(|projected| projected.callable.name.as_str());
            let callable_metadata_agrees = (identity.kind == "partial" && projected_callable.is_none())
                || validate_alias_callable_metadata(
                    &format!("checked API alias `{}`", alias.name),
                    &entry.public_name,
                    identity,
                    projected_callable,
                )
                .is_ok();
            callable_metadata_agrees && package_local_identity_target_matches(raw, api, identity)
        }
        ApiDeclaration::Partial(partial) if partial.name == declaration_name => {
            entry.kind == ExportIdentityKind::Partial
                && api_declaration_matches_canonical(
                    raw,
                    module_path,
                    &partial.name,
                    "partial",
                    &partial.anchor,
                    identity,
                )
                && match &entry.projection {
                    ExportIdentityProjection::Partial {
                        target_path,
                        target_kind,
                    } => target_path == &partial.target_path && target_kind == &partial.target_kind,
                    // A partial re-exported through a facade publishes the hop that reaches it, exactly as an alias
                    // does. The declaration the entry names is still this partial, so compare what the two records
                    // can honestly agree on -- the target each names -- and let the identity carry the rest.
                    ExportIdentityProjection::Reexport { target_path } => {
                        target_path.last() == partial.target_path.last()
                    }
                    ExportIdentityProjection::Direct | ExportIdentityProjection::Alias { .. } => false,
                }
        }
        declaration if crate::api_metadata::api_declaration_public_name(declaration) == Some(declaration_name) => {
            matches!(&entry.projection, ExportIdentityProjection::Direct)
                && api_declaration_export_kind(declaration) == Some(entry.kind)
                && api_declaration_identity_parts(declaration).is_some_and(|(name, kind, anchor)| {
                    api_declaration_matches_canonical(raw, module_path, name, kind, anchor, identity)
                })
        }
        _ => false,
    }
}

/// Return whether one checked API alias path resolves, through any number of hops, to the named declaration.
///
/// Each link of a re-export chain records the hop it was written on, so the first hop names an intermediate alias
/// rather than the declaration the canonical identity carries. Follow the remaining hops instead of demanding that the
/// first one already be terminal. Each hop records its target as spelled in its own module, so resolve it there before
/// the next step, and stop on a repeated path so a cyclic manifest cannot loop here.
pub(super) fn checked_api_alias_chain_reaches(
    api: &crate::api_metadata::CheckedApiMetadataPackage,
    owning_module: &[String],
    target_path: &[String],
    declaration_name: &str,
) -> bool {
    let mut seen = HashSet::new();
    let mut path = resolve_api_alias_target(owning_module, target_path);
    loop {
        let Some((name, module_path)) = path.split_last() else {
            return false;
        };
        let (name, module_path) = (name.clone(), module_path.to_vec());
        if name == declaration_name {
            return true;
        }
        if !seen.insert(path.clone()) {
            return false;
        }
        let Some(module) = api.modules.iter().find(|module| module.module_path == module_path) else {
            return false;
        };
        let Some(next) = module.declarations.iter().find_map(|declaration| match declaration {
            ApiDeclaration::Alias(alias) if alias.name == name => Some(alias.target_path.clone()),
            _ => None,
        }) else {
            return false;
        };
        path = resolve_api_alias_target(&module_path, &next);
    }
}

/// Return whether an alias hop's target republishes the same callable projection, as a re-export chain does.
///
/// Alias projections record the declaration a chain ultimately reaches, not the hop each link was written on. Comparing
/// a link against the terminal path rejects every chain longer than one alias, so compare what the two links can
/// honestly agree on instead: the declaration whose projection they both carry.
pub(super) fn alias_target_republishes_projection(
    api: &crate::api_metadata::CheckedApiMetadataPackage,
    owning_module: &[String],
    target_path: &[String],
    projected: &ApiProjectedFunction,
) -> bool {
    let resolved = resolve_api_alias_target(owning_module, target_path);
    let Some((declaration_name, module_path)) = resolved.split_last() else {
        return false;
    };
    api.modules
        .iter()
        .find(|module| module.module_path == module_path)
        .is_some_and(|module| {
            module.declarations.iter().any(|declaration| {
                matches!(
                    declaration,
                    ApiDeclaration::Alias(target)
                        if target.name == *declaration_name
                            && target
                                .projected_function
                                .as_ref()
                                .is_some_and(|hop| hop.source_path == projected.source_path)
                )
            })
        })
}

/// Require a package-local alias target to name the exact checked declaration and source span.
pub(super) fn package_local_identity_target_matches(
    raw: &RawLibraryManifest,
    api: &crate::api_metadata::CheckedApiMetadataPackage,
    identity: &CanonicalIdentityExport,
) -> bool {
    let CanonicalIdentityOriginExport::Package { library, module_path } = &identity.origin else {
        return true;
    };
    if library != &raw.name {
        return true;
    }
    api.modules
        .iter()
        .find(|module| module.module_path == *module_path)
        .is_some_and(|module| {
            module.declarations.iter().any(|declaration| match declaration {
                ApiDeclaration::Partial(partial) => api_declaration_matches_canonical(
                    raw,
                    &module.module_path,
                    &partial.name,
                    "partial",
                    &partial.anchor,
                    identity,
                ),
                declaration => api_declaration_identity_parts(declaration).is_some_and(|(name, kind, anchor)| {
                    api_declaration_matches_canonical(raw, &module.module_path, name, kind, anchor, identity)
                }),
            })
        })
}

/// Bind a local checked API declaration to the exact canonical name, kind, module, and source span it authorizes.
pub(super) fn api_declaration_matches_canonical(
    raw: &RawLibraryManifest,
    module_path: &[String],
    name: &str,
    kind: &str,
    anchor: &crate::api_metadata::SourceAnchor,
    identity: &CanonicalIdentityExport,
) -> bool {
    identity.declaration_name == name
        && identity.kind == kind
        && usize::try_from(identity.declaration_span.start).ok() == Some(anchor.span.start)
        && usize::try_from(identity.declaration_span.end).ok() == Some(anchor.span.end)
        && identity.declaration_span.expansion == anchor.span.expansion
        && matches!(
            &identity.origin,
            CanonicalIdentityOriginExport::Package {
                library,
                module_path: identity_module_path,
            } if library == &raw.name && identity_module_path == module_path
        )
}

/// Return the canonical declaration fields represented by one non-projection checked API declaration.
pub(super) fn api_declaration_identity_parts(
    declaration: &ApiDeclaration,
) -> Option<(&str, &str, &crate::api_metadata::SourceAnchor)> {
    match declaration {
        ApiDeclaration::Function(value) => Some((&value.name, "function", &value.anchor)),
        ApiDeclaration::Model(value) => Some((&value.name, "model", &value.anchor)),
        ApiDeclaration::Class(value) => Some((&value.name, "class", &value.anchor)),
        ApiDeclaration::Trait(value) => Some((&value.name, "trait", &value.anchor)),
        ApiDeclaration::Enum(value) => Some((&value.name, "enum", &value.anchor)),
        ApiDeclaration::Newtype(value) => Some((
            &value.name,
            if value.is_rusttype { "rusttype" } else { "newtype" },
            &value.anchor,
        )),
        ApiDeclaration::TypeAlias(value) => Some((&value.name, "type_alias", &value.anchor)),
        ApiDeclaration::Const(value) => Some((&value.name, "const", &value.anchor)),
        ApiDeclaration::Static(value) => Some((&value.name, "static", &value.anchor)),
        ApiDeclaration::Alias(_) | ApiDeclaration::Partial(_) => None,
    }
}

/// Map one non-projection API declaration to its identity-graph kind.
pub(super) fn api_declaration_export_kind(declaration: &ApiDeclaration) -> Option<ExportIdentityKind> {
    match declaration {
        ApiDeclaration::Function(_) => Some(ExportIdentityKind::Function),
        ApiDeclaration::Model(_) => Some(ExportIdentityKind::Model),
        ApiDeclaration::Class(_) => Some(ExportIdentityKind::Class),
        ApiDeclaration::Trait(_) => Some(ExportIdentityKind::Trait),
        ApiDeclaration::Enum(_) => Some(ExportIdentityKind::Enum),
        ApiDeclaration::Newtype(_) => Some(ExportIdentityKind::Newtype),
        ApiDeclaration::TypeAlias(_) => Some(ExportIdentityKind::TypeAlias),
        ApiDeclaration::Const(_) => Some(ExportIdentityKind::Const),
        ApiDeclaration::Static(_) => Some(ExportIdentityKind::Static),
        ApiDeclaration::Alias(_) | ApiDeclaration::Partial(_) => None,
    }
}

/// Check that a source-path encoding names the same declaration as the canonical identity behind it.
///
/// The frontend records an export's path as *the source spelled it at the reference site*. A canonical identity
/// records *the declaration site*. Those are deliberately different things, and the module prefix in front of the
/// declaration name is exactly where they diverge:
///
/// - an absolute import spells `["crate", "feature", "Item"]` where the identity resolves to module `["feature"]`;
/// - a relative import spells `["super", "items", "LineItem"]`, and resolving it needs the importing module, which a
///   manifest-only comparison does not have;
/// - a bare sibling import inside a nested module spells `["c", "X"]` where the identity resolves to `["a", "c"]`;
/// - a re-export chain spells the hop it was written on, `["pricing", "LineItem"]`, while the identity stays anchored
///   at the original declaration in `["items"]`;
/// - a facade re-export from another package spells the facade rather than the upstream's declaring module.
///
/// Every one of those is a correct program, and asserting prefix equality rejected all of them. That is the same
/// spelling-is-not-identity mistake the identity model exists to remove, so this compares only what a path can
/// honestly prove: that it names the declaration the identity names, or that it names another export of this package
/// that carries the very same identity. The second case is a projection over a projection -- re-exporting an alias
/// publishes the alias's path, whose last segment is the local name the alias chose, not the declaration it renames.
/// Structural agreement between the identity graph and the raw exports is enforced separately by the per-projection
/// checks in `validate_export_identity_binding` and by the API-declaration backing checks, and consumers resolve by
/// identity rather than by these strings.
pub(super) fn canonical_identity_matches_path(
    siblings: &[super::ExportIdentity],
    identity: &CanonicalIdentityExport,
    path: &[String],
) -> bool {
    // `pub from crate.public_api import PublicVault as ExportedVault` writes the package root explicitly, so the
    // projection path it records carries a leading `crate` segment. A public path is rooted at the package name and
    // never has one, so compare the module tail the two records actually share.
    let path = match path.split_first() {
        Some((root, rest)) if root == "crate" => rest,
        _ => path,
    };
    if path.last() == Some(&identity.declaration_name) {
        return true;
    }
    // The path may name a projection hop rather than the declaration. A re-export of an alias publishes the alias's
    // path -- `["provider", "run"]` -- while the identity behind it is the declaration the alias renames, `helper`.
    // That is the intended model: a local name for a declaration does not become a second declaration. Accept the
    // path when this package publishes it as an export carrying this same identity; that hop's own graph entry
    // proves its binding, so the chain stays validated one hop at a time instead of demanding that the last hop
    // spell a name it deliberately replaced.
    siblings.iter().any(|sibling| {
        sibling.public_path.len() > 1
            && &sibling.public_path[1..] == path
            && sibling.canonical.as_ref() == Some(identity)
    })
}

/// Validate all canonical field identities exported for one nominal declaration.
pub(super) fn validate_nominal_member_identities(
    owner: &str,
    owner_identity: Option<&CanonicalIdentityExport>,
    fields: &[super::FieldExport],
    properties: &[super::PropertyExport],
    methods: &[super::MethodExport],
    required: bool,
    owner_inherits_members: bool,
) -> Result<(), LibraryManifestError> {
    let mut seen = BTreeSet::new();
    for field in fields {
        validate_record_field_metadata(owner, field)?;
        validate_member_identity(
            &format!("{owner} field `{}`", field.name),
            &field.name,
            field.canonical.as_ref(),
            "field",
            owner_identity,
            required,
            owner_inherits_members,
        )?;
        record_member_identity(owner, &field.name, field.canonical.as_ref(), &mut seen)?;
    }
    for property in properties {
        validate_member_identity(
            &format!("{owner} property `{}`", property.name),
            &property.name,
            property.canonical.as_ref(),
            "property",
            owner_identity,
            required,
            owner_inherits_members,
        )?;
        record_member_identity(owner, &property.name, property.canonical.as_ref(), &mut seen)?;
    }
    validate_method_identities(
        owner,
        owner_identity,
        methods,
        required,
        Some(&mut seen),
        owner_inherits_members,
    )
}

/// Validate additive stored-record metadata without refusing an older manifest that omitted every new field.
fn validate_record_field_metadata(owner: &str, field: &super::FieldExport) -> Result<(), LibraryManifestError> {
    if field.default_is_empty && !field.has_default
        || field.renamed_from_lifecycle.is_some() && field.renamed_from.is_empty()
    {
        return Err(LibraryManifestError::Invalid(format!(
            "{owner} field `{}` carries default or lifecycle facts without their owning declaration metadata",
            field.name
        )));
    }
    if field.record_identity.is_empty() {
        if !field.renamed_from.is_empty() || !field.record_canonical {
            return Err(LibraryManifestError::Invalid(format!(
                "{owner} field `{}` carries record metadata without its derived identity",
                field.name
            )));
        }
        return Ok(());
    }
    let expected = hex::encode(field.expected_record_identity());
    if field.record_identity != expected {
        return Err(LibraryManifestError::Invalid(format!(
            "{owner} field `{}` record identity must be the lowercase hexadecimal identity of its earliest name",
            field.name
        )));
    }
    Ok(())
}

/// Validate all canonical method identities exported for one nominal declaration.
pub(super) fn validate_method_identities(
    owner: &str,
    owner_identity: Option<&CanonicalIdentityExport>,
    methods: &[super::MethodExport],
    required: bool,
    mut shared_seen: Option<&mut BTreeSet<(String, CanonicalIdentityExport)>>,
    owner_inherits_members: bool,
) -> Result<(), LibraryManifestError> {
    let mut seen = BTreeSet::new();
    for method in methods {
        let expected_name = method.alias_of.as_deref().unwrap_or(&method.name);
        validate_member_identity(
            &format!("{owner} method `{}`", method.name),
            expected_name,
            method.canonical.as_ref(),
            "method",
            owner_identity,
            required,
            owner_inherits_members,
        )?;
        if let Some(identity) = &method.canonical
            && !seen.insert((method.name.clone(), identity))
        {
            return Err(LibraryManifestError::Invalid(format!(
                "{owner} contains duplicate canonical method identity `{}`",
                method.name
            )));
        }
        if let Some(shared_seen) = shared_seen.as_deref_mut() {
            record_member_identity(owner, &method.name, method.canonical.as_ref(), shared_seen)?;
        }
    }
    Ok(())
}

/// Validate one member identity against its owning root declaration and public metadata.
pub(super) fn validate_member_identity(
    owner: &str,
    expected_name: &str,
    identity: Option<&CanonicalIdentityExport>,
    expected_kind: &str,
    owner_identity: Option<&CanonicalIdentityExport>,
    required: bool,
    owner_inherits_members: bool,
) -> Result<(), LibraryManifestError> {
    let Some(identity) = identity else {
        return if required {
            Err(LibraryManifestError::Invalid(format!(
                "{owner} is missing its canonical member identity"
            )))
        } else {
            Ok(())
        };
    };
    if !required {
        return Err(LibraryManifestError::Invalid(format!(
            "{owner} cannot publish canonical member metadata in schema v1"
        )));
    }
    validate_canonical_identity(identity, owner, true)?;
    if identity.kind != expected_kind {
        return Err(LibraryManifestError::Invalid(format!(
            "{owner} publishes canonical kind `{}` instead of `{expected_kind}`",
            identity.kind
        )));
    }
    if identity.declaration_name != expected_name {
        return Err(LibraryManifestError::Invalid(format!(
            "{owner} publishes canonical declaration name `{}` instead of `{expected_name}`",
            identity.declaration_name
        )));
    }
    let Some(owner_identity) = owner_identity else {
        return Err(LibraryManifestError::Invalid(format!(
            "{owner} has no canonical owner export"
        )));
    };
    // A member this owner declares is anchored inside it: same origin, span nested within the declaration.
    let anchored_in_owner = identity.origin == owner_identity.origin
        && identity.declaration_span.start >= owner_identity.declaration_span.start
        && identity.declaration_span.end <= owner_identity.declaration_span.end;
    if anchored_in_owner {
        return Ok(());
    }

    // An inherited member is anchored at the ancestor that declared it, so it sits outside this owner and may live in
    // another module entirely. That is the identity model working as intended: a member keeps the identity minted at
    // its declaration site instead of acquiring a fresh one per subclass, and the declaring class proves that identity
    // through its own export. Requiring containment unconditionally rejected every public class that inherits, because
    // class collection clones the parent's fields, properties, and methods together with the parent's identities.
    //
    // Only a declared inheritance admits a foreign anchor. An owner that inherits nothing must still contain every
    // member it publishes, which keeps this check meaningful for models, traits, newtypes, and standalone classes.
    if owner_inherits_members {
        return Ok(());
    }
    if identity.origin != owner_identity.origin {
        return Err(LibraryManifestError::Invalid(format!(
            "{owner} publishes a canonical origin different from its owner declaration"
        )));
    }
    Err(LibraryManifestError::Invalid(format!(
        "{owner} publishes a canonical declaration span outside its owner declaration"
    )))
}

/// Record one member identity and reject duplicate canonical members on the same owner.
pub(super) fn record_member_identity(
    owner: &str,
    public_name: &str,
    identity: Option<&CanonicalIdentityExport>,
    seen: &mut BTreeSet<(String, CanonicalIdentityExport)>,
) -> Result<(), LibraryManifestError> {
    if let Some(identity) = identity
        && !seen.insert((public_name.to_string(), identity.clone()))
    {
        return Err(LibraryManifestError::Invalid(format!(
            "{owner} contains duplicate canonical member identity `{}`",
            identity.declaration_name
        )));
    }
    Ok(())
}

/// Find the canonical root identity for one top-level exported nominal declaration.
pub(super) fn root_canonical_identity<'a>(
    raw: &'a RawLibraryManifest,
    kind: ExportIdentityKind,
    name: &str,
) -> Option<&'a CanonicalIdentityExport> {
    raw.contract_metadata
        .identity_graph
        .exports
        .iter()
        .find(|entry| entry.public_path.len() == 2 && entry.kind == kind && entry.public_name == name)
        .and_then(|entry| entry.canonical.as_ref())
}

/// Validate the structural invariants shared by root and member canonical identities.
pub(super) fn validate_canonical_identity(
    identity: &CanonicalIdentityExport,
    owner: &str,
    member: bool,
) -> Result<(), LibraryManifestError> {
    if identity.declaration_name.trim().is_empty() {
        return Err(LibraryManifestError::Invalid(format!(
            "{owner} has an empty canonical declaration name"
        )));
    }
    if identity.declaration_span.end < identity.declaration_span.start {
        return Err(LibraryManifestError::Invalid(format!(
            "{owner} has an inverted canonical declaration span"
        )));
    }
    if matches!(
        SemanticSourceTargetKind::from_kind_str(&identity.kind),
        SemanticSourceTargetKind::Other(_)
    ) {
        return Err(LibraryManifestError::Invalid(format!(
            "{owner} has unknown canonical declaration kind `{}`",
            identity.kind
        )));
    }
    let expected_namespace = if member {
        CanonicalIdentityNamespaceExport::Member
    } else {
        CanonicalIdentityNamespaceExport::OrdinaryLexical
    };
    if identity.namespace != expected_namespace {
        return Err(LibraryManifestError::Invalid(format!(
            "{owner} has canonical namespace `{:?}` instead of `{:?}`",
            identity.namespace, expected_namespace
        )));
    }
    match &identity.origin {
        CanonicalIdentityOriginExport::Package { library, .. } if library.trim().is_empty() => {
            return Err(LibraryManifestError::Invalid(format!(
                "{owner} has an empty canonical package origin"
            )));
        }
        CanonicalIdentityOriginExport::RustCrate { path } if path.is_empty() => {
            return Err(LibraryManifestError::Invalid(format!(
                "{owner} has an empty canonical Rust origin"
            )));
        }
        _ => {}
    }
    if identity.hydrate().is_none() {
        return Err(LibraryManifestError::Invalid(format!(
            "{owner} cannot hydrate its canonical identity on this compiler target"
        )));
    }
    Ok(())
}
