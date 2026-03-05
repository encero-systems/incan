//! Method lowering: model methods, class methods, trait impl methods, and general method lowering.

use std::collections::HashMap;

use super::super::super::decl::{FunctionParam, IrFunction, IrImpl, Visibility};
use super::super::super::types::IrType;
use super::super::super::{IrSpan, Mutability};
use super::super::AstLowering;
use super::super::errors::LoweringError;
use crate::frontend::ast::{self, Spanned};
use incan_core::lang::keywords::{self, KeywordId};

impl AstLowering {
    /// Lower model methods into an impl block.
    pub(in crate::backend::ir::lower) fn lower_model_methods(
        &mut self,
        type_name: &str,
        type_params: &[ast::TypeParam],
        methods: &[Spanned<ast::MethodDecl>],
    ) -> Result<IrImpl, LoweringError> {
        let prev = self.current_impl_type.replace(type_name.to_string());
        // IMPORTANT: always restore `current_impl_type` even if lowering fails, since lowering continues after
        // collecting errors.
        let lowered = methods
            .iter()
            .map(|m| self.lower_method(&m.node))
            .collect::<Result<Vec<_>, LoweringError>>();
        self.current_impl_type = prev;
        let lowered_methods = lowered?;

        Ok(IrImpl {
            target_type: type_name.to_string(),
            type_params: Self::lower_type_params(type_params),
            trait_name: None,
            methods: lowered_methods,
        })
    }

    /// Lower trait implementation for a class.
    ///
    /// Only methods matching trait signatures go in `impl Trait for Type`.
    pub(in crate::backend::ir::lower) fn lower_trait_impl(
        &mut self,
        type_name: &str,
        type_params: &[ast::TypeParam],
        trait_name: &str,
        impl_methods: &[Spanned<ast::MethodDecl>],
    ) -> Result<IrImpl, LoweringError> {
        // Avoid holding an immutable borrow of `self` across lowering calls.
        let trait_decl = self.trait_decls.get(trait_name).cloned().ok_or_else(|| LoweringError {
            message: format!("Unknown trait '{trait_name}'"),
            span: IrSpan::default(),
        })?;
        let trait_methods = trait_decl.methods;

        let mut methods: Vec<IrFunction> = Vec::new();
        for trait_method in &trait_methods {
            let method_name = trait_method.node.name.as_str();

            // Prefer the implementing type's override, if present.
            let mut found_override: Option<&ast::MethodDecl> = None;
            for m in impl_methods {
                if m.node.name == method_name {
                    found_override = Some(&m.node);
                    break;
                }
            }
            if let Some(m) = found_override {
                methods.push(self.lower_impl_method_for_trait(m)?);
                continue;
            }

            // Otherwise, expand a default method body into the impl (RFC 000: defaults may assume adopter fields).
            if trait_method.node.body.is_some() {
                methods.push(self.lower_impl_method_for_trait(&trait_method.node)?);
                continue;
            }

            // Required trait method with no default implementation.
            return Err(LoweringError {
                message: format!(
                    "Type '{type_name}' does not implement required method '{method_name}' for trait '{trait_name}'"
                ),
                span: IrSpan::default(),
            });
        }

        Ok(IrImpl {
            target_type: type_name.to_string(),
            type_params: Self::lower_type_params(type_params),
            trait_name: Some(trait_name.to_string()),
            methods,
        })
    }

    fn lower_impl_method_for_trait(&mut self, m: &ast::MethodDecl) -> Result<IrFunction, LoweringError> {
        self.scopes.push(HashMap::new());

        // Handle receiver (self) parameter
        let mut params = Vec::new();
        if let Some(receiver) = &m.receiver {
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
            .params
            .iter()
            .map(|p| {
                let base_ty = self.lower_type(&p.node.ty.node);
                FunctionParam {
                    name: p.node.name.clone(),
                    ty: base_ty,
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

        let return_type = self.lower_type(&m.return_type.node);
        let body = if let Some(ref body_stmts) = m.body {
            self.lower_statements(body_stmts)?
        } else {
            vec![]
        };

        // RFC 023: detect @rust.extern decorator to mark this method as externally-backed.
        let is_extern = Self::has_rust_extern_decorator(&m.decorators);
        let rust_attributes = self.extract_passthrough_attributes(&m.decorators);

        self.scopes.pop();

        Ok(IrFunction {
            name: m.name.clone(),
            params,
            return_type,
            body,
            is_async: m.is_async(),
            visibility: Visibility::Private,
            type_params: vec![],
            is_extern,
            rust_attributes,
        })
    }

    /// Lower class methods into an impl block.
    pub(in crate::backend::ir::lower) fn lower_class_methods(
        &mut self,
        type_name: &str,
        type_params: &[ast::TypeParam],
        methods: &[Spanned<ast::MethodDecl>],
    ) -> Result<IrImpl, LoweringError> {
        let prev = self.current_impl_type.replace(type_name.to_string());
        // IMPORTANT: always restore `current_impl_type` even if lowering fails, since lowering continues after
        // collecting errors.
        let lowered = methods
            .iter()
            .map(|m| self.lower_method(&m.node))
            .collect::<Result<Vec<_>, LoweringError>>();
        self.current_impl_type = prev;
        let lowered_methods = lowered?;

        Ok(IrImpl {
            target_type: type_name.to_string(),
            type_params: Self::lower_type_params(type_params),
            trait_name: None,
            methods: lowered_methods,
        })
    }

    /// Lower a method declaration into a function.
    pub(in crate::backend::ir::lower) fn lower_method(
        &mut self,
        m: &ast::MethodDecl,
    ) -> Result<IrFunction, LoweringError> {
        self.scopes.push(HashMap::new());

        let mut params: Vec<FunctionParam> = Vec::new();

        // Add self parameter if receiver is present
        if let Some(receiver) = m.receiver {
            let is_mut = matches!(receiver, ast::Receiver::Mutable);
            params.push(FunctionParam {
                name: "self".to_string(),
                ty: IrType::Unknown, // Will be determined by impl context
                mutability: if is_mut {
                    Mutability::Mutable
                } else {
                    Mutability::Immutable
                },
                is_self: true,
                default: None,
            });
            // Add self to scope
            if let Some(scope) = self.scopes.last_mut() {
                scope.insert("self".to_string(), IrType::Unknown);
            }
        }

        // Add regular parameters
        let other_params: Vec<FunctionParam> = m
            .params
            .iter()
            .map(|p| {
                let base_ty = self.lower_type(&p.node.ty.node);
                // For mutable parameters, wrap in RefMut
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
                    ty: base_ty,
                    mutability: if p.node.is_mut {
                        Mutability::Mutable
                    } else {
                        Mutability::Immutable
                    },
                    is_self: p.node.name == keywords::as_str(KeywordId::SelfKw),
                    default: match &p.node.default {
                        Some(default_expr) => self.lower_expr_spanned(default_expr).ok(),
                        None => None,
                    },
                }
            })
            .collect();
        params.extend(other_params);

        let return_type = self.lower_type(&m.return_type.node);
        let body = if let Some(ref body_stmts) = m.body {
            self.lower_statements(body_stmts)?
        } else {
            // Abstract method with no body
            vec![]
        };
        self.scopes.pop();

        Ok(IrFunction {
            name: m.name.clone(),
            params,
            return_type,
            body,
            is_async: m.is_async(),
            visibility: Visibility::Private,
            type_params: vec![],
            is_extern: false,
            rust_attributes: self.extract_passthrough_attributes(&m.decorators),
        })
    }
}
