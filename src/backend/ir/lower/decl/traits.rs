//! Trait declaration lowering.

use super::super::super::Mutability;
use super::super::super::decl::{FunctionParam, IrFunction, IrTrait, Visibility};
use super::super::super::types::IrType;
use super::super::AstLowering;
use super::super::errors::LoweringError;
use crate::frontend::ast;

impl AstLowering {
    /// Lower a trait declaration.
    pub(in crate::backend::ir::lower) fn lower_trait(&mut self, t: &ast::TraitDecl) -> Result<IrTrait, LoweringError> {
        let methods: Vec<IrFunction> = t
            .methods
            .iter()
            .map(|m| {
                self.scopes.push(std::collections::HashMap::new());

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
                        let ty = self.lower_type(&p.node.ty.node);
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

                let return_type = self.lower_type(&m.node.return_type.node);
                // IMPORTANT: We intentionally do NOT emit trait method bodies into the Rust trait itself.
                // Default methods are expanded into each adopting `impl Trait for Type` block during lowering, which
                // allows bodies to assume adopter fields (RFC 000) without generating invalid Rust trait default
                // methods like `self.name`.
                let body = vec![];

                self.scopes.pop();

                Ok(IrFunction {
                    name: m.node.name.clone(),
                    params,
                    return_type,
                    body,
                    is_async: m.node.is_async(),
                    visibility: Visibility::Private,
                    type_params: vec![],
                    is_extern: false,
                })
            })
            .collect::<Result<Vec<_>, LoweringError>>()?;

        Ok(IrTrait {
            name: t.name.clone(),
            methods,
            visibility: Self::map_visibility(t.visibility),
        })
    }
}
