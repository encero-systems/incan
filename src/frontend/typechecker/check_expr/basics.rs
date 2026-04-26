//! Check basic expressions (identifiers, literals, and `self`).
//!
//! These helpers implement the low-level building blocks used throughout expression checking:
//! name resolution against the [`SymbolTable`], literal typing, and resolving `self` inside methods.

use crate::frontend::ast::*;
use crate::frontend::diagnostics::errors;
use crate::frontend::symbols::*;
use crate::frontend::typechecker::IdentKind;
use incan_core::lang::types::collections::{self, CollectionTypeId};

use super::TypeChecker;

impl TypeChecker {
    /// Resolve an identifier to its type.
    pub(in crate::frontend::typechecker::check_expr) fn check_ident(&mut self, name: &str, span: Span) -> ResolvedType {
        // Note: `math` module requires `import math` (like Python).
        // When imported, it's registered as a Module symbol and found via normal lookup.

        let Some(sym) = self.lookup_symbol(name) else {
            self.errors.push(errors::unknown_symbol(name, span));
            return ResolvedType::Unknown;
        };

        let (kind, ty) = match &sym.kind {
            SymbolKind::Variable(info) => (IdentKind::Value, info.ty.clone()),
            SymbolKind::Static(info) => (IdentKind::Static, info.ty.clone()),
            SymbolKind::Function(info) => {
                if !info.type_params.is_empty() {
                    self.errors.push(errors::generic_function_reference(name, span));
                    return ResolvedType::Unknown;
                }
                (
                    IdentKind::Value,
                    ResolvedType::Function(info.params.clone(), Box::new(info.return_type.clone())),
                )
            }
            SymbolKind::Type(_) => (IdentKind::TypeName, ResolvedType::Named(name.to_string())),
            SymbolKind::Variant(info) => (IdentKind::Variant, ResolvedType::Named(info.enum_name.clone())),
            SymbolKind::Field(info) => (IdentKind::Value, info.ty.clone()),
            SymbolKind::Module(info) => {
                // Some `from rust::... import ...` forms are represented as module symbols instead of dedicated
                // Rust-module placeholders. Keep them on the external-Rust path, but do not guess a concrete type from
                // the identifier spelling alone.
                if info.path.first().is_some_and(|seg| seg == "rust") {
                    (IdentKind::RustImport, ResolvedType::Unknown)
                } else {
                    (IdentKind::Module, ResolvedType::Named(name.to_string()))
                }
            }
            SymbolKind::Trait(_) => (IdentKind::Trait, ResolvedType::Named(name.to_string())),
            SymbolKind::RustItem(info) => {
                if let Some(meta) = &info.metadata
                    && meta.visibility == incan_core::interop::RustVisibility::Restricted
                {
                    self.errors
                        .push(errors::rust_item_not_public(name, meta.canonical_path.as_str(), span));
                    self.type_info
                        .ident_kinds
                        .insert((span.start, span.end), IdentKind::RustImport);
                    return ResolvedType::Unknown;
                }
                // RFC 041: carry canonical Rust path and (when available) extracted rust-inspect metadata.
                let resolved = match &info.metadata {
                    Some(meta) => match &meta.kind {
                        incan_core::interop::RustItemKind::Function(sig) => {
                            let params = sig
                                .params
                                .iter()
                                .map(|p| {
                                    CallableParam::positional(
                                        self.resolved_type_from_rust_display(p.type_display.as_str()),
                                    )
                                })
                                .collect();
                            let ret = self.resolved_type_from_rust_display(sig.return_type.as_str());
                            ResolvedType::Function(params, Box::new(ret))
                        }
                        incan_core::interop::RustItemKind::Constant { type_display } => {
                            self.resolved_type_from_rust_display(type_display.as_str())
                        }
                        incan_core::interop::RustItemKind::Unsupported { description } => {
                            self.errors.push(errors::rust_item_shape_not_supported(
                                info.path.as_str(),
                                description.as_str(),
                                span,
                            ));
                            ResolvedType::Unknown
                        }
                        _ => ResolvedType::RustPath(info.path.clone()),
                    },
                    None => ResolvedType::RustPath(info.path.clone()),
                };
                (IdentKind::RustImport, resolved)
            }
        };

        self.type_info.ident_kinds.insert((span.start, span.end), kind);
        ty
    }

    /// Resolve a literal value to its type.
    pub(in crate::frontend::typechecker::check_expr) fn check_literal(&self, lit: &Literal) -> ResolvedType {
        match lit {
            Literal::Int(_) => ResolvedType::Int,
            Literal::Float(_) => ResolvedType::Float,
            Literal::String(_) => ResolvedType::Str,
            Literal::Bytes(_) => ResolvedType::Bytes,
            Literal::Bool(_) => ResolvedType::Bool,
            Literal::None => ResolvedType::Generic(
                collections::as_str(CollectionTypeId::Option).to_string(),
                vec![ResolvedType::Unknown],
            ),
        }
    }

    /// Resolve the `self` expression inside a method body.
    pub(in crate::frontend::typechecker::check_expr) fn check_self(&mut self, span: Span) -> ResolvedType {
        if let Some(var_info) = self.lookup_variable_info("self") {
            return var_info.ty.clone();
        }
        self.errors.push(errors::unknown_symbol("self", span));
        ResolvedType::Unknown
    }
}
