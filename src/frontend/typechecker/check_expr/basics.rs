//! Check basic expressions (identifiers, literals, and `self`).
//!
//! These helpers implement the low-level building blocks used throughout expression checking:
//! name resolution against the [`SymbolTable`], literal typing, and resolving `self` inside methods.

use crate::frontend::ast::*;
use crate::frontend::diagnostics::{CompileError, errors};
use crate::frontend::symbols::*;
use crate::frontend::typechecker::IdentKind;
use incan_core::lang::types::collections::{self, CollectionTypeId};
use incan_semantics_core::SemanticSourceTargetKind;

use super::TypeChecker;

/// Return whether a metadata-free Rust import path follows Rust's constant naming convention.
fn rust_path_last_segment_looks_like_const(path: &str) -> bool {
    let segment = path.rsplit("::").next().unwrap_or(path).trim_start_matches("r#");
    segment
        .chars()
        .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_')
        && segment.chars().any(|ch| ch.is_ascii_uppercase())
}

impl TypeChecker {
    /// Resolve an identifier to its type.
    pub(in crate::frontend::typechecker::check_expr) fn check_ident(&mut self, name: &str, span: Span) -> ResolvedType {
        // Note: `math` module requires `import math` (like Python).
        // When imported, it's registered as a Module symbol and found via normal lookup.

        if let Some(consumed_span) = self.consumed_iterator_bindings.get(name).copied() {
            self.errors.push(CompileError::type_error(
                format!(
                    "iterator binding `{name}` was consumed by an iterator operation at byte range {}..{}; clone or recreate the iterator before reusing it",
                    consumed_span.start, consumed_span.end
                ),
                span,
            ));
        }
        if let Some(transfer_span) = self.transferred_c_resource_bindings.get(name).copied() {
            self.errors.push(CompileError::type_error(
                format!(
                    "C resource binding `{name}` was transferred to native code at byte range {}..{}; it cannot be used again",
                    transfer_span.start, transfer_span.end
                ),
                span,
            ));
        }

        let Some(sym_id) = self.symbols.lookup(name) else {
            if name == "log" {
                self.type_info
                    .expressions
                    .ident_kinds
                    .insert((span.start, span.end), IdentKind::Value);
                self.type_info.record_ambient_logger_binding(span);
                return ResolvedType::Named("Logger".to_string());
            }
            self.errors.push(errors::unknown_symbol(name, span));
            return ResolvedType::Unknown;
        };
        if self.symbols.identity_of(sym_id).is_some_and(|identity| {
            identity.kind == SemanticSourceTargetKind::Receiver && identity.declaration_name == "cls"
        }) {
            // `cls` is only a constructor callee. Keeping the source receiver in the symbol table gives constructor
            // calls a canonical identity without turning the class token into an ordinary runtime value.
            self.errors.push(errors::unknown_symbol(name, span));
            return ResolvedType::Unknown;
        }
        // ---- RFC 120: record which canonical identity this reference resolved to ----
        //
        // Strictly the resolved binding's own identity: import bindings carry their proven target identity (attached
        // at definition or back-filled when the proof lands — see `SymbolTable::backfill_import_identity`), so no
        // name-keyed fallback exists here that a shadowing definition could make stale.
        if let Some(identity) = self.symbols.identity_of(sym_id).cloned() {
            self.type_info.record_resolved_identity(span, identity);
        }
        let Some(sym) = self.symbols.get(sym_id) else {
            self.errors.push(errors::unknown_symbol(name, span));
            return ResolvedType::Unknown;
        };
        if self.checking_callable_default && matches!(&sym.kind, SymbolKind::Field(_) | SymbolKind::Property(_)) {
            self.errors.push(errors::unknown_symbol(name, span));
            return ResolvedType::Unknown;
        }
        if let Some(span_carrier) = self.c_abi_span_bindings.get(name).copied() {
            if let Some(consumed_at) = self
                .consumed_c_abi_span_bindings
                .get(&(span_carrier.binding_span.start, span_carrier.binding_span.end))
                .copied()
            {
                self.errors.push(CompileError::type_error(
                    format!(
                        "checked mutable C span `{name}` was already consumed at byte range {}..{}; it cannot be used again",
                        consumed_at.start, consumed_at.end
                    ),
                    span,
                ));
            } else {
                self.errors.push(CompileError::type_error(
                    format!(
                        "checked C span `{name}` has no ordinary value surface; use its closed bridge methods only in the declared checked C call"
                    ),
                    span,
                ));
            }
            return ResolvedType::Unknown;
        }
        let source_target = self.source_target_for_symbol(name, &sym.kind);

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
            SymbolKind::FunctionOverloads(_) => {
                self.errors.push(CompileError::type_error(
                    format!(
                        "Cannot use overloaded function '{name}' as a value; call it directly so an overload can be selected"
                    ),
                    span,
                ));
                return ResolvedType::Unknown;
            }
            SymbolKind::Type(info) => {
                if !self.is_type_receiver_span(span) {
                    if !self.is_type_token_value_span(span) {
                        self.errors.push(errors::type_name_used_as_value(name, span));
                        self.type_info
                            .expressions
                            .ident_kinds
                            .insert((span.start, span.end), IdentKind::TypeName);
                        return ResolvedType::Unknown;
                    }
                    let ty = if matches!(info, TypeInfo::Builtin) && sym.scope > 0 {
                        ResolvedType::TypeVar(name.to_string())
                    } else {
                        resolve_type(&Type::Simple(name.to_string()), &self.symbols)
                    };
                    self.type_info
                        .expressions
                        .ident_kinds
                        .insert((span.start, span.end), IdentKind::Value);
                    if let Some(target) = source_target {
                        self.record_source_target(span, target.module_path, target.name, target.kind);
                    }
                    return ResolvedType::TypeToken(Box::new(ty));
                }
                let ty = if matches!(info, TypeInfo::Builtin) && sym.scope > 0 {
                    ResolvedType::TypeVar(name.to_string())
                } else {
                    ResolvedType::Named(name.to_string())
                };
                (IdentKind::TypeName, ty)
            }
            SymbolKind::Variant(info) => (IdentKind::Variant, ResolvedType::Named(info.enum_name.clone())),
            SymbolKind::Field(info) => (IdentKind::Value, info.ty.clone()),
            SymbolKind::Property(info) => (IdentKind::Value, info.return_type.clone()),
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
            SymbolKind::Capability(_) => {
                // RFC 104 capabilities name an authority to perform an operation. Nothing in the language holds one as
                // a value, so a bare reference is always a mistake rather than a use this stage should type.
                self.errors.push(CompileError::type_error(
                    format!(
                        "Capability '{name}' names a runtime authority, not a value; grant it or list it under `requires` instead of referencing it directly"
                    ),
                    span,
                ));
                return ResolvedType::Unknown;
            }
            SymbolKind::Trait(_) => {
                if !self.is_type_receiver_span(span) {
                    self.errors.push(errors::type_name_used_as_value(name, span));
                    self.type_info
                        .expressions
                        .ident_kinds
                        .insert((span.start, span.end), IdentKind::Trait);
                    return ResolvedType::Unknown;
                }
                (IdentKind::Trait, ResolvedType::Named(name.to_string()))
            }
            SymbolKind::RustItem(info) => {
                if let Some(meta) = &info.metadata
                    && meta.visibility == incan_core::interop::RustVisibility::Restricted
                {
                    self.errors
                        .push(errors::rust_item_not_public(name, meta.canonical_path.as_str(), span));
                    self.type_info
                        .expressions
                        .ident_kinds
                        .insert((span.start, span.end), IdentKind::RustImport);
                    return ResolvedType::Unknown;
                }
                // RFC 041: carry canonical Rust path and (when available) extracted rust-inspect metadata.
                let ident_kind = match &info.metadata {
                    Some(meta) if matches!(meta.kind, incan_core::interop::RustItemKind::Constant { .. }) => {
                        IdentKind::RustValue
                    }
                    None if rust_path_last_segment_looks_like_const(info.path.as_str()) => IdentKind::RustValue,
                    _ => IdentKind::RustImport,
                };
                let resolved = match &info.metadata {
                    Some(meta) => match &meta.kind {
                        incan_core::interop::RustItemKind::Function(sig) => {
                            self.resolved_function_type_from_rust_sig_for_owner_path(sig, false, info.path.as_str())
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
                (ident_kind, resolved)
            }
        };

        self.type_info
            .expressions
            .ident_kinds
            .insert((span.start, span.end), kind);
        if let Some(target) = source_target {
            self.record_source_target(span, target.module_path, target.name, target.kind);
        }
        ty
    }

    /// Resolve a literal value to its type.
    pub(in crate::frontend::typechecker::check_expr) fn check_literal(&self, lit: &Literal) -> ResolvedType {
        match lit {
            Literal::Int(_) => ResolvedType::Int,
            Literal::Float(_) => ResolvedType::Float,
            Literal::Decimal(_) => ResolvedType::Unknown,
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
        if let Some(symbol_id) = self.symbols.lookup("self") {
            let identity = self
                .symbols
                .identity_of(symbol_id)
                .filter(|identity| {
                    identity.kind == SemanticSourceTargetKind::Receiver && identity.declaration_name == "self"
                })
                .cloned();
            let ty = self.symbols.get(symbol_id).and_then(|symbol| match &symbol.kind {
                SymbolKind::Variable(info) if identity.is_some() => Some(info.ty.clone()),
                _ => None,
            });
            if let Some(identity) = identity {
                self.type_info.record_resolved_identity(span, identity);
            }
            if let Some(ty) = ty {
                return ty;
            }
        }
        self.errors.push(errors::unknown_symbol("self", span));
        ResolvedType::Unknown
    }
}
