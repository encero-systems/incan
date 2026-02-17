//! Check expressions and resolve their types.
//!
//! This module owns the expression-checking entrypoint (`check_expr`) and delegates to themed
//! submodules for maintainability. Expression checking is error-accumulating: on invalid input it
//! returns [`ResolvedType::Unknown`] so later checks can continue.
//!
//! ## See also
//! - [`super::TypeChecker`]: the main type checker entrypoint.

use crate::frontend::ast::*;
use crate::frontend::diagnostics::errors;
use crate::frontend::symbols::{FieldInfo, ResolvedType};
use incan_core::lang::keywords;
use incan_semantics_core::SurfaceExprTypeCheck;
use std::collections::HashMap;

use super::TypeChecker;

mod access;
mod basics;
mod calls;
mod collections;
mod comps;
mod control_flow;
mod match_;
mod ops;

impl TypeChecker {
    /// Resolve a field by canonical name or alias, returning the canonical name and FieldInfo.
    ///
    /// - `allow_alias`: whether alias lookup is allowed (models only).
    /// - `allow_numeric_alias`: whether numeric spellings like `"1"` may match aliases.
    fn resolve_field_info<'a>(
        &self,
        fields: &'a HashMap<String, FieldInfo>,
        field_name: &str,
        allow_alias: bool,
        allow_numeric_alias: bool,
    ) -> Option<(String, &'a FieldInfo)> {
        if let Some(info) = fields.get(field_name) {
            return Some((field_name.to_string(), info));
        }
        if !allow_alias {
            return None;
        }
        if !allow_numeric_alias && field_name.parse::<usize>().is_ok() {
            return None;
        }
        fields
            .iter()
            .find(|(_, info)| info.alias.as_deref() == Some(field_name))
            .map(|(name, info)| (name.clone(), info))
    }
    // ========================================================================
    // Expressions
    // ========================================================================

    /// Validate an expression and return its resolved type.
    ///
    /// Dispatches to specialized helpers (`check_call`, `check_binary`, `check_match`, etc.)
    /// and accumulates errors. Returns [`ResolvedType::Unknown`] when the expression is
    /// invalid so checking can continue.
    pub(crate) fn check_expr(&mut self, expr: &Spanned<Expr>) -> ResolvedType {
        let ty = match &expr.node {
            Expr::Ident(name) => self.check_ident(name, expr.span),
            Expr::Literal(lit) => self.check_literal(lit),
            Expr::SelfExpr => self.check_self(expr.span),
            Expr::Binary(left, op, right) => self.check_binary(left, *op, right, expr.span),
            Expr::Unary(op, operand) => self.check_unary(*op, operand, expr.span),
            Expr::Call(callee, args) => self.check_call(callee, args, expr.span),
            Expr::Index(base, index) => self.check_index(base, index, expr.span),
            Expr::Slice(base, slice) => self.check_slice(base, slice, expr.span),
            Expr::Field(base, field) => self.check_field(base, field, expr.span),
            Expr::MethodCall(base, method, args) => self.check_method_call(base, method, args, expr.span),
            Expr::Surface(surface_expr) => self.check_surface_expr(surface_expr, expr.span),
            Expr::Try(inner) => self.check_try(inner, expr.span),
            Expr::Match(subject, arms) => self.check_match(subject, arms, expr.span),
            Expr::If(if_expr) => self.check_if_expr(if_expr, expr.span),
            Expr::ListComp(comp) => self.check_list_comp(comp, expr.span),
            Expr::DictComp(comp) => self.check_dict_comp(comp, expr.span),
            Expr::Closure(params, body) => self.check_closure(params, body, expr.span),
            Expr::Tuple(elems) => self.check_tuple(elems),
            Expr::List(elems) => self.check_list(elems),
            Expr::Dict(entries) => self.check_dict(entries),
            Expr::Set(elems) => self.check_set(elems),
            Expr::Paren(inner) => self.check_expr(inner),
            Expr::Constructor(name, args) => self.check_constructor(name, args, expr.span),
            Expr::FString(parts) => {
                for part in parts {
                    if let FStringPart::Expr(e) = part {
                        self.check_expr(e);
                    }
                }
                ResolvedType::Str
            }
            Expr::Yield(inner) => {
                // Yield returns the type of its inner expression, or Unit
                if let Some(inner) = inner {
                    self.check_expr(inner)
                } else {
                    ResolvedType::Unit
                }
            }
            Expr::Range {
                start,
                end,
                inclusive: _,
            } => self.check_range_expr(start, end),
        };

        // Record for downstream stages (lowering/codegen).
        self.record_expr_type(expr.span, ty.clone());
        ty
    }

    /// Typecheck a surface expression via the semantics registry.
    fn check_surface_expr(&mut self, expr: &SurfaceExpr, span: Span) -> ResolvedType {
        use crate::semantics_registry::semantics_registry;

        let Some(action) = semantics_registry().typecheck_surface_expr_action(&expr.key) else {
            // No pack claimed this surface expression — report as unknown.
            let label = match &expr.key {
                incan_semantics_core::SurfaceFeatureKey::SoftKeyword(id) => keywords::as_str(*id).to_string(),
                incan_semantics_core::SurfaceFeatureKey::Decorator(_) => "decorator-surface-feature".to_string(),
            };
            self.errors.push(errors::unknown_symbol(&label, span));
            return ResolvedType::Unknown;
        };

        match (action, &expr.payload) {
            (SurfaceExprTypeCheck::AwaitCheck, SurfaceExprPayload::PrefixUnary(inner)) => self.check_await(inner, span),
        }
    }
}
