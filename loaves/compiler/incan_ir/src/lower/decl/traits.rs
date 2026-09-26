//! Trait declaration lowering.

use std::collections::HashSet;

use incan_lang::lang::magic_methods::{self, MagicMethodId};
use incan_lang::lang::surface::methods::iterator_methods::{self, IteratorMethodId};
use incan_lang::lang::traits as core_traits;
use incan_lang::lang::traits::TraitId;
use incan_lang::lang::types::collections::{self, CollectionTypeId};
use incan_lang::lang::{callables, stdlib, trait_bounds};

use super::super::super::Mutability;
use super::super::super::decl::{
    FunctionParam, FunctionParamDefault, IrFunction, IrTrait, IrTraitBound, Visibility,
};
use super::super::super::types::IrType;
use super::super::AstLowering;
use super::super::errors::LoweringError;
use super::methods::PropertyLoweringMode;
use incan_frontend::ast;
use incan_frontend::symbols::ResolvedType;

impl AstLowering {
    /// Resolve a trait declaration to the canonical source callable role owned by `std.traits.callable`.
    ///
    /// SDK component projects compile the contents of the public `std` namespace as crate-local module paths. Restore
    /// that public mount only for the trusted provider-build boundary so a user package named `traits.callable` cannot
    /// acquire compiler-managed callable behavior.
    fn source_callable_trait(&self, trait_name: &str) -> Option<callables::CallableTraitId> {
        let mut module_path = self
            .current_source_module_name
            .as_deref()?
            .split('.')
            .map(str::to_string)
            .collect::<Vec<_>>();
        let internal_stdlib_source = module_path.first().map(String::as_str) == Some(stdlib::INCAN_STD_NAMESPACE);
        if self.sdk_provider_build || internal_stdlib_source {
            if internal_stdlib_source {
                // Normal Oven consumers re-emit compiler-owned provider source below the compiler-reserved
                // `__incan_std` namespace. It has the same callable contract as the published Loaf fixture, even though
                // the consumer itself must not acquire general SDK-provider lowering privileges.
                module_path[0] = stdlib::STDLIB_ROOT.to_string();
            } else if module_path.first().is_none_or(|segment| segment != stdlib::STDLIB_ROOT) {
                module_path.insert(0, stdlib::STDLIB_ROOT.to_string());
            }
        }
        callables::module_path_matches(&module_path)
            .then(|| callables::from_str(trait_name))
            .flatten()
    }

    /// Map a supertrait name and resolved type arguments to IR for Rust trait bounds (RFC 042).
    fn lower_supertrait_from_resolved(&self, trait_name: &str, type_args: &[ResolvedType]) -> (String, Vec<IrType>) {
        let path = self.supertrait_rust_path(trait_name);
        let ir_args = type_args.iter().map(|ty| self.lower_resolved_type(ty)).collect();
        (path, ir_args)
    }

    /// Preserve an imported Rust supertrait's absolute path across aliases and dependency-module lowering.
    fn supertrait_rust_path(&self, trait_name: &str) -> String {
        if let Some(path) = self.rust_import_aliases.get(trait_name) {
            return format!("::{}", path.join("::"));
        }
        trait_bounds::incan_to_rust(trait_name)
            .map(str::to_string)
            .unwrap_or_else(|| trait_name.to_string())
    }

    /// Lower `with` supertraits from the AST when typechecker output is unavailable (e.g. dependency lowering).
    fn lower_supertraits_from_ast(
        &mut self,
        t: &ast::TraitDecl,
        type_param_names: &HashSet<&str>,
    ) -> Vec<(String, Vec<IrType>)> {
        t.traits
            .iter()
            .map(|bound| {
                let path = self.supertrait_rust_path(&bound.node.name);
                let ir_args = bound
                    .node
                    .type_args
                    .iter()
                    .map(|ty| self.lower_type_with_type_params(&ty.node, Some(type_param_names)))
                    .collect();
                (path, ir_args)
            })
            .collect()
    }

    /// Return the method type parameters whose values a trait default body copies out of a list or dict (#1756).
    ///
    /// The Rust trait slot carries no body (defaults are expanded into each adopting implementation), so its generics
    /// come from the declaration alone. A default that reads `items[0]` or slices `items[1:]` on a `list[K]`, or reads
    /// `table[key]` on a `dict[str, V]`, copies a value of that type parameter, and each expansion states `Clone` for
    /// it; the slot states it too, so an expanded method is never stricter than the slot it fills, wherever the
    /// adopter lives.
    fn default_body_copied_type_params(
        &self,
        body: &[ast::Spanned<ast::Statement>],
        method_type_params: &HashSet<&str>,
    ) -> HashSet<String> {
        let mut copied = HashSet::new();
        if method_type_params.is_empty() {
            return copied;
        }
        let Some(info) = self.type_info.as_ref() else {
            return copied;
        };
        incan_frontend::ast_walk::any_expr_in_body(body, |expr| {
            let (base, slice) = match expr {
                ast::Expr::Index(base, _) => (base, false),
                ast::Expr::Slice(base, _) => (base, true),
                _ => return false,
            };
            if let Some(element) = info
                .expr_type(base.span)
                .and_then(|base_ty| copied_collection_element(base_ty, slice))
            {
                collect_type_param_mentions(element, method_type_params, &mut copied);
            }
            false
        });
        copied
    }

    /// Lower a trait declaration.
    pub(in crate::lower) fn lower_trait(&mut self, t: &ast::TraitDecl) -> Result<IrTrait, LoweringError> {
        let type_param_names: HashSet<&str> = t.type_params.iter().map(|tp| tp.name.as_str()).collect();
        let trait_methods = self.methods_with_partials(
            &t.name,
            &t.methods,
            &t.method_aliases,
            &t.method_partials,
            ast::Span::default(),
            true,
        )?;
        let mut methods: Vec<IrFunction> = trait_methods
            .iter()
            .map(|m| {
                if t.name == core_traits::as_str(TraitId::Iterator)
                    && m.node.name == iterator_methods::as_str(IteratorMethodId::Sum)
                    && m.node.body.is_some()
                {
                    let mut method = self.lower_method_with_type_params(&m.node, Some(&type_param_names))?;
                    method.visibility = Visibility::Private;
                    return Ok(method);
                }
                self.push_scope();
                let method_type_param_names: HashSet<&str> =
                    m.node.type_params.iter().map(|tp| tp.name.as_str()).collect();
                let combined_type_param_names: HashSet<&str> = type_param_names
                    .iter()
                    .copied()
                    .chain(method_type_param_names.iter().copied())
                    .collect();
                let mut hidden_type_params = Vec::new();
                let mut hidden_counter = 0usize;

                // Handle receiver (self) parameter
                let mut params = Vec::new();
                if let Some(receiver) = &m.node.receiver {
                    params.push(FunctionParam {
                        name: "self".to_string(),
                        ty: IrType::SelfType,
                        mutability: match receiver {
                            ast::Receiver::Immutable => Mutability::Immutable,
                            ast::Receiver::Mutable => Mutability::Mutable,
                        },
                        is_self: true,
                        kind: ast::ParamKind::Normal,
                        default: None,
                    });
                }

                // Add regular parameters
                let other_params: Vec<FunctionParam> = m
                    .node
                    .params
                    .iter()
                    .map(|p| {
                        let base_ty = self.lower_callable_param_type(
                            &p.node.ty.node,
                            Some(&combined_type_param_names),
                            &mut hidden_type_params,
                            &mut hidden_counter,
                        );
                        let base_ty =
                            self.apply_mutable_rust_type_argument_projections(p.node.is_mut, &p.node.ty, base_ty);
                        let ty = Self::lower_param_container_type(p.node.kind, base_ty);
                        Ok(FunctionParam {
                            name: p.node.name.clone(),
                            ty,
                            mutability: self.lower_parameter_mutability(p),
                            is_self: false,
                            kind: p.node.kind,
                            default: self
                                .lower_param_default_expr(p.node.default.as_ref())?
                                .map(FunctionParamDefault::source),
                        })
                    })
                    .collect::<Result<_, LoweringError>>()?;
                params.extend(other_params);

                let return_type =
                    self.lower_callable_return_type(&m.node.return_type.node, Some(&combined_type_param_names));
                // IMPORTANT: We intentionally do NOT emit trait method bodies into the Rust trait itself.
                // Default methods are expanded into each adopting `impl Trait for Type` block during lowering, which
                // allows bodies to assume adopter fields (RFC 000) without generating invalid Rust trait default
                // methods like `self.name`.
                let body = vec![];

                self.pop_scope();

                let mut all_type_params = self.lower_callable_type_params(&m.node.type_params);
                if let Some(default_body) = &m.node.body {
                    let copied = self.default_body_copied_type_params(default_body, &method_type_param_names);
                    for type_param in all_type_params
                        .iter_mut()
                        .filter(|type_param| copied.contains(type_param.name.as_str()))
                    {
                        if !type_param
                            .bounds
                            .iter()
                            .any(|bound| bound.trait_path == trait_bounds::rust::CLONE)
                        {
                            type_param
                                .bounds
                                .push(IrTraitBound::simple(trait_bounds::rust::CLONE));
                        }
                    }
                }
                all_type_params.extend(hidden_type_params);

                Ok(IrFunction {
                    name: m.node.name.clone(),
                    docstring: m.node.body.as_ref().and_then(|body| super::callable_docstring(body)),
                    params,
                    return_type,
                    body,
                    is_async: m.node.is_async(),
                    is_generator: false,
                    visibility: Visibility::Private,
                    type_params: all_type_params,
                    is_extern: false,
                    rust_extern_name: None,
                    rust_attributes: self.extract_passthrough_attributes(&m.node.decorators),
                    lint_allows: self.extract_rust_lint_allows(&m.node.decorators),
                })
            })
            .collect::<Result<Vec<_>, LoweringError>>()?;
        if t.name == core_traits::as_str(TraitId::Iterator) {
            methods.retain(|method| {
                method.name == magic_methods::as_str(MagicMethodId::Next)
                    || method.name == iterator_methods::as_str(IteratorMethodId::Sum)
            });
        }

        for property in &t.properties {
            methods.push(self.lower_property_with_type_params(
                property,
                Some(&type_param_names),
                PropertyLoweringMode::TraitDecl,
            )?);
        }

        let supertraits: Vec<(String, Vec<IrType>)> = if let Some(ti) = self
            .type_info
            .as_ref()
            .and_then(|info| info.traits.direct_supertraits.get(&t.name))
        {
            ti.iter()
                .map(|(name, args)| self.lower_supertrait_from_resolved(name, args))
                .collect()
        } else {
            self.lower_supertraits_from_ast(t, &type_param_names)
        };

        Ok(IrTrait {
            name: t.name.clone(),
            source_callable: self.source_callable_trait(&t.name),
            docstring: t.docstring.clone(),
            type_params: self.lower_type_params(&t.type_params),
            supertraits,
            methods,
            visibility: Self::map_visibility(t.visibility),
        })
    }
}

/// Return the element an index read (or, with `slice`, a slice) of a list or dict of this type copies out.
///
/// A list index or slice copies list elements and a dict index copies a value; a dict has no slice.
fn copied_collection_element(ty: &ResolvedType, slice: bool) -> Option<&ResolvedType> {
    match ty {
        ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => copied_collection_element(inner, slice),
        ResolvedType::Generic(name, args) => match (collections::from_str(name), args.as_slice()) {
            (Some(CollectionTypeId::List), [element]) => Some(element),
            (Some(CollectionTypeId::Dict), [_, value]) if !slice => Some(value),
            _ => None,
        },
        _ => None,
    }
}

/// Collect the names of `type_params` that `ty` mentions, at any depth.
fn collect_type_param_mentions(ty: &ResolvedType, type_params: &HashSet<&str>, mentioned: &mut HashSet<String>) {
    match ty {
        ResolvedType::TypeVar(name) | ResolvedType::Named(name) => {
            if type_params.contains(name.as_str()) {
                mentioned.insert(name.clone());
            }
        }
        ResolvedType::Generic(_, args) | ResolvedType::Tuple(args) => {
            for arg in args {
                collect_type_param_mentions(arg, type_params, mentioned);
            }
        }
        ResolvedType::FrozenList(inner)
        | ResolvedType::FrozenSet(inner)
        | ResolvedType::Ref(inner)
        | ResolvedType::RefMut(inner) => collect_type_param_mentions(inner, type_params, mentioned),
        ResolvedType::FrozenDict(key, value) => {
            collect_type_param_mentions(key, type_params, mentioned);
            collect_type_param_mentions(value, type_params, mentioned);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_provider_callable_module_retains_public_std_identity() {
        let mut lowering = AstLowering::new();
        lowering.set_current_source_module_name(Some("traits.callable".to_string()));
        lowering.set_sdk_provider_build(true);
        assert_eq!(
            lowering.source_callable_trait("Callable1"),
            Some(callables::CallableTraitId::Callable1)
        );

        lowering.set_sdk_provider_build(false);
        assert_eq!(
            lowering.source_callable_trait("Callable1"),
            None,
            "ordinary packages must not acquire compiler callable behavior from a same-named module"
        );
    }

    #[test]
    fn loaf_source_callable_module_restores_internal_std_identity() {
        let mut lowering = AstLowering::new();
        lowering.set_current_source_module_name(Some("__incan_std.traits.callable".to_string()));
        lowering.set_sdk_provider_build(true);

        assert_eq!(
            lowering.source_callable_trait("Callable1"),
            Some(callables::CallableTraitId::Callable1)
        );
    }

    #[test]
    fn normal_oven_internal_stdlib_callable_module_keeps_its_bridge() {
        let mut lowering = AstLowering::new();
        lowering.set_current_source_module_name(Some("__incan_std.traits.callable".to_string()));
        assert_eq!(
            lowering.source_callable_trait("Callable1"),
            Some(callables::CallableTraitId::Callable1),
            "normal Oven provider source is emitted below the reserved internal stdlib namespace"
        );
    }
}
