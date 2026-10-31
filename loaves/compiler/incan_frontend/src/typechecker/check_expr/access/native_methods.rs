//! Dispatch native methods through admitted metadata and retain one specialized callable contract.

use std::collections::HashMap;

use super::{RustPathMethodCall, RustTraitMethodCall, RustTraitQualifiedCall, TypeChecker};
use crate::diagnostics::errors;
use crate::symbols::ResolvedType;
use crate::typechecker::helpers::render_resolved_type_as_rust_arg;
use crate::typechecker::{IdentKind, OpenRustGenericCall};
use incan_lang::interop::{
    RustCollectionFamily, RustFunctionSig, RustItemKind, RustItemMetadata, RustMethodSig, RustTypeInfo,
};
use incan_lang::lang::types::numerics::{self, NumericTypeId};

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
    /// Retain an imported native value's inherent length method through ordinary metadata dispatch. Builtin source
    /// collections and text keep their own length contract; native lengths return int or usize. No native identity
    /// or missing method is invented.
    pub(in crate::typechecker) fn retain_native_length_call(
        &mut self,
        receiver: &crate::ast::Spanned<crate::ast::Expr>,
        ty: &ResolvedType,
        span: crate::ast::Span,
    ) -> bool {
        if matches!(ty, ResolvedType::Generic(name, _) if crate::typechecker::helpers::collection_type_id(name).is_some())
            || matches!(ty, ResolvedType::RustPath(path) if self.rust_array_parts(path).is_some())
        {
            return false;
        }
        let Some((path, _, _, _)) = self.rust_identity_for_type(ty) else {
            return false;
        };
        let Some(metadata) = self.rust_item_metadata_for_path(&path) else {
            return false;
        };
        let RustItemKind::Type(info) = &metadata.kind else {
            return false;
        };
        if !info.methods.iter().any(|method| {
            method.name == "len"
                && Self::rust_signature_has_receiver(&method.signature)
                && method.signature.params.len() == 1
        }) {
            return false;
        }
        let previous_errors = self.errors.len();
        let result = self.resolve_rust_path_method_call(RustPathMethodCall {
            rust_path: &path,
            method: "len",
            receiver_metadata: Some(&metadata),
            receiver_binding: None,
            receiver_expr: receiver,
            type_args: &[],
            args: &[],
            arg_types: &[],
            receiver_span: receiver.span,
            expected_return_ty: None,
            span,
        });
        if self.errors.len() != previous_errors {
            return true;
        }
        let Some(call) = self.type_info.rust.checked_calls.get_mut(&span) else {
            return true;
        };
        let shared = matches!(
            call.parameters.first().map(|parameter| &parameter.ty),
            Some(ResolvedType::Ref(_))
        );
        let sized_length = matches!(
            result,
            Some(ResolvedType::Int | ResolvedType::Numeric(NumericTypeId::USize))
        );
        if !shared || !sized_length {
            self.errors.push(errors::rust_item_shape_not_supported(
                &path,
                "length requires a shared `len(&self)` method returning int or usize",
                span,
            ));
            return true;
        }
        call.has_receiver = false;
        if matches!(result, Some(ResolvedType::Numeric(NumericTypeId::USize))) {
            self.type_info
                .expressions
                .numeric_conversion_destinations
                .insert(span, ResolvedType::Int);
        }
        true
    }

    /// Dispatch one native method request without reconstructing receiver or declaration facts.
    pub(super) fn resolve_rust_path_method_call(&mut self, call: RustPathMethodCall<'_>) -> Option<ResolvedType> {
        let preserves_shape = RustCollectionFamily::for_canonical_path(call.rust_path)
            .is_some_and(|family| family.preserves_lookup_arg_shape(call.method));
        let Some(metadata) =
            self.rust_metadata_for_receiver_method(call.rust_path, call.method, call.receiver_metadata)
        else {
            return Some(self.resolve_missing_native_method(call, None, preserves_shape));
        };
        match &metadata.kind {
            RustItemKind::Type(owner) => self.resolve_native_type_method(call, &metadata, owner, preserves_shape),
            RustItemKind::Module(_) => self.resolve_native_module_function(call),
            RustItemKind::Trait(trait_info) if call.receiver_binding.is_none() => self
                .resolve_rust_declared_trait_method(
                    RustPathMethodCall {
                        receiver_metadata: Some(&metadata),
                        ..call
                    },
                    trait_info,
                ),
            RustItemKind::Trait(trait_info) if call.receiver_binding.is_some() => {
                Some(self.resolve_rust_trait_qualified_call(RustTraitQualifiedCall {
                    rust_path: call.rust_path,
                    trait_info,
                    method: call.method,
                    type_args: call.type_args,
                    args: call.args,
                    arg_types: call.arg_types,
                    expected_return_ty: call.expected_return_ty,
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
            // A function or constant item is no method receiver surface: refuse it rather than leave the call untyped.
            RustItemKind::Function(_) | RustItemKind::Constant { .. } => {
                let description = if matches!(metadata.kind, RustItemKind::Function(_)) {
                    "function"
                } else {
                    "constant"
                };
                self.errors.push(errors::rust_item_shape_not_supported(
                    call.rust_path,
                    description,
                    call.span,
                ));
                self.preserve_unresolved_rust_call_argument_returns(call.args);
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
        // ---- Inherent member and receiver role ----
        let receiver_is_type = self.native_receiver_is_type(&call);
        let selected = owner.methods.iter().find(|candidate| {
            candidate.name == call.method
                && (receiver_is_type || Self::rust_signature_has_receiver(&candidate.signature))
        });

        // ---- Trait and dereference alternatives ----
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

        // ---- Written associated-call receiver ----
        let Some(selected) = selected else {
            return Some(self.resolve_missing_native_method(call, Some(metadata), preserves_shape));
        };
        if receiver_is_type && Self::rust_signature_has_receiver(&selected.signature) {
            // Associated-call syntax supplies self as an ordinary written argument. Normalize that slot once so
            // inference, boundary validation and retained native storage all include it.
            let mut explicit = selected.clone();
            let arguments = owner
                .type_params
                .iter()
                .map(|name| ResolvedType::TypeVar(name.clone()))
                .collect::<Vec<_>>();
            explicit.signature = self.specialize_rust_owner_signature(
                &explicit.signature,
                call.rust_path,
                &owner.type_params,
                &arguments,
            );
            if let Some(receiver) = explicit.signature.params.first_mut() {
                let Some(native_owner) = selected.impl_self_type.as_deref() else {
                    return Some(self.refuse_missing_rust_method_signature(call.rust_path, call.method, call.span));
                };
                receiver.type_display = match Self::rust_display_borrow_kind(&receiver.type_display) {
                    Some((true, _)) => format!("&mut {native_owner}"),
                    Some((false, _)) => format!("&{native_owner}"),
                    None => native_owner.to_owned(),
                };
                receiver.name = Some("receiver".into());
            }
            return self.check_native_inherent_method(call, owner, &explicit, true, preserves_shape);
        }
        self.check_native_inherent_method(call, owner, selected, receiver_is_type, preserves_shape)
    }

    /// Validate an imported extension signature or refuse the requested public item by name.
    /// A trait-qualified receiver without metadata cannot authorize an assumed receiver borrow mode.
    /// The imported signature checks the arguments, but selection retained no implementation for this receiver, so a
    /// call the arguments admit is refused with the reason selection recorded: tied candidates, or none.
    fn resolve_missing_native_method(
        &mut self,
        call: RustPathMethodCall<'_>,
        metadata: Option<&RustItemMetadata>,
        preserves_shape: bool,
    ) -> ResolvedType {
        let claimed = metadata
            .and_then(|metadata| self.rust_extension_trait_import_for_call(metadata, call.method))
            .or_else(|| self.unique_rust_trait_import_for_method_call(call.method));
        if let Some(signature) = claimed.as_ref().and_then(|import| import.signature.as_ref()) {
            let errors_before = self.errors.len();
            let result = self.validate_rust_trait_method_call(RustTraitMethodCall {
                rust_path: call.rust_path,
                method: call.method,
                sig: signature,
                type_args: call.type_args,
                args: call.args,
                arg_types: call.arg_types,
                preserves_lookup_arg_shape: preserves_shape,
                span: call.span,
            });
            let tied = self.tied_rust_trait_callees.remove(&call.span);
            if self.errors.len() == errors_before {
                let gap = match tied.as_deref() {
                    Some(candidates) => errors::NativeCalleeGap::Ambiguous(candidates),
                    None => errors::NativeCalleeGap::NoCandidate,
                };
                let callee = format!("rust::{}.{}", call.rust_path, call.method);
                self.errors
                    .push(errors::rust_callee_not_selected(&callee, gap, call.span));
            }
            return result;
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
        self.preserve_unresolved_rust_call_argument_returns(call.args);
        self.refuse_missing_rust_method_signature(call.rust_path, call.method, call.span)
    }

    /// Validate a free function selected as a public child of a Rust module through ordinary function checking.
    fn resolve_native_module_function(&mut self, call: RustPathMethodCall<'_>) -> Option<ResolvedType> {
        let path = format!("{}::{}", call.rust_path, call.method);
        let selected = self.rust_item_metadata_for_path(&path)?;
        let RustItemKind::Function(signature) = &selected.kind else {
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
        if call
            .receiver_metadata
            .is_some_and(|metadata| matches!(metadata.kind, RustItemKind::Constant { .. }))
        {
            return false;
        }
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
        let errors_before = self.errors.len();
        let signature = &selected.signature;
        let passive_read = !receiver_is_type && self.defer_native_method_receiver(&call, signature);
        if !receiver_is_type
            && Self::rust_signature_has_receiver(signature)
            && signature.params.first().is_some_and(|parameter| {
                Self::rust_display_borrow_kind(&parameter.type_display).is_some_and(|(mutable, _)| mutable)
            })
        {
            self.validate_exclusive_rust_receiver(call.receiver_expr, call.span);
        }
        let Some(specialized) = self.specialize_native_method(&call, owner, signature, receiver_is_type) else {
            return Some(ResolvedType::Unknown);
        };

        // ---- Written arguments and contextual Self ----
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
        // An associated call through a public alias keeps that source owner when it returns the alias target.
        // Native storage still freezes the same structural target, while subsequent calls retain the public path
        // that selected its specialized impl rather than the target's unspecialized method inventory.
        let alias = ResolvedType::RustPath(call.rust_path.to_owned());
        let result = if matches!(&result, ResolvedType::RustPath(_) | ResolvedType::Generic(_, _))
            && owner.alias_target.is_some()
            && self.rust_trait_carriers_match(&result, &alias)
        {
            alias
        } else {
            result
        };
        // ---- Retained native callee ----
        let receiver = (!receiver_is_type && Self::rust_signature_has_receiver(signature))
            .then(|| {
                self.rust_native_value_type(call.receiver_expr)
                    .or_else(|| self.type_info.expr_type(call.receiver_span).cloned())
            })
            .flatten()
            .map(|receiver| self.rusttype_native_target(&receiver).unwrap_or(receiver))
            .map(|receiver| self.rust_closed_owner(&receiver, owner, call.rust_path))
            .map(|receiver| self.primitive_receiver(receiver, owner));
        let retained = self.retain_checked_rust_method(
            selected,
            if specialized.specializes_owner {
                &[]
            } else {
                &signature.type_params
            },
            owner,
            call.rust_path,
            call.args,
            call.arg_types,
            receiver.as_ref(),
            &result,
            call.span,
        );
        match retained {
            // The retained receiver can fix a watched local's open owner arguments (#2481).
            Ok(()) if !receiver_is_type => {
                self.fix_open_rust_binding_by_receiver(call.receiver_expr, owner, call.rust_path, call.span)
            }
            Err(refusal) if !passive_read && self.errors.len() == errors_before => self.errors.push(refusal),
            _ => {}
        }
        let result = self
            .type_info
            .rust
            .checked_calls
            .get(&call.span)
            .map(|checked| self.rust_source_result_type(&checked.return_type, &result))
            .unwrap_or(result);
        if receiver_is_type
            && signature.return_type.trim() == "Self"
            && let Some(rusttype) = self.rusttype_associated_receiver(&call)
        {
            return Some(rusttype);
        }
        Some(result)
    }

    /// Keep the source operation for a native receiver read whose own checking needs no open owner arguments.
    /// Rechecking this expression after a fixing use retains the callee with its final native layout.
    fn defer_native_method_receiver(&mut self, call: &RustPathMethodCall<'_>, signature: &RustFunctionSig) -> bool {
        let expression = crate::ast::Spanned::new(
            crate::ast::Expr::MethodCall(
                Box::new(call.receiver_expr.clone()),
                call.method.to_owned(),
                call.type_args.to_vec(),
                call.args.to_vec(),
            ),
            call.span,
        );
        self.defer_open_rust_receiver_read(call.receiver_expr, signature, expression)
    }

    /// Normalize transparent scalar reexports without flattening instantiated native owners to source collections.
    fn primitive_receiver(&self, receiver: ResolvedType, owner: &RustTypeInfo) -> ResolvedType {
        if let ResolvedType::RustPath(path) = &receiver
            && owner.alias_target.as_deref().and_then(numerics::from_str).is_some()
        {
            return self.resolved_type_from_rust_display(path);
        }
        receiver
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
                .insert(call.span, arguments.clone());
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
                .insert(call.span, arguments.clone());
        }
        // ---- Expected owner ----
        // An expectation that still names the enclosing callee's open binder fixes no owner argument.
        let contextual_receiver = if specializes_owner && arguments.is_empty() {
            call.expected_return_ty
                .filter(|expected| !Self::contains_unbound_rust_type_var(expected))
                .and_then(|expected| {
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
            let closed = |parameter: &str| {
                inferred
                    .get(parameter)
                    .filter(|ty| !matches!(ty, ResolvedType::Unknown | ResolvedType::CallSiteInfer))
                    .cloned()
            };
            if let Some(inferred) = self
                .owner_arguments_with_defaults(owner, closed, call.rust_path)
                .into_iter()
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
            call.span,
            OpenRustGenericCall {
                owner,
                method: call.method.to_owned(),
                type_params: parameters,
                owner_parameters: true,
                native: None,
                native_item: None,
            },
        );
    }

    /// Return the owner's type arguments in declaration order: each one `closed` gives, else its declared default
    /// (`owner_default`), which sees the arguments before it. An argument neither gives stays `None`, and a later
    /// default then sees only the arguments before that gap.
    pub(super) fn owner_arguments_with_defaults(
        &self,
        owner: &RustTypeInfo,
        closed: impl Fn(&str) -> Option<ResolvedType>,
        anchor: &str,
    ) -> Vec<Option<ResolvedType>> {
        let mut arguments: Vec<Option<ResolvedType>> = Vec::with_capacity(owner.type_params.len());
        for (index, parameter) in owner.type_params.iter().enumerate() {
            let argument = closed(parameter).or_else(|| {
                let earlier = arguments.iter().map_while(Option::clone).collect::<Vec<_>>();
                self.owner_default(owner, index, &earlier, anchor)
            });
            arguments.push(argument);
        }
        arguments
    }

    /// Retain the selected native result and normalize associated projections. Source projection stays separate:
    /// borrowed byte slices keep their native reference even when the source-facing result is bytes.
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
        if crate::trace_flags::trace_rust_facts() {
            eprintln!(
                "rust result: {path}.{} {} => {native:?}, source {result:?}",
                method.name, method.signature.return_type
            );
        }
        native
    }
}
