//! Resolve associated types from retained impl templates; native consumers never infer them from argument position.

use std::cell::Cell;

use super::TypeChecker;
use crate::resolved_type_subst::map_resolved_type_leaves;
use crate::symbols::ResolvedType;
use crate::typechecker::helpers::render_resolved_type_as_rust_arg;
use incan_lang::interop::{RustFunctionSig, RustItemKind, ancestral_rust_path};
use quote::ToTokens;

impl TypeChecker {
    /// Normalize only qualified associated-type displays after signature substitution; ordinary native displays keep
    /// their original carrier spelling and source declaration identities.
    pub(super) fn rust_signature_projections(&self, mut signature: RustFunctionSig) -> RustFunctionSig {
        if signature.return_type.contains(" as ")
            && let Some(value) = self.rust_projected_type(&signature.return_type, 0)
        {
            signature.return_type = render_resolved_type_as_rust_arg(&value);
        }
        for parameter in &mut signature.params {
            if parameter.type_display.contains(" as ")
                && let Some(value) = self.rust_projected_type(&parameter.type_display, 0)
            {
                parameter.type_display = render_resolved_type_as_rust_arg(&value);
            }
        }
        signature
    }

    /// Resolve a named associated type only when one closed impl template supplies its value.
    pub(in crate::typechecker) fn rust_associated_type(
        &self,
        ty: &ResolvedType,
        trait_path: &str,
        member: &str,
    ) -> Option<ResolvedType> {
        self.rust_associated_type_at_depth(ty, trait_path, member, 0)
    }

    /// Bound recursive projection normalization and refuse competing associated values rather than choosing one.
    fn rust_associated_type_at_depth(
        &self,
        ty: &ResolvedType,
        trait_path: &str,
        member: &str,
        depth: usize,
    ) -> Option<ResolvedType> {
        if depth >= 12 {
            return None;
        }
        let path = self.rust_canonical_path_for_receiver_type(ty)?;
        let metadata = self.rust_item_metadata_for_path(&path)?;
        let RustItemKind::Type(info) = metadata.kind else {
            return None;
        };
        let owner = self.rust_closed_owner(ty, &info, &path);
        let mut selected = None;
        for implementation in &info.implemented_traits {
            if ancestral_rust_path(&implementation.path) != ancestral_rust_path(trait_path) {
                continue;
            }
            let Some(binding) = implementation
                .associated_type_bindings
                .iter()
                .find(|binding| binding.name == member)
            else {
                continue;
            };
            let Some(bindings) = self.rust_impl_self_bindings(implementation, &owner) else {
                continue;
            };
            if implementation
                .type_params
                .iter()
                .any(|name| !bindings.contains_key(name))
            {
                continue;
            }
            let substitutions = bindings
                .iter()
                .map(|(name, value)| (name.as_str(), render_resolved_type_as_rust_arg(value)))
                .collect();
            let display = Self::substitute_rust_owner_display(
                &binding.value_path,
                &substitutions,
                &render_resolved_type_as_rust_arg(&owner),
            );
            let value = self.rust_projected_type(&display, depth + 1)?;
            if selected.as_ref().is_some_and(|previous| previous != &value) {
                return None;
            }
            selected = Some(value);
        }
        selected
    }

    /// Normalize qualified projections structurally, including projections nested in ordinary result containers.
    pub(super) fn rust_projected_type(&self, display: &str, depth: usize) -> Option<ResolvedType> {
        if depth >= 12 {
            return None;
        }
        if let Ok(syn::Type::Path(path)) = syn::parse_str::<syn::Type>(display)
            && let Some(qualified) = path.qself
        {
            if qualified.position == 0 || path.path.segments.len() != qualified.position + 1 {
                return None;
            }
            let owner = self.native_rust_signature_type(&qualified.ty.to_token_stream().to_string(), "");
            let trait_path = path
                .path
                .segments
                .iter()
                .take(qualified.position)
                .map(|segment| segment.ident.to_string())
                .collect::<Vec<_>>()
                .join("::");
            let member = path.path.segments.last()?.ident.to_string();
            return self.rust_associated_type_at_depth(&owner, &trait_path, &member, depth + 1);
        }
        let ty = self.native_rust_signature_type(display, "");
        let failed = Cell::new(false);
        let resolved = map_resolved_type_leaves(&ty, &|leaf| {
            let ResolvedType::RustPath(path) = leaf else {
                return None;
            };
            if !path.trim_start().starts_with('<') {
                return None;
            }
            let value = self.rust_projected_type(path, depth + 1);
            if value.is_none() {
                failed.set(true);
            }
            value
        });
        (!failed.get()).then_some(resolved)
    }
}
