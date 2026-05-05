//! Function declaration lowering.

use super::super::super::Mutability;
use super::super::super::decl::{FunctionParam, IrFunction, IrTraitBound, IrTraitBoundOrigin};
use super::super::super::types::IrType;
use super::super::AstLowering;
use super::super::errors::LoweringError;
use crate::frontend::ast;
use incan_core::lang::types::collections::{self, CollectionTypeId};

/// Return whether a lowered callable return type is the canonical `Generator[...]` wrapper.
fn return_type_is_generator(ty: &IrType) -> bool {
    matches!(ty, IrType::NamedGeneric(name, _)
        if collections::from_str(name.as_str()) == Some(CollectionTypeId::Generator))
}

/// Return whether a function body contains a source `yield` expression.
fn body_contains_yield(body: &[ast::Spanned<ast::Statement>]) -> bool {
    body.iter().any(|stmt| match &stmt.node {
        ast::Statement::Expr(expr) | ast::Statement::Return(Some(expr)) => matches!(expr.node, ast::Expr::Yield(_)),
        ast::Statement::If(stmt) => {
            body_contains_yield(&stmt.then_body)
                || stmt.elif_branches.iter().any(|(_, body)| body_contains_yield(body))
                || stmt.else_body.as_ref().is_some_and(|body| body_contains_yield(body))
        }
        ast::Statement::Loop(stmt) => body_contains_yield(&stmt.body),
        ast::Statement::While(stmt) => body_contains_yield(&stmt.body),
        ast::Statement::For(stmt) => body_contains_yield(&stmt.body),
        ast::Statement::VocabBlock(stmt) => body_contains_yield(&stmt.body),
        _ => false,
    })
}

impl AstLowering {
    /// Lower a function declaration.
    ///
    /// # Parameters
    ///
    /// * `f` - The AST function declaration
    ///
    /// # Returns
    ///
    /// The corresponding IR function.
    pub(in crate::backend::ir::lower) fn lower_function(
        &mut self,
        f: &ast::FunctionDecl,
    ) -> Result<IrFunction, LoweringError> {
        self.push_scope();

        let type_param_names: std::collections::HashSet<&str> =
            f.type_params.iter().map(|tp| tp.name.as_str()).collect();
        let mut hidden_type_params = Vec::new();
        let mut hidden_counter = 0usize;

        let params: Vec<FunctionParam> = f
            .params
            .iter()
            .map(|p| {
                let base_ty = self.lower_callable_param_type(
                    &p.node.ty.node,
                    Some(&type_param_names),
                    &mut hidden_type_params,
                    &mut hidden_counter,
                );
                let param_ty = Self::lower_param_container_type(p.node.kind, base_ty);
                // For mutable parameters, wrap in RefMut to track that it's a &mut reference
                let ty = if p.node.is_mut {
                    IrType::RefMut(Box::new(param_ty.clone()))
                } else {
                    param_ty.clone()
                };
                self.define_local_binding(p.node.name.clone(), ty.clone(), false);
                // Track mutable parameters
                if p.node.is_mut {
                    self.mutable_vars.insert(p.node.name.clone(), true);
                }
                FunctionParam {
                    name: p.node.name.clone(),
                    ty: param_ty, // Store the emitted parameter type (emit will add &mut for mutable params)
                    mutability: if p.node.is_mut {
                        Mutability::Mutable
                    } else {
                        Mutability::Immutable
                    },
                    is_self: false,
                    kind: p.node.kind,
                    default: match &p.node.default {
                        Some(default_expr) => self.lower_expr_spanned(default_expr).ok(),
                        None => None,
                    },
                }
            })
            .collect();

        let return_type = self.lower_callable_return_type(&f.return_type.node, Some(&type_param_names));
        let is_generator = return_type_is_generator(&return_type) && body_contains_yield(&f.body);
        let body = self.lower_statements(&f.body)?;
        self.pop_scope();

        // RFC 023: detect @rust.extern decorator to mark this function as externally-backed.
        let is_extern = Self::has_rust_extern_decorator(&f.decorators);
        let rust_attributes = self.extract_passthrough_attributes(&f.decorators);
        let lint_allows = self.extract_rust_lint_allows(&f.decorators);

        let mut all_type_params = Self::lower_type_params(&f.type_params);
        all_type_params.extend(hidden_type_params);
        if is_generator {
            for type_param in &mut all_type_params {
                for trait_path in ["Send", "Static"] {
                    if !type_param.bounds.iter().any(|bound| {
                        bound.origin == IrTraitBoundOrigin::RustCapability && bound.trait_path == trait_path
                    }) {
                        type_param.bounds.push(IrTraitBound {
                            trait_path: trait_path.to_string(),
                            type_args: Vec::new(),
                            assoc_types: Vec::new(),
                            origin: IrTraitBoundOrigin::RustCapability,
                        });
                    }
                }
            }
        }

        Ok(IrFunction {
            name: f.name.clone(),
            params,
            return_type,
            body,
            is_async: f.is_async(),
            is_generator,
            visibility: Self::map_visibility(f.visibility),
            type_params: all_type_params,
            is_extern,
            rust_attributes,
            lint_allows,
        })
    }
}
