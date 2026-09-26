//! Declared type-parameter bounds of the standard library's public bounded models and classes (#1280).
//!
//! A consumer's signature that names such a model with its own type parameter must declare the same bounds, and the
//! checker refuses one that does not. The bounds are read from the library's source here, in the same terms a
//! consumer's own bound on the trait resolves to, so the two can be compared.

use std::collections::{HashMap, HashSet};

use crate::ast;
use crate::symbols::TypeBoundInfo;
use crate::typechecker::nominal_type_param_bounds::builtin_bound;
use incan_lang::lang::stdlib;
use incan_lang::lang::traits as builtin_traits;

use super::StdlibAstCache;

/// The declared bounds of one nominal's type parameters, in declaration order: `(parameter, bounds)`.
type DeclaredBounds = Vec<(String, Vec<TypeBoundInfo>)>;

impl StdlibAstCache {
    /// Look up the declared plain trait bounds of a public bounded stdlib model or class, `(type parameter, bounds)`
    /// in declaration order; empty when the type declares none the checker can compare (#1280).
    pub fn lookup_type_param_bounds(&mut self, module_path: &[String], type_name: &str) -> DeclaredBounds {
        self.ensure_loaded(module_path);
        let key = module_path.join(".");
        self.cache
            .get(&key)
            .and_then(|data| data.type_param_bounds.get(type_name))
            .cloned()
            .unwrap_or_default()
    }
}

/// Collect the declared plain trait bounds of a stdlib module's public bounded models and classes (#1280).
///
/// A bound is recorded when it names a builtin trait (`Clone`, `Eq`, `Ord`, ...), whose identity a consumer's own
/// bound resolves to in the same terms, or a trait this module declares or imports from another stdlib module, with
/// that module and the trait's declaration name as its identity. A bound on any other trait is left out, so a
/// consumer's bound is never compared against an identity the two could not share.
pub(super) fn extract_type_param_bounds(
    program: &ast::Program,
    module_path: &[String],
) -> HashMap<String, DeclaredBounds> {
    let local_traits: HashSet<&str> = program
        .declarations
        .iter()
        .filter_map(|declaration| match &declaration.node {
            ast::Declaration::Trait(trait_decl) => Some(trait_decl.name.as_str()),
            _ => None,
        })
        .collect();
    let imported_traits = stdlib_imported_names(program);

    let mut bounds_by_type = HashMap::new();
    for declaration in &program.declarations {
        let (type_name, type_params) = match &declaration.node {
            ast::Declaration::Model(model) if model.visibility == ast::Visibility::Public => {
                (&model.name, &model.type_params)
            }
            ast::Declaration::Class(class) if class.visibility == ast::Visibility::Public => {
                (&class.name, &class.type_params)
            }
            _ => continue,
        };
        let bounds: DeclaredBounds = type_params
            .iter()
            .map(|param| {
                let bounds = param
                    .bounds
                    .iter()
                    .filter(|bound| bound.type_args.is_empty())
                    .filter_map(|bound| {
                        let (trait_module, trait_name) = if local_traits.contains(bound.name.as_str()) {
                            (module_path.to_vec(), bound.name.clone())
                        } else if let Some(imported) = imported_traits.get(&bound.name) {
                            imported.clone()
                        } else if builtin_traits::from_str(&bound.name).is_some() {
                            return Some(builtin_bound(&bound.name));
                        } else {
                            return None;
                        };
                        Some(TypeBoundInfo {
                            name: bound.name.clone(),
                            source_name: Some(trait_name),
                            type_args: Vec::new(),
                            module_path: Some(trait_module),
                            implementation_type_params: Vec::new(),
                        })
                    })
                    .collect();
                (param.name.clone(), bounds)
            })
            .collect();
        if bounds.iter().any(|(_, bounds)| !bounds.is_empty()) {
            bounds_by_type.insert(type_name.clone(), bounds);
        }
    }
    bounds_by_type
}

/// Map each name a module imports from another stdlib module to `(module path, declaration name)`.
fn stdlib_imported_names(program: &ast::Program) -> HashMap<String, (Vec<String>, String)> {
    let mut imported = HashMap::new();
    for declaration in &program.declarations {
        let ast::Declaration::Import(import) = &declaration.node else {
            continue;
        };
        let ast::ImportKind::From { module, items } = &import.kind else {
            continue;
        };
        if module
            .segments
            .first()
            .is_none_or(|segment| segment != stdlib::STDLIB_ROOT)
        {
            continue;
        }
        for item in items {
            let local_name = item.alias.clone().unwrap_or_else(|| item.name.clone());
            imported.insert(local_name, (module.segments.clone(), item.name.clone()));
        }
    }
    imported
}
