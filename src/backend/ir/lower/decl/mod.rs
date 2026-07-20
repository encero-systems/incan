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

pub(in crate::backend::ir::lower) use functions::callable_docstring;

use super::super::decl::{
    IrDecl, IrDeclKind, IrImportOrigin, IrImportQualifier, IrInteropAdapterKind, IrInteropDirection, IrInteropEdge,
    Visibility,
};
use super::super::expr::{IrCallArg, IrCallArgKind, IrExprKind};
use super::super::types::IrType;
use super::super::{IrSpan, TypedExpr};
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

    /// Map callable visibility, keeping private source stdlib helpers visible across generated sibling modules.
    pub(in crate::backend::ir::lower) fn map_callable_visibility(
        &self,
        vis: crate::frontend::ast::Visibility,
    ) -> Visibility {
        let mapped = Self::map_visibility(vis);
        if mapped == Visibility::Private
            && self
                .current_source_module_name
                .as_deref()
                .is_some_and(|name| name.starts_with("__incan_std.") || name.starts_with("std."))
        {
            return Visibility::Crate;
        }
        mapped
    }

    /// Map nominal type visibility, keeping private source stdlib helper types aligned with crate-visible helpers.
    pub(in crate::backend::ir::lower) fn map_type_visibility(
        &self,
        vis: crate::frontend::ast::Visibility,
    ) -> Visibility {
        let mapped = Self::map_visibility(vis);
        if mapped == Visibility::Private
            && self
                .current_source_module_name
                .as_deref()
                .is_some_and(|name| name.starts_with("__incan_std.") || name.starts_with("std."))
        {
            return Visibility::Crate;
        }
        mapped
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
                if c.name == "__derives__" {
                    return Err(LoweringError {
                        message: "internal __derives__ metadata is not emitted".to_string(),
                        span: IrSpan::default(),
                    });
                }
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
            ast::Declaration::Static(s) => {
                let mut value = self.lower_expr_spanned(&s.value)?;
                self.rewrite_checked_registry_entry_subject(&s.name, &mut value)?;
                let visibility = match s.visibility {
                    ast::Visibility::Public => Visibility::Public,
                    ast::Visibility::Private => Visibility::Private,
                };
                IrDeclKind::Static {
                    visibility,
                    name: s.name.clone(),
                    ty: self.lower_type(&s.ty.node),
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
                visibility: self.map_type_visibility(a.visibility),
                name: a.name.clone(),
                type_params: Self::lower_type_params(&a.type_params),
                ty: self.lower_type(&a.target.node),
                is_rusttype: false,
                interop_edges: Vec::new(),
            },
            ast::Declaration::Alias(a) => {
                let (target_path, target_origin, target_qualifier) = self.alias_reexport_target(&a.target.segments);
                IrDeclKind::SymbolAlias {
                    visibility: Self::map_visibility(a.visibility),
                    name: a.name.clone(),
                    target_path,
                    target_origin,
                    target_qualifier,
                }
            }
            ast::Declaration::Partial(_) => {
                return Err(LoweringError {
                    message: "Partial callable presets are not lowered by this syntax-only slice".to_string(),
                    span: IrSpan::default(),
                });
            }
            ast::Declaration::Newtype(n) => {
                if n.is_rusttype {
                    let interop_edges = self.lower_interop_edges(&n.interop_edges)?;
                    return Ok(IrDecl::new(IrDeclKind::TypeAlias {
                        visibility: Self::map_visibility(n.visibility),
                        name: n.name.clone(),
                        type_params: Self::lower_type_params(&n.type_params),
                        ty: self.lower_type(&n.underlying.node),
                        is_rusttype: true,
                        interop_edges,
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
            ast::Declaration::TestModule(_) => {
                return Err(LoweringError {
                    message: "Test modules are not lowered to production IR".to_string(),
                    span: IrSpan::default(),
                });
            }
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

    /// Materialize the canonical source identity for a frontend-approved explicit registry entry.
    ///
    /// `RegistrySubject.current_unit()` and `.package()` are ordinary typed source constructors. The placeholders in
    /// their source implementations must not escape into loaded runtime entries, however: the frontend has already
    /// fixed their subject kind and this lowering step supplies the compilation boundary's canonical identity. No
    /// runtime discovery or Rust-side registry implementation is involved.
    fn rewrite_checked_registry_entry_subject(
        &self,
        entry_name: &str,
        value: &mut TypedExpr,
    ) -> Result<(), LoweringError> {
        let Some(entry) = self.type_info.as_ref().and_then(|info| {
            info.registry
                .explicit_entries
                .iter()
                .find(|entry| entry.entry_name == entry_name)
        }) else {
            return Ok(());
        };

        let (expected_source_constructor, checked_constructor, identity) = match entry.subject_kind {
            incan_semantics_core::SemanticRegistrySubjectKind::CompilationUnit => (
                "current_unit",
                "_checked_current_unit",
                self.current_source_module_name
                    .clone()
                    .unwrap_or_else(|| "<unknown-compilation-unit>".to_string()),
            ),
            incan_semantics_core::SemanticRegistrySubjectKind::Package => (
                "package",
                "_checked_package",
                self.registry_package_identity
                    .clone()
                    .unwrap_or_else(|| "<unpackaged>".to_string()),
            ),
            incan_semantics_core::SemanticRegistrySubjectKind::Function
            | incan_semantics_core::SemanticRegistrySubjectKind::Method => {
                return Err(LoweringError {
                    message: "checked explicit registry entries only support compilation-unit and package subjects"
                        .to_string(),
                    span: IrSpan::default(),
                });
            }
        };

        let IrExprKind::MethodCall { method, args, .. } = &mut value.kind else {
            return Err(LoweringError {
                message: "checked registry entry lowering expected registry.entry(...)".to_string(),
                span: IrSpan::default(),
            });
        };
        if method != "entry" {
            return Err(LoweringError {
                message: "checked registry entry lowering expected registry.entry(...)".to_string(),
                span: IrSpan::default(),
            });
        }
        let Some(subject) = args.iter_mut().find(|arg| arg.name.as_deref() == Some("subject")) else {
            return Err(LoweringError {
                message: "checked registry entry lowering expected a named subject argument".to_string(),
                span: IrSpan::default(),
            });
        };
        let IrExprKind::MethodCall {
            method: subject_method,
            args: subject_args,
            ..
        } = &mut subject.expr.kind
        else {
            return Err(LoweringError {
                message: "checked registry entry lowering expected a RegistrySubject constructor".to_string(),
                span: IrSpan::default(),
            });
        };
        if subject_method != expected_source_constructor {
            return Err(LoweringError {
                message: "checked registry entry subject no longer matches its frontend-approved artifact".to_string(),
                span: IrSpan::default(),
            });
        }
        *subject_method = checked_constructor.to_string();
        *subject_args = vec![IrCallArg {
            name: None,
            kind: IrCallArgKind::Positional,
            expr: TypedExpr::new(IrExprKind::String(identity), IrType::String),
        }];
        Ok(())
    }

    /// Resolve the path that should be used when emitting a module-level alias declaration.
    ///
    /// A source alias can target a local import binding, but generated Rust public re-exports must point at the
    /// imported item path itself. Expression lowering still keeps the source binding for ordinary calls.
    fn alias_reexport_target(
        &self,
        segments: &[String],
    ) -> (Vec<String>, Option<IrImportOrigin>, Option<IrImportQualifier>) {
        if let [target] = segments
            && let Some(imported) = self.imported_alias_targets.get(target)
        {
            return (
                imported.path.clone(),
                Some(imported.origin.clone()),
                Some(imported.qualifier),
            );
        }
        (segments.to_vec(), None, None)
    }

    fn lower_interop_edges(
        &mut self,
        edges: &[ast::Spanned<ast::InteropEdgeDecl>],
    ) -> Result<Vec<IrInteropEdge>, LoweringError> {
        edges
            .iter()
            .map(|edge| {
                let direction = match edge.node.direction {
                    ast::InteropDirection::From => IrInteropDirection::From,
                    ast::InteropDirection::Into => IrInteropDirection::Into,
                };
                let adapter_kind = match edge.node.adapter_kind {
                    ast::InteropAdapterKind::Via => IrInteropAdapterKind::Via,
                    ast::InteropAdapterKind::Try => IrInteropAdapterKind::Try,
                };
                Ok(IrInteropEdge {
                    direction,
                    ty: self.lower_type(&edge.node.ty.node),
                    adapter_kind,
                    adapter: self.lower_expr_spanned(&edge.node.adapter)?,
                })
            })
            .collect()
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
