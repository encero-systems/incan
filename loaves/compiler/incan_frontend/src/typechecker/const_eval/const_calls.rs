//! Const calls, newtype constants, model constructors and literals.

use super::*;

impl TypeChecker {
    /// Validate a const model, partial constructor or expected numeric newtype call without explicit type arguments.
    pub(super) fn eval_const_call(
        &mut self,
        callee: &Spanned<Expr>,
        args: &[CallArg],
        expected: Option<&ResolvedType>,
        stack: &mut Vec<String>,
        decl_span: Span,
        span: Span,
    ) -> Option<ConstEvalResult> {
        if let Expr::Ident(name) = &callee.node
            && self.is_const_model_constructor_name(name)
        {
            return self.eval_const_model_constructor(name, args, expected, stack, decl_span, span);
        }
        if let Expr::Ident(name) = &callee.node
            && let Some(result) = self.eval_const_partial_constructor_call(name, args, expected, stack, decl_span, span)
        {
            return Some(result);
        }
        let Some(ResolvedType::Named(expected_name)) = expected else {
            self.errors.push(errors::const_expression_not_allowed(span));
            return None;
        };
        let Expr::Ident(name) = &callee.node else {
            self.errors.push(errors::const_expression_not_allowed(span));
            return None;
        };
        let Some(TypeInfo::Newtype(newtype)) = self.lookup_type_info(name) else {
            self.errors.push(errors::const_expression_not_allowed(span));
            return None;
        };
        let underlying = newtype.underlying.clone();
        if name != expected_name {
            self.errors.push(errors::const_expression_not_allowed(span));
            return None;
        }
        let [CallArg::Positional(arg)] = args else {
            self.errors.push(errors::const_expression_not_allowed(span));
            return None;
        };
        let Some(target) = super::numeric_type_id_for_compat(&underlying) else {
            self.errors.push(errors::const_expression_not_allowed(span));
            return None;
        };
        let Expr::Literal(Literal::Int(literal)) = &arg.node else {
            self.errors.push(errors::const_expression_not_allowed(span));
            return None;
        };
        if !unsigned_int_literal_fits_numeric_target(literal, target) {
            self.errors.push(CompileError::type_error(
                format!("Integer literal {} does not fit in {}", literal.repr, underlying),
                arg.span,
            ));
            return None;
        }
        if literal.suffix.is_none() {
            self.retain_body_ir_const_newtype(name, &underlying, literal.magnitude, span);
        }
        Some(ConstEvalResult {
            ty: ResolvedType::Named(expected_name.clone()),
            kind: ConstKind::RustNative,
            value: None,
        })
    }

    /// Retain only an already range-checked unsuffixed sized integer constructor, without altering legacy evaluation.
    pub(super) fn retain_body_ir_const_newtype(
        &mut self,
        name: &str,
        underlying: &ResolvedType,
        magnitude: u128,
        span: Span,
    ) {
        use incan_lang::lang::types::numerics::{NumericFamily, info_for};
        use incan_semantics_core::body_ir::TypedNumericConstant;
        let ResolvedType::Numeric(kind) = underlying else {
            return;
        };
        let Some(owner) = self.named_type_reference_identity(name) else {
            return;
        };
        if owner.kind != incan_semantics_core::SemanticSourceTargetKind::Newtype {
            return;
        }
        let value = match info_for(*kind).family {
            NumericFamily::SignedInteger => match i128::try_from(magnitude) {
                Ok(value) => TypedNumericConstant::Signed { kind: *kind, value },
                Err(_) => return,
            },
            NumericFamily::UnsignedInteger => TypedNumericConstant::Unsigned {
                kind: *kind,
                value: magnitude,
            },
            _ => return,
        };
        self.type_info
            .consts
            .body_ir_newtype_values
            .insert(span, super::type_info::BodyIrNewtypeConstantValue { owner, value });
    }

    /// Return whether a name resolves to a model constructor that can be considered for const literal validation.
    pub(super) fn is_const_model_constructor_name(&self, name: &str) -> bool {
        self.lookup_type_info(name)
            .is_some_and(|info| matches!(info, TypeInfo::Model(_)))
    }

    /// Evaluate a model-constructor partial call inside a const initializer.
    pub(super) fn eval_const_partial_constructor_call(
        &mut self,
        callee_name: &str,
        args: &[CallArg],
        expected: Option<&ResolvedType>,
        stack: &mut Vec<String>,
        decl_span: Span,
        call_span: Span,
    ) -> Option<ConstEvalResult> {
        let projection = self.type_info.partial_projection(callee_name)?.clone();
        if projection.target_kind != PartialProjectionTargetKind::ModelConstructor {
            return None;
        }
        let target_name = projection.target_path.last()?;
        let merged_args = merge_named_partial_args(
            projection.presets.iter().map(|preset| PartialPresetRef {
                name: preset.name.as_str(),
                value: &preset.value,
            }),
            args,
        )?;
        self.eval_const_model_constructor(target_name, &merged_args, expected, stack, decl_span, call_span)
    }

    /// Evaluate a model constructor in a const initializer.
    ///
    /// This keeps `const` model literals declaration-safe: every provided field must itself be const-evaluable, all
    /// required fields must be explicit, and omitted defaults are rejected because model defaults are ordinary runtime
    /// expressions rather than const metadata.
    pub(super) fn eval_const_model_constructor(
        &mut self,
        type_name: &str,
        args: &[CallArg],
        expected: Option<&ResolvedType>,
        stack: &mut Vec<String>,
        decl_span: Span,
        call_span: Span,
    ) -> Option<ConstEvalResult> {
        if let Some(expected_ty) = expected
            && !matches!(expected_ty, ResolvedType::Named(name) if name == type_name)
            && !matches!(expected_ty, ResolvedType::Unknown)
        {
            return Some(ConstEvalResult {
                ty: ResolvedType::Named(type_name.to_string()),
                kind: ConstKind::RustNative,
                value: None,
            });
        }

        let Some(TypeInfo::Model(model)) = self.lookup_type_info(type_name).cloned() else {
            self.errors.push(errors::const_expression_not_allowed(call_span));
            return None;
        };

        let mut provided = std::collections::HashSet::new();
        let mut result_kind = ConstKind::RustNative;
        let mut had_error = false;

        for arg in args {
            let CallArg::Named(field_name, value) = arg else {
                self.errors
                    .push(errors::positional_constructor_args_not_supported(type_name, call_span));
                had_error = true;
                continue;
            };

            let Some((canonical_name, field_info)) = Self::resolve_const_model_field(&model.fields, &field_name.node)
            else {
                self.eval_const_expr(value, None, stack, decl_span);
                self.errors
                    .push(errors::missing_field(type_name, &field_name.node, field_name.span));
                had_error = true;
                continue;
            };

            if !provided.insert(canonical_name.clone()) {
                self.errors.push(errors::duplicate_field_in_call(
                    type_name,
                    canonical_name.as_str(),
                    value.span,
                ));
                had_error = true;
                continue;
            }

            let Some(field_result) = self.eval_const_expr(value, Some(&field_info.ty), stack, decl_span) else {
                had_error = true;
                continue;
            };
            if field_result.kind == ConstKind::Frozen {
                result_kind = ConstKind::Frozen;
            }

            if field_result.ty != field_info.ty {
                match self.const_numeric_value_checked_against_numeric_expected(
                    &field_result,
                    &field_info.ty,
                    value.span,
                ) {
                    Some(true) => {}
                    Some(false) => had_error = true,
                    None => {
                        self.errors.push(errors::field_type_mismatch(
                            &field_name.node,
                            &field_info.ty.to_string(),
                            &field_result.ty.to_string(),
                            field_name.span,
                        ));
                        had_error = true;
                    }
                }
            }
        }

        for (field_name, field_info) in &model.fields {
            if provided.contains(field_name) {
                continue;
            }
            if field_info.has_default {
                self.errors.push(CompileError::type_error(
                    format!(
                        "Const model constructor '{}' must provide field '{}' explicitly; model defaults are not evaluated in const initializers",
                        type_name, field_name
                    ),
                    call_span,
                ));
            } else {
                self.errors.push(errors::missing_required_constructor_field(
                    type_name, field_name, call_span,
                ));
            }
            had_error = true;
        }

        if had_error {
            return None;
        }

        Some(ConstEvalResult {
            ty: ResolvedType::Named(type_name.to_string()),
            kind: result_kind,
            value: None,
        })
    }

    /// Resolve a model constructor field by canonical source name or model alias.
    pub(super) fn resolve_const_model_field<'a>(
        fields: &'a std::collections::HashMap<String, crate::symbols::FieldInfo>,
        field_name: &str,
    ) -> Option<(String, &'a crate::symbols::FieldInfo)> {
        fields
            .get(field_name)
            .map(|info| (field_name.to_string(), info))
            .or_else(|| {
                fields
                    .iter()
                    .find(|(_, info)| info.alias.as_deref() == Some(field_name))
                    .map(|(name, info)| (name.clone(), info))
            })
    }

    /// Retain an integer literal only when its exact signed magnitude fits the default constant carrier. An
    /// unretained value keeps its declared numeric carrier and proceeds through ordinary expected-type checking,
    /// which validates the full magnitude instead of publishing a saturated integer to Body IR.
    pub(super) fn eval_const_integer_literal(
        literal: &IntLiteral,
        negative: bool,
        expected: Option<&ResolvedType>,
    ) -> ConstEvalResult {
        let value = literal
            .signed_value(negative)
            .and_then(|value| i64::try_from(value).ok())
            .map(ConstValue::Int);
        let ty = if value.is_some() {
            ResolvedType::Int
        } else {
            literal
                .suffix
                .map(ResolvedType::from_numeric_id)
                .or_else(|| {
                    expected
                        .filter(|ty| super::numeric_type_id_for_compat(ty).is_some())
                        .cloned()
                })
                .unwrap_or(ResolvedType::Int)
        };
        ConstEvalResult {
            ty,
            kind: ConstKind::RustNative,
            value,
        }
    }

    /// Evaluate a literal in a const context, optionally checking it against an expected type.
    pub(super) fn eval_const_literal(
        &mut self,
        lit: &Literal,
        expected: Option<&ResolvedType>,
        span: Span,
        _decl_span: Span,
    ) -> ConstEvalResult {
        match lit {
            Literal::Int(il) => Self::eval_const_integer_literal(il, false, expected),
            Literal::Float(f) => ConstEvalResult {
                ty: ResolvedType::Float,
                kind: ConstKind::RustNative,
                value: Some(ConstValue::Float(f.value)),
            },
            Literal::Decimal(_) => ConstEvalResult {
                ty: ResolvedType::Unknown,
                kind: ConstKind::RustNative,
                value: None,
            },
            Literal::Bool(b) => ConstEvalResult {
                ty: ResolvedType::Bool,
                kind: ConstKind::RustNative,
                value: Some(ConstValue::Bool(*b)),
            },
            Literal::String(s) => ConstEvalResult {
                ty: frozen_str_ty(),
                kind: ConstKind::Frozen,
                value: Some(ConstValue::FrozenStr(s.clone())),
            },
            Literal::Bytes(b) => ConstEvalResult {
                ty: frozen_bytes_ty(),
                kind: ConstKind::Frozen,
                value: Some(ConstValue::FrozenBytes(b.clone())),
            },
            Literal::None => {
                // None is ambiguous without annotation.
                let ty = expected.cloned().unwrap_or(ResolvedType::Unknown);
                if matches!(ty, ResolvedType::Unknown) {
                    self.errors.push(errors::const_none_type_inference(span));
                }
                ConstEvalResult {
                    ty,
                    kind: ConstKind::RustNative,
                    value: None,
                }
            }
        }
    }
}
