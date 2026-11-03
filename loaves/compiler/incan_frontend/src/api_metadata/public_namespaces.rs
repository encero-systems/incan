//! API alias projections, public namespace materialization, and alias target resolution.

use super::*;

/// Attach checked function projections to public aliases that target decorated or ordinary public functions.
///
/// Metadata package consumers should not need to force producer module initialization just to discover declaration-side
/// decorator facts. This projection pass resolves aliases across the already checked API package and carries the target
/// function's decorators and checked callable shape onto facade aliases.
pub fn materialize_api_alias_projections(modules: &mut [CheckedApiMetadata]) {
    let mut projections = HashMap::new();
    let mut type_projections = HashMap::new();
    let mut aliases = Vec::new();

    for module in modules.iter() {
        for declaration in &module.declarations {
            match declaration {
                ApiDeclaration::Function(function) => {
                    projections.insert(
                        declaration_path(&module.module_path, &function.name),
                        ApiProjectedFunction {
                            source_path: declaration_path(&module.module_path, &function.name),
                            callable: callable_from_function(function),
                            decorators: function.decorators.clone(),
                        },
                    );
                }
                ApiDeclaration::TypeAlias(alias) if alias.type_alias.target.has_native_union() => {
                    type_projections.insert(
                        declaration_path(&module.module_path, &alias.name),
                        alias.type_alias.target.clone(),
                    );
                }
                ApiDeclaration::Alias(alias) => {
                    let path = declaration_path(&module.module_path, &alias.name);
                    // External projections are already bound to admitted artifacts. Local aliases are refreshed
                    // from their current declaration after native representation publication updates that target.
                    if alias
                        .target_path
                        .first()
                        .is_some_and(|root| matches!(root.as_str(), "pub" | "std"))
                    {
                        if let Some(function) = &alias.projected_function {
                            projections.insert(path.clone(), function.clone());
                        }
                        if let Some(ty) = &alias.projected_type
                            && ty.has_native_union()
                        {
                            type_projections.insert(path.clone(), ty.clone());
                        }
                    }
                    aliases.push(ApiAliasProjectionRequest {
                        path: declaration_path(&module.module_path, &alias.name),
                        target_path: resolve_api_alias_target(&module.module_path, &alias.target_path),
                        name: alias.name.clone(),
                        anchor: alias.anchor.clone(),
                    });
                }
                _ => {}
            }
        }
    }

    let mut changed = true;
    while changed {
        changed = false;
        for alias in &aliases {
            if projections.contains_key(&alias.path) && type_projections.contains_key(&alias.path) {
                continue;
            }
            // An alias records its target as the source spelled it -- `pub run = alias helper` inside `provider`
            // records `["helper"]` -- while projections are keyed by resolved declaration path, so look the target up
            // under the path its spelling resolves to. The alias's recorded `target_path` is deliberately left alone:
            // it is compared against the identity graph downstream, and rewriting it here would move that comparison
            // rather than fix this one.
            let resolved = projections.get(&alias.target_path);
            if !projections.contains_key(&alias.path)
                && let Some(target) = resolved
            {
                let projection = projected_function_for_alias(alias, target);
                projections.insert(alias.path.clone(), projection);
                changed = true;
            }
            if !type_projections.contains_key(&alias.path) {
                let target = type_projections.get(&alias.target_path).cloned();
                if let Some(target) = target {
                    type_projections.insert(alias.path.clone(), target);
                    changed = true;
                }
            }
        }
    }

    for module in modules {
        for declaration in &mut module.declarations {
            if let ApiDeclaration::Alias(alias) = declaration {
                let alias_path = declaration_path(&module.module_path, &alias.name);
                alias.projected_function = projections.get(&alias_path).cloned();
                if let Some(ty) = type_projections.get(&alias_path) {
                    alias.projected_type = Some(ty.clone());
                }
            }
        }
    }
}

/// Whether a checked API module path names the package's root module, whose declarations the package publishes under
/// its own name rather than under a module namespace.
pub(crate) fn is_package_root_module(module_path: &[String]) -> bool {
    matches!(module_path, [root] if root == "lib" || root == "main")
}

/// Materialize the deterministic public namespace graph owned by one checked package API.
pub fn materialize_checked_api_public_namespaces(
    api: &mut CheckedApiMetadataPackage,
) -> Result<(), CheckedApiPublicNamespaceError> {
    let mut module_paths = HashSet::new();
    for module in &api.modules {
        if !module_paths.insert(module.module_path.clone()) {
            return Err(CheckedApiPublicNamespaceError::DuplicateSourceModule {
                module_path: module.module_path.clone(),
            });
        }
    }

    let mut namespaces = HashMap::<Vec<String>, CheckedApiPublicNamespace>::new();
    for module in &api.modules {
        if is_package_root_module(&module.module_path) {
            continue;
        }
        for len in 1..=module.module_path.len() {
            let path = module.module_path[..len].to_vec();
            namespaces
                .entry(path.clone())
                .or_insert_with(|| CheckedApiPublicNamespace {
                    path,
                    members: Vec::new(),
                    child_modules: Vec::new(),
                });
            if len > 1 {
                let parent = module.module_path[..len - 1].to_vec();
                let child = module.module_path[len - 1].clone();
                namespaces
                    .entry(parent.clone())
                    .or_insert_with(|| CheckedApiPublicNamespace {
                        path: parent,
                        members: Vec::new(),
                        child_modules: Vec::new(),
                    })
                    .child_modules
                    .push(child);
            }
        }
        for declaration in module
            .declarations
            .iter()
            .filter(|declaration| checked_api_declaration_is_public_namespace_member(declaration))
        {
            let Some(name) = api_declaration_public_name(declaration) else {
                continue;
            };
            let mut source_path = module.module_path.clone();
            source_path.push(name.to_string());
            let member = CheckedApiPublicNamespaceMember {
                name: name.to_string(),
                source_path,
            };
            let namespace = namespaces.get_mut(&module.module_path).ok_or_else(|| {
                CheckedApiPublicNamespaceError::MissingMaterializedNamespace {
                    module_path: module.module_path.clone(),
                }
            })?;
            namespace.members.push(member.clone());
            if module.module_path.len() > 1 {
                let parent_path = module.module_path[..module.module_path.len() - 1].to_vec();
                let parent = namespaces.get_mut(parent_path.as_slice()).ok_or({
                    CheckedApiPublicNamespaceError::MissingMaterializedNamespace {
                        module_path: parent_path,
                    }
                })?;
                parent.members.push(member);
            }
        }
    }
    let mut namespaces = namespaces.into_values().collect::<Vec<_>>();
    for namespace in &mut namespaces {
        namespace
            .members
            .sort_by(|left, right| (&left.name, &left.source_path).cmp(&(&right.name, &right.source_path)));
        namespace.members.dedup();
        namespace.child_modules.sort();
        namespace.child_modules.dedup();
    }
    namespaces.sort_by(|left, right| left.path.cmp(&right.path));
    api.public_namespaces = namespaces;
    Ok(())
}

/// Validate that a serialized namespace projection exactly matches its checked declaration modules.
pub fn validate_checked_api_public_namespaces(
    api: &CheckedApiMetadataPackage,
) -> Result<(), CheckedApiPublicNamespaceError> {
    if api.public_namespaces.is_empty() {
        return Ok(());
    }
    let serialized = api.public_namespaces.clone();
    let mut derived = api.clone();
    derived.public_namespaces.clear();
    materialize_checked_api_public_namespaces(&mut derived)?;
    if serialized != derived.public_namespaces {
        return Err(CheckedApiPublicNamespaceError::SerializedProjectionMismatch);
    }
    Ok(())
}

/// Return every materialized public module path carried by one checked package API.
pub fn checked_api_public_module_paths(api: &CheckedApiMetadataPackage) -> Vec<Vec<String>> {
    if !api.public_namespaces.is_empty() {
        return api
            .public_namespaces
            .iter()
            .map(|namespace| namespace.path.clone())
            .collect();
    }
    let mut compatibility = api.clone();
    materialize_checked_api_public_namespaces(&mut compatibility)
        .map(|()| {
            compatibility
                .public_namespaces
                .into_iter()
                .map(|namespace| namespace.path)
                .collect()
        })
        .unwrap_or_default()
}

/// Return source API modules whose public declarations participate directly in one materialized namespace.
pub fn checked_api_modules_for_public_namespace<'a>(
    api: &'a CheckedApiMetadataPackage,
    module_path: &[String],
) -> Vec<&'a CheckedApiMetadata> {
    let source_modules = checked_api_public_namespace(api, module_path)
        .map(|namespace| {
            namespace
                .members
                .iter()
                .filter_map(|member| member.source_path.get(..member.source_path.len().saturating_sub(1)))
                .map(<[String]>::to_vec)
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();
    api.modules
        .iter()
        .filter(|module| source_modules.contains(&module.module_path))
        .collect()
}

/// Return one materialized namespace, deriving a compatibility projection only for older artifacts.
pub fn checked_api_public_namespace(
    api: &CheckedApiMetadataPackage,
    module_path: &[String],
) -> Option<CheckedApiPublicNamespace> {
    if let Some(namespace) = api
        .public_namespaces
        .iter()
        .find(|namespace| namespace.path == module_path)
    {
        return Some(namespace.clone());
    }
    let mut compatibility = api.clone();
    materialize_checked_api_public_namespaces(&mut compatibility).ok()?;
    compatibility
        .public_namespaces
        .into_iter()
        .find(|namespace| namespace.path == module_path)
}

/// Return whether checked metadata authorizes a declaration in an automatic package namespace.
pub fn checked_api_declaration_is_public_namespace_member(declaration: &ApiDeclaration) -> bool {
    match declaration {
        ApiDeclaration::Alias(alias) => alias.is_public,
        _ => true,
    }
}

/// Return the public spelling carried by one checked API declaration.
pub fn api_declaration_public_name(declaration: &ApiDeclaration) -> Option<&str> {
    match declaration {
        ApiDeclaration::Function(item) => Some(&item.name),
        ApiDeclaration::Model(item) => Some(&item.name),
        ApiDeclaration::Class(item) => Some(&item.name),
        ApiDeclaration::Trait(item) => Some(&item.name),
        ApiDeclaration::Enum(item) => Some(&item.name),
        ApiDeclaration::Newtype(item) => Some(&item.name),
        ApiDeclaration::TypeAlias(item) => Some(&item.name),
        ApiDeclaration::Const(item) => Some(&item.name),
        ApiDeclaration::Static(item) => Some(&item.name),
        ApiDeclaration::Alias(item) => Some(&item.name),
        ApiDeclaration::Partial(item) => Some(&item.name),
    }
}

/// Build the API declaration path for a module-local name.
pub(super) fn declaration_path(module_path: &[String], name: &str) -> Vec<String> {
    let mut path = module_path.to_vec();
    path.push(name.to_string());
    path
}

/// Resolve an alias target, as the source spelled it, to the declaration path the checked API keys it by.
///
/// Checked API modules, projections and namespace members are keyed by resolved declaration paths, while an alias
/// records its target as written in the module that declares it. Two spellings differ from the path they name:
///
/// - a bare name names a declaration of the owning module -- `pub run = alias helper` inside `provider` records
///   `["helper"]` for `["provider", "helper"]`;
/// - a `crate.` path starts at the package root -- `pub from crate.inner import greet` records `["crate", "inner",
///   "greet"]` for `["inner", "greet"]`.
///
/// Any other path is read from the package root as written, the reading a consumer of the published manifest applies
/// to the same target. The alias projections and the manifest's check that a namespace alias backs its identity-graph
/// entry both resolve through this function, so the two cannot read one spelling differently.
pub(crate) fn resolve_api_alias_target(owning_module: &[String], target_path: &[String]) -> Vec<String> {
    match target_path {
        [name] => declaration_path(owning_module, name),
        [root, rest @ ..] if root == "crate" && !rest.is_empty() => rest.to_vec(),
        _ => target_path.to_vec(),
    }
}

/// Build callable metadata from a checked API function export.
pub(super) fn callable_from_function(function: &ApiFunction) -> ApiCallableMetadata {
    ApiCallableMetadata {
        ownership: function.ownership.clone(),
        name: function.name.clone(),
        anchor: function.anchor.clone(),
        type_params: function.type_params.clone(),
        receiver: None,
        params: function.params.clone(),
        return_type: function.return_type.clone(),
        is_async: function.is_async,
    }
}

/// Build projected callable metadata for an alias re-export.
pub(super) fn projected_function_for_alias(
    alias: &ApiAliasProjectionRequest,
    target: &ApiProjectedFunction,
) -> ApiProjectedFunction {
    let mut projected = target.clone();
    projected.callable.name = alias.name.clone();
    projected.callable.anchor = alias.anchor.clone();
    projected
}

/// Look up the checked export kind for a public name.
pub(super) fn checked_kind<'a>(
    exports: &'a HashMap<String, CheckedNamedExport>,
    name: &str,
) -> Option<&'a CheckedExportKind> {
    exports.get(name).map(|export| &export.kind)
}

/// Return whether a declaration visibility is public.
pub(super) fn public(visibility: Visibility) -> bool {
    matches!(visibility, Visibility::Public)
}
