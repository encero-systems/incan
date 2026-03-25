//! Declaration lowering for AST to IR conversion.
//!
//! This module handles lowering of all declaration types: functions, models, classes, enums, newtypes, traits, and
//! imports.
//!
//! The logic is split across submodules by declaration kind; all methods live on `impl AstLowering`.

mod classes;
mod enums;
mod functions;
mod helpers;
mod imports;
mod methods;
mod models;
mod newtypes;
mod traits;

use super::super::IrSpan;
use super::super::decl::{IrDecl, IrDeclKind, Visibility};
use super::super::types::IrType;
use super::AstLowering;
use super::errors::LoweringError;
use crate::frontend::ast;
use incan_core::lang::decorators::{self, DecoratorId};

impl AstLowering {
    /// Map frontend visibility (`pub` / private) to IR visibility for Rust emission.
    pub(in crate::backend::ir::lower) fn map_visibility(vis: crate::frontend::ast::Visibility) -> Visibility {
        match vis {
            crate::frontend::ast::Visibility::Private => Visibility::Private,
            crate::frontend::ast::Visibility::Public => Visibility::Public,
        }
    }

    /// Lower a declaration to IR.
    ///
    /// # Parameters
    ///
    /// * `decl` - The AST declaration to lower
    ///
    /// # Returns
    ///
    /// The corresponding IR declaration.
    ///
    /// # Errors
    ///
    /// Returns `LoweringError` if the declaration cannot be lowered.
    pub(in crate::backend::ir::lower) fn lower_declaration(
        &mut self,
        decl: &ast::Declaration,
    ) -> Result<IrDecl, LoweringError> {
        let kind = match decl {
            ast::Declaration::Function(f) => IrDeclKind::Function(self.lower_function(f)?),
            ast::Declaration::Const(c) => {
                let value = self.lower_expr_spanned(&c.value)?;
                // RFC 008: In const context, annotations imply frozen/static types.
                // Prefer frozen annotation if present; otherwise use the initializer type.
                let ty = if let Some(ann) = &c.ty {
                    self.lower_const_annotation_type(&ann.node)
                } else if !matches!(value.ty, IrType::Unknown) {
                    value.ty.clone()
                } else {
                    IrType::Unknown
                };
                let visibility = match c.visibility {
                    ast::Visibility::Public => Visibility::Public,
                    ast::Visibility::Private => Visibility::Private,
                };
                IrDeclKind::Const {
                    visibility,
                    name: c.name.clone(),
                    ty,
                    value,
                }
            }
            ast::Declaration::Model(m) => {
                let struct_ir = self.lower_model(m)?;
                // Register struct name for constructor detection
                self.struct_names
                    .insert(struct_ir.name.clone(), IrType::Struct(struct_ir.name.clone()));
                IrDeclKind::Struct(struct_ir)
            }
            ast::Declaration::Class(c) => {
                let struct_ir = self.lower_class(c)?;
                // Register struct name for constructor detection
                self.struct_names
                    .insert(struct_ir.name.clone(), IrType::Struct(struct_ir.name.clone()));
                IrDeclKind::Struct(struct_ir)
            }
            ast::Declaration::Enum(e) => {
                let enum_ir = self.lower_enum(e)?;
                // Register enum name for type resolution
                self.enum_names
                    .insert(enum_ir.name.clone(), IrType::Enum(enum_ir.name.clone()));
                IrDeclKind::Enum(enum_ir)
            }
            ast::Declaration::TypeAlias(a) => IrDeclKind::TypeAlias {
                visibility: Self::map_visibility(a.visibility),
                name: a.name.clone(),
                type_params: Self::lower_type_params(&a.type_params),
                ty: self.lower_type(&a.target.node),
            },
            ast::Declaration::Newtype(n) => {
                if n.is_rusttype {
                    return Ok(IrDecl::new(IrDeclKind::TypeAlias {
                        visibility: Self::map_visibility(n.visibility),
                        name: n.name.clone(),
                        type_params: Self::lower_type_params(&n.type_params),
                        ty: self.lower_type(&n.underlying.node),
                    }));
                }
                // Note: newtype checked construction hook selection is done in `lower_program` when we see the full
                // newtype declaration.
                let struct_ir = self.lower_newtype(n)?;
                // Register struct name for constructor detection
                self.struct_names
                    .insert(struct_ir.name.clone(), IrType::Struct(struct_ir.name.clone()));
                IrDeclKind::Struct(struct_ir)
            }
            ast::Declaration::Import(i) => self.lower_import(i),
            ast::Declaration::Trait(t) => IrDeclKind::Trait(self.lower_trait(t)?),
            ast::Declaration::Docstring(_) => {
                // Skip docstrings in codegen
                return Err(LoweringError {
                    message: "Docstrings are not lowered to IR".to_string(),
                    span: IrSpan::default(),
                });
            }
        };
        Ok(IrDecl::new(kind))
    }

    /// RFC 023: Check if a decorator list contains `@rust.extern`.
    ///
    /// Used during lowering to mark functions whose body is provided by a Rust backing module.
    /// Uses `from_segments` on the full decorator path (e.g. `["rust", "extern"]`) since the `name` field only stores
    /// the last segment.
    pub(in crate::backend::ir::lower) fn has_rust_extern_decorator(
        decorators_list: &[ast::Spanned<ast::Decorator>],
    ) -> bool {
        decorators_list
            .iter()
            .any(|d| decorators::from_segments(&d.node.path.segments) == Some(DecoratorId::RustExtern))
    }
}
