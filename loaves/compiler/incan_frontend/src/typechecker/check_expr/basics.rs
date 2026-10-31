//! Check basic expressions (identifiers, literals, and `self`).
//!
//! These helpers implement the low-level building blocks used throughout expression checking: name resolution against
//! the [`SymbolTable`], literal typing, and resolving `self` inside methods.

use crate::ast::*;
use crate::diagnostics::{CompileError, errors};
use crate::symbols::*;
use crate::typechecker::{IdentKind, SourceTargetInfo};
use incan_lang::lang::types::collections::{self, CollectionTypeId};
use incan_semantics_core::SemanticSourceTargetKind;

use super::{GenericPartialTarget, TypeChecker};

/// Return whether a metadata-free Rust import path follows Rust's constant naming convention.
fn rust_path_last_segment_looks_like_const(path: &str) -> bool {
    let segment = path.rsplit("::").next().unwrap_or(path).trim_start_matches("r#");
    segment
        .chars()
        .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_')
        && segment.chars().any(|ch| ch.is_ascii_uppercase())
}

impl TypeChecker {
    /// Resolve an identifier to its type, including the ambient logger's checked factory return identity.
    pub(in crate::typechecker::check_expr) fn check_ident(&mut self, name: &str, span: Span) -> ResolvedType {
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

        // The first read's use is what fixes an open constructor's type arguments; note it on any watched binding by
        // this name before anything else decides how the read resolves (#1720, #2481).
        self.mark_open_rust_generic_binding_read(name, span);

        let Some(sym_id) = self.symbols.lookup(name) else {
            if name == "log" {
                self.type_info.expressions.ident_kinds.insert(span, IdentKind::Value);
                self.type_info.record_ambient_logger_binding(span);
                return self.ambient_logger_type().unwrap_or(ResolvedType::Unknown);
            }
            self.errors.push(errors::unknown_symbol(name, span));
            return ResolvedType::Unknown;
        };
        self.observe_binding_read(sym_id, span);
        self.note_mut_param_read(name, span);
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
        let Ok(source_target) = self.identifier_value_surface(name, sym_id, span) else {
            return ResolvedType::Unknown;
        };
        self.resolve_identifier_symbol(name, sym_id, span, source_target)
    }

    /// Validate a resolved binding's ordinary value surface before type dispatch. Retain the source target at its
    /// original point before output-slot checks; symbol IDs remain stable because this phase defines no bindings.
    fn identifier_value_surface(
        &mut self,
        name: &str,
        sym_id: SymbolId,
        span: Span,
    ) -> Result<Option<SourceTargetInfo>, ()> {
        let Some(sym) = self.symbols.get(sym_id) else {
            self.errors.push(errors::unknown_symbol(name, span));
            return Err(());
        };
        if self.checking_callable_default && matches!(&sym.kind, SymbolKind::Field(_) | SymbolKind::Property(_)) {
            self.errors.push(errors::unknown_symbol(name, span));
            return Err(());
        }
        if let Some(span_carrier) = self.c_abi_span_bindings.get(name).copied() {
            if let Some(consumed_at) = self
                .consumed_c_abi_span_bindings
                .get(&span_carrier.binding_span)
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
            return Err(());
        }
        if let SymbolKind::Variable(info) = &sym.kind
            && matches!(&info.ty, ResolvedType::Named(identity) if identity == incan_lang::lang::c_abi::SCOPED_C_STRING_VIEW_TYPE_ID)
        {
            // A scoped C text view is read only as the receiver of `copy_utf8`, in the closure depth it was bound at
            // (RFC 116); any other read would let it outlive the memory it points into.
            let receiver = self.scoped_c_string_view_receivers.contains(&span);
            let bound_depth = self.scoped_c_string_view_bindings.get(&sym.span).copied().unwrap_or(0);
            if !receiver || self.closure_depth > bound_depth {
                self.errors.push(errors::scoped_c_string_view_escapes(span));
                return Err(());
            }
        }
        let source_target = self.source_target_for_symbol(name, &sym.kind);
        let slot_depth = self
            .pending_c_abi_output_slots
            .get(name)
            .filter(|_| {
                matches!(&sym.kind, SymbolKind::Variable(info)
                if info.ty == ResolvedType::Unknown || matches!(&info.ty, ResolvedType::Named(identity)
                    if incan_lang::lang::c_abi::parse_output_slot_type_identity(identity).is_some()))
            })
            .map(|slot| slot.closure_depth)
            .or_else(|| {
                let SymbolKind::Variable(info) = &sym.kind else {
                    return None;
                };
                let ResolvedType::Named(identity) = &info.ty else {
                    return None;
                };
                self.type_info
                    .c_abi
                    .output_slots
                    .iter()
                    .find(|slot| &slot.identity == identity)
                    .map(|slot| slot.closure_depth)
            });
        if let Some(depth) = slot_depth
            && (!self.c_abi_raw_argument_spans.contains(&span) || self.closure_depth > depth)
        {
            self.errors.push(CompileError::type_error(
                "C output slots have no ordinary value surface and cannot be captured; use the checked raw call and take()".to_string(), span,
            ));
            return Err(());
        }
        Ok(source_target)
    }

    /// Dispatch an already validated symbol ID without snapshotting its potentially large declaration metadata.
    /// Type tokens and Rust items use their own value boundaries; ordinary symbols publish their retained kind/type.
    fn resolve_identifier_symbol(
        &mut self,
        name: &str,
        sym_id: SymbolId,
        span: Span,
        source_target: Option<SourceTargetInfo>,
    ) -> ResolvedType {
        let Some(sym) = self.symbols.get(sym_id) else {
            self.errors.push(errors::unknown_symbol(name, span));
            return ResolvedType::Unknown;
        };
        let (kind, ty) = match &sym.kind {
            SymbolKind::Variable(info) => (IdentKind::Value, info.ty.clone()),
            SymbolKind::Static(info) => (IdentKind::Static, info.ty.clone()),
            SymbolKind::Function(info) => {
                if !info.type_params.is_empty() {
                    // A local partial instantiates the generic function it names (RFC 084); anywhere else a generic
                    // function is not a value.
                    if !self.is_generic_partial_target_span(span) {
                        self.errors.push(errors::generic_function_reference(name, span));
                        return ResolvedType::Unknown;
                    }
                    self.generic_partial_target = Some(GenericPartialTarget::of_function(name, info));
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
            SymbolKind::Type(_) => {
                return self.check_type_identifier(name, sym_id, span, source_target);
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
                    self.type_info.expressions.ident_kinds.insert(span, IdentKind::Trait);
                    return ResolvedType::Unknown;
                }
                (IdentKind::Trait, ResolvedType::Named(name.to_string()))
            }
            SymbolKind::RustItem(_) => {
                return self.check_rust_identifier(name, sym_id, span, source_target);
            }
        };
        self.finish_identifier_read(span, kind, ty, source_target)
    }

    /// Check a type binding as a receiver or explicit type-token value. Scoped builtin placeholders retain their
    /// type-variable meaning; a bare type name remains a named refusal and records no successful source target.
    fn check_type_identifier(
        &mut self,
        name: &str,
        sym_id: SymbolId,
        span: Span,
        source_target: Option<SourceTargetInfo>,
    ) -> ResolvedType {
        let Some(sym) = self.symbols.get(sym_id) else {
            self.errors.push(errors::unknown_symbol(name, span));
            return ResolvedType::Unknown;
        };
        let SymbolKind::Type(info) = &sym.kind else {
            self.errors.push(errors::unknown_symbol(name, span));
            return ResolvedType::Unknown;
        };
        if !self.is_type_receiver_span(span) {
            if !self.is_type_token_value_span(span) {
                self.errors.push(errors::type_name_used_as_value(name, span));
                self.type_info.expressions.ident_kinds.insert(span, IdentKind::TypeName);
                return ResolvedType::Unknown;
            }
            let ty = if matches!(info, TypeInfo::Builtin) && sym.scope > 0 {
                ResolvedType::TypeVar(name.to_string())
            } else {
                resolve_type(&Type::Simple(name.to_string()), &self.symbols)
            };
            return self.finish_identifier_read(
                span,
                IdentKind::Value,
                ResolvedType::TypeToken(Box::new(ty)),
                source_target,
            );
        }
        let ty = if matches!(info, TypeInfo::Builtin) && sym.scope > 0 {
            ResolvedType::TypeVar(name.to_string())
        } else {
            ResolvedType::Named(name.to_string())
        };
        self.finish_identifier_read(span, IdentKind::TypeName, ty, source_target)
    }

    /// Resolve a Rust item through the same stable symbol ID, retaining canonical constants before signature
    /// resolution and publishing function-value metadata only after immutable symbol inspection has finished.
    fn check_rust_identifier(
        &mut self,
        name: &str,
        sym_id: SymbolId,
        span: Span,
        source_target: Option<SourceTargetInfo>,
    ) -> ResolvedType {
        let Some(Symbol {
            kind: SymbolKind::RustItem(info),
            ..
        }) = self.symbols.get(sym_id)
        else {
            self.errors.push(errors::unknown_symbol(name, span));
            return ResolvedType::Unknown;
        };
        let mut native_function_value = None;
        if let Some(meta) = &info.metadata
            && meta.visibility == incan_lang::interop::RustVisibility::Restricted
        {
            self.errors
                .push(errors::rust_item_not_public(name, meta.canonical_path.as_str(), span));
            self.type_info
                .expressions
                .ident_kinds
                .insert(span, IdentKind::RustImport);
            return ResolvedType::Unknown;
        }
        let kind = match &info.metadata {
            Some(meta) if matches!(meta.kind, incan_lang::interop::RustItemKind::Constant { .. }) => {
                IdentKind::RustValue
            }
            None if rust_path_last_segment_looks_like_const(info.path.as_str()) => IdentKind::RustValue,
            _ => IdentKind::RustImport,
        };
        let ty = match &info.metadata {
            Some(meta) => match &meta.kind {
                incan_lang::interop::RustItemKind::Function(sig) => {
                    native_function_value = Some((info.path.clone(), sig.clone()));
                    self.resolved_function_type_from_rust_sig_for_owner_path(sig, false, info.path.as_str())
                }
                incan_lang::interop::RustItemKind::Constant { type_display } => {
                    self.type_info.rust.checked_constants.insert(span, info.path.clone());
                    self.native_rust_signature_type(type_display.as_str(), info.path.as_str())
                }
                incan_lang::interop::RustItemKind::Unsupported { description } => {
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
        let ty = native_function_value
            .and_then(|(path, signature)| self.retain_rust_function_value(&path, &signature, span))
            .unwrap_or(ty);
        self.finish_identifier_read(span, kind, ty, source_target)
    }

    /// Publish a successful identifier read's classification before its source target, returning the already
    /// resolved type unchanged. Refused reads retain responsibility for their own narrower metadata.
    fn finish_identifier_read(
        &mut self,
        span: Span,
        kind: IdentKind,
        ty: ResolvedType,
        source_target: Option<SourceTargetInfo>,
    ) -> ResolvedType {
        self.type_info.expressions.ident_kinds.insert(span, kind);
        if let Some(target) = source_target {
            self.record_source_target(span, target.module_path, target.name, target.kind);
        }
        ty
    }

    /// Resolve a literal value to its type.
    pub(in crate::typechecker::check_expr) fn check_literal(&mut self, lit: &Literal, span: Span) -> ResolvedType {
        match lit {
            Literal::Int(value) => match value.suffix {
                Some(_) => self.check_suffixed_int_literal(value, false, span),
                None => ResolvedType::Int,
            },
            Literal::Float(value) => match value.suffix {
                Some(_) => self.check_suffixed_float_literal(value, span),
                None => ResolvedType::Float,
            },
            Literal::Decimal(value) => {
                self.errors.push(
                    CompileError::type_error(
                        format!("Decimal literal {} requires a decimal destination", value.repr),
                        span,
                    )
                    .with_hint("Declare a destination such as `value: decimal[10, 2] = 19.99d`"),
                );
                ResolvedType::Unknown
            }
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
    pub(in crate::typechecker::check_expr) fn check_self(&mut self, span: Span) -> ResolvedType {
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
