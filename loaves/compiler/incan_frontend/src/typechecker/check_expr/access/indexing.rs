//! Tuple, type, JSON and collection indexing and slicing.

use super::*;

impl TypeChecker {
    /// Return the constant a tuple index spells: an integer literal, a negated one, which counts from the end, or
    /// either in parentheses.
    ///
    /// Body IR shares this classification so checked negative and parenthesized tuple indices survive as constants,
    /// without folding ordinary arithmetic or collection indices at the replacement boundary.
    pub(crate) fn constant_tuple_index(index: &Spanned<Expr>) -> Option<i64> {
        match &index.node {
            Expr::Literal(Literal::Int(literal)) => Some(literal.value),
            Expr::Unary(UnaryOp::Neg, operand) => match &operand.node {
                Expr::Literal(Literal::Int(literal)) => literal.value.checked_neg(),
                _ => None,
            },
            Expr::Paren(inner) => Self::constant_tuple_index(inner),
            _ => None,
        }
    }

    /// Normalize a tuple index (supports negative indices) and emit bounds errors.
    pub(super) fn resolve_tuple_index(&mut self, raw_idx: i64, len: usize, span: Span) -> Option<usize> {
        let len_i = len as i64;
        let mut idx = raw_idx;
        if idx < 0 {
            idx += len_i;
        }
        if idx < 0 || idx >= len_i {
            self.errors.push(errors::tuple_index_out_of_bounds(raw_idx, len, span));
            return None;
        }
        Some(idx as usize)
    }

    /// Recognize `TypeName[T]` receiver expressions used for type-owned calls.
    ///
    /// The subscript supplies type arguments; it does not change which type owns the call. Resolving to `Unknown`
    /// here left `FactoryBox[int].make(...)` with no receiver type, so method resolution never matched a declaration
    /// and recorded no identity for the call. Lowering then had nothing to project with and emitted the source
    /// spelling, while the declaration was emitted under its projection. Keeping the nominal type gives the
    /// subscripted spelling the same receiver the bare `FactoryBox.make(...)` form already has.
    pub(super) fn resolve_type_index_expression(
        &self,
        base_ty: &ResolvedType,
        base: &Spanned<Expr>,
        index: &Spanned<Expr>,
    ) -> Option<ResolvedType> {
        let ResolvedType::Named(name) = base_ty else {
            return None;
        };
        if !matches!(self.type_info.ident_kind(base.span), Some(IdentKind::TypeName)) {
            return None;
        }
        // `Deque[str]` applies a type argument; dropping it here left the receiver generic over nothing, so a
        // classmethod returning `Self` produced `Deque[Unknown]` and every later substitution carried the
        // unknown forward. See #1494, where it surfaced as a string literal reaching a `String` parameter
        // unconverted -- the literal was correct for the element type it had been given.
        match self.type_arguments_from_index_expression(index) {
            Some(args) if !args.is_empty() => Some(ResolvedType::Generic(name.clone(), args)),
            _ => Some(base_ty.clone()),
        }
    }

    /// Read one type application's arguments out of the expression the parser produced for its brackets.
    ///
    /// A type application and a subscript are the same syntax, so the arguments arrive as an expression rather
    /// than as types: `Deque[str]` is an index whose index is an identifier, and `Dict[str, int]` one whose index
    /// is a tuple. Only identifiers and nested applications are read; anything else is a genuine subscript and
    /// returns `None` so the caller keeps its existing behavior rather than inventing a type from a value.
    pub(super) fn type_arguments_from_index_expression(&self, index: &Spanned<Expr>) -> Option<Vec<ResolvedType>> {
        let elements: Vec<&Spanned<Expr>> = match &index.node {
            Expr::Tuple(items) => items.iter().collect(),
            _ => vec![index],
        };
        let mut args = Vec::new();
        for element in elements {
            args.push(self.type_argument_from_expression(element)?);
        }
        Some(args)
    }

    /// Resolve one type argument spelled as an expression, including a nested application such as `list[str]`.
    pub(super) fn type_argument_from_expression(&self, expr: &Spanned<Expr>) -> Option<ResolvedType> {
        match &expr.node {
            Expr::Ident(name) => Some(resolve_type(&Type::Simple(name.clone()), &self.symbols)),
            Expr::Paren(inner) => self.type_argument_from_expression(inner),
            Expr::Index(base, index) => {
                let Expr::Ident(name) = &base.node else {
                    return None;
                };
                let args = self.type_arguments_from_index_expression(index)?;
                Some(ResolvedType::Generic(name.clone(), args))
            }
            _ => None,
        }
    }

    /// Return whether `ty` is the compiler-owned `std.json.JsonValue` wrapper over the raw runtime carrier.
    pub(super) fn is_std_json_value_type(&self, ty: &ResolvedType) -> bool {
        let type_name = match ty {
            ResolvedType::Named(name) | ResolvedType::Generic(name, _) => name,
            _ => return false,
        };
        if !stdlib::is_json_value_type_name(type_name) {
            return false;
        }
        matches!(
            self.lookup_semantic_type_info(type_name),
            Some(TypeInfo::Newtype(info))
                if matches!(
                    &info.underlying,
                    ResolvedType::RustPath(path) if path == stdlib::JSON_VALUE_RUST_PATH
                ) || matches!(&info.underlying, ResolvedType::Named(name) if name == "RustJsonValue")
        )
    }

    /// Return whether an index type can select one of `JsonValue`'s source-authored `__getitem__` overloads.
    pub(super) fn is_std_json_value_index_type(index_ty: &ResolvedType) -> bool {
        matches!(index_ty, ResolvedType::Int | ResolvedType::Str | ResolvedType::Unknown) || is_frozen_str(index_ty)
    }

    /// Type-check an indexing expression (`base[index]`) and retain its exact element type. Explicit references
    /// retain the collection's element type, and owned and frozen byte sequences expose `u8` elements matching their
    /// checked byte storage; source indices remain signed. Mutability and ownership remain governed by the original
    /// indexed place.
    pub(in crate::typechecker::check_expr) fn check_index(
        &mut self,
        base: &Spanned<Expr>,
        index: &Spanned<Expr>,
        span: Span,
    ) -> ResolvedType {
        let mut base_ty = self.check_type_receiver_expr(base);
        if let Some(ty) = self.resolve_type_index_expression(&base_ty, base, index) {
            return ty;
        }
        while let ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) = base_ty {
            base_ty = *inner;
        }
        let index_ty = self.check_expr(index);
        // A native fixed-size array reads its checked element type at any integer index.
        if let ResolvedType::RustPath(path) = &base_ty
            && let Some((element, _length)) = self.rust_array_parts(path)
        {
            if !is_intlike_for_index(&index_ty) {
                self.errors
                    .push(errors::index_type_mismatch("int", &index_ty.to_string(), index.span));
            }
            return element;
        }
        if self.is_std_json_value_type(&base_ty) && !Self::is_std_json_value_index_type(&index_ty) {
            self.errors.push(errors::json_value_index_type_mismatch(
                &index_ty.to_string(),
                index.span,
            ));
            return option_ty(ResolvedType::Named(stdlib::JSON_VALUE_TYPE_NAME.to_string()));
        }

        match base_ty {
            ResolvedType::Generic(name, args) => match collection_type_id(name.as_str()) {
                Some(CollectionTypeId::List) if !args.is_empty() => {
                    if !is_intlike_for_index(&index_ty) {
                        self.errors
                            .push(errors::index_type_mismatch("int", &index_ty.to_string(), index.span));
                    }
                    args[0].clone()
                }
                Some(CollectionTypeId::Dict) if args.len() >= 2 => {
                    let key_ty = &args[0];
                    if self.types_compatible(&index_ty, key_ty) {
                        // A numeric key of a narrower type is widened to the key type where it is lowered (RFC 009).
                        self.record_value_destination_if_compatible(index.span, &index_ty, key_ty);
                    } else {
                        self.errors.push(errors::index_type_mismatch(
                            &key_ty.to_string(),
                            &index_ty.to_string(),
                            index.span,
                        ));
                    }
                    args[1].clone()
                }
                Some(CollectionTypeId::Tuple) => self.check_tuple_element_index(&args, index, span),
                _ => {
                    let receiver_ty = ResolvedType::Generic(name, args);
                    if let Some(ret) = self.resolve_index_dunder(&receiver_ty, index, &index_ty, span) {
                        ret
                    } else {
                        self.errors
                            .push(errors::missing_method(&receiver_ty.to_string(), "__getitem__", span));
                        ResolvedType::Unknown
                    }
                }
            },
            ty if matches!(ty, ResolvedType::Str | ResolvedType::Bytes)
                || is_frozen_str(&ty)
                || is_frozen_bytes(&ty) =>
            {
                if !is_intlike_for_index(&index_ty) {
                    self.errors
                        .push(errors::index_type_mismatch("int", &index_ty.to_string(), index.span));
                }
                if matches!(ty, ResolvedType::Bytes) || is_frozen_bytes(&ty) {
                    ResolvedType::Numeric(numerics::NumericTypeId::U8)
                } else {
                    ResolvedType::Str
                }
            }
            // A `const` `FrozenList[T]` is read like a list: `items[i]` is the element at an `int` index, counted from
            // the end when negative, or an `IndexError` at run time (#1757).
            ResolvedType::FrozenList(elem_ty) => {
                if !is_intlike_for_index(&index_ty) {
                    self.errors
                        .push(errors::index_type_mismatch("int", &index_ty.to_string(), index.span));
                }
                *elem_ty
            }
            // A `const` `FrozenDict[K, V]` is read like a dict: `table[key]` is the value, or a `KeyError` at run
            // time. A text key (`str` or `FrozenStr`) accepts any text probe (#1757).
            ResolvedType::FrozenDict(key_ty, value_ty) => {
                let text_probe_for_text_key = is_str_like(&index_ty) && is_str_like(&key_ty);
                if !text_probe_for_text_key && self.types_compatible(&index_ty, &key_ty) {
                    self.record_value_destination_if_compatible(index.span, &index_ty, &key_ty);
                } else if !text_probe_for_text_key {
                    self.errors.push(errors::index_type_mismatch(
                        &key_ty.to_string(),
                        &index_ty.to_string(),
                        index.span,
                    ));
                }
                *value_ty
            }
            ResolvedType::Tuple(elements) => self.check_tuple_element_index(&elements, index, span),
            ty if self.is_user_operator_receiver(&ty) => {
                if let Some(ret) = self.resolve_index_dunder(&ty, index, &index_ty, span) {
                    ret
                } else {
                    self.errors
                        .push(errors::missing_method(&ty.to_string(), "__getitem__", span));
                    ResolvedType::Unknown
                }
            }
            _ => ResolvedType::Unknown,
        }
    }

    /// Require a constant integer tuple index and bounds-check it before selecting the element type.
    ///
    /// Generic tuple syntax and resolved tuple carriers share this diagnostic and indexing contract.
    pub(super) fn check_tuple_element_index(
        &mut self,
        elements: &[ResolvedType],
        index: &Spanned<Expr>,
        span: Span,
    ) -> ResolvedType {
        let Some(raw) = Self::constant_tuple_index(index) else {
            self.errors.push(errors::tuple_index_requires_int_literal(index.span));
            return ResolvedType::Unknown;
        };
        if let Some(index) = self.resolve_tuple_index(raw, elements.len(), span) {
            return elements.get(index).cloned().unwrap_or(ResolvedType::Unknown);
        }
        ResolvedType::Unknown
    }

    /// Type-check a slicing expression (`base[start:end:step]`) and return the sliced type. Text and byte slices
    /// produce owned values, including when the receiver is frozen. Native byte views use their inspected Deref
    /// targets and retain an ordinary borrowed slice call; every bound is checked exactly once.
    pub(in crate::typechecker::check_expr) fn check_slice(
        &mut self,
        base: &Spanned<Expr>,
        slice: &SliceExpr,
        span: Span,
    ) -> ResolvedType {
        let base_ty = self.check_expr(base);

        let start_ty = slice.start.as_ref().map(|s| self.check_expr(s));
        let end_ty = slice.end.as_ref().map(|e| self.check_expr(e));
        let step_ty = slice.step.as_ref().map(|st| self.check_expr(st));

        // Helper: validate that an already-computed type is int-like (or Unknown during inference).
        let check_intlike_ty = |ty: &ResolvedType, span: Span, errors: &mut Vec<_>| {
            if !is_intlike_for_index(ty) {
                errors.push(errors::index_type_mismatch("int", &ty.to_string(), span));
            }
        };
        // Helper: if a slice component exists, validate its already-computed type using the component span.
        let check_component = |ty_opt: Option<&ResolvedType>, expr_opt: Option<&Spanned<Expr>>, errors: &mut Vec<_>| {
            if let (Some(ty), Some(expr)) = (ty_opt, expr_opt) {
                check_intlike_ty(ty, expr.span, errors);
            }
        };

        if self.retain_rust_byte_slice(base, &base_ty, span).is_some() {
            check_component(start_ty.as_ref(), slice.start.as_deref(), &mut self.errors);
            check_component(end_ty.as_ref(), slice.end.as_deref(), &mut self.errors);
            check_component(step_ty.as_ref(), slice.step.as_deref(), &mut self.errors);
            return ResolvedType::Bytes;
        }

        match base_ty {
            ty if self.is_user_operator_receiver(&ty) => {
                let errors_before = self.errors.len();
                check_component(start_ty.as_ref(), slice.start.as_deref(), &mut self.errors);
                check_component(end_ty.as_ref(), slice.end.as_deref(), &mut self.errors);
                check_component(step_ty.as_ref(), slice.step.as_deref(), &mut self.errors);
                if self.errors.len() > errors_before {
                    return ResolvedType::Unknown;
                }
                self.resolve_slice_dunder(&ty, slice, span).unwrap_or_else(|| {
                    self.errors
                        .push(errors::missing_method(&ty.to_string(), SLICE_HOOK, span));
                    ResolvedType::Unknown
                })
            }
            ResolvedType::Generic(name, args) => match collection_type_id(name.as_str()) {
                Some(CollectionTypeId::List) => {
                    // Validate slice bounds/step for lists as well (indices must be int-like).
                    check_component(start_ty.as_ref(), slice.start.as_deref(), &mut self.errors);
                    check_component(end_ty.as_ref(), slice.end.as_deref(), &mut self.errors);
                    check_component(step_ty.as_ref(), slice.step.as_deref(), &mut self.errors);
                    ResolvedType::Generic(collection_name(CollectionTypeId::List).to_string(), args)
                }
                _ => ResolvedType::Unknown,
            },
            ty if matches!(ty, ResolvedType::Str | ResolvedType::Bytes)
                || is_frozen_str(&ty)
                || is_frozen_bytes(&ty) =>
            {
                // Reuse the already checked components so their diagnostics retain their original spans.
                check_component(start_ty.as_ref(), slice.start.as_deref(), &mut self.errors);
                check_component(end_ty.as_ref(), slice.end.as_deref(), &mut self.errors);
                check_component(step_ty.as_ref(), slice.step.as_deref(), &mut self.errors);
                if matches!(ty, ResolvedType::Bytes) || is_frozen_bytes(&ty) {
                    ResolvedType::Bytes
                } else {
                    ResolvedType::Str
                }
            }
            _ => ResolvedType::Unknown,
        }
    }

    /// Resolve `base[start:end:step]` on a user type through its `__getslice__` hook (`Sliceable[T]`), recording the
    /// operator call for lowering.
    ///
    /// The hook takes each part as an `Option[int]`: `Some` of the written part, `None` for an omitted one. The parts
    /// are checked once as `int` by the caller; the hook is resolved against placeholder `Option[int]` arguments at
    /// a zero-width span after the slice, so no fact of a written part is checked or recorded twice. Returns `None`
    /// when the type defines no `__getslice__`.
    pub(super) fn resolve_slice_dunder(
        &mut self,
        base_ty: &ResolvedType,
        slice: &SliceExpr,
        span: Span,
    ) -> Option<ResolvedType> {
        // Two generated slices at one source range differ only in their expansion, so the key keeps it.
        let placeholder_span = Span::new(span.end, span.end).with_expansion(span.expansion);
        let placeholder = |present: bool| {
            let value = if present {
                Expr::Call(
                    Box::new(Spanned::new(
                        Expr::Ident(constructors::as_str(ConstructorId::Some).to_string()),
                        placeholder_span,
                    )),
                    Vec::new(),
                    vec![CallArg::Positional(Spanned::new(
                        Expr::Literal(Literal::Int(IntLiteral::synthetic(0))),
                        placeholder_span,
                    ))],
                )
            } else {
                Expr::Literal(Literal::None)
            };
            CallArg::Positional(Spanned::new(value, placeholder_span))
        };
        let args = [slice.start.is_some(), slice.end.is_some(), slice.step.is_some()]
            .into_iter()
            .map(placeholder)
            .collect::<Vec<_>>();
        let arg_types = vec![option_ty(ResolvedType::Int); args.len()];
        let ret = self.resolve_operator_dunder(base_ty, SLICE_HOOK, &args, &arg_types, span, None)?;
        self.type_info
            .record_resolved_operator_call(span, SLICE_HOOK, ResolvedOperatorKind::Slice);
        Some(ret)
    }
}
