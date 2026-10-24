//! Checked native autodereference: retain every adjustment as an ordinary external trait call.

use super::{RustPathMethodCall, RustTraitQualifiedCall, TypeChecker};
use crate::ast::{Expr, Span, Spanned};
use crate::symbols::{CallableParam, ResolvedType};
use crate::typechecker::helpers::render_resolved_type_as_rust_arg;
use crate::typechecker::type_info::RustCheckedCall;
use crate::typechecker::{IdentKind, RustArgCoercionKind};
use incan_lang::interop::{RustFunctionSig, RustImplementedTrait, RustItemKind, RustTypeInfo, ancestral_rust_path};
use incan_lang::lang::types::collections::{CollectionTypeId, from_str as collection_type};

const DEREF_TRAIT: &str = "std::ops::Deref";
const DEREF_MUT_TRAIT: &str = "std::ops::DerefMut";

impl TypeChecker {
    /// Resolve a public field through metadata-selected shared Deref targets. Retain the same field identity and
    /// native payload as a direct read, plus every executable adjustment; no target is inferred from a field name.
    pub(super) fn resolve_rust_deref_field(
        &mut self,
        base: &Spanned<Expr>,
        source: &ResolvedType,
        field: &str,
        span: Span,
    ) -> Option<ResolvedType> {
        let mut owner = self.rust_native_value_type(base).unwrap_or_else(|| source.clone());
        let mut steps = Vec::new();
        for _ in 0..12 {
            let path = self.rust_canonical_path_for_receiver_type(&owner)?;
            let metadata = self.rust_item_metadata_for_path(&path)?;
            let RustItemKind::Type(info) = &metadata.kind else {
                return None;
            };
            if Self::rust_field_for_source_name(&info.fields, field).is_some() {
                if steps.is_empty() {
                    return None;
                }
                let result = self.rust_path_field_type(&render_resolved_type_as_rust_arg(&owner), field, span)?;
                self.type_info
                    .rust
                    .field_adjustments
                    .insert((span.start, span.end), steps);
                return Some(result);
            }
            owner = self.rust_closed_owner(&owner, info, &path);
            let target = self.rust_associated_type(&owner, DEREF_TRAIT, "Target")?;
            if target == owner {
                return None;
            }
            steps.push(self.rust_deref_adjustment(&owner, &target, false)?);
            owner = target;
        }
        None
    }

    /// Retain native dereferences when their metadata-selected target satisfies a source parameter. Shared parameters
    /// keep the loan; owned parameters require the target's own Clone implementation so the caller retains its owner.
    /// These adjustments apply only at a call boundary, never to assignment compatibility.
    pub(in crate::typechecker) fn retain_argument_dereferences(
        &mut self,
        actual: &ResolvedType,
        expected: &ResolvedType,
        span: Span,
    ) -> Option<()> {
        // ---- Parameter boundary ----
        let (expected, owned) = match expected {
            ResolvedType::Ref(inner) => (inner.as_ref(), false),
            ResolvedType::RefMut(_) => return None,
            expected => (expected, true),
        };
        if owned && self.types_compatible(actual, expected) {
            return None;
        }
        let mut owner = self.rust_inference_type(actual, &[]);
        if owned {
            self.rust_canonical_path_for_receiver_type(&owner)?;
        }

        // ---- Metadata-selected adjustments ----
        let mut steps = Vec::new();
        for _ in 0..12 {
            let target = self.rust_associated_type(&owner, DEREF_TRAIT, "Target")?;
            if target == owner {
                return None;
            }
            steps.push(self.rust_deref_adjustment(&owner, &target, false)?);
            if self.rust_trait_carriers_match(&target, expected) {
                if owned {
                    steps.push(self.rust_clone_adjustment(&target)?);
                }
                self.type_info
                    .rust
                    .argument_adjustments
                    .insert((span.start, span.end), steps);
                return Some(());
            }
            owner = target;
        }
        None
    }

    /// Select only an inspected slice inherent method after the native array-to-slice receiver adjustment. Array
    /// trait methods do not enter this fallback: borrowing a slice must never replace an array's consuming impl.
    pub(super) fn resolve_rust_array_slice_method(&mut self, call: RustPathMethodCall<'_>) -> Option<ResolvedType> {
        if !call.type_args.is_empty() {
            return None;
        }
        let source = self
            .rust_native_value_type(call.receiver_expr)
            .or_else(|| self.type_info.expr_type(call.receiver_span).cloned())?;
        let referent = match &source {
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => inner.as_ref(),
            other => other,
        };
        let ResolvedType::RustPath(path) = referent else {
            return None;
        };
        let (element, _) = self.rust_array_parts(path)?;
        let metadata = self.rust_item_metadata_for_path("[]")?;
        let RustItemKind::Type(info) = &metadata.kind else {
            return None;
        };
        let method = info
            .methods
            .iter()
            .find(|method| method.name == call.method && Self::rust_signature_has_receiver(&method.signature))?;
        let owner = ResolvedType::Generic("[]".into(), vec![element.clone()]);
        let signature = self.specialize_rust_owner_signature(&method.signature, "[]", &info.type_params, &[element]);
        let mutable = signature.params.first().is_some_and(|parameter| {
            Self::rust_display_borrow_kind(&parameter.type_display).is_some_and(|(mutable, _)| mutable)
        });
        if mutable && matches!(source, ResolvedType::Ref(_)) {
            self.errors.push(crate::diagnostics::errors::type_mismatch(
                &format!("&mut {path}"),
                &source.to_string(),
                call.receiver_span,
            ));
            return Some(ResolvedType::Unknown);
        }
        if mutable {
            self.refuse_write_through_read_only_binding(call.receiver_expr, call.span);
            self.validate_rust_borrow_mutability(RustArgCoercionKind::Borrow { mutable: true }, call.receiver_expr);
        }
        let errors_before = self.errors.len();
        let result = self.validate_rust_method_call(
            &format!("[].{}", call.method),
            &signature,
            call.args,
            call.arg_types,
            false,
            call.span,
        );
        if self.errors.len() == errors_before {
            self.retain_checked_rust_method(
                method,
                info,
                "[]",
                call.args,
                call.arg_types,
                Some(&owner),
                &result,
                call.span,
            );
        }
        Some(result)
    }

    /// Materialize an owned dereference target through its admitted Clone implementation. The exact trait call is
    /// retained for Body IR and rustc's predicate validation; the wrapper's Clone implementation is never selected.
    fn rust_clone_adjustment(&self, target: &ResolvedType) -> Option<RustCheckedCall> {
        let path = self.rust_canonical_path_for_receiver_type(target)?;
        let metadata = self.rust_item_metadata_for_complete_type(&path)?;
        let RustItemKind::Type(info) = &metadata.kind else {
            return None;
        };
        let owner = self.rust_closed_owner(target, info, &path);
        let implementation = self.rust_adjustment_implementation(info, &owner, "std::clone::Clone")?;
        let owner = self.rust_storage_owner(&owner, info, &path);
        Some(RustCheckedCall {
            storage: None,
            source_type_parameters: self.native_call_type_parameters(),
            receiver_adjustments: Vec::new(),
            path: format!("{}::clone", implementation.path),
            type_arguments: vec![owner.clone()],
            parameters: vec![CallableParam::positional(ResolvedType::Ref(Box::new(owner.clone())))],
            return_type: owner,
            result_item_path: Some(path),
            has_receiver: true,
        })
    }

    /// Select an adjustment trait by its admitted defining identity, including standard-library reexports. The
    /// closed owner must satisfy the impl's Self template before a call can be retained.
    fn rust_adjustment_implementation<'a>(
        &self,
        info: &'a RustTypeInfo,
        owner: &ResolvedType,
        trait_path: &str,
    ) -> Option<&'a RustImplementedTrait> {
        let requested = self
            .rust_definition_for_path(trait_path)
            .unwrap_or_else(|| trait_path.to_owned());
        info.implemented_traits.iter().find(|implementation| {
            let identity = self
                .rust_definition_for_path(&implementation.path)
                .unwrap_or_else(|| implementation.path.clone());
            ancestral_rust_path(&identity) == ancestral_rust_path(&requested)
                && self.rust_impl_self_bindings(implementation, owner).is_some()
        })
    }

    /// Search instantiated Deref targets for an inherent or imported trait instance method. Associated functions do not
    /// participate in value lookup, and an adjustment chain remains explicit through Body IR rather than changing
    /// receiver types.
    pub(super) fn resolve_rust_autoderef_call(
        &mut self,
        call: RustPathMethodCall<'_>,
        info: &RustTypeInfo,
    ) -> Option<ResolvedType> {
        if matches!(
            self.type_info.ident_kind(call.receiver_span),
            Some(IdentKind::TypeName | IdentKind::RustImport)
        ) {
            return None;
        }
        let source = self.type_info.expr_type(call.receiver_span)?.clone();
        // A rusttype receiver dereferences through the native type it names.
        let source = self.rusttype_native_target(&source).unwrap_or(source);
        let mut owner = self.rust_closed_owner(&source, info, call.rust_path);
        let mut steps = Vec::new();
        for _ in 0..12 {
            let target = self.rust_associated_type(&owner, DEREF_TRAIT, "Target")?;
            if target == owner {
                return None;
            }
            steps.push((owner, target.clone()));
            let path = self.rust_adjusted_owner_path(&target)?;
            let metadata = self.rust_item_metadata_for_path(&path)?;
            let RustItemKind::Type(info) = &metadata.kind else {
                return None;
            };
            owner = self.rust_closed_owner(&target, info, &path);
            let Some(method) = info
                .methods
                .iter()
                .find(|method| method.name == call.method && Self::rust_signature_has_receiver(&method.signature))
            else {
                let result = self.resolve_retained_rust_trait_call_on_owner(
                    RustPathMethodCall {
                        rust_path: &path,
                        receiver_metadata: Some(&metadata),
                        ..call
                    },
                    info,
                    &owner,
                    true,
                    None,
                );
                if let Some(result) = result {
                    self.retain_rust_deref_steps(call.span, &steps)?;
                    return Some(result);
                }
                continue;
            };
            let mutable = method.signature.params.first().is_some_and(|parameter| {
                Self::rust_display_borrow_kind(&parameter.type_display).is_some_and(|(mutable, _)| mutable)
            });
            let adjustments = steps
                .iter()
                .map(|(owner, target)| self.rust_deref_adjustment(owner, target, mutable))
                .collect::<Option<Vec<_>>>()?;
            let arguments = self.rust_owner_type_args(&owner, &path).unwrap_or_default();
            let signature =
                self.specialize_rust_owner_signature(&method.signature, &path, &info.type_params, &arguments);
            let Some(signature) = self.specialize_written_method_arguments(&call, &path, signature) else {
                return Some(ResolvedType::Unknown);
            };
            if mutable {
                self.refuse_write_through_read_only_binding(call.receiver_expr, call.span);
                self.validate_rust_borrow_mutability(RustArgCoercionKind::Borrow { mutable: true }, call.receiver_expr);
            }
            let errors_before = self.errors.len();
            let result = self.validate_rust_method_call_with_expected(
                &format!("{path}.{}", call.method),
                &signature,
                call.args,
                call.arg_types,
                false,
                call.span,
                call.expected_return_ty,
            );
            if self.errors.len() != errors_before {
                return Some(result);
            }
            self.retain_checked_rust_method(
                method,
                info,
                &path,
                call.args,
                call.arg_types,
                Some(&owner),
                &result,
                call.span,
            );
            if let Some(retained) = self
                .type_info
                .rust
                .checked_calls
                .get_mut(&(call.span.start, call.span.end))
            {
                retained.receiver_adjustments = adjustments;
            }
            return Some(result);
        }
        None
    }

    /// Close a dereference target's method binders with the type arguments the call writes (`parse[i64]()`), and
    /// retain them as the call site's instantiation. A call that writes none keeps its signature; wrong arity is
    /// refused here.
    fn specialize_written_method_arguments(
        &mut self,
        call: &RustPathMethodCall<'_>,
        path: &str,
        signature: RustFunctionSig,
    ) -> Option<RustFunctionSig> {
        if call.type_args.is_empty() {
            return Some(signature);
        }
        if !self.validate_rust_method_type_arg_arity(call.method, &signature.type_params, call.type_args, call.span) {
            return None;
        }
        let arguments = call
            .type_args
            .iter()
            .map(|ty| self.resolve_type_checked(ty))
            .collect::<Vec<_>>();
        let specialized = self.specialize_rust_method_signature(&signature, path, &arguments);
        self.type_info
            .calls
            .call_site_monomorph_type_args
            .insert((call.span.start, call.span.end), arguments);
        Some(specialized)
    }

    /// Resolve an explicit trait receiver through the same native owner selection and checked dereference calls as
    /// value syntax. The trait's name constrains lookup; source argument zero remains the evaluated receiver, whose
    /// retained native carrier takes priority over source result expectations. The destination constrains trait
    /// binders.
    pub(super) fn retain_rust_trait_qualified_call(
        &mut self,
        call: &RustTraitQualifiedCall<'_>,
        receiver: &Spanned<Expr>,
        source: &ResolvedType,
    ) -> Option<ResolvedType> {
        // A bound that fixes the trait arguments keeps its retained substitution; a bound whose method has its own
        // type parameters falls through to the declared-trait selection.
        if let Some(result) = self.retain_bound_rust_trait_call(call, receiver, source) {
            return Some(result);
        }
        if let Some(result) = self.retain_rust_parameter_trait_call(call, receiver, source) {
            self.retain_explicit_rust_receiver(call.span)?;
            return Some(result);
        }
        let mut owner = self
            .type_info
            .rust
            .checked_calls
            .get(&(receiver.span.start, receiver.span.end))
            .map(|checked| checked.return_type.clone())
            .or_else(|| self.rust_native_value_type(receiver))
            .unwrap_or_else(|| source.clone());
        let mut steps = Vec::new();
        for _ in 0..12 {
            let path = self.rust_adjusted_owner_path(&owner)?;
            let metadata = self.rust_item_metadata_for_path(&path)?;
            let RustItemKind::Type(info) = &metadata.kind else {
                return None;
            };
            owner = self.rust_trait_owner(&owner, info, &path);
            let selected = self.resolve_retained_rust_trait_call_on_owner(
                RustPathMethodCall {
                    rust_path: &path,
                    method: call.method,
                    receiver_metadata: Some(&metadata),
                    receiver_binding: None,
                    receiver_expr: receiver,
                    type_args: call.type_args,
                    args: &call.args[1..],
                    arg_types: &call.arg_types[1..],
                    receiver_span: receiver.span,
                    expected_return_ty: call.expected_return_ty,
                    span: call.span,
                },
                info,
                &owner,
                true,
                Some(call.rust_path),
            );
            if let Some(result) = selected {
                self.retain_rust_deref_steps(call.span, &steps)?;
                self.retain_explicit_rust_receiver(call.span)?;
                return Some(result);
            }
            let target = self.rust_associated_type(&owner, DEREF_TRAIT, "Target")?;
            if target == owner {
                return None;
            }
            steps.push((owner, target.clone()));
            owner = target;
        }
        None
    }

    /// Keep the source receiver in declared argument slot zero for explicit `Trait.method(receiver, ...)` syntax.
    /// Native method selection already supplies that parameter's checked borrow and exact carrier.
    fn retain_explicit_rust_receiver(&mut self, span: Span) -> Option<()> {
        let retained = self.type_info.rust.checked_calls.get_mut(&(span.start, span.end))?;
        retained.has_receiver = false;
        let parameters = retained.parameters.clone();
        self.type_info.record_call_site_callable_params_exact(span, &parameters);
        Some(())
    }

    /// Recover an already selected native dereference or trait owner, including source collection and Option
    /// carriers. Source list storage is admitted only at this native boundary, never as ordinary method priority.
    fn rust_adjusted_owner_path(&self, owner: &ResolvedType) -> Option<String> {
        if let Some(path) = self.rust_canonical_path_for_receiver_type(owner) {
            return Some(path);
        }
        match owner {
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => self.rust_adjusted_owner_path(inner),
            ResolvedType::Generic(base, _) if collection_type(base) == Some(CollectionTypeId::List) => {
                Some("std::vec::Vec".into())
            }
            ResolvedType::Bytes => Some("std::vec::Vec".into()),
            _ => self.rust_result_item_path(&render_resolved_type_as_rust_arg(owner), ""),
        }
    }

    /// Attach each selected dereference with the final callee's borrow mode; never repeat method lookup in Body IR.
    fn retain_rust_deref_steps(&mut self, span: Span, steps: &[(ResolvedType, ResolvedType)]) -> Option<()> {
        let retained = self.type_info.rust.checked_calls.get(&(span.start, span.end))?;
        let mutable = matches!(retained.parameters.first()?.ty, ResolvedType::RefMut(_));
        let adjustments = steps
            .iter()
            .map(|(owner, target)| self.rust_deref_adjustment(owner, target, mutable))
            .collect::<Option<Vec<_>>>()?;
        self.type_info
            .rust
            .checked_calls
            .get_mut(&(span.start, span.end))?
            .receiver_adjustments = adjustments;
        Some(())
    }

    /// Require the exact adjustment trait on the closed owner and preserve its shared or exclusive signature.
    /// A reference implementing Deref is itself Self; removing that reference would select another implementation.
    fn rust_deref_adjustment(
        &self,
        owner: &ResolvedType,
        target: &ResolvedType,
        mutable: bool,
    ) -> Option<RustCheckedCall> {
        let path = self.rust_canonical_path_for_receiver_type(owner)?;
        let metadata = self.rust_item_metadata_for_path(&path)?;
        let RustItemKind::Type(info) = &metadata.kind else {
            return None;
        };
        let owner = self.rust_trait_owner(owner, &info, &path);
        let trait_path = if mutable { DEREF_MUT_TRAIT } else { DEREF_TRAIT };
        let implementation = self.rust_adjustment_implementation(info, &owner, trait_path)?;
        let method = if mutable { "deref_mut" } else { "deref" };
        let owner = self.rust_storage_owner(&owner, &info, &path);
        let reference = |ty: ResolvedType| {
            if mutable {
                ResolvedType::RefMut(Box::new(ty))
            } else {
                ResolvedType::Ref(Box::new(ty))
            }
        };
        Some(RustCheckedCall {
            storage: None,
            source_type_parameters: self.native_call_type_parameters(),
            receiver_adjustments: Vec::new(),
            path: format!("{}::{method}", implementation.path),
            type_arguments: vec![owner.clone()],
            parameters: vec![CallableParam::positional(reference(owner))],
            return_type: reference(target.clone()),
            result_item_path: self.rust_canonical_path_for_receiver_type(target),
            has_receiver: true,
        })
    }
}
