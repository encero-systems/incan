//! Function declaration lowering.

use std::collections::HashMap;

use super::super::super::Mutability;
use super::super::super::decl::{FunctionParam, IrFunction};
use super::super::super::types::IrType;
use super::super::AstLowering;
use super::super::errors::LoweringError;
use crate::frontend::ast;

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
        self.scopes.push(HashMap::new());

        let type_param_names: std::collections::HashSet<&str> =
            f.type_params.iter().map(|tp| tp.name.as_str()).collect();

        let params: Vec<FunctionParam> = f
            .params
            .iter()
            .map(|p| {
                // Preserve generic type variables (`T`) as `IrType::Generic("T")` so trait-bound inference can reason
                // about operations on them without relying on typechecker span annotations.
                let base_ty = match &p.node.ty.node {
                    ast::Type::Simple(name) if type_param_names.contains(name.as_str()) => {
                        IrType::Generic(name.clone())
                    }
                    _ => self.lower_type(&p.node.ty.node),
                };
                // For mutable parameters, wrap in RefMut to track that it's a &mut reference
                let ty = if p.node.is_mut {
                    IrType::RefMut(Box::new(base_ty.clone()))
                } else {
                    base_ty.clone()
                };
                if let Some(scope) = self.scopes.last_mut() {
                    scope.insert(p.node.name.clone(), ty.clone());
                }
                // Track mutable parameters
                if p.node.is_mut {
                    self.mutable_vars.insert(p.node.name.clone(), true);
                }
                FunctionParam {
                    name: p.node.name.clone(),
                    ty: base_ty, // Store the base type in the param (emit will add &mut)
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

        let return_type = match &f.return_type.node {
            ast::Type::Simple(name) if type_param_names.contains(name.as_str()) => IrType::Generic(name.clone()),
            _ => self.lower_type(&f.return_type.node),
        };
        let body = self.lower_statements(&f.body)?;
        self.scopes.pop();

        // RFC 023: detect @rust.extern decorator to mark this function as externally-backed.
        let is_extern = Self::has_rust_extern_decorator(&f.decorators);

        Ok(IrFunction {
            name: f.name.clone(),
            params,
            return_type,
            body,
            is_async: f.is_async(),
            visibility: Self::map_visibility(f.visibility),
            type_params: Self::lower_type_params(&f.type_params),
            is_extern,
        })
    }
}
