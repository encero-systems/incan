//! Checked native autodereference: retain every adjustment as an ordinary external trait call.

use super::{RustPathMethodCall, TypeChecker};
use crate::symbols::{CallableParam, ResolvedType};
use crate::typechecker::type_info::RustCheckedCall;
use crate::typechecker::{IdentKind, RustArgCoercionKind};
use incan_lang::interop::{RustItemKind, RustTypeInfo, ancestral_rust_path};

const DEREF_TRAIT: &str = "std::ops::Deref";
const DEREF_MUT_TRAIT: &str = "std::ops::DerefMut";

impl TypeChecker {
    /// Search instantiated Deref targets for an inherent instance method. Associated functions do not participate
    /// in value lookup, and an adjustment chain remains explicit through Body IR rather than changing receiver types.
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
            let path = self.rust_canonical_path_for_receiver_type(&target)?;
            let metadata = self.rust_item_metadata_for_path(&path)?;
            let RustItemKind::Type(info) = metadata.kind else {
                return None;
            };
            owner = self.rust_closed_owner(&target, &info, &path);
            let Some(method) = info
                .methods
                .iter()
                .find(|method| method.name == call.method && Self::rust_signature_has_receiver(&method.signature))
            else {
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
                &info,
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
        let trait_path = if mutable { DEREF_MUT_TRAIT } else { DEREF_TRAIT };
        let implementation = info.implemented_traits.iter().find(|implementation| {
            ancestral_rust_path(&implementation.path) == ancestral_rust_path(trait_path)
                && self.rust_impl_self_bindings(implementation, owner).is_some()
        })?;
        let method = if mutable { "deref_mut" } else { "deref" };
        let owner = self.rust_storage_owner(owner, &info, &path);
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
