//! Crate paths of the nominal types a module's checked types name without the module binding them (#1561).
//!
//! A module's checked types can name a type that another module declares and this module never imports: the field
//! `ids: list[EvidenceId]` of a model imported from `records`, where `EvidenceId` is declared in `ids`, gives the
//! argument `ids=[]` of that model's constructor the type `list[EvidenceId]`. Where the generated Rust spells such a
//! type (the element type of an empty list, the payload of a `None`), the bare name resolves to nothing in the
//! importing module (E0425). Lowering records, for each such name, the path of its declaration in the generated crate,
//! which the emitter spells instead of the bare name. The checked types themselves keep the bare name, which is the
//! identity every other backend lookup keys on.
//!
//! A name this module binds (its own declaration, an import under that name, a Rust import or a type parameter of one
//! of its declarations) keeps its spelling, and so does a name that no module of this crate declares, or that more
//! than one module declares, since the bare name cannot say which declaration it is.

use std::collections::{HashMap, HashSet};

use incan_frontend::ast;

use super::AstLowering;

impl AstLowering {
    /// Return the crate path of each nominal type of this crate that one other module declares and `program`, the
    /// module being lowered, does not bind, keyed by the type's name.
    ///
    /// Empty outside a crate of several modules, where every nominal type a module names is its own or an import.
    pub(in crate::lower) fn unbound_nominal_type_paths(&self, program: &ast::Program) -> HashMap<String, String> {
        let (Some(context), Some(info)) = (self.crate_nominal_context.as_deref(), self.type_info.as_ref()) else {
            return HashMap::new();
        };
        let bound = module_type_namespace_bindings(program);
        info.declarations
            .unique_nominal_declaring_modules
            .iter()
            .filter(|(name, _)| {
                !bound.contains(name.as_str())
                    && !self.import_aliases.contains_key(name.as_str())
                    && !self.rust_import_aliases.contains_key(name.as_str())
                    && info.resolved_import_identity(name).is_none()
            })
            .filter_map(|(name, logical_path)| {
                let rust_module_path = self.crate_module_rust_path(logical_path, context)?;
                Some((name.clone(), Self::crate_item_path(&rust_module_path, name)))
            })
            .collect()
    }
}

/// Return the names one module binds in the type namespace by declaring them: its models, classes, traits, enums,
/// newtypes, type aliases and symbol aliases, and every type parameter one of its declarations or methods introduces,
/// test modules included.
fn module_type_namespace_bindings(program: &ast::Program) -> HashSet<&str> {
    let mut bound = HashSet::new();
    collect_type_namespace_bindings(&program.declarations, &mut bound);
    bound
}

/// Collect the type-namespace names `declarations` bind into `bound` (see [`module_type_namespace_bindings`]).
fn collect_type_namespace_bindings<'a>(
    declarations: &'a [ast::Spanned<ast::Declaration>],
    bound: &mut HashSet<&'a str>,
) {
    let add_params = |bound: &mut HashSet<&'a str>, params: &'a [ast::TypeParam]| {
        bound.extend(params.iter().map(|param| param.name.as_str()));
    };
    let add_methods = |bound: &mut HashSet<&'a str>, methods: &'a [ast::Spanned<ast::MethodDecl>]| {
        for method in methods {
            bound.extend(method.node.type_params.iter().map(|param| param.name.as_str()));
        }
    };
    for declaration in declarations {
        match &declaration.node {
            ast::Declaration::Model(model) => {
                bound.insert(model.name.as_str());
                add_params(bound, &model.type_params);
                add_methods(bound, &model.methods);
            }
            ast::Declaration::Class(class) => {
                bound.insert(class.name.as_str());
                add_params(bound, &class.type_params);
                add_methods(bound, &class.methods);
            }
            ast::Declaration::Trait(trait_decl) => {
                bound.insert(trait_decl.name.as_str());
                add_params(bound, &trait_decl.type_params);
                add_methods(bound, &trait_decl.methods);
            }
            ast::Declaration::Enum(enum_decl) => {
                bound.insert(enum_decl.name.as_str());
                add_params(bound, &enum_decl.type_params);
                add_methods(bound, &enum_decl.methods);
            }
            ast::Declaration::Newtype(newtype) => {
                bound.insert(newtype.name.as_str());
                add_params(bound, &newtype.type_params);
                add_methods(bound, &newtype.methods);
            }
            ast::Declaration::TypeAlias(alias) => {
                bound.insert(alias.name.as_str());
                add_params(bound, &alias.type_params);
            }
            ast::Declaration::Alias(alias) => {
                bound.insert(alias.name.as_str());
            }
            ast::Declaration::Function(function) => add_params(bound, &function.type_params),
            ast::Declaration::TestModule(test_module) => collect_type_namespace_bindings(&test_module.body, bound),
            _ => {}
        }
    }
}
