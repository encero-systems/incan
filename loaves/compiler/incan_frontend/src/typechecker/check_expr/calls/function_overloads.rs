//! Function overload selection and overload call validation.

use super::*;

impl TypeChecker {
    /// Resolve a direct source overload call and publish only the selected declaration at the callee token.
    #[allow(clippy::too_many_arguments)] // The wrapper keeps overload inputs and the exact callee span distinct.
    pub(super) fn validate_direct_function_overload_call(
        &mut self,
        func_name: &str,
        overloads: &[FunctionOverloadInfo],
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
        callee_span: Span,
        expected_return_ty: Option<&ResolvedType>,
    ) -> ResolvedType {
        self.validate_function_overload_call_with_callee_span(
            func_name,
            overloads,
            type_args,
            args,
            span,
            Some(callee_span),
            expected_return_ty,
        )
    }

    /// Resolve an overload call, optionally recording a direct callee reference after unique candidate selection.
    #[allow(clippy::too_many_arguments)] // Candidate selection consumes independent call and identity evidence axes.
    pub(in crate::typechecker::check_expr) fn validate_function_overload_call_with_callee_span(
        &mut self,
        func_name: &str,
        overloads: &[FunctionOverloadInfo],
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
        callee_span: Option<Span>,
        expected_return_ty: Option<&ResolvedType>,
    ) -> ResolvedType {
        let baseline_errors = self.errors.clone();
        let baseline_warnings = self.warnings.clone();
        let baseline_type_info = self.type_info.clone();
        let baseline_consumed_iterator_bindings = self.consumed_iterator_bindings.clone();

        let mut matches = Vec::new();
        let mut rejections: Vec<(String, String)> = Vec::new();
        for overload in overloads {
            self.errors = baseline_errors.clone();
            self.warnings = baseline_warnings.clone();
            self.type_info = baseline_type_info.clone();
            self.consumed_iterator_bindings = baseline_consumed_iterator_bindings.clone();

            let selected_identity = self.checked_overload_identity(overload);
            let result = self.validate_function_call_with_identity(
                func_name,
                &overload.info,
                type_args,
                args,
                span,
                expected_return_ty,
                selected_identity.clone(),
            );
            if let Some(first_error) = self.errors.get(baseline_errors.len()) {
                rejections.push((
                    self.overload_candidate_signature(func_name, &overload.info),
                    first_error.message.clone(),
                ));
            }
            if self.errors.len() == baseline_errors.len() {
                matches.push((
                    selected_identity,
                    result,
                    self.errors.clone(),
                    self.warnings.clone(),
                    self.type_info.clone(),
                    self.consumed_iterator_bindings.clone(),
                ));
            }
        }

        match matches.len() {
            1 => {
                let (selected_identity, result, errors, warnings, type_info, consumed_iterator_bindings) =
                    matches.remove(0);
                self.errors = errors;
                self.warnings = warnings;
                self.type_info = type_info;
                self.consumed_iterator_bindings = consumed_iterator_bindings;
                if let (Some(callee_span), Some(identity)) = (callee_span, selected_identity) {
                    self.type_info.record_resolved_identity(callee_span, identity);
                }
                result
            }
            0 => {
                self.errors = baseline_errors;
                self.warnings = baseline_warnings;
                self.type_info = baseline_type_info;
                self.consumed_iterator_bindings = baseline_consumed_iterator_bindings;
                // A set with several candidates reports what every one of them wanted; the closest candidate is
                // then re-validated so its own diagnostics keep their argument-level spans (#1373).
                if rejections.len() > 1 {
                    self.errors
                        .push(errors::no_overload_accepts_call(func_name, &rejections, span));
                }
                let shape_match = overloads
                    .iter()
                    .find(|overload| Self::function_call_shape_accepts(&overload.info, type_args, args));
                if let Some(candidate) = shape_match.or_else(|| overloads.first()) {
                    let identity = self.checked_overload_identity(candidate);
                    self.validate_function_call_with_identity(
                        func_name,
                        &candidate.info,
                        type_args,
                        args,
                        span,
                        expected_return_ty,
                        identity,
                    )
                } else {
                    ResolvedType::Unknown
                }
            }
            _ => {
                self.errors = baseline_errors;
                self.warnings = baseline_warnings;
                self.type_info = baseline_type_info;
                self.consumed_iterator_bindings = baseline_consumed_iterator_bindings;
                self.errors.push(CompileError::type_error(
                    format!("Call to overloaded function '{func_name}' is ambiguous"),
                    span,
                ));
                ResolvedType::Unknown
            }
        }
    }

    /// Retain an overload's own provider identity, or the exact checked source declaration when it is local.
    /// An overload set's spelling never selects one member's ownership or generic contract.
    pub(super) fn checked_overload_identity(
        &self,
        candidate: &FunctionOverloadInfo,
    ) -> Option<incan_semantics_core::CanonicalSymbolId> {
        candidate.identity.clone().or_else(|| {
            self.type_info
                .declarations
                .function_bindings_by_span
                .get(&candidate.span)
                .and_then(|binding| binding.identity.clone())
        })
    }

    /// Render one overload candidate as source-shaped signature text for a rejection listing.
    ///
    /// The rendering is for diagnostics only: `name[T with Bound, U](x: int, *rest: str) -> float`. Parameter and
    /// return types use the checker's type display, and bounds use the same display the bound checker reports.
    pub(super) fn overload_candidate_signature(&self, func_name: &str, info: &FunctionInfo) -> String {
        let no_bindings = HashMap::new();
        let type_params = info
            .type_params
            .iter()
            .map(|param| {
                let bounds = info
                    .type_param_bound_details
                    .get(param)
                    .map(|bounds| {
                        bounds
                            .iter()
                            .map(|bound| self.type_bound_display(bound, &no_bindings))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                match bounds.as_slice() {
                    [] => param.clone(),
                    [single] => format!("{param} with {single}"),
                    many => format!("{param} with ({})", many.join(", ")),
                }
            })
            .collect::<Vec<_>>();
        let type_params = if type_params.is_empty() {
            String::new()
        } else {
            format!("[{}]", type_params.join(", "))
        };
        let params = info
            .params
            .iter()
            .map(|param| {
                let prefix = match param.kind {
                    ParamKind::Normal => "",
                    ParamKind::RestPositional => "*",
                    ParamKind::RestKeyword => "**",
                };
                let name = param.name.as_deref().unwrap_or("_");
                let default = if param.has_default { " = ..." } else { "" };
                format!("{prefix}{name}: {}{default}", param.ty)
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("{func_name}{type_params}({params}) -> {}", info.return_type)
    }

    /// Return whether one overload accepts the supplied generic and value-argument counts.
    pub(super) fn function_call_shape_accepts(
        info: &FunctionInfo,
        explicit_type_args: &[Spanned<Type>],
        args: &[CallArg],
    ) -> bool {
        if !explicit_type_args.is_empty() && explicit_type_args.len() != info.type_params.len() {
            return false;
        }
        let normal_params = info
            .params
            .iter()
            .filter(|param| param.kind == ParamKind::Normal)
            .collect::<Vec<_>>();
        let required = normal_params.iter().filter(|param| !param.has_default).count();
        let accepts_extra = info
            .params
            .iter()
            .any(|param| matches!(param.kind, ParamKind::RestPositional | ParamKind::RestKeyword));
        args.len() >= required && (accepts_extra || args.len() <= normal_params.len())
    }
}
