//! Argument checking, binding, and shaped unpack helpers for call expressions.

use super::TypeChecker;
use crate::frontend::ast::{CallArg, DictEntry, Expr, ListEntry, Literal, ParamKind, Span, Spanned};
use crate::frontend::diagnostics::errors;
use crate::frontend::symbols::{CallableParam, ResolvedType};
use crate::frontend::typechecker::FixedUnpackPlan;
use crate::frontend::typechecker::helpers::{collection_type_id, dict_ty, list_ty};
use incan_core::lang::types::collections::CollectionTypeId;

impl TypeChecker {
    pub(in crate::frontend::typechecker::check_expr::calls) fn call_arg_expr(arg: &CallArg) -> &Spanned<Expr> {
        match arg {
            CallArg::Positional(e)
            | CallArg::Named(_, e)
            | CallArg::PositionalUnpack(e)
            | CallArg::KeywordUnpack(e) => e,
        }
    }

    /// Return statically shaped positional unpack items for call binding.
    ///
    /// Fixed-parameter unpacking may only consume surface syntax whose arity is visible before lowering. Parentheses
    /// are transparent, while ordinary list variables remain unshaped and therefore rest-only.
    fn shaped_positional_unpack_items(expr: &Spanned<Expr>) -> Option<Vec<&Spanned<Expr>>> {
        match &expr.node {
            Expr::Tuple(items) => Some(items.iter().collect()),
            Expr::List(items) => items
                .iter()
                .map(|item| match item {
                    ListEntry::Element(value) => Some(value),
                    ListEntry::Spread(_) => None,
                })
                .collect(),
            Expr::Paren(inner) => Self::shaped_positional_unpack_items(inner),
            _ => None,
        }
    }

    /// Return the fixed positional item types for a tuple-typed unpack operand.
    fn shaped_positional_unpack_types(ty: &ResolvedType) -> Option<&[ResolvedType]> {
        match ty {
            ResolvedType::Tuple(items) => Some(items.as_slice()),
            ResolvedType::Generic(name, items)
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Tuple) =>
            {
                Some(items.as_slice())
            }
            _ => None,
        }
    }

    /// Return statically shaped keyword unpack entries for call binding.
    ///
    /// The fixed-keyword path accepts inline dictionary literals only when every key is statically known. Dynamic-key
    /// dictionary literals stay on the existing rest-only path.
    fn shaped_keyword_unpack_entries(expr: &Spanned<Expr>) -> Option<Vec<(&Spanned<Expr>, &Spanned<Expr>)>> {
        match &expr.node {
            Expr::Dict(entries) => entries
                .iter()
                .map(|entry| match entry {
                    DictEntry::Pair(key, value) if Self::static_string_key(key).is_some() => Some((key, value)),
                    DictEntry::Pair(_, _) | DictEntry::Spread(_) => None,
                })
                .collect(),
            Expr::Paren(inner) => Self::shaped_keyword_unpack_entries(inner),
            _ => None,
        }
    }

    /// Extract a statically known string key from an inline keyword-unpack dictionary entry.
    fn static_string_key(expr: &Spanned<Expr>) -> Option<&str> {
        match &expr.node {
            Expr::Literal(Literal::String(key)) => Some(key.as_str()),
            Expr::Paren(inner) => Self::static_string_key(inner),
            _ => None,
        }
    }

    /// Type-check all call arguments, including unpack arguments.
    pub(in crate::frontend::typechecker::check_expr) fn check_call_args(&mut self, args: &[CallArg]) {
        for arg in args {
            self.call_argument_depth += 1;
            self.check_expr(Self::call_arg_expr(arg));
            self.call_argument_depth -= 1;
        }
    }

    /// Type-check all call arguments and collect their resolved types.
    pub(in crate::frontend::typechecker::check_expr::calls) fn check_call_arg_types(
        &mut self,
        args: &[CallArg],
    ) -> Vec<ResolvedType> {
        args.iter()
            .map(|arg| {
                self.call_argument_depth += 1;
                let ty = self.check_expr(Self::call_arg_expr(arg));
                self.call_argument_depth -= 1;
                ty
            })
            .collect()
    }

    /// Type-check call arguments while threading parameter types into contextual-expression checks when available.
    pub(in crate::frontend::typechecker::check_expr::calls) fn check_call_arg_types_for_params(
        &mut self,
        args: &[CallArg],
        params: &[CallableParam],
    ) -> Vec<ResolvedType> {
        let normal_params: Vec<&CallableParam> =
            params.iter().filter(|param| param.kind == ParamKind::Normal).collect();
        let rest_positional = params.iter().find(|param| param.kind == ParamKind::RestPositional);
        let rest_keyword = params.iter().find(|param| param.kind == ParamKind::RestKeyword);
        let mut positional_index = 0usize;

        let mut arg_types = Vec::with_capacity(args.len());
        for arg in args {
            match arg {
                CallArg::Positional(expr) => {
                    let expected = normal_params
                        .get(positional_index)
                        .map(|param| param.ty.clone())
                        .or_else(|| rest_positional.map(|param| param.ty.clone()));
                    positional_index += 1;
                    self.call_argument_depth += 1;
                    arg_types.push(self.check_expr_with_expected(expr, expected.as_ref()));
                    self.call_argument_depth -= 1;
                }
                CallArg::Named(name, expr) => {
                    let expected = normal_params
                        .iter()
                        .find(|param| param.name() == Some(name.as_str()))
                        .map(|param| param.ty.clone())
                        .or_else(|| rest_keyword.map(|param| param.ty.clone()));
                    self.call_argument_depth += 1;
                    arg_types.push(self.check_expr_with_expected(expr, expected.as_ref()));
                    self.call_argument_depth -= 1;
                }
                CallArg::PositionalUnpack(expr) => {
                    if let Some(items) = Self::shaped_positional_unpack_items(expr) {
                        let mut item_types = Vec::with_capacity(items.len());
                        for item in items {
                            let expected = normal_params
                                .get(positional_index)
                                .map(|param| param.ty.clone())
                                .or_else(|| rest_positional.map(|param| param.ty.clone()));
                            if positional_index < normal_params.len() {
                                positional_index += 1;
                            }
                            self.call_argument_depth += 1;
                            item_types.push(self.check_expr_with_expected(item, expected.as_ref()));
                            self.call_argument_depth -= 1;
                        }
                        let plan_item_types = item_types.clone();
                        let ty = ResolvedType::Tuple(item_types);
                        self.record_expr_type(expr.span, ty.clone());
                        self.record_fixed_unpack_plan(expr.span, FixedUnpackPlan::Positional(plan_item_types));
                        arg_types.push(ty);
                    } else {
                        let expected = rest_positional.map(|param| list_ty(param.ty.clone()));
                        self.call_argument_depth += 1;
                        let ty = self.check_expr_with_expected(expr, expected.as_ref());
                        self.call_argument_depth -= 1;
                        if let Some(item_types) = Self::shaped_positional_unpack_types(&ty) {
                            self.record_fixed_unpack_plan(expr.span, FixedUnpackPlan::Positional(item_types.to_vec()));
                        }
                        arg_types.push(ty);
                    }
                }
                CallArg::KeywordUnpack(expr) => {
                    if let Some(entries) = Self::shaped_keyword_unpack_entries(expr) {
                        let mut value_types = Vec::with_capacity(entries.len());
                        for (key, value) in &entries {
                            self.call_argument_depth += 1;
                            self.check_expr(key);
                            self.call_argument_depth -= 1;
                            let expected = Self::static_string_key(key)
                                .and_then(|name| {
                                    normal_params
                                        .iter()
                                        .find(|param| param.name() == Some(name))
                                        .map(|param| param.ty.clone())
                                })
                                .or_else(|| rest_keyword.map(|param| param.ty.clone()));
                            self.call_argument_depth += 1;
                            value_types.push(self.check_expr_with_expected(value, expected.as_ref()));
                            self.call_argument_depth -= 1;
                        }
                        let value_ty = value_types.first().cloned().unwrap_or(ResolvedType::Unknown);
                        self.record_expr_type(expr.span, dict_ty(ResolvedType::Str, value_ty));
                        self.record_fixed_unpack_plan(
                            expr.span,
                            FixedUnpackPlan::Keyword(
                                entries
                                    .iter()
                                    .filter_map(|(key, _)| Self::static_string_key(key).map(str::to_string))
                                    .collect(),
                            ),
                        );
                        arg_types.push(ResolvedType::Tuple(value_types));
                    } else {
                        let expected = rest_keyword.map(|param| dict_ty(ResolvedType::Str, param.ty.clone()));
                        self.call_argument_depth += 1;
                        arg_types.push(self.check_expr_with_expected(expr, expected.as_ref()));
                        self.call_argument_depth -= 1;
                    }
                }
            }
        }
        arg_types
    }

    /// Validate call arguments against callable parameters, including rest captures and statically shaped unpacking.
    pub(in crate::frontend::typechecker) fn validate_callable_arg_bindings(
        &mut self,
        callee: &str,
        params: &[CallableParam],
        args: &[CallArg],
        arg_types: &[ResolvedType],
        type_bindings: &mut std::collections::HashMap<String, ResolvedType>,
        call_span: Span,
    ) {
        let normal_params: Vec<(usize, &CallableParam)> = params
            .iter()
            .enumerate()
            .filter(|(_, param)| param.kind == ParamKind::Normal)
            .collect();
        let rest_positional = params.iter().find(|param| param.kind == ParamKind::RestPositional);
        let rest_keyword = params.iter().find(|param| param.kind == ParamKind::RestKeyword);

        let mut normal_bound_spans: Vec<Option<Span>> = vec![None; normal_params.len()];
        let mut named_seen: std::collections::HashMap<&str, Span> = std::collections::HashMap::new();
        let mut positional_index = 0usize;
        let mut unexpected_positional = 0usize;

        for (arg, arg_ty) in args.iter().zip(arg_types.iter()) {
            let arg_span = Self::call_arg_expr(arg).span;
            match arg {
                CallArg::Positional(_) => {
                    if let Some((_, param)) = normal_params.get(positional_index) {
                        normal_bound_spans[positional_index] = Some(arg_span);
                        self.infer_type_param_bindings(&param.ty, arg_ty, type_bindings);
                        self.emit_arg_type_mismatch_if_needed(&param.ty, arg_ty, arg_span);
                        positional_index += 1;
                    } else if let Some(param) = rest_positional {
                        self.infer_type_param_bindings(&param.ty, arg_ty, type_bindings);
                        self.emit_arg_type_mismatch_if_needed(&param.ty, arg_ty, arg_span);
                    } else {
                        unexpected_positional += 1;
                    }
                }
                CallArg::PositionalUnpack(_) => {
                    if let Some(items) = Self::shaped_positional_unpack_items(Self::call_arg_expr(arg)) {
                        let fallback_item_types;
                        let item_types = match arg_ty {
                            ResolvedType::Tuple(item_types) => item_types.as_slice(),
                            _ => {
                                fallback_item_types =
                                    items.iter().map(|item| self.check_expr(item)).collect::<Vec<_>>();
                                fallback_item_types.as_slice()
                            }
                        };
                        self.record_fixed_unpack_plan(arg_span, FixedUnpackPlan::Positional(item_types.to_vec()));
                        for (item, item_ty) in items.iter().zip(item_types.iter()) {
                            let item_span = item.span;
                            if let Some((_, param)) = normal_params.get(positional_index) {
                                normal_bound_spans[positional_index] = Some(item_span);
                                self.infer_type_param_bindings(&param.ty, item_ty, type_bindings);
                                self.emit_arg_type_mismatch_if_needed(&param.ty, item_ty, item_span);
                                positional_index += 1;
                            } else if let Some(param) = rest_positional {
                                self.infer_type_param_bindings(&param.ty, item_ty, type_bindings);
                                self.emit_arg_type_mismatch_if_needed(&param.ty, item_ty, item_span);
                            } else {
                                unexpected_positional += 1;
                            }
                        }
                    } else if let Some(item_types) = Self::shaped_positional_unpack_types(arg_ty) {
                        self.record_fixed_unpack_plan(arg_span, FixedUnpackPlan::Positional(item_types.to_vec()));
                        for item_ty in item_types {
                            if let Some((_, param)) = normal_params.get(positional_index) {
                                normal_bound_spans[positional_index] = Some(arg_span);
                                self.infer_type_param_bindings(&param.ty, item_ty, type_bindings);
                                self.emit_arg_type_mismatch_if_needed(&param.ty, item_ty, arg_span);
                                positional_index += 1;
                            } else if let Some(param) = rest_positional {
                                self.infer_type_param_bindings(&param.ty, item_ty, type_bindings);
                                self.emit_arg_type_mismatch_if_needed(&param.ty, item_ty, arg_span);
                            } else {
                                unexpected_positional += 1;
                            }
                        }
                    } else if let Some(param) = rest_positional {
                        let expected = list_ty(param.ty.clone());
                        self.infer_type_param_bindings(&expected, arg_ty, type_bindings);
                        self.emit_arg_type_mismatch_if_needed(&expected, arg_ty, arg_span);
                    } else {
                        self.errors
                            .push(errors::call_unpack_without_rest(callee, "*", arg_span));
                    }
                }
                CallArg::Named(name, _) => {
                    if let Some(first_span) = named_seen.insert(name.as_str(), arg_span) {
                        self.errors
                            .push(errors::duplicate_call_argument(callee, name, first_span));
                    }

                    if let Some((normal_idx, (_, param))) = normal_params
                        .iter()
                        .enumerate()
                        .find(|(_, (_, param))| param.name() == Some(name.as_str()))
                    {
                        if normal_bound_spans[normal_idx].is_some() {
                            self.errors
                                .push(errors::duplicate_call_argument(callee, name, arg_span));
                            continue;
                        }
                        normal_bound_spans[normal_idx] = Some(arg_span);
                        self.infer_type_param_bindings(&param.ty, arg_ty, type_bindings);
                        self.emit_arg_type_mismatch_if_needed(&param.ty, arg_ty, arg_span);
                    } else if let Some(param) = rest_keyword {
                        self.infer_type_param_bindings(&param.ty, arg_ty, type_bindings);
                        self.emit_arg_type_mismatch_if_needed(&param.ty, arg_ty, arg_span);
                    } else {
                        self.errors
                            .push(errors::unknown_keyword_argument(callee, name, arg_span));
                    }
                }
                CallArg::KeywordUnpack(_) => {
                    if let Some(entries) = Self::shaped_keyword_unpack_entries(Self::call_arg_expr(arg)) {
                        let fallback_value_types;
                        let value_types = match arg_ty {
                            ResolvedType::Tuple(value_types) => value_types.as_slice(),
                            _ => {
                                fallback_value_types = entries
                                    .iter()
                                    .map(|(_, value)| self.check_expr(value))
                                    .collect::<Vec<_>>();
                                fallback_value_types.as_slice()
                            }
                        };
                        self.record_fixed_unpack_plan(
                            arg_span,
                            FixedUnpackPlan::Keyword(
                                entries
                                    .iter()
                                    .filter_map(|(key, _)| Self::static_string_key(key).map(str::to_string))
                                    .collect(),
                            ),
                        );
                        for ((key, value), value_ty) in entries.iter().zip(value_types.iter()) {
                            if let Some(name) = Self::static_string_key(key) {
                                if let Some(first_span) = named_seen.insert(name, key.span) {
                                    self.errors
                                        .push(errors::duplicate_call_argument(callee, name, first_span));
                                }

                                if let Some((normal_idx, (_, param))) = normal_params
                                    .iter()
                                    .enumerate()
                                    .find(|(_, (_, param))| param.name() == Some(name))
                                {
                                    if normal_bound_spans[normal_idx].is_some() {
                                        self.errors
                                            .push(errors::duplicate_call_argument(callee, name, key.span));
                                        continue;
                                    }
                                    normal_bound_spans[normal_idx] = Some(value.span);
                                    self.infer_type_param_bindings(&param.ty, value_ty, type_bindings);
                                    self.emit_arg_type_mismatch_if_needed(&param.ty, value_ty, value.span);
                                } else if let Some(param) = rest_keyword {
                                    self.infer_type_param_bindings(&param.ty, value_ty, type_bindings);
                                    self.emit_arg_type_mismatch_if_needed(&param.ty, value_ty, value.span);
                                } else {
                                    self.errors
                                        .push(errors::unknown_keyword_argument(callee, name, key.span));
                                }
                            } else if let Some(param) = rest_keyword {
                                self.infer_type_param_bindings(&param.ty, value_ty, type_bindings);
                                self.emit_arg_type_mismatch_if_needed(&param.ty, value_ty, value.span);
                            } else {
                                self.errors
                                    .push(errors::call_unpack_without_rest(callee, "**", key.span));
                            }
                        }
                    } else if let Some(param) = rest_keyword {
                        let expected = dict_ty(ResolvedType::Str, param.ty.clone());
                        self.infer_type_param_bindings(&expected, arg_ty, type_bindings);
                        self.emit_arg_type_mismatch_if_needed(&expected, arg_ty, arg_span);
                    } else {
                        self.errors
                            .push(errors::call_unpack_without_rest(callee, "**", arg_span));
                    }
                }
            }
        }

        if unexpected_positional > 0 {
            self.errors.push(errors::builtin_arity(
                callee,
                normal_params.len(),
                normal_params.len() + unexpected_positional,
                call_span,
            ));
        }

        for (idx, (_, param)) in normal_params.iter().enumerate() {
            if normal_bound_spans[idx].is_none()
                && !param.has_default
                && let Some(name) = param.name()
            {
                self.errors
                    .push(errors::missing_required_argument(callee, name, call_span));
            }
        }
    }

    /// Emit a call-argument mismatch unless RFC 017 allows a validated-newtype coercion at this argument span.
    fn emit_arg_type_mismatch_if_needed(&mut self, expected: &ResolvedType, actual: &ResolvedType, span: Span) {
        if !self.types_compatible(actual, expected) {
            if self.record_validated_newtype_coercion_if_possible(actual, expected, span) {
                return;
            }
            self.errors
                .push(errors::type_mismatch(&expected.to_string(), &actual.to_string(), span));
        }
    }
}
