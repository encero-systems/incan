//! Checked Rust method retention, receiver and argument checks, and Rust trait method calls.

use super::*;

impl TypeChecker {
    /// Retain an inherent Rust callee using the impl's declared native binders, rather than the owner's arity.
    /// Receiver references are explicit checked parameter types; no downstream consumer guesses a borrow from a name.
    /// Metadata-defaulted receiver arguments use the same storage arity as constructors and retained dereferences.
    /// The impl's instantiated Self owns the receiver slot; source collection and text projections never replace it.
    /// The selecting caller supplies written method binders; associated owner arguments never occupy those slots.
    /// A callee that cannot be retained returns the call's refusal, which the caller reports unless the call is already
    /// refused: no accepted call leaves checking without a callee. Argument validation refuses an unsafe method, and
    /// an associated call with open owner arguments or a method with open result parameters is retained again once
    /// its binding's first typed use fixes them, or refused (#1720, #2481).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn retain_checked_rust_method(
        &mut self,
        method: &incan_lang::interop::RustMethodSig,
        written_method_parameters: &[String],
        owner: &incan_lang::interop::RustTypeInfo,
        path: &str,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        receiver: Option<&ResolvedType>,
        result: &ResolvedType,
        span: Span,
    ) -> Result<(), CompileError> {
        if method.signature.is_unsafe {
            return Ok(());
        }
        if owner.has_const_params {
            return Err(errors::rust_receiver_const_generics_not_supported(path, span));
        }
        let callee = format!("rust::{path}.{}", method.name);
        let native_parameters = method
            .impl_type_params
            .iter()
            .chain(&method.signature.type_params)
            .cloned()
            .collect::<Vec<_>>();
        let mut bindings = self.rust_method_bindings(
            method,
            written_method_parameters,
            owner,
            path,
            &native_parameters,
            args,
            arg_types,
            receiver,
            result,
            span,
        );
        let type_arguments = match self.rust_method_type_arguments(&native_parameters, owner, &bindings) {
            Ok(type_arguments) => type_arguments,
            Err(open) => {
                return self.refuse_open_rust_method(
                    method,
                    path,
                    &native_parameters,
                    &bindings,
                    receiver,
                    result,
                    &open,
                    span,
                );
            }
        };
        if crate::trace_flags::trace_rust_facts() {
            eprintln!(
                "rust method: {path}.{} native arguments {type_arguments:?}, bindings {bindings:?}",
                method.name
            );
        }
        for (parameter, argument) in native_parameters.iter().zip(&type_arguments) {
            bindings.entry(parameter.clone()).or_insert_with(|| argument.clone());
        }
        let mut parameters = self.type_info.call_site_callable_params(span).unwrap_or(&[]).to_vec();
        let ordinary = if Self::rust_signature_has_receiver(&method.signature) {
            &method.signature.params[1..]
        } else {
            &method.signature.params
        };
        for (parameter, declared) in parameters.iter_mut().zip(ordinary) {
            let template = self.native_rust_signature_type(&declared.type_display, path);
            parameter.ty = self.instantiate_rust_signature_type(&template, &native_parameters, &bindings);
        }
        if let Some(receiver) = receiver {
            let Some(parameter) =
                self.checked_rust_method_receiver(method, owner, path, &native_parameters, &bindings, receiver)
            else {
                let gap = errors::NativeCalleeGap::NoCandidate;
                return Err(errors::rust_callee_not_selected(&callee, gap, span));
            };
            parameters.insert(0, parameter);
        }
        let mut native_result =
            self.native_method_result(method, path, &native_parameters, &bindings, receiver, result);
        if method.signature.is_async {
            native_result = ResolvedType::Generic("Awaitable".into(), vec![native_result]);
        }
        self.retain_rust_value_receiver_copy(owner, path, receiver, &parameters, span);
        let result_item_path = self
            .rust_result_item_path(&render_resolved_type_as_rust_arg(&native_result), path)
            .or_else(|| self.rust_result_item_path(&method.signature.return_type, path));
        // Primitive re-exports have no nominal owner. Retain their admitted definition identity so native
        // resolution uses the primitive impl bucket, rather than walking a public alias as though it were an ADT.
        let callee_owner = self
            .rust_definition_for_path(path)
            .filter(|definition| definition == "str")
            .unwrap_or_else(|| path.to_owned());
        self.retain_native_callback_parameters(
            &format!("{callee_owner}::{}", method.name),
            &method.signature,
            &native_parameters,
        );
        self.type_info.rust.checked_calls.insert(
            span,
            crate::typechecker::type_info::RustCheckedCall {
                storage: None,
                source_type_parameters: self.native_call_type_parameters(),
                receiver_adjustments: Vec::new(),
                path: format!("{callee_owner}::{}", method.name),
                type_arguments,
                parameters,
                return_type: native_result,
                result_item_path,
                has_receiver: receiver.is_some(),
            },
        );
        Ok(())
    }

    /// Refuse a native call whose binders stay open, unless the statement watches its associated owner arguments or
    /// its method's result parameters. The open native result lets a later typed use fix those arguments. Input-only
    /// method binders cannot be determined from a result and keep their immediate refusal (#1720, #2481).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn refuse_open_rust_method(
        &mut self,
        method: &incan_lang::interop::RustMethodSig,
        path: &str,
        native_parameters: &[String],
        bindings: &HashMap<String, ResolvedType>,
        receiver: Option<&ResolvedType>,
        result: &ResolvedType,
        open: &[String],
        span: Span,
    ) -> Result<(), CompileError> {
        if crate::trace_flags::trace_rust_facts() {
            eprintln!(
                "rust method unresolved: {path}.{} parameters {native_parameters:?}, receiver {receiver:?}, template {:?}, bindings {bindings:?}",
                method.name, method.impl_self_type
            );
        }
        if self.open_rust_generic_calls.contains_key(&span) {
            let native = self.native_method_result(method, path, native_parameters, bindings, receiver, result);
            let native_item = self
                .rust_result_item_path(&render_resolved_type_as_rust_arg(&native), path)
                .or_else(|| self.rust_result_item_path(&method.signature.return_type, path));
            if let Some(call) = self.open_rust_generic_calls.get_mut(&span) {
                call.native = Some(native);
                call.native_item = native_item;
            }
            return Ok(());
        }
        let native = self.native_method_result(method, path, native_parameters, bindings, receiver, result);
        if Self::native_result_carries_parameters(&native, open)
            && open
                .iter()
                .all(|parameter| method.signature.type_params.contains(parameter))
        {
            self.open_rust_generic_calls.insert(
                span,
                crate::typechecker::OpenRustGenericCall {
                    owner: path.to_owned(),
                    method: method.name.clone(),
                    type_params: open.to_vec(),
                    owner_parameters: false,
                    native: Some(native),
                    native_item: None,
                },
            );
            return Ok(());
        }
        let callee = format!("rust::{path}.{}", method.name);
        let gap = errors::NativeCalleeGap::OpenTypeParameters(open);
        Err(errors::rust_callee_not_selected(&callee, gap, span))
    }

    /// Apply an inherent method's declared receiver mode to its instantiated impl Self, or to the receiver's storage
    /// owner when metadata records no impl Self. `None` means the method takes no receiver form this value has.
    pub(super) fn checked_rust_method_receiver(
        &self,
        method: &incan_lang::interop::RustMethodSig,
        owner: &incan_lang::interop::RustTypeInfo,
        path: &str,
        native_parameters: &[String],
        bindings: &HashMap<String, ResolvedType>,
        receiver: &ResolvedType,
    ) -> Option<CallableParam> {
        let storage = match method.impl_self_type.as_deref() {
            Some(template) => self.instantiate_rust_signature_type(
                &self.native_rust_signature_type(template, path),
                native_parameters,
                bindings,
            ),
            None => self.rust_storage_owner(receiver, owner, path),
        };
        self.checked_rust_receiver_parameter(&method.signature, &storage)
    }

    /// Read argument types after contextual checking, falling back to the original types when no refinement was
    /// retained.
    pub(super) fn rust_method_argument_types_after_context(
        &self,
        args: &[CallArg],
        original: &[ResolvedType],
    ) -> Vec<ResolvedType> {
        args.iter()
            .zip(original)
            .map(|(argument, original)| {
                self.type_info
                    .expr_type(Self::call_arg_expr(argument).span)
                    .cloned()
                    .unwrap_or_else(|| original.clone())
            })
            .collect()
    }

    /// Infer impl and open method binders, then preserve validated written method selections over source projections.
    /// The selecting caller owns binder roles; `_` and unknown arguments leave inference unchanged. An instantiated
    /// impl Self is authoritative for its own binders: a source-facing provisional result such as `Option[&T]` must
    /// not conflict with `Option<&T>::copied` and hide the receiver's exact `T`. Rechecking an original signature must
    /// not replace explicit unsized Rust text with the owned source string passed by borrow.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn rust_method_bindings(
        &self,
        method: &incan_lang::interop::RustMethodSig,
        written_method_parameters: &[String],
        owner: &incan_lang::interop::RustTypeInfo,
        path: &str,
        native_parameters: &[String],
        args: &[CallArg],
        arg_types: &[ResolvedType],
        receiver: Option<&ResolvedType>,
        result: &ResolvedType,
        span: Span,
    ) -> HashMap<String, ResolvedType> {
        let refreshed_types = self.rust_method_argument_types_after_context(args, arg_types);
        let mut bindings = self.infer_rust_signature_bindings(
            &method.signature,
            path,
            native_parameters,
            args,
            &refreshed_types,
            Some(result),
        );
        // An open result contributes no binding. Keep it from masking the exact owner's concrete argument below.
        bindings.retain(|_, value| !matches!(value, ResolvedType::Unknown | ResolvedType::CallSiteInfer));
        if let (Some(receiver), Some(template)) = (receiver, method.impl_self_type.as_deref()) {
            let receiver = match receiver {
                ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => inner.as_ref(),
                other => other,
            };
            let template = self.native_rust_signature_type(template, path);
            let mut receiver_bindings = HashMap::new();
            self.infer_type_param_bindings(
                &self.rust_inference_type(&template, native_parameters),
                &self.rust_inference_type(&self.rust_closed_owner(receiver, owner, path), &[]),
                &mut receiver_bindings,
            );
            for parameter in &method.impl_type_params {
                if let Some(argument) = receiver_bindings
                    .remove(parameter)
                    .filter(|argument| !matches!(argument, ResolvedType::Unknown | ResolvedType::CallSiteInfer))
                    .filter(|argument| {
                        !Self::mentions_foreign_type_variable(argument, &self.native_call_type_parameters())
                    })
                {
                    bindings.insert(parameter.clone(), argument);
                }
            }
        }
        if let Some(written) = self.type_info.calls.call_site_monomorph_type_args.get(&span)
            && !written_method_parameters.is_empty()
            && written.len() == written_method_parameters.len()
        {
            for (parameter, argument) in written_method_parameters.iter().zip(written) {
                if !matches!(argument, ResolvedType::Unknown | ResolvedType::CallSiteInfer) {
                    bindings.insert(parameter.clone(), self.rust_inference_type(argument, native_parameters));
                }
            }
        }
        bindings
    }

    /// Retain an admitted non-generic Copy receiver's layout when a native method consumes it by value. References
    /// and primitive text are already represented by their checked carriers and cannot supply nominal Copy evidence.
    pub(super) fn retain_rust_value_receiver_copy(
        &mut self,
        owner: &RustTypeInfo,
        path: &str,
        receiver: Option<&ResolvedType>,
        parameters: &[CallableParam],
        span: Span,
    ) {
        let nominal_receiver = receiver.is_some_and(|receiver| match receiver {
            ResolvedType::RustPath(path) => path != "char" && path != "str",
            ResolvedType::Named(_) | ResolvedType::Generic(_, _) => true,
            _ => false,
        });
        if !nominal_receiver
            || !owner.type_params.is_empty()
            || !parameters
                .first()
                .is_some_and(|parameter| !matches!(parameter.ty, ResolvedType::Ref(_) | ResolvedType::RefMut(_)))
            || self.derive_support(&ResolvedType::RustPath(path.into()), DeriveId::Copy) != DeriveSupport::Supported
        {
            return;
        }
        if owner.variants.is_empty() {
            self.retain_rust_copy_owner(path, owner);
        } else if let Some(layout) = self.checked_rust_enum_layout(path, owner) {
            self.type_info
                .rust
                .checked_enums
                .insert(self.rust_public_type_path(path), layout);
        }
        self.type_info.rust.copy_receivers.insert(span, path.into());
    }

    /// Exercise the retained generic method contract from the existing lookup unit: written native arguments win,
    /// placeholders still infer from borrowed source text, and impl binders retain their native declaration order.
    #[cfg(test)]
    pub(in crate::typechecker::check_expr) fn verify_written_native_method_arguments() {
        let mut checker = Self::new();
        let path = "native::Map";
        let key = ResolvedType::Numeric(numerics::NumericTypeId::U8);
        let value = ResolvedType::RustPath("std::string::String".into());
        let receiver = ResolvedType::Generic(path.into(), vec![key.clone(), value.clone()]);
        let owner = RustTypeInfo {
            type_params: vec!["K".into(), "V".into()],
            ..RustTypeInfo::default()
        };
        let method = incan_lang::interop::RustMethodSig {
            name: "get".into(),
            impl_type_params: vec!["K".into(), "V".into()],
            impl_self_type: Some("native::Map<K, V>".into()),
            signature: RustFunctionSig {
                receiver_contract: None,
                bounds: Vec::new(),
                type_params: vec!["Q".into()],
                params: vec![
                    incan_lang::interop::RustParam {
                        name: Some("self".into()),
                        type_display: "&Self".into(),
                    },
                    incan_lang::interop::RustParam {
                        name: Some("key".into()),
                        type_display: "&Q".into(),
                    },
                ],
                return_type: "()".into(),
                is_async: false,
                is_unsafe: false,
            },
        };
        for (written, expected) in [
            (
                ResolvedType::RustPath("str".into()),
                ResolvedType::RustPath("str".into()),
            ),
            (ResolvedType::CallSiteInfer, ResolvedType::Str),
            (value.clone(), value.clone()),
        ] {
            let span = Span::new(10, 20);
            let args = [CallArg::Positional(Spanned::new(
                Expr::Ident("probe".into()),
                Span::new(14, 19),
            ))];
            let argument_types = [ResolvedType::Str];
            let written = vec![written];
            let specialized = checker.specialize_rust_method_signature(&method.signature, path, &written);
            let result = checker.validate_rust_method_call(path, &specialized, &args, &argument_types, true, span);
            assert!(
                checker.errors.is_empty(),
                "written arguments must validate: {:?}",
                checker.errors
            );
            checker
                .type_info
                .calls
                .call_site_monomorph_type_args
                .insert(span, written);
            let retained = checker.retain_checked_rust_method(
                &method,
                &method.signature.type_params,
                &owner,
                path,
                &args,
                &argument_types,
                Some(&receiver),
                &result,
                span,
            );
            assert!(retained.is_ok(), "a closed callee must be retained: {retained:?}");
            // Closing the receiver projects its owned String to source Str; unsized native Q must stay distinct.
            assert_eq!(
                checker
                    .type_info
                    .rust
                    .checked_calls
                    .get(&span)
                    .map(|call| &call.type_arguments),
                Some(&vec![key.clone(), ResolvedType::Str, expected]),
            );
        }
    }

    /// Select native arguments in declaration order after the receiver and written arguments constrain them.
    /// Only metadata-declared owner defaults fill absent bindings; unresolved inputs grant no executable call, and the
    /// parameters left open are returned by name. A binding that names a type variable other than an enclosing source
    /// type parameter, such as the callee's own `K` read back from its unresolved result, is open too.
    pub(super) fn rust_method_type_arguments(
        &self,
        parameters: &[String],
        owner: &incan_lang::interop::RustTypeInfo,
        bindings: &HashMap<String, ResolvedType>,
    ) -> Result<Vec<ResolvedType>, Vec<String>> {
        let source_parameters = self.native_call_type_parameters();
        let closed = |parameter: &str| {
            bindings
                .get(parameter)
                .filter(|ty| !matches!(ty, ResolvedType::Unknown | ResolvedType::CallSiteInfer))
                .filter(|ty| !Self::mentions_foreign_type_variable(ty, &source_parameters))
                .cloned()
        };
        let owner_arguments = self.owner_arguments_with_defaults(owner, closed, "");
        let arguments = parameters
            .iter()
            .map(|parameter| {
                closed(parameter).or_else(|| {
                    let index = owner.type_params.iter().position(|name| name == parameter)?;
                    owner_arguments.get(index).cloned().flatten()
                })
            })
            .collect::<Vec<_>>();
        let open = parameters
            .iter()
            .zip(&arguments)
            .filter(|(_, argument)| argument.is_none())
            .map(|(parameter, _)| parameter.clone())
            .collect::<Vec<_>>();
        if open.is_empty() {
            Ok(arguments.into_iter().flatten().collect())
        } else {
            Err(open)
        }
    }

    /// Return whether `ty` names a type variable that is not one of the enclosing source type parameters.
    pub(in crate::typechecker) fn mentions_foreign_type_variable(
        ty: &ResolvedType,
        source_parameters: &[String],
    ) -> bool {
        let foreign = std::cell::Cell::new(false);
        map_resolved_type_leaves(ty, &|leaf| {
            if matches!(leaf, ResolvedType::TypeVar(name) if !source_parameters.contains(name)) {
                foreign.set(true);
            }
            None
        });
        foreign.get()
    }

    /// Apply the declared receiver mode to an already selected native Self, including Self that is itself a reference.
    /// Inherent selection removes a source receiver borrow before this boundary; trait selection must preserve it.
    /// Metadata can spell owned Self as its concrete carrier, including primitive owners. Other typed receivers
    /// require their own retained carrier and are refused rather than treated as owned Self.
    pub(super) fn checked_rust_receiver_parameter(
        &self,
        signature: &RustFunctionSig,
        receiver: &ResolvedType,
    ) -> Option<CallableParam> {
        let display = signature.params.first()?.type_display.trim();
        let carrier = render_resolved_type_as_rust_arg(receiver);
        let definition = self.rust_definition_for_path(&carrier).unwrap_or(carrier);
        let ty = match Self::rust_display_borrow_kind(display) {
            Some((true, _)) => ResolvedType::RefMut(Box::new(receiver.clone())),
            Some((false, _)) => ResolvedType::Ref(Box::new(receiver.clone())),
            None if display == "self" || display == "Self" || display == definition => receiver.clone(),
            None => return None,
        };
        Some(CallableParam::positional(ty))
    }

    /// Return whether a method-call receiver is a bare imported Rust trait that is known to declare `method`.
    ///
    /// The trait-import registry learns a trait's method names from inspected metadata or, failing that, from the
    /// compiler's fallback trait vocabulary. Either way the spelling `Trait.method(...)` is then a trait-qualified call
    /// rather than an associated function on a type, which is what decides whether a missing signature is a
    /// receiver-mode gap the call must report.
    pub(super) fn receiver_names_rust_trait_import(&self, receiver_binding: Option<&str>, method: &str) -> bool {
        receiver_binding
            .and_then(|binding| self.type_info.rust.trait_imports.get(binding))
            .is_some_and(|import| import.methods.contains(method))
    }

    /// Check a trait-qualified call `Trait.method(receiver, args...)` against inspected Rust trait metadata (#1375).
    ///
    /// Rust resolves `Trait::method` only against the trait's own associated items, and the method's declared receiver
    /// decides how the explicit first argument is passed: `&mut self` needs an exclusive borrow, `&self` a shared one,
    /// and `self` moves the value. [`Self::retain_rust_trait_qualified_call`] records that mode and checks the
    /// remaining arguments and the result through the ordinary trait-method validation, so no consumer derives it from
    /// the method or trait name; a method the trait does not declare is reported here instead of by rustc. A receiver
    /// of unknown type records no mode and gives the call a result of unknown type, as without metadata.
    pub(super) fn resolve_rust_trait_qualified_call(&mut self, call: RustTraitQualifiedCall<'_>) -> ResolvedType {
        let RustTraitQualifiedCall {
            rust_path,
            trait_info,
            method,
            type_args: _,
            args,
            arg_types,
            expected_return_ty: _,
            span,
        } = call;
        let Some(sig) = trait_info.items.iter().find_map(|item| match item {
            RustTraitAssoc::Function { name, signature } if name == method => Some(signature),
            _ => None,
        }) else {
            self.errors
                .push(errors::rust_trait_method_not_declared(rust_path, method, span));
            self.preserve_unresolved_rust_call_argument_returns(args);
            return ResolvedType::Unknown;
        };
        if !Self::rust_signature_has_receiver(sig) {
            return self
                .resolve_rust_trait_associated_call(&call, sig)
                .unwrap_or_else(|| self.refuse_missing_rust_method_signature(rust_path, method, span));
        }

        // ---- The explicit receiver argument takes the declared receiver mode ----
        let (Some(CallArg::Positional(receiver_expr)), Some(receiver_ty)) = (args.first(), arg_types.first()) else {
            self.errors.push(errors::builtin_arity(
                format!("rust::{rust_path}.{method}").as_str(),
                sig.params.len(),
                args.len(),
                span,
            ));
            self.preserve_unresolved_rust_call_argument_returns(args);
            return ResolvedType::Unknown;
        };
        // A receiver of unknown type, such as the guard `borrow_mut()` returns when nothing types it, may be the
        // trait's `Self` or a guard or reference that reaches one. Neither the borrow that turns it into the declared
        // receiver nor `Self` in the result can be decided, so the call keeps the facts it has without metadata: no
        // receiver borrow, a result of unknown type, and its arguments handed to Rust as they are (#1561).
        if Self::is_unknown_behind_references(receiver_ty) {
            self.preserve_unresolved_rust_call_argument_returns(args);
            return ResolvedType::Unknown;
        }
        if let Some(result) = self.retain_rust_trait_qualified_call(&call, receiver_expr, receiver_ty) {
            return result;
        }
        self.preserve_unresolved_rust_call_argument_returns(args);
        ResolvedType::Unknown
    }

    /// Return whether `ty` is unknown, directly or behind shared and mutable references.
    pub(super) fn is_unknown_behind_references(ty: &ResolvedType) -> bool {
        match ty {
            ResolvedType::Unknown => true,
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => Self::is_unknown_behind_references(inner),
            _ => false,
        }
    }

    /// Enforce RFC 054's arity-complete bracket contract for inspected Rust methods.
    pub(super) fn validate_rust_method_type_arg_arity(
        &mut self,
        method: &str,
        declared_type_params: &[String],
        type_args: &[Spanned<Type>],
        span: Span,
    ) -> bool {
        if type_args.is_empty() {
            return true;
        }
        if type_args.len() == declared_type_params.len() {
            return true;
        }
        self.errors.push(errors::explicit_type_arg_arity(
            method,
            declared_type_params,
            &Self::written_type_args(type_args),
            span,
        ));
        false
    }

    /// Validate a Rust trait method call and expose only source-meaningful return types.
    ///
    /// Imported Rust trait signatures are useful for parameter planning even when Rust metadata cannot prove the
    /// receiver trait edge. Some generic Rust methods, however, return an unbound Rust type parameter such as `T`;
    /// that is not an Incan type variable and must not leak into source typing as `rust::T`. Keep those call results
    /// permissive until the Rust compiler infers the concrete type from the emitted Rust context.
    pub(super) fn validate_rust_trait_method_call(&mut self, call: RustTraitMethodCall<'_>) -> ResolvedType {
        if !self.validate_rust_method_type_arg_arity(call.method, &call.sig.type_params, call.type_args, call.span) {
            return ResolvedType::Unknown;
        }
        let resolved_type_args = call
            .type_args
            .iter()
            .map(|arg| self.resolve_type_checked(arg))
            .collect::<Vec<_>>();
        if !resolved_type_args.is_empty() {
            self.type_info
                .calls
                .call_site_monomorph_type_args
                .insert(call.span, resolved_type_args.clone());
        }
        let effective_sig = if resolved_type_args.is_empty() {
            call.sig.clone()
        } else {
            self.specialize_rust_method_signature(call.sig, call.rust_path, resolved_type_args.as_slice())
        };
        let callable_display = format!("rust::{}.{}", call.rust_path, call.method);
        let ret = self.validate_rust_method_call(
            callable_display.as_str(),
            &effective_sig,
            call.args,
            call.arg_types,
            call.preserves_lookup_arg_shape,
            call.span,
        );
        let ret = Self::substitute_rust_self_type(ret, call.rust_path);
        if Self::contains_unbound_rust_type_var(&ret)
            || (resolved_type_args.is_empty() && Self::mentions_rust_method_type_param(&ret, &call.sig.type_params))
        {
            ResolvedType::Unknown
        } else {
            ret
        }
    }

    /// Return whether a Rust return type still projects through one of the method's own type parameters.
    ///
    /// `Result<S::Ok, S::Error>` on `Serialize::serialize<S>` names associated types of a parameter the call did not
    /// bind; Rust infers `S` from the argument, and the projection is not an Incan type the checker can compare.
    pub(super) fn mentions_rust_method_type_param(ty: &ResolvedType, type_params: &[String]) -> bool {
        match ty {
            ResolvedType::RustPath(path) => path
                .split("::")
                .next()
                .is_some_and(|head| type_params.iter().any(|param| param == head.trim())),
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => {
                Self::mentions_rust_method_type_param(inner, type_params)
            }
            ResolvedType::Generic(_, args) | ResolvedType::Tuple(args) => args
                .iter()
                .any(|arg| Self::mentions_rust_method_type_param(arg, type_params)),
            ResolvedType::Function(params, ret) => {
                params
                    .iter()
                    .any(|param| Self::mentions_rust_method_type_param(&param.ty, type_params))
                    || Self::mentions_rust_method_type_param(ret, type_params)
            }
            _ => false,
        }
    }

    /// Return whether a Rust return type still contains a generic placeholder the Incan checker cannot bind.
    pub(super) fn contains_unbound_rust_type_var(ty: &ResolvedType) -> bool {
        match ty {
            ResolvedType::TypeVar(_) => true,
            ResolvedType::RustPath(path) => Self::rust_display_type_var_name(path.trim()).is_some(),
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => Self::contains_unbound_rust_type_var(inner),
            ResolvedType::Generic(_, args) | ResolvedType::Tuple(args) => {
                args.iter().any(Self::contains_unbound_rust_type_var)
            }
            ResolvedType::Function(params, ret) => {
                params
                    .iter()
                    .any(|param| Self::contains_unbound_rust_type_var(&param.ty))
                    || Self::contains_unbound_rust_type_var(ret)
            }
            _ => false,
        }
    }

    /// Refuse a method by its requested public path when admitted facts contain no callable signature.
    pub(super) fn refuse_missing_rust_method_signature(
        &mut self,
        rust_path: &str,
        method: &str,
        span: Span,
    ) -> ResolvedType {
        self.errors.push(CompileError::type_error(
            format!("Rust method `rust::{rust_path}::{method}` has no admitted callable signature"),
            span,
        ));
        ResolvedType::Unknown
    }

    /// Select the imported Rust extension trait a method call reaches when receiver metadata proves a unique match.
    ///
    /// The typechecker has both receiver metadata and import metadata, so it is the narrowest layer that can select the
    /// import without falling back to method-name heuristics.
    pub(super) fn rust_extension_trait_import_for_call(
        &self,
        receiver_metadata: &incan_lang::interop::RustItemMetadata,
        method: &str,
    ) -> Option<RustMethodTraitImportUse> {
        let RustItemKind::Type(type_info) = &receiver_metadata.kind else {
            return None;
        };
        let matches = self
            .type_info
            .rust
            .trait_imports
            .iter()
            .filter_map(|(binding, import)| {
                Self::rust_trait_import_matches_receiver(type_info, import, method).map(|signature| {
                    RustMethodTraitImportUse {
                        binding: binding.clone(),
                        trait_path: import.trait_path.clone(),
                        method: method.to_string(),
                        signature,
                    }
                })
            })
            .collect::<Vec<_>>();
        let [import_use] = matches.as_slice() else {
            return None;
        };
        Some(import_use.clone())
    }

    /// Select a unique imported Rust trait method when receiver metadata cannot prove one.
    ///
    /// Rust metadata can omit generated or re-export-heavy concrete types, or carry receiver metadata without a full
    /// implemented-trait list, while still extracting the imported trait. In that case a unique imported trait is
    /// enough for Rust method lookup; a recovered signature also preserves call-site parameter shape metadata.
    pub(super) fn unique_rust_trait_import_for_method_call(&self, method: &str) -> Option<RustMethodTraitImportUse> {
        let matches = self
            .type_info
            .rust
            .trait_imports
            .iter()
            .filter(|(_, import)| import.methods.contains(method))
            .map(|(binding, import)| RustMethodTraitImportUse {
                binding: binding.clone(),
                trait_path: import.trait_path.clone(),
                method: method.to_string(),
                signature: Self::rust_trait_method_signature(import, method),
            })
            .collect::<Vec<_>>();
        let [import_use] = matches.as_slice() else {
            return None;
        };
        Some(import_use.clone())
    }

    /// Return the trait method signature when `import` is implemented by `type_info` and declares `method`.
    pub(super) fn rust_trait_import_matches_receiver(
        type_info: &incan_lang::interop::RustTypeInfo,
        import: &RustTraitImportInfo,
        method: &str,
    ) -> Option<Option<incan_lang::interop::RustFunctionSig>> {
        if !import.methods.contains(method) {
            return None;
        }
        if !type_info.metadata_completeness.has_trait_impls() {
            return None;
        }
        let trait_suffix = Self::rust_trait_path_suffix(&import.trait_path);
        let implemented = type_info.implemented_traits.iter().any(|implemented| {
            implemented.is_unconditional()
                && (implemented.path == import.trait_path
                    || Some(implemented.path.as_str()) == import.definition_path.as_deref()
                    || Self::rust_trait_path_suffix(&implemented.path) == trait_suffix)
        });
        implemented.then(|| Self::rust_trait_method_signature(import, method))
    }

    /// Return the imported trait method signature when metadata supplied it.
    pub(super) fn rust_trait_method_signature(
        import: &RustTraitImportInfo,
        method: &str,
    ) -> Option<incan_lang::interop::RustFunctionSig> {
        import.method_signatures.get(method).cloned()
    }
}
