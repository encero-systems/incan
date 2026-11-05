//! Result combinator callbacks, open `Result` and `Option` constructor sides, and result constructors.

use super::*;

impl TypeChecker {
    /// Return whether `method` names an RFC 070 `Result[T, E]` combinator.
    pub(super) fn result_combinator_name(method: &str) -> bool {
        matches!(
            result_methods::from_str(method),
            Some(
                result_methods::ResultMethodId::Map
                    | result_methods::ResultMethodId::MapErr
                    | result_methods::ResultMethodId::AndThen
                    | result_methods::ResultMethodId::OrElse
                    | result_methods::ResultMethodId::Inspect
                    | result_methods::ResultMethodId::InspectErr
            )
        )
    }

    /// Resolve a callable function or callable object to its parameter and return types.
    pub(super) fn callable_signature_for_value_type(
        &mut self,
        ty: &ResolvedType,
        span: Span,
    ) -> Option<(Vec<CallableParam>, ResolvedType)> {
        match ty {
            ResolvedType::Function(params, ret) => Some((params.clone(), ret.as_ref().clone())),
            ResolvedType::Generic(name, _) | ResolvedType::Named(name) => {
                let type_info = self.lookup_semantic_type_info(name)?;
                let methods = match type_info {
                    TypeInfo::Model(model) => &model.methods,
                    TypeInfo::Class(class) => &class.methods,
                    TypeInfo::Enum(en) => &en.methods,
                    TypeInfo::Newtype(newtype) => &newtype.methods,
                    _ => return None,
                };
                let Some(call) = methods.get("__call__") else {
                    self.errors
                        .push(errors::missing_method(&ty.to_string(), "__call__", span));
                    return None;
                };
                Some(self.method_types_substituting_call_site_self(call, ty))
            }
            ResolvedType::Unknown => Some((Vec::new(), ResolvedType::Unknown)),
            _ => {
                self.errors
                    .push(errors::missing_method(&ty.to_string(), "__call__", span));
                None
            }
        }
    }

    /// Validate the callback passed to one `Result[T, E]` combinator and return its output type.
    pub(super) fn validate_result_combinator_callback(
        &mut self,
        _method: &str,
        callback_ty: &ResolvedType,
        input_ty: &ResolvedType,
        expected_ret: Option<&ResolvedType>,
        span: Span,
    ) -> ResolvedType {
        let Some((params, ret)) = self.callable_signature_for_value_type(callback_ty, span) else {
            return ResolvedType::Unknown;
        };
        if params.len() != 1 {
            self.errors.push(errors::type_mismatch(
                "one-parameter callable",
                &format!("{}-parameter callable", params.len()),
                span,
            ));
            return ResolvedType::Unknown;
        }
        if let Some(param) = params.first()
            && !self.types_compatible(input_ty, &param.ty)
        {
            self.errors.push(errors::type_mismatch(
                &param.ty.to_string(),
                &input_ty.to_string(),
                span,
            ));
        }
        if let Some(expected) = expected_ret
            && !self.types_compatible(&ret, expected)
        {
            self.errors
                .push(errors::type_mismatch(&expected.to_string(), &ret.to_string(), span));
        }
        ret
    }

    /// Return the result type the RFC 070 combinator `method` of a `Result[ok, err]` gives its callback, as far as the
    /// receiver and the call's expected type `expected` fix it (#1561).
    ///
    /// `and_then`'s callback returns `Result[U, err]` and `or_else`'s `Result[ok, F]`, where `U` and `F` are the
    /// call's expected `Result[U, F]` when it has one and open otherwise, so an `Ok(...)` or `Err(...)` the callback
    /// returns has the side it does not spell. `map`'s and `map_err`'s callbacks return their own result, and an
    /// observer's result needs no context, so theirs is left open.
    pub(super) fn result_callback_expected_output(
        method: ResultMethodId,
        ok: &ResolvedType,
        err: &ResolvedType,
        expected: Option<&ResolvedType>,
    ) -> ResolvedType {
        let (expected_ok, expected_err) = match expected {
            Some(ResolvedType::Generic(name, args))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Result) && args.len() == 2 =>
            {
                (args[0].clone(), args[1].clone())
            }
            _ => (ResolvedType::Unknown, ResolvedType::Unknown),
        };
        let result = |ok, err| ResolvedType::Generic("Result".to_string(), vec![ok, err]);
        match method {
            ResultMethodId::AndThen => result(expected_ok, err.clone()),
            ResultMethodId::OrElse => result(ok.clone(), expected_err),
            ResultMethodId::Map
            | ResultMethodId::MapErr
            | ResultMethodId::Inspect
            | ResultMethodId::InspectErr
            | ResultMethodId::Unwrap
            | ResultMethodId::UnwrapOr => ResolvedType::Unknown,
        }
    }

    /// Propagate only a Result method's unchanged payloads into its receiver's native generic call.
    /// A transformed payload belongs to the callback and must never bind the receiver's original type parameter.
    pub(super) fn result_receiver_expected_type(method: &str, expected: &ResolvedType) -> Option<ResolvedType> {
        let method = result_methods::from_str(method)?;
        let result = |ok, err| ResolvedType::Generic("Result".to_string(), vec![ok, err]);
        if matches!(method, ResultMethodId::Unwrap | ResultMethodId::UnwrapOr) {
            return Some(result(expected.clone(), ResolvedType::Unknown));
        }
        let ResolvedType::Generic(name, args) = expected else {
            return None;
        };
        if collection_type_id(name) != Some(CollectionTypeId::Result) || args.len() != 2 {
            return None;
        }
        match method {
            ResultMethodId::Map | ResultMethodId::AndThen => Some(result(ResolvedType::Unknown, args[1].clone())),
            ResultMethodId::MapErr | ResultMethodId::OrElse => Some(result(args[0].clone(), ResolvedType::Unknown)),
            ResultMethodId::Inspect | ResultMethodId::InspectErr => Some(expected.clone()),
            ResultMethodId::Unwrap | ResultMethodId::UnwrapOr => None,
        }
    }

    /// Give the side of an `and_then` or `or_else` call's result that its closure's `Ok(...)` or `Err(...)` leaves open
    /// the type that constructor is built with (#1561).
    ///
    /// The closure `(m) => Ok(1)` of `or_else` gives its result no error type, and one of `and_then` that returns only
    /// `Err(...)` no success type, unless the receiver or the call's expected `Result` type fixes that side. The
    /// constructor is then built with the side the enclosing function's `Result` return type has, or with `None` when
    /// that function returns no `Result`, as an `Ok(...)` or `Err(...)` bound to a local is (see
    /// [`Self::settle_open_constructor_side`]). The call's result says so, so a later use of that side is checked
    /// against the type it is built with. A callback that is not a closure literal returning such a constructor is left
    /// as it is.
    pub(super) fn settle_open_callback_result_side(
        &self,
        method: ResultMethodId,
        args: &[CallArg],
        result: ResolvedType,
    ) -> ResolvedType {
        let (open_side, unspelled_by) = match method {
            ResultMethodId::AndThen => (0, ConstructorId::Err),
            ResultMethodId::OrElse => (1, ConstructorId::Ok),
            _ => return result,
        };
        let Some(CallArg::Positional(callback)) = args.first() else {
            return result;
        };
        let Expr::Closure(_, body) = &callback.node else {
            return result;
        };
        if self.returned_result_constructor(body) != Some(unspelled_by) {
            return result;
        }
        self.settle_result_side(result, open_side)
    }

    /// Give the side an `Ok(...)` or `Err(...)` bound to a new local leaves open the type that constructor is built
    /// with (#1561).
    ///
    /// `x = Ok(1)` gives `x` no error type unless the enclosing function's `Result` return type gives the constructor
    /// one; the constructor is then built with `None` there, so the binding's type says so and a later use of that side
    /// is checked against `None`. The same holds for such a constructor inside a list, set, dict or tuple literal: a
    /// side no member of the literal fixes is built with that type for every member (`[Ok(1)]` is a
    /// `List[Result[int, None]]`), and for one that is the value of a `match` arm or of a `loop:` expression's `break`:
    /// the arms and `break` values have already unified, so only a side none of them fixes is settled. Any other value,
    /// and a side something fixed, is left as it is.
    pub(in crate::typechecker) fn settle_open_constructor_side(
        &self,
        value: &Spanned<Expr>,
        ty: ResolvedType,
    ) -> ResolvedType {
        match self.returned_result_constructor(value) {
            Some(ConstructorId::Ok) => return self.settle_result_side(ty, 1),
            Some(ConstructorId::Err) => return self.settle_result_side(ty, 0),
            _ => {}
        }
        match (&value.node, ty) {
            (Expr::List(entries), ResolvedType::Generic(name, args))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::List) && args.len() == 1 =>
            {
                let elements = entries.iter().filter_map(|entry| match entry {
                    ListEntry::Element(element) => Some(element),
                    ListEntry::Spread(_) => None,
                });
                let settled = args
                    .into_iter()
                    .map(|member_ty| self.settle_open_member_sides(member_ty, elements.clone()));
                ResolvedType::Generic(name, settled.collect())
            }
            (Expr::Set(elements), ResolvedType::Generic(name, args))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Set) && args.len() == 1 =>
            {
                let settled = args
                    .into_iter()
                    .map(|member_ty| self.settle_open_member_sides(member_ty, elements.iter()));
                ResolvedType::Generic(name, settled.collect())
            }
            (Expr::Dict(entries), ResolvedType::Generic(name, args))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Dict) && args.len() == 2 =>
            {
                let pairs = entries.iter().filter_map(|entry| match entry {
                    DictEntry::Pair(key, value) => Some((key, value)),
                    DictEntry::Spread(_) => None,
                });
                let settled = args.into_iter().enumerate().map(|(position, member_ty)| {
                    let members = pairs
                        .clone()
                        .map(move |(key, value)| if position == 0 { key } else { value });
                    self.settle_open_member_sides(member_ty, members)
                });
                ResolvedType::Generic(name, settled.collect())
            }
            (Expr::Tuple(elements), ResolvedType::Tuple(items)) if elements.len() == items.len() => {
                ResolvedType::Tuple(
                    elements
                        .iter()
                        .zip(items)
                        .map(|(element, item_ty)| self.settle_open_constructor_side(element, item_ty))
                        .collect(),
                )
            }
            (Expr::Paren(inner), ty) => self.settle_open_constructor_side(inner, ty),
            (Expr::Match(_, _) | Expr::Loop(_), ty) => branch_values(value)
                .into_iter()
                .fold(ty, |ty, branch| self.settle_open_constructor_side(branch, ty)),
            (_, ty) => ty,
        }
    }

    /// Settle the `Result` sides an `Ok(...)` or `Err(...)` leaves open where the value is used in place, as the
    /// iterable of a `for` statement or the subject of a `match`, and record the settled type as the value's own
    /// (#1561).
    ///
    /// Nothing binds such a value, so no annotation spells the side the constructor leaves open: it is settled as for a
    /// constructor bound to a local (see [`Self::settle_open_constructor_side`]), so `Err(m) => println(m)` over
    /// `match Ok(1):` in a function returning no `Result` is checked against `None`. Lowering spells the recorded type
    /// on the constructor.
    pub(in crate::typechecker) fn settle_open_constructor_sides_in_place(
        &mut self,
        value: &Spanned<Expr>,
        ty: ResolvedType,
    ) -> ResolvedType {
        let settled = self.settle_open_constructor_side(value, ty.clone());
        if settled != ty {
            self.record_expr_type(value.span, settled.clone());
            self.record_settled_branch_values(value, &settled);
        }
        settled
    }

    /// Settle the open `Result` sides of the member type `member_ty` that one collection literal's `members` share,
    /// each member in turn (see [`Self::settle_open_constructor_side`]).
    pub(super) fn settle_open_member_sides<'a>(
        &self,
        member_ty: ResolvedType,
        members: impl Iterator<Item = &'a Spanned<Expr>>,
    ) -> ResolvedType {
        members.fold(member_ty, |member_ty, member| {
            self.settle_open_constructor_side(member, member_ty)
        })
    }

    /// Give the side of a closure literal's `Result` return type that the `Ok(...)` or `Err(...)` its body returns
    /// leaves open, and that its expected return type does not fix, the type that constructor is built with (#1561).
    ///
    /// `f = () => Ok(1)` gives `f` a result with no error type. As for an `Ok(...)` bound to a local (see
    /// [`Self::settle_open_constructor_side`]), that side is the enclosing function's `Result` side, or `None` when
    /// that function returns no `Result`, so a call of `f` has a complete `Result` type. A side the expected return
    /// type fixes, such as the error side of an `and_then` callback's `Ok(...)`, which is the receiver's, stays
    /// open for the call to fix; an unknown or still-to-be-inferred expected side fixes nothing.
    pub(in crate::typechecker) fn settle_open_closure_result_side(
        &self,
        body: &Spanned<Expr>,
        return_ty: ResolvedType,
        expected_ret: Option<&ResolvedType>,
    ) -> ResolvedType {
        let open_side = match self.returned_result_constructor(body) {
            Some(ConstructorId::Ok) => 1,
            Some(ConstructorId::Err) => 0,
            _ => return self.settle_open_constructor_side(body, return_ty),
        };
        if let Some(ResolvedType::Generic(name, sides)) = expected_ret
            && collection_type_id(name.as_str()) == Some(CollectionTypeId::Result)
            && sides.len() == 2
            && !matches!(
                sides[open_side],
                ResolvedType::Unknown | ResolvedType::TypeVar(_) | ResolvedType::CallSiteInfer
            )
        {
            return return_ty;
        }
        self.settle_result_side(return_ty, open_side)
    }

    /// Fill side `side` (0 for success, 1 for error) of the `Result` type `ty` when it is open, with the type an open
    /// constructor side is built with: that side of the enclosing function's `Result` return type, or `None` when the
    /// function returns no `Result`.
    pub(super) fn settle_result_side(&self, ty: ResolvedType, side: usize) -> ResolvedType {
        match ty {
            ResolvedType::Generic(name, mut sides)
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Result)
                    && sides.len() == 2
                    && matches!(sides[side], ResolvedType::Unknown) =>
            {
                sides[side] = self.open_result_side_type(side);
                ResolvedType::Generic(name, sides)
            }
            other => other,
        }
    }

    /// The type an open side `side` (0 for success, 1 for error) of an `Ok(...)` or `Err(...)` is built with when
    /// nothing fixes it: that side of the enclosing function's `Result` return type, or `None` when the function
    /// returns no `Result` (#1561).
    pub(in crate::typechecker) fn open_result_side_type(&self, side: usize) -> ResolvedType {
        match self.symbols.enclosing_declared_return_type() {
            Some(ResolvedType::Generic(returned, returned_sides))
                if collection_type_id(returned.as_str()) == Some(CollectionTypeId::Result)
                    && returned_sides.len() == 2 =>
            {
                returned_sides[side].clone()
            }
            _ => ResolvedType::Unit,
        }
    }

    /// Return the `Ok` or `Err` constructor `expr` produces: a call of it, in parentheses, or as the value of an `if`
    /// with an `else` (the first branch that produces one), as lowering finds the constructor a closure returns.
    pub(in crate::typechecker) fn returned_result_constructor(&self, expr: &Spanned<Expr>) -> Option<ConstructorId> {
        match &expr.node {
            Expr::Paren(inner) => self.returned_result_constructor(inner),
            Expr::Call(callee, type_args, call_args) if type_args.is_empty() && call_args.len() == 1 => {
                let Expr::Ident(name) = &callee.node else {
                    return None;
                };
                self.result_constructor_named(name)
            }
            Expr::Constructor(name, call_args) if call_args.len() == 1 => self.result_constructor_named(name),
            Expr::If(if_expr) => {
                let else_body = if_expr.else_body.as_ref()?;
                let branch_value = |body: &[Spanned<Statement>]| match body.last().map(|stmt| &stmt.node) {
                    Some(Statement::Expr(value)) => self.returned_result_constructor(value),
                    _ => None,
                };
                branch_value(&if_expr.then_body).or_else(|| branch_value(else_body))
            }
            _ => None,
        }
    }

    /// The `Ok` or `Err` constructor `name` spells, unless a binding of the program shadows it.
    pub(super) fn result_constructor_named(&self, name: &str) -> Option<ConstructorId> {
        match constructors::from_str(name)? {
            constructor @ (ConstructorId::Ok | ConstructorId::Err) if !self.has_non_builtin_call_root_binding(name) => {
                Some(constructor)
            }
            _ => None,
        }
    }

    /// Whether the default of type `default_ty` passed to an `Option` or `Result` `unwrap_or` can be the payload of
    /// type `payload_ty` the call returns in its place (#1561).
    ///
    /// A type parameter of the enclosing generic declaration is one fixed type inside that declaration, whatever a
    /// caller later picks for it, so only a value of that same parameter stands in for it: `r.unwrap_or(0)` on a
    /// `Result[T, int]` is refused, where ordinary compatibility accepts any type against a type parameter. The rule
    /// holds at every depth of the payload that has the default's shape (`List[T]` takes a `List[T]` or `[]`, not a
    /// `List[int]`), and an unknown default is left to the error that made it unknown. Everything else, a payload that
    /// mentions no such parameter included, is checked by ordinary compatibility.
    pub(super) fn unwrap_or_default_fits_payload(&self, default_ty: &ResolvedType, payload_ty: &ResolvedType) -> bool {
        if matches!(default_ty, ResolvedType::Unknown | ResolvedType::Never)
            || !self.mentions_active_type_param(payload_ty)
        {
            return self.types_compatible(default_ty, payload_ty);
        }
        if let Some(param) = self.active_type_param_name(payload_ty) {
            return self.active_type_param_name(default_ty) == Some(param);
        }
        match (default_ty, payload_ty) {
            (ResolvedType::Generic(default_name, default_args), ResolvedType::Generic(payload_name, payload_args))
                if default_name == payload_name && default_args.len() == payload_args.len() =>
            {
                default_args
                    .iter()
                    .zip(payload_args)
                    .all(|(default_arg, payload_arg)| self.unwrap_or_default_fits_payload(default_arg, payload_arg))
            }
            (ResolvedType::Tuple(default_items), ResolvedType::Tuple(payload_items))
                if default_items.len() == payload_items.len() =>
            {
                default_items
                    .iter()
                    .zip(payload_items)
                    .all(|(default_item, payload_item)| self.unwrap_or_default_fits_payload(default_item, payload_item))
            }
            _ => self.types_compatible(default_ty, payload_ty),
        }
    }

    /// Whether `ty` names a type parameter of an enclosing generic declaration anywhere in it.
    pub(super) fn mentions_active_type_param(&self, ty: &ResolvedType) -> bool {
        if self.active_type_param_name(ty).is_some() {
            return true;
        }
        match ty {
            ResolvedType::Generic(_, args) | ResolvedType::Tuple(args) => {
                args.iter().any(|arg| self.mentions_active_type_param(arg))
            }
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => self.mentions_active_type_param(inner),
            _ => false,
        }
    }

    /// Typecheck one RFC 070 `Result[T, E]` combinator method call.
    pub(super) fn check_result_combinator_method(
        &mut self,
        ok_ty: ResolvedType,
        err_ty: ResolvedType,
        method: &str,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        span: Span,
    ) -> ResolvedType {
        if args.len() != 1 {
            self.errors.push(errors::type_mismatch(
                "one callable argument",
                &format!("{} argument(s)", args.len()),
                span,
            ));
            return ResolvedType::Unknown;
        }
        let Some(callback_ty) = arg_types.first() else {
            return ResolvedType::Unknown;
        };
        let Some(method_id) = result_methods::from_str(method) else {
            return ResolvedType::Unknown;
        };
        match method_id {
            result_methods::ResultMethodId::Map => {
                let ret = self.validate_result_combinator_callback(method, callback_ty, &ok_ty, None, span);
                ResolvedType::Generic("Result".to_string(), vec![ret, err_ty])
            }
            result_methods::ResultMethodId::MapErr => {
                let ret = self.validate_result_combinator_callback(method, callback_ty, &err_ty, None, span);
                ResolvedType::Generic("Result".to_string(), vec![ok_ty, ret])
            }
            result_methods::ResultMethodId::AndThen => {
                let expected = ResolvedType::Generic("Result".to_string(), vec![ResolvedType::Unknown, err_ty.clone()]);
                let ret = self.validate_result_combinator_callback(method, callback_ty, &ok_ty, Some(&expected), span);
                let result = match ret {
                    ResolvedType::Generic(name, sides)
                        if collection_type_id(name.as_str()) == Some(CollectionTypeId::Result) && sides.len() == 2 =>
                    {
                        ResolvedType::Generic(name, sides)
                    }
                    _ => ResolvedType::Generic("Result".to_string(), vec![ResolvedType::Unknown, err_ty]),
                };
                self.settle_open_callback_result_side(method_id, args, result)
            }
            result_methods::ResultMethodId::OrElse => {
                let expected = ResolvedType::Generic("Result".to_string(), vec![ok_ty.clone(), ResolvedType::Unknown]);
                let ret = self.validate_result_combinator_callback(method, callback_ty, &err_ty, Some(&expected), span);
                let result = match ret {
                    ResolvedType::Generic(name, sides)
                        if collection_type_id(name.as_str()) == Some(CollectionTypeId::Result) && sides.len() == 2 =>
                    {
                        ResolvedType::Generic(name, sides)
                    }
                    _ => ResolvedType::Generic("Result".to_string(), vec![ok_ty, ResolvedType::Unknown]),
                };
                self.settle_open_callback_result_side(method_id, args, result)
            }
            result_methods::ResultMethodId::Inspect => {
                self.validate_result_combinator_callback(method, callback_ty, &ok_ty, Some(&ResolvedType::Unit), span);
                ResolvedType::Generic("Result".to_string(), vec![ok_ty, err_ty])
            }
            result_methods::ResultMethodId::InspectErr => {
                self.validate_result_combinator_callback(method, callback_ty, &err_ty, Some(&ResolvedType::Unit), span);
                ResolvedType::Generic("Result".to_string(), vec![ok_ty, err_ty])
            }
            result_methods::ResultMethodId::Unwrap | result_methods::ResultMethodId::UnwrapOr => ResolvedType::Unknown,
        }
    }
}
