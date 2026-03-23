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

        let return_type = self.lower_callable_return_type(&f.return_type.node, Some(&type_param_names));
        let body = self.lower_statements(&f.body)?;
        self.scopes.pop();

        // RFC 023: detect @rust.extern decorator to mark this function as externally-backed.
        let is_extern = Self::has_rust_extern_decorator(&f.decorators);
        let rust_attributes = self.extract_passthrough_attributes(&f.decorators);

        let mut all_type_params = Self::lower_type_params(&f.type_params);
        all_type_params.extend(hidden_type_params);

        Ok(IrFunction {
            name: f.name.clone(),
            params,
            return_type,
            body,
            is_async: f.is_async(),
            visibility: Self::map_visibility(f.visibility),
            type_params: all_type_params,
            is_extern,
            rust_attributes,
        })
    }
}
