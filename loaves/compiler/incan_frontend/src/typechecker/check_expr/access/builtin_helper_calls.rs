//! Builtin list repetition, selected string helpers, membership probes, dict defaults, and text codecs.

use super::*;

impl TypeChecker {
    /// Bind `list.repeat(...)` arguments to the helper's fixed `value` and `count` parameters.
    ///
    /// This mirrors ordinary fixed-parameter call binding closely enough for named arguments while keeping unpacking
    /// rejected: `list.repeat` has no rest parameters and lowering expects the two canonical arguments explicitly.
    pub(super) fn bind_builtin_list_repeat_args<'a>(
        &mut self,
        args: &'a [CallArg],
        span: Span,
    ) -> (Option<&'a Spanned<Expr>>, Option<&'a Spanned<Expr>>, bool) {
        let helper = BuiltinCollectionHelperId::ListRepeat;
        let callee = collection_helpers::full_name(helper);
        let mut value: Option<&Spanned<Expr>> = None;
        let mut count: Option<&Spanned<Expr>> = None;
        let mut valid = true;
        let mut positional_index = 0usize;

        for arg in args {
            match arg {
                CallArg::Positional(expr) => {
                    let target = match positional_index {
                        0 => Some((&mut value, "value")),
                        1 => Some((&mut count, "count")),
                        _ => None,
                    };
                    positional_index += 1;
                    if let Some((slot, name)) = target {
                        if let Some(first_expr) = *slot {
                            self.errors.push(errors::duplicate_call_argument(
                                callee,
                                name,
                                first_expr.span,
                                expr.span,
                            ));
                            self.check_expr(expr);
                            valid = false;
                        } else {
                            *slot = Some(expr);
                        }
                    } else {
                        self.check_expr(expr);
                        valid = false;
                    }
                }
                CallArg::Named(name, expr) => {
                    let slot = match name.node.as_str() {
                        "value" => Some(&mut value),
                        "count" => Some(&mut count),
                        _ => None,
                    };
                    if let Some(slot) = slot {
                        if let Some(first_expr) = *slot {
                            self.errors.push(errors::duplicate_call_argument(
                                callee,
                                &name.node,
                                first_expr.span,
                                expr.span,
                            ));
                            self.check_expr(expr);
                            valid = false;
                        } else {
                            *slot = Some(expr);
                        }
                    } else {
                        self.errors
                            .push(errors::unknown_keyword_argument(callee, &name.node, name.span));
                        self.check_expr(expr);
                        valid = false;
                    }
                }
                CallArg::PositionalUnpack(expr) => {
                    self.errors
                        .push(errors::call_unpack_without_rest(callee, "*", expr.span));
                    self.check_expr(expr);
                    valid = false;
                }
                CallArg::KeywordUnpack(expr) => {
                    self.errors
                        .push(errors::call_unpack_without_rest(callee, "**", expr.span));
                    self.check_expr(expr);
                    valid = false;
                }
            }
        }

        if args.len() != 2 {
            self.errors.push(errors::builtin_arity(callee, 2, args.len(), span));
            valid = false;
        }
        if value.is_none() {
            self.errors
                .push(errors::missing_required_argument(callee, "value", span));
            valid = false;
        }
        if count.is_none() {
            self.errors
                .push(errors::missing_required_argument(callee, "count", span));
            valid = false;
        }

        (value, count, valid)
    }

    /// Type-check the built-in `list.repeat(value, count)` helper.
    ///
    /// The receiver is the built-in `list` collection surface, not a runtime list value, so this has to run before
    /// ordinary receiver expression checking would report `list` as an unknown value symbol.
    pub(super) fn check_builtin_list_repeat_call(&mut self, args: &[CallArg], span: Span) -> ResolvedType {
        let (value_arg, count_arg, valid_args) = self.bind_builtin_list_repeat_args(args, span);

        let value_ty = value_arg
            .map(|expr| self.check_expr(expr))
            .unwrap_or(ResolvedType::Unknown);

        if let Some(count_arg) = count_arg {
            let count_ty = self.check_expr(count_arg);
            if !self.types_compatible(&count_ty, &ResolvedType::Int) {
                self.errors
                    .push(errors::type_mismatch("int", &count_ty.to_string(), span));
            }
        }

        if !matches!(value_ty, ResolvedType::Unknown) && !self.is_copy_type(&value_ty) && !self.is_clone_type(&value_ty)
        {
            self.errors
                .push(errors::list_repeat_requires_clone(&value_ty.to_string(), span));
        }

        if !valid_args {
            return ResolvedType::Unknown;
        }

        list_ty(value_ty)
    }

    /// Return whether `base` resolves to the built-in `list` collection type surface.
    pub(super) fn is_builtin_list_surface_receiver(&self, base: &Spanned<Expr>) -> bool {
        let Expr::Ident(name) = &base.node else {
            return false;
        };
        name == collection_helpers::receiver(BuiltinCollectionHelperId::ListRepeat)
            && collection_type_id(name.as_str()) == Some(CollectionTypeId::List)
            && self
                .lookup_symbol(name)
                .is_some_and(|sym| matches!(sym.kind, SymbolKind::Type(TypeInfo::Builtin)))
    }

    /// Validate an ordinary positional call before retaining a selected string-helper identity.
    ///
    /// The canonical signature comes from [`StringMethodId::selected_helper_signature`], while this frontend layer
    /// projects its language-level argument kinds into [`ResolvedType`] values. Named and unpacked forms intentionally
    /// do not acquire a helper identity: the string-method registry gives them neither parameter names nor unpacking
    /// semantics, and assigning either here would make a frontend guess into a language contract.
    pub(super) fn validate_selected_string_helper_call(
        &mut self,
        method: StringMethodId,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        span: Span,
    ) -> bool {
        let Some(signature) = method.selected_helper_signature() else {
            return false;
        };

        // Named and unpacked arguments have no selected-helper binding contract yet.
        if !args.iter().all(|arg| matches!(arg, CallArg::Positional(_))) {
            return false;
        }

        // The optional `split` separator is represented by a smaller required count than its positional shape.
        let callee = format!("str.{}", string_methods::as_str(method));
        let supplied = args.len();
        let maximum = signature.positional_arguments.len();
        if supplied < signature.required_positional_arguments {
            self.errors.push(errors::builtin_arity(
                callee.as_str(),
                signature.required_positional_arguments,
                supplied,
                span,
            ));
            return false;
        }
        if supplied > maximum {
            if signature.required_positional_arguments == maximum {
                self.errors
                    .push(errors::builtin_arity(callee.as_str(), maximum, supplied, span));
            } else {
                self.errors
                    .push(errors::builtin_max_arity(callee.as_str(), maximum, supplied, span));
            }
            return false;
        }

        // Each admitted positional slot is checked against the source-owned helper signature.
        let mut valid = true;
        for ((arg, actual), expected_kind) in args
            .iter()
            .zip(arg_types.iter())
            .zip(signature.positional_arguments.iter())
        {
            let expected = match expected_kind {
                SelectedStringMethodArgumentKind::Str => ResolvedType::Str,
                SelectedStringMethodArgumentKind::ListOfStr => list_ty(ResolvedType::Str),
            };
            let CallArg::Positional(expr) = arg else {
                continue;
            };
            if !self.types_compatible(actual, &expected) {
                self.errors.push(errors::call_argument_type_mismatch(
                    callee.as_str(),
                    None,
                    &expected.to_string(),
                    &actual.to_string(),
                    expr.span,
                ));
                valid = false;
            }
        }
        valid
    }

    /// Validate a membership probe call: `dict.contains_key(key)` (#1668), `frozen_dict.contains_key(key)` (#1757) and
    /// `frozen_set.contains(item)`.
    ///
    /// The probe's type must be compatible with `member_ty`, the key or element type, so a mistyped probe fails here
    /// instead of as a rustc `Borrow` error in the generated lookup. The argument count and kinds are checked against
    /// the registry arity with every builtin collection method's ([`Self::check_builtin_collection_method_args`]), so a
    /// wrong count or a named or unpacked argument is already refused by the time this runs; `callee` names the
    /// receiver family in the type-mismatch diagnostic. When `text_probes` is set, a text member (`str` or
    /// `FrozenStr`) takes any text probe, the rule a frozen collection's `'static` text follows.
    pub(super) fn validate_membership_probe_call(
        &mut self,
        callee: &str,
        member_ty: &ResolvedType,
        text_probes: bool,
        args: &[CallArg],
        arg_types: &[ResolvedType],
    ) {
        let [CallArg::Positional(expr)] = args else {
            return;
        };
        let Some(actual) = arg_types.first() else {
            return;
        };
        if matches!(member_ty, ResolvedType::Unknown) || (text_probes && is_str_like(actual) && is_str_like(member_ty))
        {
            return;
        }
        if self.types_compatible(actual, member_ty) {
            // A numeric probe of a narrower type is widened to the member type where it is lowered (RFC 009).
            self.record_value_destination_if_compatible(expr.span, actual, member_ty);
        } else {
            self.errors.push(errors::call_argument_type_mismatch(
                callee,
                None,
                &member_ty.to_string(),
                &actual.to_string(),
                expr.span,
            ));
        }
    }

    /// Validate `dict.get(key, default)`: two positional arguments, a key of the key type and a default of the value
    /// type, since the call is the value itself (the entry's, or `default` when no entry holds `key`).
    pub(super) fn validate_dict_get_with_default(
        &mut self,
        key_ty: &ResolvedType,
        value_ty: &ResolvedType,
        args: &[CallArg],
        arg_types: &[ResolvedType],
    ) {
        const CALLEE: &str = "Dict.get";
        for ((arg, actual), expected) in args.iter().zip(arg_types).zip([key_ty, value_ty]) {
            let expr = match arg {
                CallArg::Positional(expr) => expr,
                CallArg::Named(name, _) => {
                    self.errors
                        .push(errors::unknown_keyword_argument(CALLEE, &name.node, name.span));
                    continue;
                }
                CallArg::PositionalUnpack(expr) => {
                    self.errors
                        .push(errors::call_unpack_without_rest(CALLEE, "*", expr.span));
                    continue;
                }
                CallArg::KeywordUnpack(expr) => {
                    self.errors
                        .push(errors::call_unpack_without_rest(CALLEE, "**", expr.span));
                    continue;
                }
            };
            if matches!(expected, ResolvedType::Unknown) {
                continue;
            }
            if self.types_compatible(actual, expected) {
                self.record_value_destination_if_compatible(expr.span, actual, expected);
            } else {
                self.errors.push(errors::call_argument_type_mismatch(
                    CALLEE,
                    None,
                    &expected.to_string(),
                    &actual.to_string(),
                    expr.span,
                ));
            }
        }
    }

    /// Validate the arguments of a builtin text-codec call (`str.encode` / `bytes.decode`, #1668).
    ///
    /// Both methods take an optional `encoding` and `bytes.decode` also takes an optional `errors`, positionally or
    /// by name. Every argument must be text. A literal label is checked against the `text_codecs` tables here so an
    /// unsupported codec or error policy is a compile-time error; a label that is only known at run time is left to
    /// the emitter's runtime guard, which reads the same tables.
    pub(super) fn validate_text_codec_call(
        &mut self,
        callee: &str,
        accepts_errors_policy: bool,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        span: Span,
    ) {
        const ENCODING: &str = "encoding";
        const ERRORS: &str = "errors";
        let parameters: &[&str] = if accepts_errors_policy {
            &[ENCODING, ERRORS]
        } else {
            &[ENCODING]
        };

        // ---- Bind each argument to its parameter, rejecting shapes the surface does not offer ----
        let mut bound: Vec<(&str, &Spanned<Expr>, &ResolvedType)> = Vec::new();
        let mut positional_count = 0usize;
        for (arg, actual) in args.iter().zip(arg_types.iter()) {
            match arg {
                CallArg::Positional(expr) => {
                    let Some(parameter) = parameters.get(positional_count) else {
                        self.errors
                            .push(errors::builtin_max_arity(callee, parameters.len(), args.len(), span));
                        return;
                    };
                    positional_count += 1;
                    bound.push((parameter, expr, actual));
                }
                CallArg::Named(name, expr) => {
                    let Some(parameter) = parameters.iter().find(|parameter| **parameter == name.node) else {
                        self.errors
                            .push(errors::unknown_keyword_argument(callee, &name.node, name.span));
                        return;
                    };
                    if let Some((_, first, _)) = bound.iter().find(|(seen, _, _)| seen == parameter) {
                        self.errors.push(errors::duplicate_call_argument(
                            callee, parameter, first.span, name.span,
                        ));
                        return;
                    }
                    bound.push((parameter, expr, actual));
                }
                CallArg::PositionalUnpack(expr) => {
                    self.errors
                        .push(errors::call_unpack_without_rest(callee, "*", expr.span));
                    return;
                }
                CallArg::KeywordUnpack(expr) => {
                    self.errors
                        .push(errors::call_unpack_without_rest(callee, "**", expr.span));
                    return;
                }
            }
        }

        // ---- Each bound label must be text, and a literal label must name a supported codec or policy ----
        for (parameter, expr, actual) in bound {
            if !is_str_like(actual) {
                self.errors.push(errors::call_argument_type_mismatch(
                    callee,
                    Some(parameter),
                    &ResolvedType::Str.to_string(),
                    &actual.to_string(),
                    expr.span,
                ));
                continue;
            }
            let Expr::Literal(Literal::String(label)) = &expr.node else {
                continue;
            };
            if parameter == ENCODING && !text_codecs::is_utf8_encoding_label(label) {
                self.errors
                    .push(errors::unsupported_text_encoding(callee, label, expr.span));
            }
            if parameter == ERRORS && DecodeErrorsPolicy::from_label(label).is_none() {
                self.errors
                    .push(errors::unsupported_decode_errors_policy(label, expr.span));
            }
        }
    }
}
