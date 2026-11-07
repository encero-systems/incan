//! Methods on runtime primitive, text, frozen, option, result and collection receivers.

use super::*;

impl TypeChecker {
    /// Select established concurrency and floating-point surface results before ordinary source method lookup.
    ///
    /// External runtime surfaces without a source definition retain permissive unknown results. Source-defined types
    /// outside these selected members continue to their declared signatures later in the ordered method pass.
    pub(super) fn runtime_primitive_method_return(&self, base_ty: &ResolvedType, method: &str) -> Option<ResolvedType> {
        if matches!(base_ty, ResolvedType::Named(name) if surface_types::from_str(name) == Some(SurfaceTypeId::TaskJoinError))
            && self.type_info.runtime_owner_admitted(SurfaceTypeId::TaskJoinError)
        {
            return Some(match method {
                "message" => ResolvedType::Str,
                "source" => option_ty(ResolvedType::Str),
                "is_cancelled" | "is_panic" => ResolvedType::Bool,
                _ => ResolvedType::Unknown,
            });
        }
        if let ResolvedType::Named(name) = base_ty
            && surface_types::from_str(name.as_str()).is_some_and(surface_accepts_unknown_methods)
            && self.lookup_semantic_type_info(name).is_none()
        {
            return Some(ResolvedType::Unknown);
        }
        if matches!(base_ty, ResolvedType::Named(name) if name == surface_types::as_str(SurfaceTypeId::Semaphore)) {
            return Some(match method {
                "acquire" => ResolvedType::Generic(
                    "Result".to_string(),
                    vec![
                        ResolvedType::Named(SEMAPHORE_PERMIT_TYPE_NAME.to_string()),
                        ResolvedType::Named(SEMAPHORE_ACQUIRE_ERROR_TYPE_NAME.to_string()),
                    ],
                ),
                "try_acquire" => option_ty(ResolvedType::Named(SEMAPHORE_PERMIT_TYPE_NAME.to_string())),
                "available_permits" => ResolvedType::Int,
                _ => ResolvedType::Unknown,
            });
        }
        if let ResolvedType::Generic(name, arguments) = base_ty
            && surface_types::from_str(name.as_str()) == Some(SurfaceTypeId::Mutex)
        {
            let inner = arguments.first().cloned().unwrap_or(ResolvedType::Unknown);
            return Some(match method {
                "lock" => ResolvedType::Generic("MutexGuard".to_string(), vec![inner.clone()]),
                "try_lock" => option_ty(ResolvedType::Generic("MutexGuard".to_string(), vec![inner])),
                _ => ResolvedType::Unknown,
            });
        }
        if let ResolvedType::Generic(name, arguments) = base_ty
            && surface_types::from_str(name.as_str()) == Some(SurfaceTypeId::RwLock)
        {
            let inner = arguments.first().cloned().unwrap_or(ResolvedType::Unknown);
            return Some(match method {
                "read" => ResolvedType::Generic("RwLockReadGuard".to_string(), vec![inner.clone()]),
                "write" => ResolvedType::Generic("RwLockWriteGuard".to_string(), vec![inner.clone()]),
                "try_read" => option_ty(ResolvedType::Generic(
                    "RwLockReadGuard".to_string(),
                    vec![inner.clone()],
                )),
                "try_write" => option_ty(ResolvedType::Generic("RwLockWriteGuard".to_string(), vec![inner])),
                _ => ResolvedType::Unknown,
            });
        }
        if matches!(
            base_ty,
            ResolvedType::Float | ResolvedType::Numeric(incan_lang::lang::types::numerics::NumericTypeId::F32)
        ) && let Some(id) = float_methods::from_str(method)
        {
            return Some(match id {
                float_methods::FloatMethodId::IsNan
                | float_methods::FloatMethodId::IsInfinite
                | float_methods::FloatMethodId::IsFinite => ResolvedType::Bool,
                _ => ResolvedType::Float,
            });
        }
        None
    }

    /// Check text/bytes helpers and retain exact native carrier and codec identities for the executable boundary.
    ///
    /// Encoding yields bytes; decoding can fail and yields `Result[str, ValidationError]`. Frozen and runtime text
    /// share the selected string-helper contracts, with frozen length queries handled by their own registries.
    pub(super) fn check_text_method_call(
        &mut self,
        call: &MethodCallSite<'_>,
        base_ty: &ResolvedType,
        arg_types: &[ResolvedType],
    ) -> Option<ResolvedType> {
        if (matches!(base_ty, ResolvedType::Str) || is_frozen_str(base_ty))
            && !call.type_args.is_empty()
            && let Some(method) = string_methods::from_str(call.method)
        {
            self.errors.push(errors::explicit_type_arg_arity(
                string_methods::as_str(method),
                &[],
                &Self::written_type_args(call.type_args),
                Self::explicit_type_argument_span(call.type_args, call.span),
            ));
            return Some(ResolvedType::Unknown);
        }
        if matches!(base_ty, ResolvedType::Bytes) && call.method == "as_slice" && call.args.is_empty() {
            let native = self.native_rust_signature_type("&[u8]", "std");
            self.type_info.calls.body_ir_call_result_types.insert(call.span, native);
            return Some(ResolvedType::Bytes);
        }
        if (matches!(base_ty, ResolvedType::Str) || is_frozen_str(base_ty))
            && string_methods::from_str(call.method) == Some(StringMethodId::Encode)
        {
            self.validate_text_codec_call("str.encode", false, call.args, arg_types, call.span);
            return Some(ResolvedType::Bytes);
        }
        if (matches!(base_ty, ResolvedType::Bytes) || is_frozen_bytes(base_ty))
            && bytes_methods::from_str(call.method) == Some(bytes_methods::BytesMethodId::Decode)
        {
            self.validate_text_codec_call("bytes.decode", true, call.args, arg_types, call.span);
            if is_frozen_bytes(base_ty) {
                let owner = incan_lang::lang::types::stringlike::as_str(
                    incan_lang::lang::types::stringlike::StringLikeId::FrozenBytes,
                );
                self.type_info.calls.body_ir_native_codec_members.insert(
                    call.span,
                    Self::compiler_builtin_member_identity(
                        owner,
                        bytes_methods::as_str(bytes_methods::BytesMethodId::Decode),
                    ),
                );
            }
            return Some(ResolvedType::Generic(
                "Result".to_string(),
                vec![ResolvedType::Str, ResolvedType::Named("ValidationError".to_string())],
            ));
        }
        let selected = if matches!(base_ty, ResolvedType::Str) {
            runtime_string_method_identity_and_return(call.method)
        } else if is_frozen_str(base_ty) {
            string_method_identity_and_return(call.method, true)
        } else {
            None
        };
        if let Some((id, result)) = selected {
            let owner = if is_frozen_str(base_ty) { "FrozenStr" } else { "str" };
            self.refuse_non_positional_builtin_args(&format!("{owner}.{}", string_methods::as_str(id)), call.args);
            if call.type_args.is_empty() && HelperOp::for_selected_string_method(id).is_some() {
                self.validate_selected_string_helper_call(id, call.args, arg_types, call.span);
            }
            return Some(result);
        }
        if is_frozen_bytes(base_ty)
            && let Some(id) = frozen_bytes_methods::from_str(call.method)
        {
            return Some(match id {
                frozen_bytes_methods::FrozenBytesMethodId::Len => ResolvedType::Int,
                frozen_bytes_methods::FrozenBytesMethodId::IsEmpty => ResolvedType::Bool,
                frozen_bytes_methods::FrozenBytesMethodId::Decode => ResolvedType::Generic(
                    "Result".to_string(),
                    vec![ResolvedType::Str, ResolvedType::Named("ValidationError".to_string())],
                ),
            });
        }
        None
    }

    /// Select frozen collection results, validating membership probes against their element or key contract.
    pub(super) fn check_frozen_collection_method_call(
        &mut self,
        call: &MethodCallSite<'_>,
        base_ty: &ResolvedType,
        arg_types: &[ResolvedType],
    ) -> Option<ResolvedType> {
        match base_ty {
            ResolvedType::FrozenList(_) => Some(match frozen_list_methods::from_str(call.method)? {
                frozen_list_methods::FrozenListMethodId::Len => ResolvedType::Int,
                frozen_list_methods::FrozenListMethodId::IsEmpty => ResolvedType::Bool,
            }),
            ResolvedType::FrozenSet(element) => Some(match frozen_set_methods::from_str(call.method)? {
                frozen_set_methods::FrozenSetMethodId::Len => ResolvedType::Int,
                frozen_set_methods::FrozenSetMethodId::IsEmpty => ResolvedType::Bool,
                frozen_set_methods::FrozenSetMethodId::Contains => {
                    self.validate_membership_probe_call("FrozenSet.contains", element, true, call.args, arg_types);
                    ResolvedType::Bool
                }
            }),
            ResolvedType::FrozenDict(key, _) => Some(match frozen_dict_methods::from_str(call.method)? {
                frozen_dict_methods::FrozenDictMethodId::Len => ResolvedType::Int,
                frozen_dict_methods::FrozenDictMethodId::IsEmpty => ResolvedType::Bool,
                frozen_dict_methods::FrozenDictMethodId::ContainsKey => {
                    self.validate_membership_probe_call("FrozenDict.contains_key", key, true, call.args, arg_types);
                    ResolvedType::Bool
                }
            }),
            _ => None,
        }
    }

    /// Check Option payload helpers, preserving reference payloads and refusing redundant copies after `dict.get`.
    pub(super) fn check_option_method_call(
        &mut self,
        call: &MethodCallSite<'_>,
        base_ty: &ResolvedType,
        arg_types: &[ResolvedType],
    ) -> Option<ResolvedType> {
        if !base_ty.is_option() {
            return None;
        }
        let inner = base_ty.option_inner_type().cloned().unwrap_or(ResolvedType::Unknown);
        if matches!(call.method, "copied" | "cloned")
            && !matches!(inner, ResolvedType::Ref(_) | ResolvedType::RefMut(_))
            && matches!(&call.base.node, Expr::MethodCall(_, name, _, _) if name == "get")
        {
            self.errors.push(
                errors::missing_method(&base_ty.to_string(), call.method, call.span).with_hint(format!(
                    "`get` returns the stored value itself, so `.{}()` has nothing to do; remove it",
                    call.method,
                )),
            );
            return Some(ResolvedType::Unknown);
        }
        match option_methods::from_str(call.method) {
            Some(option_methods::OptionMethodId::AsMut) => {
                if !call.args.is_empty() {
                    self.errors
                        .push(errors::builtin_arity("Option.as_mut", 0, call.args.len(), call.span));
                }
                Some(option_ty(ResolvedType::RefMut(Box::new(inner))))
            }
            Some(option_methods::OptionMethodId::Copied) => {
                if let ResolvedType::Ref(payload) | ResolvedType::RefMut(payload) = inner {
                    let payload = (*payload).clone();
                    let unresolved = matches!(&payload, ResolvedType::RustPath(path) if Self::rust_display_type_var_name(path).is_some());
                    if self.is_copy_type(&payload) || self.is_generic_placeholder_type(&payload) || unresolved {
                        return Some(option_ty(payload));
                    }
                }
                None
            }
            Some(option_methods::OptionMethodId::UnwrapOr) => {
                if let Some(default) = arg_types.first()
                    && !self.unwrap_or_default_fits_payload(default, &inner)
                {
                    self.errors.push(errors::type_mismatch(
                        &inner.to_string(),
                        &default.to_string(),
                        call.span,
                    ));
                }
                Some(inner)
            }
            Some(option_methods::OptionMethodId::Unwrap) => Some(inner),
            Some(option_methods::OptionMethodId::IsSome | option_methods::OptionMethodId::IsNone) => {
                Some(ResolvedType::Bool)
            }
            Some(option_methods::OptionMethodId::Clone) => None,
            None => None,
        }
    }

    /// Check Result payload extraction and its default argument before considering Result combinators.
    pub(super) fn check_result_unwrap_method_call(
        &mut self,
        call: &MethodCallSite<'_>,
        base_ty: &ResolvedType,
        arg_types: &[ResolvedType],
    ) -> Option<ResolvedType> {
        let ResolvedType::Generic(name, arguments) = base_ty else {
            return None;
        };
        if collection_type_id(name.as_str()) != Some(CollectionTypeId::Result) || arguments.len() != 2 {
            return None;
        }
        let ok_ty = arguments[0].clone();
        match result_methods::from_str(call.method) {
            Some(result_methods::ResultMethodId::Unwrap) => {
                if !call.args.is_empty() {
                    self.errors.push(errors::type_mismatch(
                        "no arguments",
                        &format!("{} argument(s)", call.args.len()),
                        call.span,
                    ));
                }
                Some(ok_ty)
            }
            Some(result_methods::ResultMethodId::UnwrapOr) => {
                if let Some(default) = arg_types.first()
                    && !self.unwrap_or_default_fits_payload(default, &ok_ty)
                {
                    self.errors.push(errors::type_mismatch(
                        &ok_ty.to_string(),
                        &default.to_string(),
                        call.span,
                    ));
                }
                if call.args.len() != 1 {
                    self.errors.push(errors::type_mismatch(
                        "one default argument",
                        &format!("{} argument(s)", call.args.len()),
                        call.span,
                    ));
                }
                Some(ok_ty)
            }
            Some(result_methods::ResultMethodId::IsOk | result_methods::ResultMethodId::IsErr) => {
                if !call.args.is_empty() {
                    self.errors.push(errors::type_mismatch(
                        "no arguments",
                        &format!("{} argument(s)", call.args.len()),
                        call.span,
                    ));
                }
                Some(ResolvedType::Bool)
            }
            _ => None,
        }
    }

    /// Dispatch generic collection methods by their registry family without nesting the family's validation policy.
    pub(super) fn check_generic_collection_method_call(
        &mut self,
        call: &MethodCallSite<'_>,
        base_ty: &ResolvedType,
        arg_types: &[ResolvedType],
    ) -> Option<ResolvedType> {
        let ResolvedType::Generic(name, arguments) = base_ty else {
            return None;
        };
        match collection_type_id(name.as_str())? {
            CollectionTypeId::Generator => self.check_generator_collection_method(call, arguments, arg_types),
            CollectionTypeId::List => self.check_list_collection_method(call, arguments, arg_types),
            CollectionTypeId::Dict => self.check_dict_collection_method(call, arguments, arg_types),
            CollectionTypeId::Set => Some(match set_methods::from_str(call.method)? {
                set_methods::SetMethodId::Add => ResolvedType::Unit,
                set_methods::SetMethodId::Contains => ResolvedType::Bool,
            }),
            CollectionTypeId::Option if call.method == "clone" => Some(self.check_option_clone_call(call, arguments)),
            _ => None,
        }
    }

    /// Check the established generator adapter argument contracts and retain their element/result types.
    pub(super) fn check_generator_collection_method(
        &mut self,
        call: &MethodCallSite<'_>,
        arguments: &[ResolvedType],
        arg_types: &[ResolvedType],
    ) -> Option<ResolvedType> {
        let element = arguments.first().cloned().unwrap_or(ResolvedType::Unknown);
        match iterator_methods::from_str(call.method) {
            Some(iterator_methods::IteratorMethodId::Map) => {
                let mapped = self.generator_map_return_type(&element, call.args, arg_types, call.span);
                Some(generator_ty(mapped))
            }
            Some(iterator_methods::IteratorMethodId::Filter) => {
                self.validate_generator_filter_arg(&element, call.args, arg_types, call.span);
                Some(generator_ty(element))
            }
            Some(iterator_methods::IteratorMethodId::Take) => {
                self.validate_generator_take_arg(call.args, arg_types, call.span);
                Some(generator_ty(element))
            }
            Some(iterator_methods::IteratorMethodId::Collect) => {
                if !call.args.is_empty() {
                    self.errors.push(errors::type_mismatch(
                        "no arguments",
                        &format!("{} argument(s)", call.args.len()),
                        call.span,
                    ));
                }
                Some(list_ty(element))
            }
            _ => None,
        }
    }

    /// Check list member types and copy requirements, distinguishing fresh append values from values read from places.
    pub(super) fn check_list_collection_method(
        &mut self,
        call: &MethodCallSite<'_>,
        arguments: &[ResolvedType],
        arg_types: &[ResolvedType],
    ) -> Option<ResolvedType> {
        let element = arguments.first().cloned().unwrap_or(ResolvedType::Unknown);
        Some(match list_methods::from_str(call.method)? {
            list_methods::ListMethodId::Append => {
                let clone_ty = arg_types.first().unwrap_or(&element);
                if let Some(argument) = arg_types.first()
                    && !self.types_compatible(argument, &element)
                {
                    self.errors.push(errors::type_mismatch(
                        &element.to_string(),
                        &argument.to_string(),
                        call.span,
                    ));
                }
                let fresh = matches!(call.args.first(), Some(CallArg::Positional(value) | CallArg::Named(_, value)) if self.is_fresh_value(value));
                if !fresh && !self.is_copy_type(clone_ty) && !self.is_clone_type(clone_ty) {
                    self.errors
                        .push(errors::list_append_requires_clone(&clone_ty.to_string(), call.span));
                }
                ResolvedType::Unit
            }
            list_methods::ListMethodId::Extend => {
                let other_list = list_ty(element.clone());
                if let Some(argument) = arg_types.first()
                    && !self.types_compatible(argument, &other_list)
                {
                    self.errors.push(errors::type_mismatch(
                        &other_list.to_string(),
                        &argument.to_string(),
                        call.span,
                    ));
                }
                if !self.is_copy_type(&element) && !self.is_clone_type(&element) {
                    self.errors
                        .push(errors::list_extend_requires_clone(&element.to_string(), call.span));
                }
                ResolvedType::Unit
            }
            list_methods::ListMethodId::Clone => {
                if !self.is_copy_type(&element) && !self.is_clone_type(&element) {
                    self.errors
                        .push(errors::list_clone_requires_clone(&element.to_string(), call.span));
                }
                list_ty(element.clone())
            }
            list_methods::ListMethodId::Pop => element,
            list_methods::ListMethodId::Contains => ResolvedType::Bool,
            list_methods::ListMethodId::Swap
            | list_methods::ListMethodId::Reserve
            | list_methods::ListMethodId::ReserveExact
            | list_methods::ListMethodId::Remove => ResolvedType::Unit,
            list_methods::ListMethodId::Count | list_methods::ListMethodId::Index => ResolvedType::Int,
        })
    }

    /// Check dictionary members and retain lookup-value facts so lowering can choose a read or a copy.
    ///
    /// A defaulted lookup copies out its value and validates that contract here; an optional lookup records the payload
    /// and leaves use-sensitive reads/copies to the checked lookup facts consumed downstream.
    pub(super) fn check_dict_collection_method(
        &mut self,
        call: &MethodCallSite<'_>,
        arguments: &[ResolvedType],
        arg_types: &[ResolvedType],
    ) -> Option<ResolvedType> {
        let key = arguments.first().cloned().unwrap_or(ResolvedType::Unknown);
        let value = arguments.get(1).cloned().unwrap_or(ResolvedType::Unknown);
        Some(match dict_methods::from_str(call.method)? {
            dict_methods::DictMethodId::Keys => list_ty(key),
            dict_methods::DictMethodId::Values => list_ty(value),
            dict_methods::DictMethodId::Get if call.args.len() == 2 => {
                self.validate_dict_get_with_default(&key, &value, call.args, arg_types);
                self.note_dict_lookup_value(call.span, &value);
                value
            }
            dict_methods::DictMethodId::Get => {
                self.refuse_non_positional_builtin_args("Dict.get", call.args);
                if call.args.is_empty() {
                    self.errors.push(errors::builtin_arity("Dict.get", 1, 0, call.span));
                } else if call.args.len() > 2 {
                    self.errors
                        .push(errors::builtin_max_arity("Dict.get", 2, call.args.len(), call.span));
                }
                self.note_dict_lookup_value(call.span, &value);
                option_ty(value.clone())
            }
            dict_methods::DictMethodId::Insert => ResolvedType::Unit,
            dict_methods::DictMethodId::ContainsKey => {
                self.validate_membership_probe_call("Dict.contains_key", &key, false, call.args, arg_types);
                ResolvedType::Bool
            }
        })
    }

    /// Validate Option cloning while retaining the existing permissive native-path payload rule.
    pub(super) fn check_option_clone_call(
        &mut self,
        call: &MethodCallSite<'_>,
        arguments: &[ResolvedType],
    ) -> ResolvedType {
        if !call.args.is_empty() {
            self.errors.push(errors::type_mismatch(
                "no arguments",
                &format!("{} argument(s)", call.args.len()),
                call.span,
            ));
        }
        let inner = arguments.first().cloned().unwrap_or(ResolvedType::Unknown);
        if !self.is_copy_type(&inner) && !self.is_clone_type(&inner) && !matches!(inner, ResolvedType::RustPath(_)) {
            self.errors
                .push(errors::list_clone_requires_clone(&inner.to_string(), call.span));
        }
        option_ty(inner)
    }
}
