//! Dispatch native methods through admitted metadata and retain one specialized callable contract.

use std::collections::HashMap;

use super::{RustPathMethodCall, RustTraitMethodCall, RustTraitQualifiedCall, TypeChecker};
use crate::diagnostics::errors;
use crate::symbols::ResolvedType;
use crate::typechecker::helpers::render_resolved_type_as_rust_arg;
use crate::typechecker::{IdentKind, OpenRustGenericCall, RustArgCoercionKind};
use incan_lang::interop::{
    RustCollectionFamily, RustFunctionSig, RustItemKind, RustItemMetadata, RustMethodSig, RustTypeInfo,
};

/// The signature and owner context selected before validating an inherent method's written arguments.
struct InherentSpecialization {
    /// Native signature after explicit, contextual and receiver owner arguments are applied.
    signature: RustFunctionSig,
    /// Expected native owner that closes a `Self` result.
    contextual_receiver: Option<ResolvedType>,
    /// Explicit or inferred owner/method arguments in declaration order.
    arguments: Vec<ResolvedType>,
    /// Whether arguments specialize an associated function's enclosing owner.
    specializes_owner: bool,
}

impl TypeChecker {
    /// Dispatch one native method request without reconstructing receiver or declaration facts.
    pub(super) fn resolve_rust_path_method_call(&mut self, call: RustPathMethodCall<'_>) -> Option<ResolvedType> {
        let preserves_shape = RustCollectionFamily::for_canonical_path(call.rust_path)
            .is_some_and(|family| family.preserves_lookup_arg_shape(call.method));
        if preserves_shape {
            self.type_info
                .record_regular_method_arg_shape(call.receiver_span, call.method);
        }
        let Some(metadata) =
            self.rust_metadata_for_receiver_method(call.rust_path, call.method, call.receiver_metadata)
        else {
            return Some(self.resolve_missing_native_method(call, None, preserves_shape));
        };
        match &metadata.kind {
            RustItemKind::Type(owner) => self.resolve_native_type_method(call, &metadata, owner, preserves_shape),
            RustItemKind::Module(_) => self.resolve_native_module_function(call),
            RustItemKind::Trait(trait_info) if call.receiver_binding.is_some() => {
                Some(self.resolve_rust_trait_qualified_call(RustTraitQualifiedCall {
                    rust_path: call.rust_path,
                    trait_info,
                    method: call.method,
                    type_args: call.type_args,
                    args: call.args,
                    arg_types: call.arg_types,
                    preserves_lookup_arg_shape: preserves_shape,
                    span: call.span,
                }))
            }
            RustItemKind::Unsupported { description } => {
                self.errors.push(errors::rust_item_shape_not_supported(
                    call.rust_path,
                    description,
                    call.span,
                ));
                Some(ResolvedType::Unknown)
            }
            _ => {
                self.preserve_unresolved_rust_call_argument_returns(call.args);
                Some(ResolvedType::Unknown)
            }
        }
    }

    /// Try bounded traits and dereference adjustments in their existing priority before inherent admission.
    fn resolve_native_type_method(
        &mut self,
        call: RustPathMethodCall<'_>,
        metadata: &RustItemMetadata,
        owner: &RustTypeInfo,
        preserves_shape: bool,
    ) -> Option<ResolvedType> {
        let receiver_is_type = self.native_receiver_is_type(&call);
        let selected = owner.methods.iter().find(|candidate| {
            candidate.name == call.method
                && (receiver_is_type || Self::rust_signature_has_receiver(&candidate.signature))
        });
        if selected.is_none()
            && let Some(result) = self.resolve_retained_rust_trait_call(call, owner)
        {
            return Some(result);
        }
        if !owner
            .methods
            .iter()
            .any(|candidate| candidate.name == call.method && Self::rust_signature_has_receiver(&candidate.signature))
            && let Some(result) = self.resolve_rust_autoderef_call(call, owner)
        {
            return Some(result);
        }
        let Some(selected) = selected else {
            return Some(self.resolve_missing_native_method(call, Some(metadata), preserves_shape));
        };
        self.check_native_inherent_method(call, owner, selected, receiver_is_type, preserves_shape)
    }

    /// Validate an imported extension signature or refuse the requested public item by name.
    /// A trait-qualified receiver without metadata cannot authorize an assumed receiver borrow mode.
    fn resolve_missing_native_method(
        &mut self,
        call: RustPathMethodCall<'_>,
        metadata: Option<&RustItemMetadata>,
        preserves_shape: bool,
    ) -> ResolvedType {
        let claimed = metadata
            .and_then(|metadata| self.record_rust_extension_trait_import_for_call(metadata, call.method, call.span))
            .or_else(|| self.record_unique_rust_trait_import_for_method_call(call.method, call.span));
        if let Some(signature) = claimed.as_ref().and_then(|import| import.signature.as_ref()) {
            return self.validate_rust_trait_method_call(RustTraitMethodCall {
                rust_path: call.rust_path,
                method: call.method,
                sig: signature,
                type_args: call.type_args,
                args: call.args,
                arg_types: call.arg_types,
                preserves_lookup_arg_shape: preserves_shape,
                self_type: None,
                span: call.span,
            });
        }
        if metadata.is_none() && self.receiver_names_rust_trait_import(call.receiver_binding, call.method) {
            self.errors.push(errors::rust_trait_receiver_mode_unavailable(
                call.rust_path,
                call.method,
                call.span,
            ));
            self.preserve_unresolved_rust_call_argument_returns(call.args);
            return ResolvedType::Unknown;
        }
        if claimed.is_none() {
            self.record_rust_method_trait_import_candidates(call.rust_path, call.receiver_binding, call.span);
        }
        self.preserve_unresolved_rust_call_argument_returns(call.args);
        self.refuse_missing_rust_method_signature(call.rust_path, call.method, call.span)
    }

    /// Validate a free function selected as a public child of a Rust module through ordinary function checking.
    fn resolve_native_module_function(&mut self, call: RustPathMethodCall<'_>) -> Option<ResolvedType> {
        let path = format!("{}::{}", call.rust_path, call.method);
        let selected = self.rust_item_metadata_for_path(&path)?;
        let RustItemKind::Function(signature) = selected.kind else {
            return None;
        };
        if !call.type_args.is_empty() {
            self.errors
                .push(errors::explicit_call_site_type_args_not_supported(call.span));
            return Some(ResolvedType::Unknown);
        }
        Some(self.validate_rust_function_call_with_expected(
            &path,
            &signature,
            call.args,
            call.span,
            call.expected_return_ty,
        ))
    }

    /// Distinguish associated-function syntax from a checked value receiver using the retained identifier role.
    fn native_receiver_is_type(&self, call: &RustPathMethodCall<'_>) -> bool {
        matches!(
            self.type_info.ident_kind(call.receiver_span),
            Some(IdentKind::TypeName | IdentKind::RustImport)
        )
    }

    /// Check receiver mutability, specialize the selected signature, validate arguments and retain that same callee.
    fn check_native_inherent_method(
        &mut self,
        call: RustPathMethodCall<'_>,
        owner: &RustTypeInfo,
        selected: &RustMethodSig,
        receiver_is_type: bool,
        preserves_shape: bool,
    ) -> Option<ResolvedType> {
        // ---- Receiver contract ----
        let signature = &selected.signature;
        if !receiver_is_type
            && Self::rust_signature_has_receiver(signature)
            && signature.params.first().is_some_and(|parameter| {
                Self::rust_display_borrow_kind(&parameter.type_display).is_some_and(|(mutable, _)| mutable)
            })
        {
            self.refuse_write_through_read_only_binding(call.receiver_expr, call.span);
            self.validate_rust_borrow_mutability(RustArgCoercionKind::Borrow { mutable: true }, call.receiver_expr);
        }
        let Some(specialized) = self.specialize_native_method(&call, owner, signature, receiver_is_type) else {
            return Some(ResolvedType::Unknown);
        };

        // ---- Written arguments and contextual Self ----
        if Self::rust_signature_has_receiver(signature)
            && signature.params[1..].iter().any(|parameter| {
                Self::rust_display_type_var_name(&Self::compact_rust_display(&parameter.type_display)).is_some()
            })
        {
            self.type_info
                .record_regular_method_arg_shape(call.receiver_span, call.method);
        }
        let display = format!("rust::{}.{}", call.rust_path, call.method);
        let result = self.validate_rust_method_call(
            &display,
            &specialized.signature,
            call.args,
            call.arg_types,
            preserves_shape,
            call.span,
        );
        if specialized.specializes_owner && !specialized.arguments.is_empty() {
            let parameters = if Self::rust_signature_has_receiver(&specialized.signature) {
                &specialized.signature.params[1..]
            } else {
                &specialized.signature.params
            };
            self.record_rust_call_site_params(call.span, parameters, &display);
        }
        let result = match specialized.contextual_receiver.as_ref() {
            Some(receiver) => Self::substitute_rust_self_type_with_resolved(result, receiver),
            None => Self::substitute_rust_self_type(result, call.rust_path),
        };
        // ---- Retained native callee ----
        let receiver = (!receiver_is_type && Self::rust_signature_has_receiver(signature))
            .then(|| {
                self.rust_native_value_type(call.receiver_expr)
                    .or_else(|| self.type_info.expr_type(call.receiver_span).cloned())
            })
            .flatten()
            .map(|receiver| self.rust_closed_owner(&receiver, owner, call.rust_path));
        self.retain_checked_rust_method(
            selected,
            owner,
            call.rust_path,
            call.args,
            call.arg_types,
            receiver.as_ref(),
            &result,
            call.span,
        );
        Some(result)
    }

    /// Apply one owner/method instantiation to the signature before its arguments and native record are checked.
    fn specialize_native_method(
        &mut self,
        call: &RustPathMethodCall<'_>,
        owner: &RustTypeInfo,
        signature: &RustFunctionSig,
        receiver_is_type: bool,
    ) -> Option<InherentSpecialization> {
        let specializes_owner = receiver_is_type && !owner.type_params.is_empty();
        let (arguments, contextual_receiver) =
            self.native_method_arguments(call, owner, signature, specializes_owner)?;
        if specializes_owner && arguments.is_empty() {
            self.record_open_native_owner(call, owner, signature);
        }
        let effective = if specializes_owner && !arguments.is_empty() {
            self.specialize_rust_owner_signature(signature, call.rust_path, &owner.type_params, &arguments)
        } else if !arguments.is_empty() {
            self.specialize_rust_method_signature(signature, call.rust_path, &arguments)
        } else {
            signature.clone()
        };
        let effective = if !receiver_is_type {
            self.rust_native_value_type(call.receiver_expr)
                .or_else(|| self.type_info.expr_type(call.receiver_span).cloned())
                .and_then(|receiver| self.rust_owner_type_args(&receiver, call.rust_path))
                .and_then(|mut arguments| {
                    self.complete_rust_owner_arguments(owner, &mut arguments)
                        .then_some(arguments)
                })
                .map(|arguments| {
                    self.specialize_rust_owner_signature(&effective, call.rust_path, &owner.type_params, &arguments)
                })
                .unwrap_or(effective)
        } else {
            effective
        };
        if !arguments.is_empty() {
            self.type_info
                .calls
                .call_site_monomorph_type_args
                .insert((call.span.start, call.span.end), arguments.clone());
        }
        Some(InherentSpecialization {
            signature: effective,
            contextual_receiver,
            arguments,
            specializes_owner,
        })
    }

    /// Resolve explicit and expected owner arguments, then infer any remaining arguments from the selected signature.
    fn native_method_arguments(
        &mut self,
        call: &RustPathMethodCall<'_>,
        owner: &RustTypeInfo,
        signature: &RustFunctionSig,
        specializes_owner: bool,
    ) -> Option<(Vec<ResolvedType>, Option<ResolvedType>)> {
        // ---- Written type arguments ----
        let mut arguments = Vec::new();
        if !call.type_args.is_empty() {
            if specializes_owner && owner.has_const_params {
                self.errors.push(errors::rust_receiver_const_generics_not_supported(
                    call.rust_path,
                    call.span,
                ));
                return None;
            }
            let parameters = if specializes_owner {
                &owner.type_params
            } else {
                &signature.type_params
            };
            let defaults_admit = specializes_owner && Self::rust_owner_arity_admits(owner, call.type_args.len());
            if !defaults_admit
                && !self.validate_rust_method_type_arg_arity(call.method, parameters, call.type_args, call.span)
            {
                return None;
            }
            arguments = call.type_args.iter().map(|ty| self.resolve_type_checked(ty)).collect();
            if specializes_owner && !self.complete_rust_owner_arguments(owner, &mut arguments) {
                return None;
            }
            self.type_info
                .calls
                .call_site_monomorph_type_args
                .insert((call.span.start, call.span.end), arguments.clone());
        }
        // ---- Expected owner ----
        let contextual_receiver = if specializes_owner && arguments.is_empty() {
            call.expected_return_ty.and_then(|expected| {
                let display = self.rust_display_for_owner_path(&signature.return_type, call.rust_path);
                let actual = self.resolved_type_from_rust_display(&display);
                Self::contextual_rust_receiver_type(&actual, expected, call.rust_path)
            })
        } else {
            None
        };
        if let Some(receiver) = contextual_receiver.as_ref() {
            if owner.has_const_params {
                self.errors.push(errors::rust_receiver_const_generics_not_supported(
                    call.rust_path,
                    call.span,
                ));
                return None;
            }
            if let Some(mut contextual_args) = self.rust_owner_type_args(receiver, call.rust_path)
                && self.complete_rust_owner_arguments(owner, &mut contextual_args)
            {
                arguments = contextual_args;
            }
        }
        // ---- Value-constrained owner arguments ----
        if specializes_owner && arguments.is_empty() {
            let inferred = self.infer_rust_signature_bindings(
                signature,
                call.rust_path,
                &owner.type_params,
                call.args,
                call.arg_types,
                call.expected_return_ty,
            );
            if let Some(inferred) = owner
                .type_params
                .iter()
                .enumerate()
                .map(|(index, parameter)| {
                    inferred
                        .get(parameter)
                        .filter(|ty| !matches!(ty, ResolvedType::Unknown | ResolvedType::CallSiteInfer))
                        .cloned()
                        .or_else(|| self.native_owner_default(owner, index))
                })
                .collect::<Option<Vec<_>>>()
            {
                arguments = inferred;
            }
        }
        Some((arguments, contextual_receiver))
    }

    /// Retain unresolved associated-owner binders for the enclosing statement's later expected-type reconciliation.
    fn record_open_native_owner(
        &mut self,
        call: &RustPathMethodCall<'_>,
        owner: &RustTypeInfo,
        signature: &RustFunctionSig,
    ) {
        let parameters = Self::open_rust_owner_type_args(signature, owner);
        if parameters.is_empty() {
            return;
        }
        let owner = call
            .receiver_binding
            .map(str::to_owned)
            .unwrap_or_else(|| call.rust_path.rsplit("::").next().unwrap_or(call.rust_path).to_owned());
        self.open_rust_generic_calls.insert(
            (call.span.start, call.span.end),
            OpenRustGenericCall {
                owner,
                method: call.method.to_owned(),
                type_params: parameters,
            },
        );
    }

    /// Read an owner's metadata default through the same conversion for inferred and retained native arguments.
    pub(super) fn native_owner_default(&self, owner: &RustTypeInfo, index: usize) -> Option<ResolvedType> {
        Some(self.resolved_type_from_rust_display(owner.type_param_defaults.get(index)?.as_deref()?))
    }

    /// Retain the selected native result, normalizing associated projections and preserving a proven bytes carrier.
    /// The source bytes carrier and Vec<u8> have the same ABI; retaining bytes avoids changing its indexing contract.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn native_method_result(
        &self,
        method: &RustMethodSig,
        path: &str,
        parameters: &[String],
        bindings: &HashMap<String, ResolvedType>,
        receiver: Option<&ResolvedType>,
        result: &ResolvedType,
    ) -> ResolvedType {
        let mut native = self.instantiate_rust_signature_type(
            &self.native_rust_signature_type(&method.signature.return_type, path),
            parameters,
            bindings,
        );
        if method.signature.return_type.contains(" as ") {
            let displays = bindings
                .iter()
                .map(|(name, value)| (name.as_str(), render_resolved_type_as_rust_arg(value)))
                .collect();
            let owner = receiver
                .map(render_resolved_type_as_rust_arg)
                .unwrap_or_else(|| path.to_owned());
            let display = Self::substitute_rust_owner_display(&method.signature.return_type, &displays, &owner);
            if let Some(projected) = self.rust_projected_type(&display, 0) {
                native = projected;
            }
        }
        if *result == ResolvedType::Bytes && self.rust_trait_carriers_match(&native, result) {
            return ResolvedType::Bytes;
        }
        native
    }
}
