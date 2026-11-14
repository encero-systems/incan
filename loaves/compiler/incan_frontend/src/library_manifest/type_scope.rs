//! A Loaf's nominal type scope as records of their own (RFC 179): for each module of the checked API, the type or
//! trait declaration every spelling in it names, so a consumer that needs how one module spells its types reads that
//! module's record by its path, never the Loaf's whole checked API.

use std::collections::{BTreeMap, HashSet};

use incan_lang::lang::standard_packages::standard_package_namespace_policy;
use serde::{Deserialize, Serialize};

use super::LibraryIdentityGraph;
use crate::api_metadata::{
    ApiDeclaration, CheckedApiMetadataPackage, is_package_root_module, resolve_api_alias_target,
};

/// One module's type scope: each spelling among its declarations that names a type or trait declaration of the Loaf.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleTypeScope {
    /// The module's path below the package root.
    pub module: Vec<String>,
    /// Whether the spellings were followed as a standard package's: with the roots the compiler's namespace policy
    /// gives the package read as its own below `std`. A consumer that binds the Loaf otherwise reads its manifest.
    pub standard: bool,
    /// The declaration each spelling names, by spelling.
    pub names: BTreeMap<String, DeclaredType>,
}

/// The type or trait declaration one spelling names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclaredType {
    /// The declaring module's path below the package root.
    pub module: Vec<String>,
    /// The declared name.
    pub name: String,
    /// Whether the spelling belongs to the module's declaration scope: a submodule declares it, the Loaf publishes
    /// that declaration, and the package root does not publish the same declaration under the spelling. Any other
    /// spelling is read through the package root's public names.
    pub scoped: bool,
}

/// The type scope of every module of `api`, in module order. `package` is the Loaf's package name, which roots the
/// public paths of `graph`.
pub fn module_type_scopes(
    api: &CheckedApiMetadataPackage,
    graph: &LibraryIdentityGraph,
    package: &str,
) -> Vec<ModuleTypeScope> {
    let public_path = |segments: &[String], name: &str| {
        std::iter::once(package.to_owned())
            .chain(segments.iter().cloned())
            .chain(std::iter::once(name.to_owned()))
            .collect::<Vec<_>>()
    };
    api.modules
        .iter()
        .map(|module| {
            let mut names = BTreeMap::new();
            for declaration in &module.declarations {
                let spelling = declaration.name();
                let Some((declaring, declared)) = nominal_declaration_path(api, package, &module.module_path, spelling)
                else {
                    continue;
                };
                let published = graph.canonical_for_public_path(&public_path(&declaring, &declared));
                let scoped = !is_package_root_module(&declaring)
                    && published.is_some()
                    && graph.canonical_for_public_path(&public_path(&[], spelling)) != published;
                names.insert(
                    spelling.to_owned(),
                    DeclaredType {
                        module: declaring,
                        name: declared,
                        scoped,
                    },
                );
            }
            ModuleTypeScope {
                module: module.module_path.clone(),
                standard: standard_package_namespace_policy(package).is_some(),
                names,
            }
        })
        .collect()
}

/// Follow the spelling `name` in the module `module_path` of `api` to its type or trait declaration, returning the
/// declaring module and the declared name.
///
/// An alias, public or private, is followed to its target as the module wrote it, through as many modules as the chain
/// crosses. A standard package spells its own modules below `std`, which its manifest's module paths leave out; the
/// compiler's namespace policy for `package` says which roots are its own. A spelling that reaches no type or trait
/// declaration of this package, such as an import from another package, a function or a cycle, yields `None`.
pub fn nominal_declaration_path(
    api: &CheckedApiMetadataPackage,
    package: &str,
    module_path: &[String],
    name: &str,
) -> Option<(Vec<String>, String)> {
    let own_roots = standard_package_namespace_policy(package).map(|policy| policy.namespace_roots.as_slice());
    let mut module_path = module_path.to_vec();
    let mut name = name.to_owned();
    let mut visited = HashSet::new();
    loop {
        if !visited.insert((module_path.clone(), name.clone())) {
            return None;
        }
        let module = api.modules.iter().find(|module| module.module_path == module_path)?;
        let declaration = module
            .declarations
            .iter()
            .filter(|declaration| declaration.name() == name)
            .min_by_key(|declaration| matches!(declaration, ApiDeclaration::Alias(_)))?;
        match declaration {
            ApiDeclaration::Model(_)
            | ApiDeclaration::Class(_)
            | ApiDeclaration::Enum(_)
            | ApiDeclaration::Newtype(_)
            | ApiDeclaration::TypeAlias(_)
            | ApiDeclaration::Trait(_) => return Some((module_path, name)),
            ApiDeclaration::Alias(alias) => {
                let target = resolve_api_alias_target(&module_path, &alias.target_path);
                let target = match (own_roots, target.as_slice()) {
                    (Some(roots), [std, root, ..]) if std == "std" && roots.contains(&root.as_str()) => &target[1..],
                    _ => target.as_slice(),
                };
                let (target_name, target_module) = target.split_last()?;
                name = target_name.clone();
                module_path = target_module.to_vec();
            }
            _ => return None,
        }
    }
}
