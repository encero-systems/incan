//! Iterator protocol names, iterator callbacks, and iterator adapter and terminal methods.

use super::*;

impl TypeChecker {
    /// Return the canonical stdlib iterator trait name from the shared language registry.
    pub(super) fn iterator_protocol_name() -> &'static str {
        core_traits::as_str(TraitId::Iterator)
    }

    /// Return the canonical RFC 088 iterable protocol trait spelling.
    pub(super) fn iterable_protocol_name() -> &'static str {
        core_traits::as_str(TraitId::Iterable)
    }

    /// Construct the protocol-facing `Iterator[T]` type used by RFC 088 adapter method typing.
    pub(super) fn iterator_protocol_ty(elem: ResolvedType) -> ResolvedType {
        ResolvedType::Generic(Self::iterator_protocol_name().to_string(), vec![elem])
    }

    /// Return whether `name` is the canonical RFC 088 iterable protocol trait spelling.
    pub(super) fn is_iterable_protocol_name(name: &str) -> bool {
        core_traits::from_qualified_str(name) == Some(TraitId::Iterable)
    }

    /// Return whether `name` is the canonical RFC 088 iterator protocol trait spelling.
    pub(super) fn is_iterator_protocol_name(name: &str) -> bool {
        core_traits::from_qualified_str(name) == Some(TraitId::Iterator)
    }

    /// Return the element type for values that can participate in the RFC 088 iterator protocol surface.
    ///
    /// This intentionally recognizes both explicit trait-typed values (`Iterator[T]` / `Iterable[T]`) and builtin
    /// collection values that have an obvious frontend iterator element type. It is a typechecker-only surface helper;
    /// lowering and emission use the same protocol shape to route known iterator methods through dedicated backend
    /// handling. A `Generator[T]` satisfies `Iterator[T]` (RFC 006, RFC 088), so it is an iterator value here.
    pub(in crate::typechecker) fn iterable_protocol_element_type(&self, ty: &ResolvedType) -> Option<ResolvedType> {
        match ty {
            ResolvedType::Generic(name, args)
                if (Self::is_iterator_protocol_name(name) || Self::is_iterable_protocol_name(name))
                    && args.len() == 1 =>
            {
                args.first().cloned()
            }
            ResolvedType::Generic(name, args) if Self::is_generator_name(name) && args.len() == 1 => {
                args.first().cloned()
            }
            ResolvedType::Generic(name, args)
                if matches!(
                    collection_type_id(name.as_str()),
                    Some(
                        CollectionTypeId::List
                            | CollectionTypeId::Set
                            | CollectionTypeId::FrozenList
                            | CollectionTypeId::FrozenSet
                    )
                ) && args.len() == 1 =>
            {
                args.first().cloned()
            }
            ResolvedType::FrozenList(inner) | ResolvedType::FrozenSet(inner) => Some((**inner).clone()),
            _ => None,
        }
    }

    /// Return the element type for values that are already typed as `Iterator[T]`, a `Generator[T]` included.
    pub(super) fn iterator_protocol_element_type(&self, ty: &ResolvedType) -> Option<ResolvedType> {
        match ty {
            ResolvedType::Generic(name, args)
                if (Self::is_iterator_protocol_name(name) || Self::is_generator_name(name)) && args.len() == 1 =>
            {
                args.first().cloned()
            }
            _ => None,
        }
    }

    /// Return whether `name` is the RFC 006 `Generator` type.
    pub(super) fn is_generator_name(name: &str) -> bool {
        collection_type_id(name) == Some(CollectionTypeId::Generator)
    }

    /// Validate fixed-arity RFC 088 method calls and report the same arity diagnostic style as other builtin calls.
    pub(super) fn validate_iterator_method_arity(
        &mut self,
        method: &str,
        expected: usize,
        found: usize,
        span: Span,
    ) -> bool {
        if found == expected {
            return true;
        }
        self.errors.push(errors::builtin_arity(
            &format!("{}.{method}", Self::iterator_protocol_name()),
            expected,
            found,
            span,
        ));
        false
    }

    /// Return the declared type whose method body is being checked, with its own type parameters, or `None` outside a
    /// model, class, enum or newtype method (a trait default among them).
    pub(super) fn current_method_owner_type(&self) -> Option<ResolvedType> {
        let owner = self.current_method_owner.as_ref()?;
        let type_params = match self.lookup_type_info(owner)? {
            TypeInfo::Model(model) => model.type_params.clone(),
            TypeInfo::Class(class) => class.type_params.clone(),
            TypeInfo::Newtype(newtype) => newtype.type_params.clone(),
            TypeInfo::Enum(enum_info) => enum_info.type_params.clone(),
            TypeInfo::Builtin | TypeInfo::TypeAlias => return None,
        };
        Some(if type_params.is_empty() {
            ResolvedType::Named(owner.clone())
        } else {
            ResolvedType::Generic(
                owner.clone(),
                type_params.into_iter().map(ResolvedType::TypeVar).collect(),
            )
        })
    }

    /// Return the function type an RFC 088 adapter or terminal method of an iterator gives the callback it takes at
    /// argument `index`, so a closure written there takes the iterator's element, and a fold's accumulator, as its
    /// parameter types.
    ///
    /// `accumulator` is the type of a fold's first argument. The callback of `map` and `flat_map` leaves its result
    /// to the closure's body. `None` when the receiver is not an iterator with a known element type or the method
    /// takes no callback at `index`.
    pub(super) fn iterator_callback_expectation(
        &self,
        base_ty: &ResolvedType,
        method: &str,
        index: usize,
        accumulator: Option<&ResolvedType>,
    ) -> Option<ResolvedType> {
        use iterator_methods::IteratorMethodId as M;
        let method_id = iterator_methods::from_str(method)?;
        let element = self
            .iterator_protocol_element_type(base_ty)
            .filter(|element| !matches!(element, ResolvedType::Unknown))?;
        let (params, ret) = match (method_id, index) {
            (M::Map | M::FlatMap, 0) => (vec![element], ResolvedType::Unknown),
            (M::Filter | M::TakeWhile | M::SkipWhile | M::Any | M::All | M::Find, 0) => {
                (vec![element], ResolvedType::Bool)
            }
            (M::ForEach, 0) => (vec![element], ResolvedType::Unit),
            (M::Fold | M::Reduce, 1) => {
                let accumulator = accumulator
                    .filter(|accumulator| !matches!(accumulator, ResolvedType::Unknown))?
                    .clone();
                (vec![accumulator.clone(), element], accumulator)
            }
            _ => return None,
        };
        Some(Self::iterator_callback_ty(params, ret))
    }

    /// Describe the function-typed slot an RFC 088 method of an iterator gives its callback, when it refuses a
    /// closure that captures local values, or `None` when the receiver is not an iterator or the method takes no
    /// callback.
    ///
    /// Each callback is a parameter of a standard-library method, which the functions reference gives a named function
    /// or a closure that captures nothing (#1561): `map`, `filter`, `flat_map`, `take_while` and `skip_while` of an
    /// iterator, a generator's own `map` and `filter` (RFC 006) included, keep it in the lazy iterator they return, and
    /// a terminal takes it as its parameter.
    pub(super) fn iterator_callback_slot(&self, base_ty: &ResolvedType, method: &str) -> Option<String> {
        use iterator_methods::IteratorMethodId as M;
        self.iterator_protocol_element_type(base_ty)?;
        match iterator_methods::from_str(method)? {
            M::Map | M::Filter | M::FlatMap | M::TakeWhile | M::SkipWhile => {
                Some(format!("stored by the iterator adapter '{method}'"))
            }
            M::Any | M::All | M::Find | M::Fold | M::Reduce | M::ForEach => {
                Some(format!("passed to the standard-library method '{method}'"))
            }
            _ => None,
        }
    }

    /// Build a resolved callable type from parameter and return types for adapter diagnostics.
    pub(super) fn iterator_callback_ty(params: Vec<ResolvedType>, ret: ResolvedType) -> ResolvedType {
        ResolvedType::Function(
            params.into_iter().map(CallableParam::positional).collect(),
            Box::new(ret),
        )
    }

    /// Reject `.batch(size)` calls when a non-positive literal size is visible to the frontend.
    pub(super) fn validate_iterator_batch_size_literal(&mut self, args: &[CallArg], span: Span) {
        let Some(CallArg::Positional(expr)) = args.first() else {
            return;
        };
        let Expr::Literal(Literal::Int(value)) = &expr.node else {
            return;
        };
        if value.value > 0 {
            return;
        }
        self.errors.push(CompileError::type_error(
            "Iterator.batch() size must be greater than zero".to_string(),
            span,
        ));
    }

    /// Return whether `method` consumes the receiver under RFC 088 terminal semantics.
    pub(super) fn is_iterator_terminal_method(method: &str) -> bool {
        iterator_methods::from_str(method).is_some_and(iterator_methods::is_terminal)
    }

    /// Validate `.sum()` item types against the source-owned `Sum[T]` capability surface.
    pub(super) fn iterator_sum_output_type(&mut self, elem: &ResolvedType, span: Span) -> ResolvedType {
        if self
            .temporary_trait_capability_supports_type(incan_lang::lang::trait_capabilities::iterator_sum(), elem)
            .is_some_and(|supported| supported)
        {
            return elem.clone();
        }
        self.errors.push(CompileError::type_error(
            format!(
                "Iterator.sum() requires int, float, or a newtype over a summable type; found {}",
                elem
            ),
            span,
        ));
        ResolvedType::Unknown
    }

    /// Preserve the legacy arity/type decisions, then snapshot only direct-profile selections for Body IR.
    pub(super) fn checked_iterator_sum_output_type(
        &mut self,
        method: &str,
        args: &[CallArg],
        elem: &ResolvedType,
        span: Span,
    ) -> ResolvedType {
        let valid_arity = self.validate_iterator_method_arity(method, 0, args.len(), span);
        let result = self.iterator_sum_output_type(elem, span);
        if valid_arity && self.native_iterator_sum_item_profile(&result) && !matches!(result, ResolvedType::Unknown) {
            self.type_info.body_ir_iterator_sum_items.insert(span, result.clone());
        }
        result
    }

    /// Keep source trait adoptions and `sum` members outside automatic reconstruction until their dispatch is retained.
    pub(super) fn native_iterator_sum_item_profile(&self, ty: &ResolvedType) -> bool {
        match ty {
            ResolvedType::Named(name) => match self.lookup_semantic_type_info(name) {
                Some(TypeInfo::Newtype(info)) => {
                    info.traits.is_empty()
                        && !info.methods.contains_key("sum")
                        && !info.method_aliases.contains_key("sum")
                        && !info.method_rebindings.contains_key("sum")
                }
                _ => true,
            },
            _ => true,
        }
    }

    /// Track the narrow same-binding case after a terminal iterator method consumes a direct local binding.
    pub(super) fn mark_direct_iterator_binding_consumed(&mut self, base: &Spanned<Expr>, method: &str, span: Span) {
        if !Self::is_iterator_terminal_method(method) {
            return;
        }
        let Expr::Ident(name) = &base.node else {
            return;
        };
        self.consumed_iterator_bindings.insert(name.clone(), span);
    }

    /// Validate an adapter callback whose return type is fully specified by the method contract.
    pub(super) fn validate_iterator_callback_return(
        &mut self,
        method: &str,
        actual: &ResolvedType,
        params: Vec<ResolvedType>,
        ret: ResolvedType,
        span: Span,
    ) {
        if matches!(actual, ResolvedType::Unknown) {
            return;
        }
        let expected = Self::iterator_callback_ty(params, ret);
        if !self.types_compatible(actual, &expected) {
            self.errors
                .push(errors::type_mismatch(&expected.to_string(), &actual.to_string(), span));
        }
        if !matches!(actual, ResolvedType::Function(_, _)) {
            self.errors.push(errors::missing_method(
                &actual.to_string(),
                &format!("__call__ for {method} callback"),
                span,
            ));
        }
    }

    /// Validate a mapping-style callback and return its concrete output type when it is known.
    pub(super) fn iterator_mapping_callback_return_type(
        &mut self,
        actual: &ResolvedType,
        param_ty: ResolvedType,
        span: Span,
    ) -> ResolvedType {
        let ResolvedType::Function(params, ret) = actual else {
            if !matches!(actual, ResolvedType::Unknown) {
                let expected = Self::iterator_callback_ty(vec![param_ty], ResolvedType::Unknown);
                self.errors
                    .push(errors::type_mismatch(&expected.to_string(), &actual.to_string(), span));
            }
            return ResolvedType::Unknown;
        };
        if params.len() != 1 || !self.types_compatible(&params[0].ty, &param_ty) {
            let expected = Self::iterator_callback_ty(vec![param_ty], (**ret).clone());
            self.errors
                .push(errors::type_mismatch(&expected.to_string(), &actual.to_string(), span));
        }
        (**ret).clone()
    }

    /// Typecheck one RFC 088 iterator/iterable adapter or terminal method.
    ///
    /// The frontend treats these protocol methods as a typed surface even when the receiver is a builtin collection
    /// whose methods are not represented as ordinary user-declared methods. Backend lowering and emission classify the
    /// same method family as known iterator calls.
    pub(in crate::typechecker::check_expr) fn resolve_iterator_protocol_method_call(
        &mut self,
        base_ty: &ResolvedType,
        method: &str,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        span: Span,
    ) -> Option<ResolvedType> {
        let elem = self.iterable_protocol_element_type(base_ty)?;
        let iterator_elem = self
            .iterator_protocol_element_type(base_ty)
            .unwrap_or_else(|| elem.clone());
        let method_id = iterator_methods::from_str(method)?;
        // A generator's own RFC 006 methods (`map`, `filter`, `take`, `collect`) keep their generator-typed results;
        // the rest of the RFC 088 surface reaches it as an `Iterator[T]`.
        if matches!(base_ty, ResolvedType::Generic(name, _) if Self::is_generator_name(name))
            && matches!(
                method_id,
                IteratorMethodId::Map | IteratorMethodId::Filter | IteratorMethodId::Take | IteratorMethodId::Collect
            )
        {
            return None;
        }
        // A list, set, frozen collection or `Iterable[T]` provides `iter()`; the adapters and terminals are methods of
        // the `Iterator[T]` it returns (RFC 088), so `items.map(f)` is refused where `items.iter().map(f)` is meant.
        if !matches!(method_id, IteratorMethodId::Iter) && self.iterator_protocol_element_type(base_ty).is_none() {
            self.errors.push(errors::iterator_method_on_an_iterable(
                &base_ty.to_string(),
                method,
                span,
            ));
            return Some(ResolvedType::Unknown);
        }

        if iterator_methods::is_terminal(method_id) {
            return self.check_iterator_terminal_method(method_id, method, args, arg_types, iterator_elem, span);
        }
        self.check_iterator_adapter_method(method_id, method, args, arg_types, elem, iterator_elem, span)
    }

    /// Check an adapter selected by the iterator registry, preserving iterable and iterator element distinctions.
    ///
    /// `iter()` uses the iterable's element; subsequent adapters use the iterator protocol's element. Terminal IDs
    /// decline here because the registry routes them through the terminal checker before this stage.
    pub(super) fn check_iterator_adapter_method(
        &mut self,
        id: IteratorMethodId,
        method: &str,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        elem: ResolvedType,
        iterator_elem: ResolvedType,
        span: Span,
    ) -> Option<ResolvedType> {
        match id {
            IteratorMethodId::Iter => {
                self.validate_iterator_method_arity(method, 0, args.len(), span);
                Some(Self::iterator_protocol_ty(elem))
            }
            IteratorMethodId::Map => {
                if !self.validate_iterator_method_arity(method, 1, args.len(), span) {
                    return Some(Self::iterator_protocol_ty(ResolvedType::Unknown));
                }
                let mapped = self.iterator_mapping_callback_return_type(
                    arg_types.first().unwrap_or(&ResolvedType::Unknown),
                    iterator_elem,
                    span,
                );
                Some(Self::iterator_protocol_ty(mapped))
            }
            IteratorMethodId::Filter | IteratorMethodId::TakeWhile | IteratorMethodId::SkipWhile => {
                if self.validate_iterator_method_arity(method, 1, args.len(), span) {
                    self.validate_iterator_callback_return(
                        method,
                        arg_types.first().unwrap_or(&ResolvedType::Unknown),
                        vec![iterator_elem.clone()],
                        ResolvedType::Bool,
                        span,
                    );
                }
                Some(Self::iterator_protocol_ty(iterator_elem))
            }
            IteratorMethodId::FlatMap => {
                Some(self.check_iterator_flat_map(method, args, arg_types, iterator_elem, span))
            }
            IteratorMethodId::Take | IteratorMethodId::Skip => {
                if self.validate_iterator_method_arity(method, 1, args.len(), span)
                    && let Some(arg_ty) = arg_types.first()
                    && !self.types_compatible(arg_ty, &ResolvedType::Int)
                {
                    self.errors
                        .push(errors::type_mismatch("int", &arg_ty.to_string(), span));
                }
                Some(Self::iterator_protocol_ty(iterator_elem))
            }
            IteratorMethodId::Chain => {
                if self.validate_iterator_method_arity(method, 1, args.len(), span)
                    && let Some(arg_ty) = arg_types.first()
                {
                    let expected = Self::iterator_protocol_ty(iterator_elem.clone());
                    if !self.types_compatible(arg_ty, &expected) {
                        self.errors
                            .push(errors::type_mismatch(&expected.to_string(), &arg_ty.to_string(), span));
                    }
                }
                Some(Self::iterator_protocol_ty(iterator_elem))
            }
            IteratorMethodId::Enumerate => {
                self.validate_iterator_method_arity(method, 0, args.len(), span);
                Some(Self::iterator_protocol_ty(ResolvedType::Tuple(vec![
                    ResolvedType::Int,
                    iterator_elem,
                ])))
            }
            IteratorMethodId::Zip => {
                if !self.validate_iterator_method_arity(method, 1, args.len(), span) {
                    return Some(Self::iterator_protocol_ty(ResolvedType::Unknown));
                }
                let other_elem = arg_types
                    .first()
                    .and_then(|arg_ty| self.iterable_protocol_element_type(arg_ty))
                    .unwrap_or_else(|| {
                        if let Some(arg_ty) = arg_types.first()
                            && !matches!(arg_ty, ResolvedType::Unknown)
                        {
                            let expected = ResolvedType::Generic(
                                Self::iterator_protocol_name().to_string(),
                                vec![ResolvedType::Unknown],
                            );
                            self.errors
                                .push(errors::type_mismatch(&expected.to_string(), &arg_ty.to_string(), span));
                        }
                        ResolvedType::Unknown
                    });
                Some(Self::iterator_protocol_ty(ResolvedType::Tuple(vec![
                    iterator_elem,
                    other_elem,
                ])))
            }
            IteratorMethodId::Batch => {
                if self.validate_iterator_method_arity(method, 1, args.len(), span)
                    && let Some(arg_ty) = arg_types.first()
                    && !self.types_compatible(arg_ty, &ResolvedType::Int)
                {
                    self.errors
                        .push(errors::type_mismatch("int", &arg_ty.to_string(), span));
                }
                self.validate_iterator_batch_size_literal(args, span);
                Some(Self::iterator_protocol_ty(list_ty(iterator_elem)))
            }
            IteratorMethodId::Collect
            | IteratorMethodId::Count
            | IteratorMethodId::Any
            | IteratorMethodId::All
            | IteratorMethodId::Find
            | IteratorMethodId::Reduce
            | IteratorMethodId::Fold
            | IteratorMethodId::ForEach
            | IteratorMethodId::Sum => None,
        }
    }

    /// Validate flat-map's callback and require its known return type to expose an iterable element.
    pub(super) fn check_iterator_flat_map(
        &mut self,
        method: &str,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        element: ResolvedType,
        span: Span,
    ) -> ResolvedType {
        if !self.validate_iterator_method_arity(method, 1, args.len(), span) {
            return Self::iterator_protocol_ty(ResolvedType::Unknown);
        }
        let returned = self.iterator_mapping_callback_return_type(
            arg_types.first().unwrap_or(&ResolvedType::Unknown),
            element,
            span,
        );
        let Some(element) = self.iterable_protocol_element_type(&returned) else {
            if !matches!(returned, ResolvedType::Unknown) {
                let expected =
                    ResolvedType::Generic(Self::iterable_protocol_name().to_string(), vec![ResolvedType::Unknown]);
                self.errors.push(errors::type_mismatch(
                    &expected.to_string(),
                    &returned.to_string(),
                    span,
                ));
            }
            return Self::iterator_protocol_ty(ResolvedType::Unknown);
        };
        Self::iterator_protocol_ty(element)
    }

    /// Check terminal iterator consumers selected by the registry, including callback and accumulator expectations.
    pub(super) fn check_iterator_terminal_method(
        &mut self,
        id: IteratorMethodId,
        method: &str,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        iterator_elem: ResolvedType,
        span: Span,
    ) -> Option<ResolvedType> {
        match id {
            IteratorMethodId::Collect => {
                self.validate_iterator_method_arity(method, 0, args.len(), span);
                Some(list_ty(iterator_elem))
            }
            IteratorMethodId::Count => {
                self.validate_iterator_method_arity(method, 0, args.len(), span);
                Some(ResolvedType::Int)
            }
            IteratorMethodId::Any | IteratorMethodId::All => {
                if self.validate_iterator_method_arity(method, 1, args.len(), span) {
                    self.validate_iterator_callback_return(
                        method,
                        arg_types.first().unwrap_or(&ResolvedType::Unknown),
                        vec![iterator_elem],
                        ResolvedType::Bool,
                        span,
                    );
                }
                Some(ResolvedType::Bool)
            }
            IteratorMethodId::Find => {
                if self.validate_iterator_method_arity(method, 1, args.len(), span) {
                    self.validate_iterator_callback_return(
                        method,
                        arg_types.first().unwrap_or(&ResolvedType::Unknown),
                        vec![iterator_elem.clone()],
                        ResolvedType::Bool,
                        span,
                    );
                }
                Some(option_ty(iterator_elem))
            }
            IteratorMethodId::Reduce | IteratorMethodId::Fold => {
                if !self.validate_iterator_method_arity(method, 2, args.len(), span) {
                    return Some(ResolvedType::Unknown);
                }
                let acc_ty = arg_types.first().cloned().unwrap_or(ResolvedType::Unknown);
                self.validate_iterator_callback_return(
                    method,
                    arg_types.get(1).unwrap_or(&ResolvedType::Unknown),
                    vec![acc_ty.clone(), iterator_elem],
                    acc_ty.clone(),
                    span,
                );
                Some(acc_ty)
            }
            IteratorMethodId::ForEach => {
                if self.validate_iterator_method_arity(method, 1, args.len(), span) {
                    self.validate_iterator_callback_return(
                        method,
                        arg_types.first().unwrap_or(&ResolvedType::Unknown),
                        vec![iterator_elem],
                        ResolvedType::Unit,
                        span,
                    );
                }
                Some(ResolvedType::Unit)
            }
            IteratorMethodId::Sum => Some(self.checked_iterator_sum_output_type(method, args, &iterator_elem, span)),
            IteratorMethodId::Iter
            | IteratorMethodId::Map
            | IteratorMethodId::Filter
            | IteratorMethodId::TakeWhile
            | IteratorMethodId::SkipWhile
            | IteratorMethodId::FlatMap
            | IteratorMethodId::Take
            | IteratorMethodId::Skip
            | IteratorMethodId::Chain
            | IteratorMethodId::Enumerate
            | IteratorMethodId::Zip
            | IteratorMethodId::Batch => None,
        }
    }
}
