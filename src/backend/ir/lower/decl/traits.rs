//! Trait declaration lowering.

use std::collections::HashSet;

use incan_core::lang::trait_bounds;

use super::super::super::Mutability;
use super::super::super::decl::{FunctionParam, IrFunction, IrTrait, Visibility};
use super::super::super::types::IrType;
use super::super::AstLowering;
use super::super::errors::LoweringError;
use crate::frontend::ast;
use crate::frontend::symbols::ResolvedType;

impl AstLowering {
    /// Map a supertrait name and resolved type arguments to IR for Rust trait bounds (RFC 042).
    fn lower_supertrait_from_resolved(&self, trait_name: &str, type_args: &[ResolvedType]) -> (String, Vec<IrType>) {
        let path = trait_bounds::incan_to_rust(trait_name)
            .map(str::to_string)
            .unwrap_or_else(|| trait_name.to_string());
        let ir_args = type_args.iter().map(|ty| self.lower_resolved_type(ty)).collect();
        (path, ir_args)
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
                let path = trait_bounds::incan_to_rust(&bound.node.name)
                    .map(str::to_string)
                    .unwrap_or_else(|| bound.node.name.clone());
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

    /// Lower a trait declaration.
    pub(in crate::backend::ir::lower) fn lower_trait(&mut self, t: &ast::TraitDecl) -> Result<IrTrait, LoweringError> {
        let type_param_names: HashSet<&str> = t.type_params.iter().map(|tp| tp.name.as_str()).collect();
        let methods: Vec<IrFunction> = t
            .methods
            .iter()
            .map(|m| {
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
                        default: None,
                    });
                }

                // Add regular parameters
                let other_params: Vec<FunctionParam> = m
                    .node
                    .params
                    .iter()
                    .map(|p| {
                        let ty = self.lower_callable_param_type(
                            &p.node.ty.node,
                            Some(&combined_type_param_names),
                            &mut hidden_type_params,
                            &mut hidden_counter,
                        );
                        FunctionParam {
                            name: p.node.name.clone(),
                            ty,
                            mutability: if p.node.is_mut {
                                Mutability::Mutable
                            } else {
                                Mutability::Immutable
                            },
                            is_self: false,
                            default: match &p.node.default {
                                Some(default_expr) => self.lower_expr_spanned(default_expr).ok(),
                                None => None,
                            },
                        }
                    })
                    .collect();
                params.extend(other_params);

                let return_type =
                    self.lower_callable_return_type(&m.node.return_type.node, Some(&combined_type_param_names));
                // IMPORTANT: We intentionally do NOT emit trait method bodies into the Rust trait itself.
                // Default methods are expanded into each adopting `impl Trait for Type` block during lowering, which
                // allows bodies to assume adopter fields (RFC 000) without generating invalid Rust trait default
                // methods like `self.name`.
                let body = vec![];

                self.pop_scope();

                let mut all_type_params = Self::lower_type_params(&m.node.type_params);
                all_type_params.extend(hidden_type_params);

                Ok(IrFunction {
                    name: m.node.name.clone(),
                    params,
                    return_type,
                    body,
                    is_async: m.node.is_async(),
                    visibility: Visibility::Private,
                    type_params: all_type_params,
                    is_extern: false,
                    rust_attributes: self.extract_passthrough_attributes(&m.node.decorators),
                    lint_allows: self.extract_rust_lint_allows(&m.node.decorators),
                })
            })
            .collect::<Result<Vec<_>, LoweringError>>()?;

        let supertraits: Vec<(String, Vec<IrType>)> = if let Some(ti) = self
            .type_info
            .as_ref()
            .and_then(|info| info.trait_direct_supertraits.get(&t.name))
        {
            ti.iter()
                .map(|(name, args)| self.lower_supertrait_from_resolved(name, args))
                .collect()
        } else {
            self.lower_supertraits_from_ast(t, &type_param_names)
        };

        Ok(IrTrait {
            name: t.name.clone(),
            type_params: Self::lower_type_params(&t.type_params),
            supertraits,
            methods,
            visibility: Self::map_visibility(t.visibility),
        })
    }
}
