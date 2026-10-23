//! Checked native autodereference: retain every adjustment as an ordinary external trait call.

use super::{RustPathMethodCall, RustTraitQualifiedCall, TypeChecker};
use crate::ast::{Expr, Span, Spanned};
use crate::symbols::{CallableParam, ResolvedType};
use crate::typechecker::helpers::render_resolved_type_as_rust_arg;
use crate::typechecker::type_info::RustCheckedCall;
use crate::typechecker::{IdentKind, RustArgCoercionKind};
use incan_lang::interop::{RustItemKind, RustTypeInfo, ancestral_rust_path};
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

    /// Retain native dereferences only when their metadata-selected target satisfies a source shared parameter.
    /// This is a call-boundary adjustment, never assignment compatibility or an owned-value conversion.
    pub(in crate::typechecker) fn retain_borrowed_argument_dereferences(
        &mut self,
        actual: &ResolvedType,
        expected: &ResolvedType,
        span: Span,
    ) -> Option<()> {
        let ResolvedType::Ref(expected) = expected else {
            return None;
        };
        let mut owner = self.rust_inference_type(actual, &[]);
        let mut steps = Vec::new();
        for _ in 0..12 {
            let target = self.rust_associated_type(&owner, DEREF_TRAIT, "Target")?;
            if target == owner {
                return None;
            }
            steps.push(self.rust_deref_adjustment(&owner, &target, false)?);
            if self.rust_trait_carriers_match(&target, expected) {
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
            if mutable {
                self.refuse_write_through_read_only_binding(call.receiver_expr, call.span);
                self.validate_rust_borrow_mutability(RustArgCoercionKind::Borrow { mutable: true }, call.receiver_expr);
            }
            let errors_before = self.errors.len();
            let result = self.validate_rust_method_call(
                &format!("{path}.{}", call.method),
                &signature,
                call.args,
                call.arg_types,
                false,
                call.span,
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

    /// Resolve an explicit trait receiver through the same native owner selection and checked dereference calls as
    /// value syntax. The trait's name constrains lookup; source argument zero remains the evaluated receiver.
    pub(super) fn retain_rust_trait_qualified_call(
        &mut self,
        call: &RustTraitQualifiedCall<'_>,
        receiver: &Spanned<Expr>,
        source: &ResolvedType,
    ) -> Option<ResolvedType> {
        if let Some(result) = self.retain_bound_rust_trait_call(call, receiver, source) {
            return Some(result);
        }
        let mut owner = source.clone();
        let mut steps = Vec::new();
        for _ in 0..12 {
            let path = self.rust_adjusted_owner_path(&owner)?;
            let metadata = self.rust_item_metadata_for_path(&path)?;
            let RustItemKind::Type(info) = &metadata.kind else {
                return None;
            };
            owner = self.rust_closed_owner(&owner, info, &path);
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
                    expected_return_ty: None,
                    span: call.span,
                },
                info,
                &owner,
                true,
                Some(call.rust_path),
            );
            if let Some(result) = selected {
                self.retain_rust_deref_steps(call.span, &steps)?;
                let retained = self
                    .type_info
                    .rust
                    .checked_calls
                    .get_mut(&(call.span.start, call.span.end))?;
                retained.has_receiver = false;
                let parameters = retained.parameters.clone();
                self.type_info
                    .record_call_site_callable_params_exact(call.span, &parameters);
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
    fn rust_deref_adjustment(
        &self,
        owner: &ResolvedType,
        target: &ResolvedType,
        mutable: bool,
    ) -> Option<RustCheckedCall> {
        let path = self.rust_canonical_path_for_receiver_type(owner)?;
        let metadata = self.rust_item_metadata_for_path(&path)?;
        let RustItemKind::Type(info) = metadata.kind else {
            return None;
        };
        let owner = self.rust_closed_owner(owner, &info, &path);
        let trait_path = if mutable { DEREF_MUT_TRAIT } else { DEREF_TRAIT };
        let implementation = info.implemented_traits.iter().find(|implementation| {
            ancestral_rust_path(&implementation.path) == ancestral_rust_path(trait_path)
                && self.rust_impl_self_bindings(implementation, &owner).is_some()
        })?;
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
