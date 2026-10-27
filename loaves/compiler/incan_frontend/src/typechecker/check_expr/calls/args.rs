//! Argument checking, binding, and shaped unpack helpers for call expressions.

use super::TypeChecker;
use crate::ast::{CallArg, DictEntry, Expr, ListEntry, Literal, ParamKind, Span, Spanned};
use crate::diagnostics::errors;
use crate::symbols::{CallableParam, ResolvedType};
use crate::typechecker::FixedUnpackPlan;
use crate::typechecker::helpers::{collection_type_id, dict_ty, list_ty};
use incan_lang::lang::types::collections::CollectionTypeId;
use incan_lang::lang::types::numerics::{self, NumericFamily};

/// Select the narrow numeric bridge accepted while validating one call surface.
///
/// Ordinary Incan callables retain the global lossless-numeric rule. Checked C calls additionally admit the
/// ordinary `int` carrier for an exact integer ABI parameter; generated wrappers perform the corresponding
/// fallible conversion before crossing the foreign boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CallArgumentCompatibility {
    Ordinary,
    CheckedCIntegerBridge,
}

/// Call-wide facts shared by each argument binding under one compatibility rule.
///
/// Keeping these facts together makes the selected compatibility rule explicit without widening the internal
/// validator's argument list as new checked call surfaces are added.
struct CallArgumentValidationContext<'a> {
    callee: &'a str,
    call_span: Span,
    compatibility: CallArgumentCompatibility,
}

/// Per-call binding state shared by ordinary arguments and shaped unpack items. Parameter and source names are
/// borrowed; only the validator's existing slot, duplicate-name and cursor inventories are retained here.
struct CallArgumentBindings<'params, 'args, 'call> {
    normal_params: Vec<(usize, &'params CallableParam)>,
    positional_param_indices: Vec<usize>,
    rest_positional: Option<&'params CallableParam>,
    rest_keyword: Option<&'params CallableParam>,
    normal_bound_spans: Vec<Option<Span>>,
    named_seen: std::collections::HashMap<&'args str, Span>,
    positional_index: usize,
    unexpected_positional: usize,
    type_bindings: &'call mut std::collections::HashMap<String, ResolvedType>,
    context: CallArgumentValidationContext<'call>,
}

impl<'params, 'args, 'call> CallArgumentBindings<'params, 'args, 'call> {
    /// Build the fixed/rest parameter projection once for this call. Preset slots remain name-addressable but do
    /// not consume positional arguments, matching the partial callable's retained declaration contract.
    fn new(
        params: &'params [CallableParam],
        type_bindings: &'call mut std::collections::HashMap<String, ResolvedType>,
        context: CallArgumentValidationContext<'call>,
    ) -> Self {
        let normal_params: Vec<_> = params
            .iter()
            .enumerate()
            .filter(|(_, param)| param.kind == ParamKind::Normal)
            .collect();
        let positional_param_indices = normal_params
            .iter()
            .enumerate()
            .filter_map(|(normal_idx, (_, param))| (!param.is_partial_preset).then_some(normal_idx))
            .collect();
        let normal_bound_spans = vec![None; normal_params.len()];
        Self {
            normal_params,
            positional_param_indices,
            rest_positional: params.iter().find(|param| param.kind == ParamKind::RestPositional),
            rest_keyword: params.iter().find(|param| param.kind == ParamKind::RestKeyword),
            normal_bound_spans,
            named_seen: std::collections::HashMap::new(),
            positional_index: 0,
            unexpected_positional: 0,
            type_bindings,
            context,
        }
    }

    /// Infer binders before checking one parameter's compatibility, retaining the selected call-local integer
    /// bridge and argument-adjustment behavior for both direct arguments and unpacked items.
    fn validate_parameter(
        &mut self,
        checker: &mut TypeChecker,
        param: &CallableParam,
        actual: &ResolvedType,
        span: Span,
    ) {
        checker.infer_call_argument_type_param_bindings(&param.ty, actual, self.type_bindings);
        checker.emit_arg_type_mismatch_if_needed(
            self.context.callee,
            param.name(),
            &param.ty,
            actual,
            span,
            self.context.compatibility,
        );
    }

    /// Bind one positional value to the next non-preset slot or positional rest parameter. Rest values do not
    /// advance the fixed-slot cursor; surplus values are counted for the call-wide arity diagnostic.
    fn bind_positional(&mut self, checker: &mut TypeChecker, actual: &ResolvedType, span: Span) {
        if let Some(&normal_idx) = self.positional_param_indices.get(self.positional_index) {
            let (_, param) = self.normal_params[normal_idx];
            self.normal_bound_spans[normal_idx] = Some(span);
            self.validate_parameter(checker, param, actual, span);
            self.positional_index += 1;
        } else if let Some(param) = self.rest_positional {
            self.validate_parameter(checker, param, actual, span);
        } else {
            self.unexpected_positional += 1;
        }
    }

    /// Bind a named value after recording its source-name occurrence. A slot already bound emits its separate
    /// duplicate diagnostic and skips inference, preserving the distinction between name and slot collisions.
    fn bind_named(
        &mut self,
        checker: &mut TypeChecker,
        name: &'args str,
        name_span: Span,
        actual: &ResolvedType,
        value_span: Span,
    ) {
        if let Some(first_span) = self.named_seen.insert(name, name_span) {
            checker.errors.push(errors::duplicate_call_argument(
                self.context.callee,
                name,
                first_span,
                name_span,
            ));
        }
        if let Some((normal_idx, (_, param))) = self
            .normal_params
            .iter()
            .enumerate()
            .find(|(_, (_, param))| param.name() == Some(name))
        {
            if let Some(first_span) = self.normal_bound_spans[normal_idx] {
                checker.errors.push(errors::duplicate_call_argument(
                    self.context.callee,
                    name,
                    first_span,
                    name_span,
                ));
                return;
            }
            let param = *param;
            self.normal_bound_spans[normal_idx] = Some(value_span);
            self.validate_parameter(checker, param, actual, value_span);
        } else if let Some(param) = self.rest_keyword {
            self.validate_parameter(checker, param, actual, value_span);
        } else {
            checker
                .errors
                .push(errors::unknown_keyword_argument(self.context.callee, name, name_span));
        }
    }

    /// Expand a statically shaped positional unpack before binding its items. Missing tuple facts retain the
    /// existing fallback expression checks; unshaped operands require the declared positional rest carrier.
    fn validate_positional_unpack(&mut self, checker: &mut TypeChecker, expr: &Spanned<Expr>, actual: &ResolvedType) {
        if let Some(items) = TypeChecker::shaped_positional_unpack_items(expr) {
            let fallback_item_types;
            let item_types = match actual {
                ResolvedType::Tuple(item_types) => item_types.as_slice(),
                _ => {
                    fallback_item_types = items.iter().map(|item| checker.check_expr(item)).collect::<Vec<_>>();
                    fallback_item_types.as_slice()
                }
            };
            checker.record_fixed_unpack_plan(expr.span, FixedUnpackPlan::Positional(item_types.to_vec()));
            for (item, item_ty) in items.iter().zip(item_types.iter()) {
                self.bind_positional(checker, item_ty, item.span);
            }
        } else if let Some(item_types) = TypeChecker::shaped_positional_unpack_types(actual) {
            checker.record_fixed_unpack_plan(expr.span, FixedUnpackPlan::Positional(item_types.to_vec()));
            for item_ty in item_types {
                self.bind_positional(checker, item_ty, expr.span);
            }
        } else if let Some(param) = self.rest_positional {
            let expected = list_ty(param.ty.clone());
            checker.infer_call_argument_type_param_bindings(&expected, actual, self.type_bindings);
            checker.emit_arg_type_mismatch_if_needed(
                self.context.callee,
                param.name(),
                &expected,
                actual,
                expr.span,
                self.context.compatibility,
            );
        } else {
            checker
                .errors
                .push(errors::call_unpack_without_rest(self.context.callee, "*", expr.span));
        }
    }

    /// Expand fixed keyword entries in source order, retaining key spans for name collisions and value spans for
    /// type checks. An unshaped mapping is compatible only with the declared keyword rest carrier.
    fn validate_keyword_unpack(
        &mut self,
        checker: &mut TypeChecker,
        expr: &'args Spanned<Expr>,
        actual: &ResolvedType,
    ) {
        if let Some(entries) = TypeChecker::shaped_keyword_unpack_entries(expr) {
            let fallback_value_types;
            let value_types = match actual {
                ResolvedType::Tuple(value_types) => value_types.as_slice(),
                _ => {
                    fallback_value_types = entries
                        .iter()
                        .map(|(_, value)| checker.check_expr(value))
                        .collect::<Vec<_>>();
                    fallback_value_types.as_slice()
                }
            };
            checker.record_fixed_unpack_plan(
                expr.span,
                FixedUnpackPlan::Keyword(
                    entries
                        .iter()
                        .filter_map(|(key, _)| TypeChecker::static_string_key(key).map(str::to_string))
                        .collect(),
                ),
            );
            for ((key, value), value_ty) in entries.iter().zip(value_types.iter()) {
                if let Some(name) = TypeChecker::static_string_key(key) {
                    self.bind_named(checker, name, key.span, value_ty, value.span);
                } else if let Some(param) = self.rest_keyword {
                    self.validate_parameter(checker, param, value_ty, value.span);
                } else {
                    checker
                        .errors
                        .push(errors::call_unpack_without_rest(self.context.callee, "**", key.span));
                }
            }
        } else if let Some(param) = self.rest_keyword {
            let expected = dict_ty(ResolvedType::Str, param.ty.clone());
            checker.infer_call_argument_type_param_bindings(&expected, actual, self.type_bindings);
            checker.emit_arg_type_mismatch_if_needed(
                self.context.callee,
                param.name(),
                &expected,
                actual,
                expr.span,
                self.context.compatibility,
            );
        } else {
            checker
                .errors
                .push(errors::call_unpack_without_rest(self.context.callee, "**", expr.span));
        }
    }

    /// Emit aggregate arity and missing-slot errors only after every argument has contributed its own diagnostics
    /// and binding facts. Required slots follow declaration order, including name-addressable preset positions.
    fn finish(self, checker: &mut TypeChecker) {
        if self.unexpected_positional > 0 {
            checker.errors.push(errors::builtin_arity(
                self.context.callee,
                self.positional_param_indices.len(),
                self.positional_param_indices.len() + self.unexpected_positional,
                self.context.call_span,
            ));
        }
        for (idx, (_, param)) in self.normal_params.iter().enumerate() {
            if self.normal_bound_spans[idx].is_none()
                && !param.has_default
                && let Some(name) = param.name()
            {
                checker.errors.push(errors::missing_required_argument(
                    self.context.callee,
                    name,
                    self.context.call_span,
                ));
            }
        }
    }
}

impl TypeChecker {
    /// Infer a call argument's binders through the outer shared borrow already admitted by argument compatibility.
    /// Matching reference arguments retain structural inference. This adjustment belongs to argument binding rather
    /// than general type inference, where applying a borrow to a return type would change the checked contract.
    fn infer_call_argument_type_param_bindings(
        &self,
        expected: &ResolvedType,
        actual: &ResolvedType,
        bindings: &mut std::collections::HashMap<String, ResolvedType>,
    ) {
        let expected = match expected {
            ResolvedType::Ref(inner) if !matches!(actual, ResolvedType::Ref(_) | ResolvedType::RefMut(_)) => inner,
            _ => expected,
        };
        self.infer_type_param_bindings(expected, actual, bindings);
    }

    /// Return the expression carried by a call argument.
    pub(in crate::typechecker::check_expr) fn call_arg_expr(arg: &CallArg) -> &Spanned<Expr> {
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
    pub(in crate::typechecker::check_expr) fn check_call_args(&mut self, args: &[CallArg]) {
        for arg in args {
            self.call_argument_depth += 1;
            self.check_expr(Self::call_arg_expr(arg));
            self.call_argument_depth -= 1;
        }
    }

    /// Type-check all call arguments and collect their resolved types.
    pub(in crate::typechecker::check_expr::calls) fn check_call_arg_types(
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

    /// Type-check call arguments against a generic callee's parameters, each closure argument after the others, so a
    /// closure written for a parameter whose function type names one of the callee's type parameters takes the types
    /// the other arguments bind them to: `apply_twice((x) => x + 1, 3)` for `apply_twice[T](f: (T) -> T, value: T)`
    /// checks `x` as `int`.
    ///
    /// `bindings` are the type parameters the call already fixes (brackets, the expected result). A call with an
    /// unpacked argument, or whose closures leave no type parameter open, is checked in order by
    /// [`Self::check_call_arg_types_for_params`].
    pub(in crate::typechecker::check_expr) fn check_call_arg_types_binding_closures_last(
        &mut self,
        args: &[CallArg],
        params: &[CallableParam],
        bindings: &std::collections::HashMap<String, ResolvedType>,
    ) -> Vec<ResolvedType> {
        let Some(slots) = Self::call_arg_parameter_slots(args, params) else {
            return self.check_call_arg_types_for_params(args, params);
        };
        let is_closure = |arg: &CallArg| matches!(Self::call_arg_expr(arg).node, Expr::Closure(_, _));
        let open_closure = args
            .iter()
            .zip(&slots)
            .any(|(arg, slot)| is_closure(arg) && slot.as_ref().is_some_and(Self::mentions_type_variable));
        if !open_closure {
            return self.check_call_arg_types_for_params(args, params);
        }
        let mut bindings = bindings.clone();
        let mut arg_types = vec![ResolvedType::Unknown; args.len()];
        for (index, (arg, slot)) in args.iter().zip(&slots).enumerate() {
            if is_closure(arg) {
                continue;
            }
            self.call_argument_depth += 1;
            let ty = self.check_expr_with_expected(Self::call_arg_expr(arg), slot.as_ref());
            self.call_argument_depth -= 1;
            if let Some(slot) = slot {
                self.infer_type_param_bindings(slot, &ty, &mut bindings);
            }
            arg_types[index] = ty;
        }
        for (index, (arg, slot)) in args.iter().zip(&slots).enumerate() {
            if !is_closure(arg) {
                continue;
            }
            let expected = slot
                .as_ref()
                .map(|slot| crate::resolved_type_subst::substitute_resolved_type(slot, &bindings));
            self.call_argument_depth += 1;
            arg_types[index] = self.check_expr_with_expected(Self::call_arg_expr(arg), expected.as_ref());
            self.call_argument_depth -= 1;
        }
        arg_types
    }

    /// Return the parameter type each positional or named argument binds to, as
    /// [`Self::check_call_arg_types_for_params`] binds them, or `None` when an argument is unpacked.
    fn call_arg_parameter_slots(args: &[CallArg], params: &[CallableParam]) -> Option<Vec<Option<ResolvedType>>> {
        let normal_params: Vec<&CallableParam> =
            params.iter().filter(|param| param.kind == ParamKind::Normal).collect();
        let positional_params: Vec<&CallableParam> = normal_params
            .iter()
            .copied()
            .filter(|param| !param.is_partial_preset)
            .collect();
        let rest_positional = params.iter().find(|param| param.kind == ParamKind::RestPositional);
        let rest_keyword = params.iter().find(|param| param.kind == ParamKind::RestKeyword);
        let mut positional_index = 0usize;
        args.iter()
            .map(|arg| match arg {
                CallArg::Positional(_) => {
                    let slot = positional_params
                        .get(positional_index)
                        .map(|param| param.ty.clone())
                        .or_else(|| rest_positional.map(|param| param.ty.clone()));
                    positional_index += 1;
                    Some(slot)
                }
                CallArg::Named(name, _) => Some(
                    normal_params
                        .iter()
                        .find(|param| param.name() == Some(name.node.as_str()))
                        .map(|param| param.ty.clone())
                        .or_else(|| rest_keyword.map(|param| param.ty.clone())),
                ),
                CallArg::PositionalUnpack(_) | CallArg::KeywordUnpack(_) => None,
            })
            .collect()
    }

    /// Return whether a type names a type variable anywhere in its structure.
    fn mentions_type_variable(ty: &ResolvedType) -> bool {
        match ty {
            ResolvedType::TypeVar(_) => true,
            ResolvedType::Generic(_, args) | ResolvedType::Tuple(args) => args.iter().any(Self::mentions_type_variable),
            ResolvedType::Function(params, ret) => {
                params.iter().any(|param| Self::mentions_type_variable(&param.ty)) || Self::mentions_type_variable(ret)
            }
            ResolvedType::FrozenList(inner)
            | ResolvedType::FrozenSet(inner)
            | ResolvedType::TypeToken(inner)
            | ResolvedType::Ref(inner)
            | ResolvedType::RefMut(inner) => Self::mentions_type_variable(inner),
            ResolvedType::FrozenDict(key, value) => {
                Self::mentions_type_variable(key) || Self::mentions_type_variable(value)
            }
            _ => false,
        }
    }

    /// Type-check call arguments while threading parameter types into contextual-expression checks when available.
    pub(in crate::typechecker::check_expr) fn check_call_arg_types_for_params(
        &mut self,
        args: &[CallArg],
        params: &[CallableParam],
    ) -> Vec<ResolvedType> {
        let normal_params: Vec<&CallableParam> =
            params.iter().filter(|param| param.kind == ParamKind::Normal).collect();
        let positional_params: Vec<&CallableParam> = normal_params
            .iter()
            .copied()
            .filter(|param| !param.is_partial_preset)
            .collect();
        let rest_positional = params.iter().find(|param| param.kind == ParamKind::RestPositional);
        let rest_keyword = params.iter().find(|param| param.kind == ParamKind::RestKeyword);
        let mut positional_index = 0usize;

        let mut arg_types = Vec::with_capacity(args.len());
        for arg in args {
            match arg {
                CallArg::Positional(expr) => {
                    let expected = positional_params
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
                        .find(|param| param.name() == Some(name.node.as_str()))
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
                            let expected = positional_params
                                .get(positional_index)
                                .map(|param| param.ty.clone())
                                .or_else(|| rest_positional.map(|param| param.ty.clone()));
                            if positional_index < positional_params.len() {
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
    pub(in crate::typechecker) fn validate_callable_arg_bindings(
        &mut self,
        callee: &str,
        params: &[CallableParam],
        args: &[CallArg],
        arg_types: &[ResolvedType],
        type_bindings: &mut std::collections::HashMap<String, ResolvedType>,
        call_span: Span,
    ) {
        self.validate_callable_arg_bindings_with_compatibility(
            params,
            args,
            arg_types,
            type_bindings,
            CallArgumentValidationContext {
                callee,
                call_span,
                compatibility: CallArgumentCompatibility::Ordinary,
            },
        );
    }

    /// Validate a checked C call while admitting the bounded ordinary-integer bridge for exact ABI integers.
    pub(in crate::typechecker::check_expr) fn validate_checked_c_callable_arg_bindings(
        &mut self,
        callee: &str,
        params: &[CallableParam],
        args: &[CallArg],
        arg_types: &[ResolvedType],
        type_bindings: &mut std::collections::HashMap<String, ResolvedType>,
        call_span: Span,
    ) {
        self.validate_callable_arg_bindings_with_compatibility(
            params,
            args,
            arg_types,
            type_bindings,
            CallArgumentValidationContext {
                callee,
                call_span,
                compatibility: CallArgumentCompatibility::CheckedCIntegerBridge,
            },
        );
    }

    /// Validate call arguments against callable parameters under one explicitly selected compatibility rule.
    fn validate_callable_arg_bindings_with_compatibility(
        &mut self,
        params: &[CallableParam],
        args: &[CallArg],
        arg_types: &[ResolvedType],
        type_bindings: &mut std::collections::HashMap<String, ResolvedType>,
        context: CallArgumentValidationContext<'_>,
    ) {
        let mut bindings = CallArgumentBindings::new(params, type_bindings, context);
        for (arg, arg_ty) in args.iter().zip(arg_types.iter()) {
            let arg_span = Self::call_arg_expr(arg).span;
            match arg {
                CallArg::Positional(_) => bindings.bind_positional(self, arg_ty, arg_span),
                CallArg::PositionalUnpack(_) => {
                    bindings.validate_positional_unpack(self, Self::call_arg_expr(arg), arg_ty);
                }
                CallArg::Named(name, _) => {
                    bindings.bind_named(self, name.node.as_str(), name.span, arg_ty, arg_span);
                }
                CallArg::KeywordUnpack(_) => {
                    bindings.validate_keyword_unpack(self, Self::call_arg_expr(arg), arg_ty);
                }
            }
        }

        bindings.finish(self);
    }

    /// Validate a source call argument, retaining metadata-selected dereferences and owned target materialization at
    /// this boundary. An owned value at a shared parameter is borrowed by Body IR's retained parameter contract;
    /// assignment compatibility does not acquire these adjustments. RFC 017 coercions stay scoped to this span.
    fn emit_arg_type_mismatch_if_needed(
        &mut self,
        callee: &str,
        parameter: Option<&str>,
        expected: &ResolvedType,
        actual: &ResolvedType,
        span: Span,
        compatibility: CallArgumentCompatibility,
    ) {
        // ---- Native argument adjustments ----
        if self.retain_argument_dereferences(actual, expected, span).is_some() {
            return;
        }

        // ---- Source compatibility ----
        let compatible = match expected {
            ResolvedType::Ref(inner) if !matches!(actual, ResolvedType::Ref(_) | ResolvedType::RefMut(_)) => {
                self.types_compatible(actual, inner)
            }
            _ => self.types_compatible(actual, expected),
        };

        // ---- Boundary coercions and refusal ----
        if !compatible
            && !(compatibility == CallArgumentCompatibility::CheckedCIntegerBridge
                && Self::checked_c_integer_bridge_compatible(actual, expected))
        {
            if self.record_validated_newtype_coercion_if_possible(actual, expected, span) {
                return;
            }
            self.errors.push(errors::call_argument_type_mismatch(
                callee,
                parameter,
                &expected.to_string(),
                &actual.to_string(),
                span,
            ));
        }
    }

    /// Return whether an ordinary Incan integer may cross one checked C integer boundary.
    ///
    /// This deliberately does not make numeric narrowing generally compatible. The checked C emitter owns the only
    /// conversion and uses `TryInto`, so a negative or out-of-range value fails before the foreign call begins.
    pub(in crate::typechecker::check_expr) fn checked_c_integer_bridge_compatible(
        actual: &ResolvedType,
        expected: &ResolvedType,
    ) -> bool {
        matches!(actual, ResolvedType::Int)
            && matches!(expected, ResolvedType::Numeric(id) if matches!(
                numerics::info_for(*id).family,
                NumericFamily::SignedInteger | NumericFamily::UnsignedInteger
            ))
    }
}
