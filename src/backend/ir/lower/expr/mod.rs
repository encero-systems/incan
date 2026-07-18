//! Expression lowering for AST to IR conversion.
//!
//! This module handles lowering of all expression types: literals, identifiers, binary/unary operations, function
//! calls, method calls, comprehensions, etc.
//!
//! Large helpers (calls, patterns, comprehensions, pow helpers) are split into submodules; all methods live on `impl
//! AstLowering`.

mod calls;
mod comprehensions;
mod helpers;
mod patterns;

use std::collections::HashMap;

use super::super::TypedExpr;
use super::super::expr::{
    BuiltinFn, CollectionMethodKind, IrCallArg, IrCallArgKind, IrDictEntry, IrExpr, IrExprKind, IrListEntry,
    IrMethodDispatch, Literal as IrLiteral, MethodCallArgPolicy, MethodKind, NumericResizePolicy, RaceArm, UnaryOp,
    VarAccess, VarRefKind,
};
use super::super::types::IrType;
use super::AstLowering;
use super::errors::LoweringError;
use crate::frontend::ast::{self, Spanned};
use crate::frontend::library_manifest_index::LibraryManifestIndexEntry;
use crate::frontend::partial_projection::{PartialPresetRef, merge_named_partial_args};
use crate::frontend::typechecker::{
    IdentKind, PartialProjectionTargetKind, ResolvedMethodDispatch, ResolvedOperatorKind,
};
use incan_core::interop::RustCollectionFamily;
use incan_core::lang::magic_methods::{self, MagicMethodId};
use incan_core::lang::surface::collection_helpers::{self, BuiltinCollectionHelperId};
use incan_core::lang::surface::result_methods::ResultMethodId;
use incan_core::lang::surface::types::{self as surface_types, SurfaceTypeId, TASK_JOIN_ERROR_TYPE_NAME};
use incan_core::lang::traits::{self as builtin_traits, TraitId};
use incan_core::lang::types::collections::{self as collection_types, CollectionTypeId};
use incan_semantics_core::SurfaceExprLoweringAction;

impl AstLowering {
    /// Return the public dependency owner for a method receiver when lowering can prove one.
    fn public_library_for_method_receiver(&self, receiver: &TypedExpr) -> Option<String> {
        match &receiver.kind {
            IrExprKind::Call {
                canonical_path: Some(path),
                ..
            } => Self::public_library_from_canonical_path(path),
            IrExprKind::InteropCoerce { expr, .. } => self.public_library_for_method_receiver(expr),
            _ => self.public_library_for_nominal_receiver_type(&receiver.ty),
        }
    }

    /// Return the library key from a canonical `pub::<library>::...` path.
    fn public_library_from_canonical_path(path: &[String]) -> Option<String> {
        if path.first().map(String::as_str) == Some("pub") {
            path.get(1).cloned()
        } else {
            None
        }
    }

    /// Return the public dependency owner for an explicitly imported nominal receiver type.
    ///
    /// If the receiver type is not directly imported, this falls back only when exactly one loaded public dependency
    /// exports that nominal type and no local declaration shadows it.
    fn public_library_for_nominal_receiver_type(&self, ty: &IrType) -> Option<String> {
        let name = match ty {
            IrType::Struct(name) | IrType::Enum(name) | IrType::NamedGeneric(name, _) => {
                name.rsplit("::").next().unwrap_or(name)
            }
            IrType::Ref(inner) | IrType::RefMut(inner) => {
                return self.public_library_for_nominal_receiver_type(inner);
            }
            _ => return None,
        };

        if let Some(path) = self.import_aliases.get(name)
            && path.first().map(String::as_str) == Some("pub")
        {
            return path.get(1).cloned();
        }

        if self.struct_names.contains_key(name) || self.enum_names.contains_key(name) {
            return None;
        }

        let manifest_index = self.provider_plan.as_deref()?.library_manifest_index();
        let matches = manifest_index
            .known_libraries()
            .into_iter()
            .filter(|library| {
                let Some(LibraryManifestIndexEntry::Loaded { manifest, .. }) = manifest_index.get(library) else {
                    return false;
                };
                manifest.exports.models.iter().any(|item| item.name == name)
                    || manifest.exports.classes.iter().any(|item| item.name == name)
                    || manifest.exports.newtypes.iter().any(|item| item.name == name)
                    || manifest.exports.enums.iter().any(|item| item.name == name)
            })
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [library] => Some(library.clone()),
            _ => None,
        }
    }

    /// Return the source-defined `std.logging.Logger.<method>` signature, including default expressions.
    fn std_logging_logger_method_signature(
        &mut self,
        method: &str,
    ) -> Result<Option<super::super::FunctionSignature>, LoweringError> {
        self.callable_signature_for_imported_stdlib_type_method_path(
            &["std".to_string(), "logging".to_string(), "Logger".to_string()],
            method,
        )
    }

    /// Merge typechecker call-site metadata with the source-defined `std.logging.Logger` method declaration.
    ///
    /// The call-site snapshot carries the selected parameter types; the stdlib declaration carries source defaults such
    /// as `fields={}`. Keeping the merge here lets emission stay independent from logging-specific method names.
    fn std_logging_callable_signature_for_call(
        &mut self,
        span: ast::Span,
        method: &str,
    ) -> Result<Option<super::super::FunctionSignature>, LoweringError> {
        let call_site = self.callable_signature_for_call_span(span);
        let stdlib = self.std_logging_logger_method_signature(method)?;
        match (call_site, stdlib) {
            (Some(mut call_site), Some(stdlib)) => {
                for (param, stdlib_param) in call_site.params.iter_mut().zip(stdlib.params.iter()) {
                    if param.default.is_none() {
                        param.default = stdlib_param.default.clone();
                    }
                }
                Ok(Some(call_site))
            }
            (Some(call_site), None) => Ok(Some(call_site)),
            (None, stdlib) => Ok(stdlib),
        }
    }

    /// Lower `race for value:` to the IR race expression used by Rust emission.
    fn lower_race_for_expr(
        &mut self,
        race: &ast::RaceForExpr,
        expr_span: ast::Span,
    ) -> Result<TypedExpr, LoweringError> {
        let result_ty = self
            .type_info
            .as_ref()
            .and_then(|info| info.expr_type(expr_span))
            .map(|ty| self.lower_resolved_type(ty))
            .unwrap_or(IrType::Unknown);

        let mut arms = Vec::with_capacity(race.arms.len());
        for arm in &race.arms {
            let awaitable = self.lower_awaitable_operand(&arm.awaitable)?;
            let binding_ty = Self::race_binding_type_for_awaitable(&awaitable);

            self.push_scope();
            self.define_local_binding(race.binding.clone(), binding_ty, false);
            let body_result = self.lower_race_arm_body(&arm.body);
            self.pop_scope();

            arms.push(RaceArm {
                awaitable,
                body: body_result?,
            });
        }

        Ok(TypedExpr::new(
            IrExprKind::Race {
                binding: race.binding.clone(),
                arms,
            },
            result_ty,
        ))
    }

    /// Lower one race arm body with the arm-local winner binding already in scope.
    fn lower_race_arm_body(&mut self, body: &ast::RaceForBody) -> Result<TypedExpr, LoweringError> {
        match body {
            ast::RaceForBody::Expr(expr) => self.lower_expr_spanned(expr),
            ast::RaceForBody::Block(stmts) => self.lower_race_arm_block_body(stmts),
        }
    }

    /// Lower an await operand, applying typechecker-proven wrapper delegation when a concrete `Awaitable[T]` wrapper
    /// delegates to one awaitable field.
    fn lower_awaitable_operand(&mut self, operand: &Spanned<ast::Expr>) -> Result<TypedExpr, LoweringError> {
        let lowered = self.lower_expr_spanned(operand)?;
        let Some(field) = self.awaitable_delegation_field_for_span(operand.span) else {
            return Ok(lowered);
        };
        Ok(TypedExpr::new(
            IrExprKind::Field {
                object: Box::new(lowered),
                field,
            },
            IrType::Unknown,
        ))
    }

    /// Return the delegated field name for an expression whose resolved type is a wrapper `Awaitable[T]`.
    fn awaitable_delegation_field_for_span(&self, span: ast::Span) -> Option<String> {
        let type_info = self.type_info.as_ref()?;
        let expr_ty = type_info.expr_type(span)?;
        let type_name = match expr_ty {
            crate::frontend::symbols::ResolvedType::Named(name)
            | crate::frontend::symbols::ResolvedType::Generic(name, _) => name,
            crate::frontend::symbols::ResolvedType::Ref(inner)
            | crate::frontend::symbols::ResolvedType::RefMut(inner) => match inner.as_ref() {
                crate::frontend::symbols::ResolvedType::Named(name)
                | crate::frontend::symbols::ResolvedType::Generic(name, _) => name,
                _ => return None,
            },
            _ => return None,
        };
        type_info
            .expressions
            .awaitable_delegation_fields
            .get(type_name)
            .cloned()
    }

    /// Lower a block race arm, treating a trailing expression statement as the arm value.
    fn lower_race_arm_block_body(&mut self, stmts: &[Spanned<ast::Statement>]) -> Result<TypedExpr, LoweringError> {
        let Some((last, prefix)) = stmts.split_last() else {
            return Ok(TypedExpr::new(
                IrExprKind::Block {
                    stmts: Vec::new(),
                    value: None,
                },
                IrType::Unit,
            ));
        };

        let lowered_stmts = self.lower_statements(prefix)?;
        if let ast::Statement::Expr(expr) = &last.node {
            let value = self.lower_expr_spanned(expr)?;
            let ty = value.ty.clone();
            Ok(TypedExpr::new(
                IrExprKind::Block {
                    stmts: lowered_stmts,
                    value: Some(Box::new(value)),
                },
                ty,
            ))
        } else {
            let mut lowered_stmts = lowered_stmts;
            lowered_stmts.push(self.lower_statement(&last.node, last.span)?);
            Ok(TypedExpr::new(
                IrExprKind::Block {
                    stmts: lowered_stmts,
                    value: None,
                },
                IrType::Unit,
            ))
        }
    }

    /// Infer the arm-local winner binding type from a lowered awaitable expression.
    fn race_binding_type_for_awaitable(awaitable: &TypedExpr) -> IrType {
        match &awaitable.ty {
            IrType::NamedGeneric(name, args)
                if surface_types::from_str(name.as_str()) == Some(SurfaceTypeId::JoinHandle) && args.len() == 1 =>
            {
                IrType::Result(
                    Box::new(args[0].clone()),
                    Box::new(IrType::Struct(TASK_JOIN_ERROR_TYPE_NAME.to_string())),
                )
            }
            IrType::NamedGeneric(name, args)
                if builtin_traits::from_str(name.as_str()) == Some(TraitId::Awaitable) && args.len() == 1 =>
            {
                args[0].clone()
            }
            _ => awaitable.ty.clone(),
        }
    }

    /// Lower `list.repeat(...)` arguments in canonical helper-parameter order.
    ///
    /// The surface helper accepts named arguments, but `BuiltinFn::ListRepeat` stores arguments positionally for
    /// emission, so lowering must bind `value` before `count` instead of preserving source order.
    fn lower_builtin_list_repeat_args(&mut self, args: &[ast::CallArg]) -> Result<Vec<TypedExpr>, LoweringError> {
        let mut value = None;
        let mut count = None;
        let mut positional_index = 0usize;

        for arg in args {
            match arg {
                ast::CallArg::Positional(expr) => {
                    match positional_index {
                        0 if value.is_none() => value = Some(expr),
                        1 if count.is_none() => count = Some(expr),
                        _ => {}
                    }
                    positional_index += 1;
                }
                ast::CallArg::Named(name, expr) => match name.as_str() {
                    "value" if value.is_none() => value = Some(expr),
                    "count" if count.is_none() => count = Some(expr),
                    _ => {}
                },
                ast::CallArg::PositionalUnpack(_) | ast::CallArg::KeywordUnpack(_) => {}
            }
        }

        let mut lowered = Vec::with_capacity(2);
        if let Some(value) = value {
            lowered.push(self.lower_expr_spanned(value)?);
        }
        if let Some(count) = count {
            lowered.push(self.lower_expr_spanned(count)?);
        }
        Ok(lowered)
    }

    /// Return whether a concrete receiver type explicitly adopts the Incan `Iterator` protocol.
    fn receiver_adopts_iterator_protocol(&self, ty: &IrType) -> bool {
        let mut ty = ty;
        while let IrType::Ref(inner) | IrType::RefMut(inner) = ty {
            ty = inner.as_ref();
        }
        match ty {
            IrType::Struct(name) | IrType::NamedGeneric(name, _) => self.iterator_adopter_names.contains(name),
            _ => false,
        }
    }

    /// Return whether a generic type parameter should keep Rust's comparison operators instead of lowering to a
    /// dunder-style method call.
    ///
    /// Generic `T with Ord`/`PartialOrd` bounds lower to Rust trait bounds such as `PartialOrd`; they do not introduce
    /// inherent `__lt__`/`__le__` methods on `T`. Keeping the operator form lets Rust type-check the generic bound.
    fn generic_comparison_uses_rust_operator(&self, left: &Spanned<ast::Expr>, method: &str) -> bool {
        if !matches!(method, "__ne__" | "__lt__" | "__le__" | "__gt__" | "__ge__") {
            return false;
        }
        self.type_info
            .as_ref()
            .and_then(|info| info.expr_type(left.span))
            .map(|ty| self.lower_resolved_type(ty))
            .is_some_and(|ty| matches!(ty, IrType::Generic(_)))
    }

    /// Lower a control-flow condition, rewriting validated `__bool__` hooks into direct method calls.
    pub(in crate::backend::ir::lower) fn lower_condition_expr(
        &mut self,
        expr: &Spanned<ast::Expr>,
    ) -> Result<TypedExpr, LoweringError> {
        let receiver = self.lower_expr_spanned(expr)?;
        if let Some(resolved_operator) = self
            .type_info
            .as_ref()
            .and_then(|info| info.resolved_operator_call(expr.span).cloned())
            && resolved_operator.kind == ResolvedOperatorKind::Truthiness
        {
            return Ok(TypedExpr::new(
                IrExprKind::MethodCall {
                    receiver: Box::new(receiver),
                    method: resolved_operator.method,
                    dispatch: None,
                    type_args: Vec::new(),
                    args: Vec::new(),
                    callable_signature: self.callable_signature_for_call_span(expr.span),
                    arg_policy: MethodCallArgPolicy::Default,
                },
                IrType::Bool,
            ));
        }
        Ok(receiver)
    }

    /// Return the element type carried by a lowered list spread operand.
    fn lowered_list_spread_element_type(ty: &IrType) -> Option<IrType> {
        match ty {
            IrType::List(elem) => Some((**elem).clone()),
            _ => None,
        }
    }

    /// Return the key/value types carried by a lowered dict spread operand.
    fn lowered_dict_spread_entry_types(ty: &IrType) -> Option<(IrType, IrType)> {
        match ty {
            IrType::Dict(key, value) => Some(((**key).clone(), (**value).clone())),
            _ => None,
        }
    }

    /// Classify an IR type as a Rust collection family.
    fn rust_collection_family_for_ir_type(ty: &IrType) -> Option<RustCollectionFamily> {
        match ty {
            IrType::Struct(name) | IrType::NamedGeneric(name, _) => {
                RustCollectionFamily::for_canonical_path(name).or(RustCollectionFamily::for_type_name(name))
            }
            IrType::Ref(inner) | IrType::RefMut(inner) => Self::rust_collection_family_for_ir_type(inner),
            _ => None,
        }
    }

    /// Return the ordinary argument policy for a method call.
    fn regular_method_call_arg_policy(
        &self,
        receiver_span: crate::frontend::ast::Span,
        receiver: &TypedExpr,
        method: &str,
        args: &[IrCallArg],
    ) -> MethodCallArgPolicy {
        if self
            .type_info
            .as_ref()
            .is_some_and(|info| info.preserves_regular_method_arg_shape(receiver_span, method))
        {
            return MethodCallArgPolicy::PreserveShape;
        }

        if Self::rust_collection_family_for_ir_type(&receiver.ty)
            .is_some_and(|family| family.preserves_lookup_arg_shape(method))
        {
            return MethodCallArgPolicy::PreserveShape;
        }

        // Fallback for unresolved Rust-interop receivers when optional rust-inspect metadata is unavailable or local
        // type inference did not retain the receiver family. Keep lookup calls like `counts.get(word)` borrow-shaped
        // rather than forcing an extra `&`/`.into()` conversion on already string-like probe values.
        if matches!(receiver.ty, IrType::Unknown)
            && matches!(method, "get" | "contains" | "contains_key")
            && args.first().is_some_and(|arg| {
                matches!(
                    arg.expr.ty,
                    IrType::String | IrType::StrRef | IrType::StaticStr | IrType::FrozenStr
                )
            })
        {
            return MethodCallArgPolicy::PreserveShape;
        }

        MethodCallArgPolicy::Default
    }

    /// Lower an expression using the available typechecker output (if present).
    ///
    /// This wraps [`Self::lower_expr`] and then overrides the inferred IR type using the typechecker span-to-type map.
    /// This is a stepping stone toward fully typed lowering.
    pub fn lower_expr_spanned(&mut self, expr: &Spanned<ast::Expr>) -> Result<TypedExpr, LoweringError> {
        if let Some((kind, ty)) = self.lower_partial_constructor_call(expr)? {
            return Ok(TypedExpr::new(kind, ty));
        }
        let mut lowered = self.lower_expr(&expr.node, expr.span)?;
        if let Some(info) = &self.type_info
            && let Some(res_ty) = info.expr_type(expr.span)
        {
            // Preserve reference wrappers introduced by lowering (e.g. mutable parameters are tracked as
            // `RefMut(T)` in IR), while still benefiting from the typechecker's inner type information.
            //
            // The frontend type system does not model references, so `expr_type` typically returns `T` where
            // lowering may have already marked the same binding as `Ref(T)`/`RefMut(T)`.
            //
            // Likewise, RFC-008 const lowering may have already refined `str`/`bytes` to their static IR forms.
            // Keep those backend-specific const representations intact so later emission can materialize owned
            // values only when required.
            let inferred = self.lower_resolved_type(res_ty);
            lowered.ty = match &lowered.ty {
                IrType::Ref(existing_inner) => {
                    IrType::Ref(Box::new(Self::merge_inferred_ir_type(existing_inner, inferred)))
                }
                IrType::RefMut(existing_inner) => {
                    IrType::RefMut(Box::new(Self::merge_inferred_ir_type(existing_inner, inferred)))
                }
                IrType::StaticStr => IrType::StaticStr,
                IrType::StaticBytes => IrType::StaticBytes,
                existing => Self::merge_inferred_ir_type(existing, inferred),
            };
        }
        if matches!(expr.node, ast::Expr::Ident(_))
            && let IrType::TypeToken(inner) = &lowered.ty
        {
            lowered.kind = IrExprKind::TypeToken {
                ty: inner.as_ref().clone(),
            };
        }
        if let Some(kind) = self.ident_kind_for_lowering(expr) {
            match (&expr.node, &mut lowered.kind) {
                (ast::Expr::Ident(name), _) if matches!(kind, IdentKind::Static) => {
                    lowered.kind = IrExprKind::StaticRead { name: name.clone() };
                }
                (ast::Expr::Ident(name), IrExprKind::Var { ref_kind, .. }) => {
                    *ref_kind = match kind {
                        IdentKind::Value => *ref_kind,
                        IdentKind::Static => *ref_kind,
                        IdentKind::TypeName => VarRefKind::TypeName,
                        IdentKind::Variant => VarRefKind::TypeName,
                        IdentKind::Module => VarRefKind::ExternalName,
                        IdentKind::RustImport => VarRefKind::ExternalRustName,
                        IdentKind::RustValue => VarRefKind::Value,
                        IdentKind::Trait => VarRefKind::TypeName,
                    };
                    if matches!(kind, IdentKind::TypeName | IdentKind::Variant | IdentKind::Trait)
                        && matches!(lowered.ty, IrType::Unknown)
                        && let Some(ty) = self.synthetic_type_ident_ir_type(name)
                    {
                        lowered.ty = ty;
                    }
                }
                (_, IrExprKind::Var { ref_kind, .. }) => {
                    *ref_kind = match kind {
                        IdentKind::Value => *ref_kind,
                        IdentKind::Static => *ref_kind,
                        IdentKind::TypeName => VarRefKind::TypeName,
                        IdentKind::Variant => VarRefKind::TypeName,
                        IdentKind::Module => VarRefKind::ExternalName,
                        IdentKind::RustImport => VarRefKind::ExternalRustName,
                        IdentKind::RustValue => VarRefKind::Value,
                        IdentKind::Trait => VarRefKind::TypeName,
                    };
                }
                _ => {}
            }
        }
        // Apply any rusttype method return coercion recorded by the typechecker (e.g. &str → String).
        lowered = self.wrap_with_rust_return_coercion(lowered, expr.span)?;
        // Apply RFC 017 implicit validated-newtype coercions at typechecker-approved destination sites.
        lowered = self.wrap_with_validated_newtype_coercion(lowered, expr.span)?;
        Ok(lowered)
    }

    /// Lower a known model-constructor partial call through the ordinary constructor lowering path.
    fn lower_partial_constructor_call(
        &mut self,
        expr: &Spanned<ast::Expr>,
    ) -> Result<Option<(IrExprKind, IrType)>, LoweringError> {
        let ast::Expr::Call(callee, type_args, args) = &expr.node else {
            return Ok(None);
        };
        if !type_args.is_empty() {
            return Ok(None);
        }
        let ast::Expr::Ident(callee_name) = &callee.node else {
            return Ok(None);
        };
        let Some(projection) = self
            .type_info
            .as_ref()
            .and_then(|info| info.partial_projection(callee_name))
            .cloned()
        else {
            return Ok(None);
        };
        if projection.target_kind != PartialProjectionTargetKind::ModelConstructor {
            return Ok(None);
        }
        let Some(target_name) = projection.target_path.last() else {
            return Ok(None);
        };
        if !self.struct_names.contains_key(target_name) && !self.import_aliases.contains_key(target_name) {
            return Ok(None);
        }
        let Some(merged_args) = merge_named_partial_args(
            projection.presets.iter().map(|preset| PartialPresetRef {
                name: preset.name.as_str(),
                value: &preset.value,
            }),
            args,
        ) else {
            return Ok(None);
        };
        self.lower_constructor_call(target_name, &[], &merged_args, expr.span)
            .map(Some)
    }

    /// Return the identifier classification that lowering should use for this expression.
    ///
    /// Most source expressions use span-keyed frontend metadata. Synthetic expressions created by lowering, such as
    /// user-defined decorator factory calls, intentionally use the default span so they do not collide with call-site
    /// expression types. Those synthetic nodes still need metadata-backed classification for type names and module
    /// statics; otherwise they fall back to value-shaped Rust emission.
    fn ident_kind_for_lowering(&self, expr: &Spanned<ast::Expr>) -> Option<IdentKind> {
        if let Some(kind) = self.type_info.as_ref().and_then(|info| info.ident_kind(expr.span)) {
            return Some(kind);
        }
        if expr.span != ast::Span::default() {
            return None;
        }
        let ast::Expr::Ident(name) = &expr.node else {
            return None;
        };
        if self
            .type_info
            .as_ref()
            .is_some_and(|info| info.static_binding(name).is_some())
        {
            return Some(IdentKind::Static);
        }
        if self.synthetic_type_ident_ir_type(name).is_some() {
            return Some(IdentKind::TypeName);
        }
        None
    }

    /// Return the known IR type for a synthetic type-like identifier.
    fn synthetic_type_ident_ir_type(&self, name: &str) -> Option<IrType> {
        self.struct_names
            .get(name)
            .cloned()
            .or_else(|| self.enum_names.get(name).cloned())
            .or_else(|| {
                self.class_decls
                    .contains_key(name)
                    .then(|| IrType::Struct(name.to_string()))
            })
            .or_else(|| {
                self.trait_decls
                    .contains_key(name)
                    .then(|| IrType::Struct(name.to_string()))
            })
    }

    /// Lower an expression to IR.
    ///
    /// Handles all expression types including:
    /// - Literals (int, float, string, bool)
    /// - Identifiers (variable references)
    /// - Binary and unary operations
    /// - Function and method calls
    /// - Field and index access
    /// - Control flow expressions (if, match)
    /// - Collections (list, dict, set, tuple)
    /// - Comprehensions (list, dict)
    /// - Closures and async/await
    ///
    /// `expr_span` must be the span of the whole `expr` node (as in [`Self::lower_expr_spanned`]). It is required for
    /// [`Expr::Call`](ast::Expr::Call) and [`Expr::MethodCall`](ast::Expr::MethodCall) so lowering can align with the
    /// typechecker’s span-keyed metadata (RFC 054 monomorph snapshots).
    pub fn lower_expr(&mut self, expr: &ast::Expr, expr_span: ast::Span) -> Result<TypedExpr, LoweringError> {
        let (kind, ty) = match expr {
            // ---- Identifiers ----
            ast::Expr::Ident(name) => {
                let lowered_name = self.symbol_aliases.get(name).cloned().unwrap_or_else(|| name.clone());
                let ty = self.lookup_var(&lowered_name);
                if self
                    .type_info
                    .as_ref()
                    .is_some_and(|info| info.is_ambient_logger_binding(expr_span))
                {
                    let logger_name = self.current_default_logger_name();
                    let func = TypedExpr::new(
                        IrExprKind::Var {
                            name: "get_logger".to_string(),
                            access: VarAccess::Copy,
                            ref_kind: VarRefKind::Value,
                        },
                        IrType::Unknown,
                    );
                    let arg = IrCallArg {
                        name: None,
                        kind: IrCallArgKind::Positional,
                        expr: TypedExpr::new(
                            IrExprKind::Literal(IrLiteral::StaticStr(logger_name)),
                            IrType::StaticStr,
                        ),
                    };
                    return Ok(TypedExpr::new(
                        IrExprKind::Call {
                            func: Box::new(func),
                            type_args: Vec::new(),
                            args: vec![arg],
                            callable_signature: self.callable_signature_for_imported_stdlib_path(&[
                                "std".to_string(),
                                "logging".to_string(),
                                "get_logger".to_string(),
                            ])?,
                            canonical_path: Some(vec![
                                "std".to_string(),
                                "logging".to_string(),
                                "get_logger".to_string(),
                            ]),
                        },
                        IrType::Struct("Logger".to_string()),
                    ));
                }
                // Imported string-like bindings are dependency-owned path references, not local owned strings that can
                // be consumed by the current block's last-use analysis.
                let inferred_import_ty = self
                    .type_info
                    .as_ref()
                    .and_then(|info| info.expr_type(expr_span).cloned())
                    .map(|ty| self.lower_resolved_type(&ty));
                let access = if self.import_aliases.contains_key(name)
                    && matches!(
                        inferred_import_ty.as_ref().unwrap_or(&ty),
                        IrType::String | IrType::StaticStr | IrType::StrRef | IrType::FrozenStr
                    ) {
                    VarAccess::Read
                } else {
                    self.select_var_access_for_ident(&lowered_name, &ty)
                };
                (
                    IrExprKind::Var {
                        name: lowered_name.clone(),
                        access,
                        ref_kind: if self.is_static_binding(&lowered_name) {
                            VarRefKind::StaticBinding
                        } else {
                            VarRefKind::Value
                        },
                    },
                    ty,
                )
            }

            // ---- Literals ----
            ast::Expr::Literal(lit) => match lit {
                ast::Literal::Int(il) if il.fits_i64() => (IrExprKind::Int(il.value), IrType::Int),
                ast::Literal::Int(il) => (IrExprKind::IntLiteral(il.repr.clone()), IrType::Int),
                ast::Literal::Float(fl) => (IrExprKind::Float(fl.value), IrType::Float),
                ast::Literal::Decimal(dl) => (IrExprKind::Decimal(dl.repr.clone()), IrType::Unknown),
                ast::Literal::String(s) => {
                    let ty = self
                        .type_info
                        .as_ref()
                        .and_then(|info| info.expr_type(expr_span))
                        .map(|ty| self.lower_resolved_type(ty))
                        .filter(|ty| matches!(ty, IrType::FrozenStr | IrType::StaticStr | IrType::StrRef))
                        .unwrap_or(IrType::String);
                    (IrExprKind::String(s.clone()), ty)
                }
                ast::Literal::Bytes(bytes) => {
                    let ty = self
                        .type_info
                        .as_ref()
                        .and_then(|info| info.expr_type(expr_span))
                        .map(|ty| self.lower_resolved_type(ty))
                        .filter(|ty| matches!(ty, IrType::FrozenBytes | IrType::StaticBytes))
                        .unwrap_or(IrType::Bytes);
                    (IrExprKind::Bytes(bytes.clone()), ty)
                }
                ast::Literal::Bool(b) => (IrExprKind::Bool(*b), IrType::Bool),
                ast::Literal::None => (IrExprKind::None, IrType::Option(Box::new(IrType::Unknown))),
            },

            // ---- Self expression ----
            ast::Expr::SelfExpr => (
                IrExprKind::Var {
                    name: "self".to_string(),
                    access: VarAccess::Borrow,
                    ref_kind: VarRefKind::Value,
                },
                IrType::Unknown,
            ),

            // ---- Binary operations ----
            ast::Expr::Binary(l, op, r) => {
                if let Some(resolved_operator) = self
                    .type_info
                    .as_ref()
                    .and_then(|info| info.resolved_operator_call(expr_span).cloned())
                    && resolved_operator.kind == ResolvedOperatorKind::Binary
                    // `__eq__` is represented in generated Rust as `PartialEq::eq`, not as an inherent method.
                    && resolved_operator.method != magic_methods::as_str(MagicMethodId::Eq)
                    && !self.generic_comparison_uses_rust_operator(l, &resolved_operator.method)
                {
                    let receiver = self.lower_expr_spanned(l)?;
                    let arg_expr = self.lower_expr_spanned(r)?;
                    let provider_crate = AstLowering::nominal_receiver_type_name(&receiver.ty)
                        .and_then(|type_name| self.sdk_provider_crate_for_type(type_name));
                    let result_ty = self
                        .type_info
                        .as_ref()
                        .and_then(|info| info.expr_type(expr_span))
                        .map(|ty| self.lower_resolved_type(ty))
                        .unwrap_or(IrType::Unknown);
                    let result_ty = provider_crate
                        .as_deref()
                        .map(|provider_crate| self.pub_external_type(provider_crate, result_ty.clone()))
                        .unwrap_or(result_ty);
                    let callable_signature = self.callable_signature_for_call_span(expr_span);
                    let callable_signature = match (provider_crate.as_deref(), callable_signature) {
                        (Some(provider_crate), Some(signature)) => {
                            Some(self.compiled_provider_external_signature(provider_crate, signature))
                        }
                        (_, signature) => signature,
                    };
                    (
                        IrExprKind::MethodCall {
                            receiver: Box::new(receiver),
                            method: resolved_operator.method,
                            dispatch: None,
                            type_args: Vec::new(),
                            args: vec![IrCallArg {
                                name: None,
                                kind: IrCallArgKind::Positional,
                                expr: arg_expr,
                            }],
                            callable_signature,
                            arg_policy: MethodCallArgPolicy::Default,
                        },
                        result_ty,
                    )
                } else if let Some(resolved_operator) = self
                    .type_info
                    .as_ref()
                    .and_then(|info| info.resolved_operator_call(expr_span).cloned())
                    && resolved_operator.kind == ResolvedOperatorKind::Contains
                {
                    let item = self.lower_expr_spanned(l)?;
                    let receiver = self.lower_expr_spanned(r)?;
                    let contains_call = IrExprKind::MethodCall {
                        receiver: Box::new(receiver),
                        method: resolved_operator.method,
                        dispatch: None,
                        type_args: Vec::new(),
                        args: vec![IrCallArg {
                            name: None,
                            kind: IrCallArgKind::Positional,
                            expr: item,
                        }],
                        callable_signature: self.callable_signature_for_call_span(expr_span),
                        arg_policy: MethodCallArgPolicy::Default,
                    };
                    if matches!(op, ast::BinaryOp::NotIn) {
                        (
                            IrExprKind::UnaryOp {
                                op: UnaryOp::Not,
                                operand: Box::new(IrExpr::new(contains_call, IrType::Bool)),
                            },
                            IrType::Bool,
                        )
                    } else {
                        (contains_call, IrType::Bool)
                    }
                } else {
                    // Special handling for `in` and `not in` operators
                    // - `x in collection` → builtin-aware `collection.contains(x)`
                    // - `x not in collection` → `!collection.contains(x)`
                    match op {
                        ast::BinaryOp::In | ast::BinaryOp::NotIn => {
                            let item = self.lower_expr_spanned(l)?;
                            let collection = self.lower_expr_spanned(r)?;

                            // Generate `collection.contains(item)` using the same receiver-aware classification path as
                            // ordinary method syntax so containment keeps builtin semantics for strings, lists, sets,
                            // and dicts without emitter-side name guessing.
                            let contains_args = vec![IrCallArg {
                                name: None,
                                kind: IrCallArgKind::Positional,
                                expr: item,
                            }];
                            let contains_kind = MethodKind::for_receiver(&collection.ty, "contains").or_else(|| {
                                let mut receiver_ty = &collection.ty;
                                while let IrType::Ref(inner) | IrType::RefMut(inner) = receiver_ty {
                                    receiver_ty = inner.as_ref();
                                }
                                matches!(receiver_ty, IrType::Dict(_, _))
                                    .then_some(MethodKind::Collection(CollectionMethodKind::Contains))
                            });
                            let contains_call = if let Some(kind) = contains_kind {
                                IrExprKind::KnownMethodCall {
                                    receiver: Box::new(collection),
                                    kind,
                                    args: contains_args,
                                }
                            } else {
                                let arg_policy = self.regular_method_call_arg_policy(
                                    r.span,
                                    &collection,
                                    "contains",
                                    &contains_args,
                                );
                                IrExprKind::MethodCall {
                                    receiver: Box::new(collection),
                                    method: "contains".to_string(),
                                    dispatch: None,
                                    type_args: Vec::new(),
                                    args: contains_args,
                                    callable_signature: None,
                                    arg_policy,
                                }
                            };

                            if matches!(op, ast::BinaryOp::NotIn) {
                                // Wrap in negation for `not in`
                                (
                                    IrExprKind::UnaryOp {
                                        op: UnaryOp::Not,
                                        operand: Box::new(IrExpr::new(contains_call, IrType::Bool)),
                                    },
                                    IrType::Bool,
                                )
                            } else {
                                (contains_call, IrType::Bool)
                            }
                        }
                        _ => {
                            let left = self.lower_expr_spanned(l)?;
                            let right = self.lower_expr_spanned(r)?;
                            // For Pow, compute exponent kind for policy-based result type
                            let pow_exp_kind = if matches!(op, ast::BinaryOp::Pow) {
                                Some(Self::pow_exponent_kind(r, &right.ty))
                            } else {
                                None
                            };
                            let result_ty = self.binary_result_type(&left.ty, &right.ty, op, pow_exp_kind);
                            (
                                IrExprKind::BinOp {
                                    op: self.lower_binop(op, expr_span)?,
                                    left: Box::new(left),
                                    right: Box::new(right),
                                },
                                result_ty,
                            )
                        }
                    }
                }
            }

            // ---- Unary operations ----
            ast::Expr::Unary(op, e) => {
                let operand = self.lower_expr_spanned(e)?;
                if let Some(resolved_operator) = self
                    .type_info
                    .as_ref()
                    .and_then(|info| info.resolved_operator_call(expr_span).cloned())
                    && resolved_operator.kind == ResolvedOperatorKind::Unary
                {
                    let result_ty = self
                        .type_info
                        .as_ref()
                        .and_then(|info| info.expr_type(expr_span))
                        .map(|ty| self.lower_resolved_type(ty))
                        .unwrap_or(IrType::Unknown);
                    (
                        IrExprKind::MethodCall {
                            receiver: Box::new(operand),
                            method: resolved_operator.method,
                            dispatch: None,
                            type_args: Vec::new(),
                            args: Vec::new(),
                            callable_signature: self.callable_signature_for_call_span(expr_span),
                            arg_policy: MethodCallArgPolicy::Default,
                        },
                        result_ty,
                    )
                } else {
                    let ty = operand.ty.clone();
                    (
                        IrExprKind::UnaryOp {
                            op: match op {
                                ast::UnaryOp::Neg => UnaryOp::Neg,
                                ast::UnaryOp::Not => UnaryOp::Not,
                                ast::UnaryOp::Invert => UnaryOp::Not,
                            },
                            operand: Box::new(operand),
                        },
                        ty,
                    )
                }
            }

            // ---- Function / constructor calls (delegated to calls submodule) ----
            ast::Expr::Call(f, type_args, args) => {
                return self
                    .lower_call_expr(f, type_args, args, expr_span)
                    .map(|(k, t)| TypedExpr::new(k, t));
            }

            // ---- Method calls ----
            ast::Expr::MethodCall(o, m, type_args, args) => {
                if self.imported_module_function_callee_path(&o.node, m).is_some() {
                    let callee = ast::Spanned::new(ast::Expr::Field(o.clone(), m.clone()), expr_span);
                    return self
                        .lower_call_expr(&callee, type_args, args, expr_span)
                        .map(|(kind, ty)| TypedExpr::new(kind, ty));
                }

                if Self::is_explicit_builtin_namespace_expr(o)
                    && let Some(builtin) = BuiltinFn::from_name(m)
                {
                    let args_ir = self.lower_call_args(args)?.into_iter().map(|a| a.expr).collect();
                    return Ok(TypedExpr::new(
                        IrExprKind::BuiltinCall {
                            func: builtin,
                            args: args_ir,
                        },
                        IrType::Unknown,
                    ));
                }

                if matches!(&o.node, ast::Expr::Ident(name)
                    if collection_helpers::from_parts(name, m) == Some(BuiltinCollectionHelperId::ListRepeat)
                        && collection_types::from_str(name.as_str()) == Some(CollectionTypeId::List))
                    && self
                        .type_info
                        .as_ref()
                        .is_some_and(|info| matches!(info.ident_kind(o.span), Some(IdentKind::TypeName)))
                {
                    let args_ir = self.lower_builtin_list_repeat_args(args)?;
                    let expr_ty = self
                        .type_info
                        .as_ref()
                        .and_then(|info| info.expr_type(expr_span))
                        .map(|ty| self.lower_resolved_type(ty))
                        .unwrap_or(IrType::Unknown);
                    return Ok(TypedExpr::new(
                        IrExprKind::BuiltinCall {
                            func: BuiltinFn::ListRepeat,
                            args: args_ir,
                        },
                        expr_ty,
                    ));
                }

                let receiver = if let ast::Expr::Index(base, _) = &o.node
                    && let ast::Expr::Ident(name) = &base.node
                    && self.type_info.as_ref().is_some_and(|info| {
                        matches!(info.ident_kind(base.span), Some(IdentKind::TypeName | IdentKind::Trait))
                    }) {
                    let ty = self
                        .type_info
                        .as_ref()
                        .and_then(|info| info.expr_type(o.span))
                        .map(|ty| self.lower_resolved_type(ty))
                        .unwrap_or_else(|| self.struct_names.get(name).cloned().unwrap_or(IrType::Unknown));
                    TypedExpr::new(
                        IrExprKind::Var {
                            name: name.clone(),
                            access: VarAccess::Copy,
                            ref_kind: VarRefKind::TypeName,
                        },
                        ty,
                    )
                } else {
                    self.lower_expr_spanned(o)?
                };
                let mut args_ir = self.lower_call_args(args)?;
                let lowered_type_args = self.lower_call_site_type_args(expr_span, type_args);
                let method_name = self.resolve_method_rebinding(&receiver.ty, m);
                let arg_policy = self.regular_method_call_arg_policy(o.span, &receiver, &method_name, &args_ir);
                if !matches!(arg_policy, MethodCallArgPolicy::PreserveShape) {
                    for (arg_ir, arg_ast) in args_ir.iter_mut().zip(args.iter()) {
                        let arg_span = match arg_ast {
                            ast::CallArg::Positional(expr)
                            | ast::CallArg::Named(_, expr)
                            | ast::CallArg::PositionalUnpack(expr)
                            | ast::CallArg::KeywordUnpack(expr) => expr.span,
                        };
                        arg_ir.expr = self.wrap_with_rust_arg_coercion(arg_ir.expr.clone(), arg_span)?;
                    }
                }
                let mut expr_ty = self
                    .type_info
                    .as_ref()
                    .and_then(|info| info.expr_type(expr_span))
                    .map(|ty| self.lower_resolved_type(ty))
                    .unwrap_or(IrType::Unknown);
                if magic_methods::from_str(&method_name) == Some(MagicMethodId::ClassName)
                    && matches!(expr_ty, IrType::String)
                {
                    expr_ty = IrType::StaticStr;
                }
                let dispatch = self
                    .type_info
                    .as_ref()
                    .and_then(|info| info.resolved_method_call(expr_span).cloned())
                    .map(|resolved| match resolved.dispatch {
                        ResolvedMethodDispatch::Trait { trait_path, type_args } => IrMethodDispatch::Trait {
                            trait_path,
                            type_args: type_args.iter().map(|ty| self.lower_resolved_type(ty)).collect(),
                        },
                    })
                    .or_else(|| {
                        self.type_info
                            .as_ref()
                            .and_then(|info| info.rust_method_trait_import_use(expr_span))
                            .map(|import_use| IrMethodDispatch::RustExtensionTraitImport {
                                binding: import_use.binding.clone(),
                            })
                    });

                if let Some(policy) = numeric_resize_policy(&method_name)
                    && args_ir.is_empty()
                    && lowered_type_args.is_empty()
                {
                    let target_ty = match (policy, &expr_ty) {
                        (NumericResizePolicy::Try, IrType::Option(inner)) => (**inner).clone(),
                        _ => expr_ty.clone(),
                    };
                    (
                        IrExprKind::NumericResize {
                            expr: Box::new(receiver),
                            policy,
                            to_type: target_ty,
                        },
                        expr_ty,
                    )
                } else if let Some(kind) = MethodKind::for_receiver(&receiver.ty, &method_name).or_else(|| {
                    if self.receiver_adopts_iterator_protocol(&receiver.ty) {
                        MethodKind::for_iterator_method_name(&method_name)
                    } else if matches!(
                        MethodKind::for_result_method_name(&method_name),
                        Some(MethodKind::Result(ResultMethodId::Inspect | ResultMethodId::InspectErr))
                    ) {
                        MethodKind::for_result_method_name(&method_name)
                    } else {
                        None
                    }
                }) {
                    (
                        IrExprKind::KnownMethodCall {
                            receiver: Box::new(receiver),
                            kind,
                            args: args_ir,
                        },
                        expr_ty,
                    )
                } else {
                    let imported_type_method_signature = match &o.node {
                        ast::Expr::Ident(name) => match self.import_aliases.get(name).cloned() {
                            Some(path) => self.callable_signature_for_imported_stdlib_type_method_path(&path, m)?,
                            None => None,
                        },
                        _ => None,
                    };
                    let public_receiver_library = self.public_library_for_method_receiver(&receiver);
                    let imported_pub_method_signature = public_receiver_library.as_deref().and_then(|library| {
                        self.callable_signature_for_imported_pub_type_method(library, &receiver.ty, m)
                    });
                    let compiled_provider_crate = AstLowering::nominal_receiver_type_name(&receiver.ty)
                        .and_then(|type_name| self.sdk_provider_crate_for_type(type_name));
                    let compiled_provider_method_signature =
                        self.callable_signature_for_compiled_provider_type_method(&receiver.ty, m);
                    let call_site_signature = self.callable_signature_for_call_span(expr_span);
                    let std_logging_signature = if matches!(
                        &receiver.ty,
                        IrType::Struct(name) | IrType::NamedGeneric(name, _) if name.rsplit("::").next() == Some("Logger")
                    ) {
                        self.std_logging_callable_signature_for_call(expr_span, m)?
                    } else {
                        None
                    };
                    let callable_signature = match (
                        std_logging_signature.or(call_site_signature),
                        imported_type_method_signature
                            .or(imported_pub_method_signature)
                            .or(compiled_provider_method_signature),
                    ) {
                        (Some(mut call_site), Some(imported)) => {
                            for (param, imported_param) in call_site.params.iter_mut().zip(imported.params.iter()) {
                                if param.default.is_none() {
                                    param.default = imported_param.default.clone();
                                }
                            }
                            Some(call_site)
                        }
                        (Some(call_site), None) => Some(call_site),
                        (None, imported) => imported,
                    };
                    let callable_signature = match (public_receiver_library.clone(), callable_signature) {
                        (Some(library), Some(signature)) => Some(self.pub_external_signature(&library, signature)),
                        (None, Some(signature)) => match compiled_provider_crate.as_deref() {
                            Some(provider_crate) => {
                                Some(self.compiled_provider_external_signature(provider_crate, signature))
                            }
                            None => Some(signature),
                        },
                        (_, None) => None,
                    };
                    let expr_ty = compiled_provider_crate
                        .as_deref()
                        .map(|provider_crate| self.pub_external_type(provider_crate, expr_ty.clone()))
                        .unwrap_or(expr_ty);
                    // Unknown method - keep as string-based call
                    (
                        IrExprKind::MethodCall {
                            receiver: Box::new(receiver),
                            method: method_name,
                            dispatch,
                            type_args: lowered_type_args,
                            args: args_ir,
                            callable_signature,
                            arg_policy,
                        },
                        expr_ty,
                    )
                }
            }

            // ---- Index access ----
            ast::Expr::Index(o, i) => {
                let obj = self.lower_expr_spanned(o)?;
                let idx = self.lower_expr_spanned(i)?;
                if let Some(resolved_operator) = self
                    .type_info
                    .as_ref()
                    .and_then(|info| info.resolved_operator_call(expr_span).cloned())
                    && resolved_operator.kind == ResolvedOperatorKind::Index
                {
                    let dispatch = self
                        .type_info
                        .as_ref()
                        .and_then(|info| info.resolved_method_call(expr_span).cloned())
                        .map(|resolved| match resolved.dispatch {
                            ResolvedMethodDispatch::Trait { trait_path, type_args } => IrMethodDispatch::Trait {
                                trait_path,
                                type_args: type_args.iter().map(|ty| self.lower_resolved_type(ty)).collect(),
                            },
                        });
                    let result_ty = self
                        .type_info
                        .as_ref()
                        .and_then(|info| info.expr_type(expr_span))
                        .map(|ty| self.lower_resolved_type(ty))
                        .unwrap_or(IrType::Unknown);
                    (
                        IrExprKind::MethodCall {
                            receiver: Box::new(obj),
                            method: resolved_operator.method,
                            dispatch,
                            type_args: Vec::new(),
                            args: vec![IrCallArg {
                                name: None,
                                kind: IrCallArgKind::Positional,
                                expr: idx,
                            }],
                            callable_signature: self.callable_signature_for_call_span(expr_span),
                            arg_policy: MethodCallArgPolicy::Default,
                        },
                        result_ty,
                    )
                } else {
                    let elem_ty = match &obj.ty {
                        IrType::List(e) => (**e).clone(),
                        IrType::Dict(_, v) => (**v).clone(),
                        IrType::String => IrType::String,
                        _ => IrType::Unknown,
                    };
                    (
                        IrExprKind::Index {
                            object: Box::new(obj),
                            index: Box::new(idx),
                        },
                        elem_ty,
                    )
                }
            }

            // ---- Field access ----
            ast::Expr::Field(o, f) => {
                if let ast::Expr::Ident(type_name) = &o.node
                    && f.starts_with("__incan_original_")
                {
                    return Ok(TypedExpr::new(
                        IrExprKind::AssociatedFunction {
                            type_name: type_name.clone(),
                            function_name: f.clone(),
                        },
                        IrType::Unknown,
                    ));
                }
                // Prefer spanned lowering so typechecker output can drive the receiver type.
                // This is important for RFC 021 alias-aware field access, especially for `self.<alias>`.
                let obj = self.lower_expr_spanned(o)?;
                if let Some(access) = self
                    .type_info
                    .as_ref()
                    .and_then(|info| info.computed_property_access(expr_span))
                {
                    let result_ty = self
                        .type_info
                        .as_ref()
                        .and_then(|info| info.expr_type(expr_span))
                        .map(|ty| self.lower_resolved_type(ty))
                        .unwrap_or(IrType::Unknown);
                    (
                        IrExprKind::MethodCall {
                            receiver: Box::new(obj),
                            method: access.property.clone(),
                            type_args: Vec::new(),
                            args: Vec::new(),
                            dispatch: None,
                            callable_signature: None,
                            arg_policy: MethodCallArgPolicy::Default,
                        },
                        result_ty,
                    )
                } else {
                    if let Some(rust_field) = self
                        .type_info
                        .as_ref()
                        .and_then(|info| info.rust_field_access_name(expr_span))
                    {
                        return Ok(TypedExpr::new(
                            IrExprKind::Field {
                                object: Box::new(obj),
                                field: rust_field.to_string(),
                            },
                            IrType::Unknown,
                        ));
                    }
                    // RFC 021: resolve field alias to canonical name if object is a known struct type
                    let struct_name = obj.ty.nominal_type_name().or_else(|| match &obj.kind {
                        IrExprKind::Var { name, .. } if name == "self" => self.current_impl_type.as_deref(),
                        _ => None,
                    });
                    let field = match struct_name {
                        Some(struct_name) => self.resolve_field_alias(struct_name, f),
                        None => f.clone(),
                    };
                    (
                        IrExprKind::Field {
                            object: Box::new(obj),
                            field,
                        },
                        IrType::Unknown,
                    )
                }
            }

            // ---- Surface expressions (routed through semantics registry) ----
            ast::Expr::Surface(surface_expr) => {
                use crate::semantics_registry::semantics_registry;

                let action = semantics_registry()
                    .lower_surface_expr_action(&surface_expr.key)
                    .ok_or_else(|| LoweringError {
                        message: format!(
                            "no lowering action registered for surface expression {:?}",
                            surface_expr.key
                        ),
                        span: super::super::IrSpan::default(),
                    })?;

                match (action, &surface_expr.payload) {
                    (SurfaceExprLoweringAction::Await, ast::SurfaceExprPayload::PrefixUnary(inner)) => {
                        // Preserve explicit grouping: `await (x?)` should keep the grouped `Try` operand shape
                        // instead of applying await/try normalization for the unparenthesized `await x()?` case.
                        let parenthesized_operand = matches!(&inner.node, ast::Expr::Paren(_));
                        let lowered_inner = self.lower_awaitable_operand(inner)?;
                        if parenthesized_operand {
                            let ty = lowered_inner.ty.clone();
                            (IrExprKind::Await(Box::new(lowered_inner)), ty)
                        } else {
                            super::super::surface_semantics::lower_await_expression(lowered_inner)
                        }
                    }
                    (SurfaceExprLoweringAction::RaceFor, ast::SurfaceExprPayload::RaceFor(race)) => {
                        let lowered = self.lower_race_for_expr(race, expr_span)?;
                        (lowered.kind, lowered.ty)
                    }
                    _ => {
                        return Err(LoweringError {
                            message: format!(
                                "surface expression {:?} has an unsupported payload for lowering",
                                surface_expr.key
                            ),
                            span: super::super::IrSpan::default(),
                        });
                    }
                }
            }

            ast::Expr::VocabBlock(block) => {
                return Err(LoweringError {
                    message: format!(
                        "vocab expression declaration `{}` reached lowering before desugaring",
                        block.keyword
                    ),
                    span: super::super::IrSpan::default(),
                });
            }

            // ---- Try (?) ----
            ast::Expr::Try(e) => {
                let inner = self.lower_expr_spanned(e)?;
                let ty = match &inner.ty {
                    IrType::Result(ok, _) => (**ok).clone(),
                    _ => inner.ty.clone(),
                };
                (IrExprKind::Try(Box::new(inner)), ty)
            }

            // ---- Match expressions (delegated to patterns submodule) ----
            ast::Expr::Match(s, arms) => {
                let scrutinee = self.lower_expr_spanned(s)?;
                let arms_ir = self.lower_match_arms(arms, &scrutinee)?;
                let ty = arms_ir.first().map(|a| a.body.ty.clone()).unwrap_or(IrType::Unknown);
                (
                    IrExprKind::Match {
                        scrutinee: Box::new(scrutinee),
                        arms: arms_ir,
                    },
                    ty,
                )
            }

            // ---- If expressions ----
            ast::Expr::If(i) => {
                let cond = self.lower_condition_expr(&i.condition)?;
                let then_stmts = self.lower_statements(&i.then_body)?;
                let then_expr = TypedExpr::new(
                    IrExprKind::Block {
                        stmts: then_stmts,
                        value: None,
                    },
                    IrType::Unit,
                );
                let else_expr = i
                    .else_body
                    .as_ref()
                    .map(|b| {
                        self.lower_statements(b)
                            .map(|stmts| TypedExpr::new(IrExprKind::Block { stmts, value: None }, IrType::Unit))
                    })
                    .transpose()?;
                (
                    IrExprKind::If {
                        condition: Box::new(cond),
                        then_branch: Box::new(then_expr),
                        else_branch: else_expr.map(Box::new),
                    },
                    IrType::Unit,
                )
            }

            ast::Expr::Loop(loop_expr) => {
                self.push_scope();
                self.non_linear_context_depth += 1;
                let body_result = self.lower_statements(&loop_expr.body);
                self.non_linear_context_depth -= 1;
                let body = body_result?;
                self.pop_scope();
                (IrExprKind::Loop { body }, IrType::Unknown)
            }

            // ---- Closures ----
            ast::Expr::Closure(params, body) => {
                let recorded_param_types = self
                    .type_info
                    .as_ref()
                    .and_then(|info| match info.expr_type(expr_span) {
                        Some(crate::frontend::symbols::ResolvedType::Function(callable_params, _)) => Some(
                            callable_params
                                .iter()
                                .map(|param| self.lower_resolved_type(&param.ty))
                                .collect::<Vec<_>>(),
                        ),
                        _ => None,
                    });
                let exact_rust_param_types = self
                    .type_info
                    .as_ref()
                    .and_then(|info| info.closure_param_type_displays(expr_span))
                    .filter(|displays| displays.len() == params.len())
                    .map(|displays| {
                        displays
                            .iter()
                            .map(|display| IrType::RustDisplay(display.clone()))
                            .collect::<Vec<_>>()
                    });
                let param_pairs: Vec<(String, IrType)> = params
                    .iter()
                    .enumerate()
                    .map(|(idx, p)| {
                        let ty = exact_rust_param_types
                            .as_ref()
                            .and_then(|types| types.get(idx).cloned())
                            .or_else(|| recorded_param_types.as_ref().and_then(|types| types.get(idx).cloned()))
                            .unwrap_or_else(|| self.lower_type(&p.node.ty.node));
                        (p.node.name.clone(), ty)
                    })
                    .collect();
                let mut closure_read_counts = HashMap::new();
                self.count_expr_ident_reads(&body.node, &mut closure_read_counts);
                self.remaining_ident_reads.push(closure_read_counts);
                self.non_linear_context_depth += 1;
                self.closure_param_scopes.push((
                    self.non_linear_context_depth,
                    param_pairs.iter().map(|(name, _)| name.clone()).collect(),
                ));
                let body_ir_result = self.lower_expr_spanned(body);
                let _ = self.closure_param_scopes.pop();
                self.non_linear_context_depth -= 1;
                let _ = self.remaining_ident_reads.pop();
                let body_ir = body_ir_result?;
                let ret_ty = body_ir.ty.clone();
                let param_tys: Vec<IrType> = param_pairs.iter().map(|(_, t)| t.clone()).collect();
                (
                    IrExprKind::Closure {
                        params: param_pairs,
                        body: Box::new(body_ir),
                        captures: vec![],
                    },
                    IrType::Function {
                        params: param_tys,
                        ret: Box::new(ret_ty),
                    },
                )
            }

            // ---- Collection literals ----
            ast::Expr::Tuple(items) => {
                let items_ir: Vec<TypedExpr> = items
                    .iter()
                    .map(|i| self.lower_expr_spanned(i))
                    .collect::<Result<_, _>>()?;
                let tys: Vec<IrType> = items_ir.iter().map(|i| i.ty.clone()).collect();
                (IrExprKind::Tuple(items_ir), IrType::Tuple(tys))
            }

            ast::Expr::List(items) => {
                let items_ir: Vec<IrListEntry> = items
                    .iter()
                    .map(|i| match i {
                        ast::ListEntry::Element(value) => self.lower_expr_spanned(value).map(IrListEntry::Element),
                        ast::ListEntry::Spread(value) => self.lower_expr_spanned(value).map(IrListEntry::Spread),
                    })
                    .collect::<Result<_, _>>()?;
                let elem = items_ir
                    .iter()
                    .find_map(|entry| match entry {
                        IrListEntry::Element(value) => Some(value.ty.clone()),
                        IrListEntry::Spread(value) => Self::lowered_list_spread_element_type(&value.ty),
                    })
                    .unwrap_or(IrType::Unknown);
                (IrExprKind::List(items_ir), IrType::List(Box::new(elem)))
            }

            ast::Expr::Dict(pairs) => {
                let pairs_ir: Vec<IrDictEntry> = pairs
                    .iter()
                    .map(|entry| match entry {
                        ast::DictEntry::Pair(k, v) => Ok(IrDictEntry::Pair(
                            self.lower_expr_spanned(k)?,
                            Box::new(self.lower_expr_spanned(v)?),
                        )),
                        ast::DictEntry::Spread(value) => self.lower_expr_spanned(value).map(IrDictEntry::Spread),
                    })
                    .collect::<Result<_, LoweringError>>()?;
                let (k, v) = pairs_ir
                    .iter()
                    .find_map(|entry| match entry {
                        IrDictEntry::Pair(key, value) => Some((key.ty.clone(), value.ty.clone())),
                        IrDictEntry::Spread(value) => Self::lowered_dict_spread_entry_types(&value.ty),
                    })
                    .unwrap_or((IrType::Unknown, IrType::Unknown));
                (IrExprKind::Dict(pairs_ir), IrType::Dict(Box::new(k), Box::new(v)))
            }

            ast::Expr::Set(items) => {
                let items_ir: Vec<TypedExpr> = items
                    .iter()
                    .map(|i| self.lower_expr_spanned(i))
                    .collect::<Result<_, _>>()?;
                let elem = items_ir.first().map(|i| i.ty.clone()).unwrap_or(IrType::Unknown);
                (IrExprKind::Set(items_ir), IrType::Set(Box::new(elem)))
            }

            // ---- Parenthesized expression (transparent) ----
            ast::Expr::Paren(e) => return self.lower_expr_spanned(e),

            // ---- Constructor (variant / struct literal) ----
            ast::Expr::Constructor(name, args) => {
                let fields: Vec<(String, TypedExpr)> = args
                    .iter()
                    .map(|arg| match arg {
                        ast::CallArg::Named(n, e) => Ok((n.clone(), self.lower_expr_spanned(e)?)),
                        ast::CallArg::Positional(e)
                        | ast::CallArg::PositionalUnpack(e)
                        | ast::CallArg::KeywordUnpack(e) => Ok((String::new(), self.lower_expr_spanned(e)?)),
                    })
                    .collect::<Result<_, LoweringError>>()?;
                (
                    IrExprKind::Struct {
                        name: name.clone(),
                        fields,
                    },
                    IrType::Struct(name.clone()),
                )
            }

            // ---- Range expressions ----
            ast::Expr::Range { start, end, inclusive } => {
                let s = self.lower_expr_spanned(start)?;
                let e = self.lower_expr_spanned(end)?;
                (
                    IrExprKind::Range {
                        start: Some(Box::new(s)),
                        end: Some(Box::new(e)),
                        inclusive: *inclusive,
                    },
                    IrType::Unknown,
                )
            }

            // ---- F-strings ----
            ast::Expr::FString(parts) => {
                let ir_parts: Vec<super::super::expr::FormatPart> = parts
                    .iter()
                    .map(|part| match part {
                        ast::FStringPart::Literal(s) => Ok(super::super::expr::FormatPart::Literal(s.clone())),
                        ast::FStringPart::Expr { expr, format } => {
                            let lowered = self.lower_expr_spanned(expr)?;
                            let style = match format {
                                ast::FStringFormat::Display => super::super::expr::FormatStyle::Display,
                                ast::FStringFormat::Debug => super::super::expr::FormatStyle::Debug,
                            };
                            Ok(super::super::expr::FormatPart::Expr { expr: lowered, style })
                        }
                    })
                    .collect::<Result<Vec<_>, LoweringError>>()?;
                (IrExprKind::Format { parts: ir_parts }, IrType::String)
            }

            // ---- Slice expressions ----
            ast::Expr::Slice(target, slice) => {
                let target_expr = self.lower_expr_spanned(target)?;
                let start = slice
                    .start
                    .as_ref()
                    .map(|s| Ok(Box::new(self.lower_expr_spanned(s)?)))
                    .transpose()?;
                let end = slice
                    .end
                    .as_ref()
                    .map(|e| Ok(Box::new(self.lower_expr_spanned(e)?)))
                    .transpose()?;
                let step = slice
                    .step
                    .as_ref()
                    .map(|st| Ok(Box::new(self.lower_expr_spanned(st)?)))
                    .transpose()?;

                let result_ty = match &target_expr.ty {
                    IrType::List(inner) => IrType::List(inner.clone()),
                    IrType::String => IrType::String,
                    _ => IrType::Unknown,
                };

                (
                    IrExprKind::Slice {
                        target: Box::new(target_expr),
                        start,
                        end,
                        step,
                    },
                    result_ty,
                )
            }

            // ---- Comprehensions (delegated to comprehensions submodule) ----
            ast::Expr::Generator(generator) => self.lower_generator_expr(generator)?,
            ast::Expr::ListComp(comp) => self.lower_list_comp(comp)?,
            ast::Expr::DictComp(comp) => self.lower_dict_comp(comp)?,

            // ---- Yield (placeholder) ----
            ast::Expr::Yield(_) => (IrExprKind::Unit, IrType::Unknown),
            ast::Expr::Partial(partial) => {
                let Some(crate::frontend::symbols::ResolvedType::Function(params, ret)) = self
                    .type_info
                    .as_ref()
                    .and_then(|info| info.expr_type(expr_span).cloned())
                else {
                    return Err(LoweringError {
                        message: "Partial callable preset expression is missing typechecker projection metadata"
                            .to_string(),
                        span: expr_span.into(),
                    });
                };
                let closure_params: Vec<(String, IrType)> = params
                    .iter()
                    .enumerate()
                    .map(|(idx, param)| {
                        (
                            param.name.clone().unwrap_or_else(|| format!("__incan_arg_{idx}")),
                            Self::lower_param_container_type(param.kind, self.lower_resolved_type(&param.ty)),
                        )
                    })
                    .collect();
                let _signature = self.partial_expr_callable_signature(partial, expr_span)?;
                self.push_scope();
                for (name, ty) in &closure_params {
                    self.define_local_binding(name.clone(), ty.clone(), false);
                }
                let mut forward_args = Vec::new();
                for (name, _) in &closure_params {
                    forward_args.push(ast::CallArg::Named(
                        name.clone(),
                        ast::Spanned::new(ast::Expr::Ident(name.clone()), expr_span),
                    ));
                }
                let call = ast::Spanned::new(
                    ast::Expr::Call(partial.target.clone(), partial.type_args.clone(), forward_args),
                    expr_span,
                );
                let body_result = self.lower_expr_spanned(&call);
                self.pop_scope();
                let body = body_result?;
                let ret_ty = self.lower_resolved_type(ret.as_ref());
                (
                    IrExprKind::Closure {
                        params: closure_params.clone(),
                        body: Box::new(body),
                        captures: vec![],
                    },
                    IrType::Function {
                        params: closure_params.into_iter().map(|(_, ty)| ty).collect(),
                        ret: Box::new(ret_ty),
                    },
                )
            }
        };
        Ok(TypedExpr::new(kind, ty))
    }
}

/// Return the IR resize policy represented by a built-in numeric resize helper name.
fn numeric_resize_policy(method: &str) -> Option<NumericResizePolicy> {
    match method {
        "resize" => Some(NumericResizePolicy::Lossless),
        "try_resize" => Some(NumericResizePolicy::Try),
        "wrapping_resize" => Some(NumericResizePolicy::Wrapping),
        "saturating_resize" => Some(NumericResizePolicy::Saturating),
        _ => None,
    }
}
