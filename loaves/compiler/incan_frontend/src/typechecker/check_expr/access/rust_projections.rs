//! Resolve associated types from retained impl templates; native consumers never infer them from argument position.

use std::cell::Cell;

use super::TypeChecker;
use crate::resolved_type_subst::map_resolved_type_leaves;
use crate::symbols::ResolvedType;
use crate::typechecker::helpers::render_resolved_type_as_rust_arg;
use incan_lang::interop::{RustFunctionSig, RustImplementedTrait, RustItemKind, ancestral_rust_path};
use incan_lang::lang::types::numerics;
use quote::ToTokens;

impl TypeChecker {
    /// Normalize only qualified associated-type displays after signature substitution; ordinary native displays keep
    /// their original carrier spelling and source declaration identities.
    pub(in crate::typechecker::check_expr) fn rust_signature_projections(
        &self,
        mut signature: RustFunctionSig,
    ) -> RustFunctionSig {
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
    /// Scalar carriers select their native primitive cache entry. Conflicting values require retained predicate
    /// evidence; a unique value still leaves complete native obligations to rustc.
    /// A named Deref target retains its native owner before source collection projection so method lookup can select
    /// that owner's metadata and preserve each executable adjustment.
    /// Unanchored Self templates require bound filtering before their associated values compete with concrete owner
    /// templates. Native signature and predicate validation remains authoritative for a selected concrete call.
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
        if let Some(name) = self.generic_placeholder_name(ty) {
            return self.rust_bound_associated_type(name, trait_path, member, depth + 1);
        }
        if let ResolvedType::RustPath(name) = ty
            && self
                .current_type_param_bound_details
                .iter()
                .any(|frame| frame.contains_key(name))
        {
            return self.rust_bound_associated_type(name, trait_path, member, depth + 1);
        }
        let path = self.rust_bound_owner_item_path(ty)?;
        let metadata = self.rust_item_metadata_for_path(&path)?;
        let info = match &metadata.kind {
            RustItemKind::Type(info) => info,
            RustItemKind::Trait(info) => {
                let identity = self.rust_definition_for_path(&path).unwrap_or_else(|| path.clone());
                let requested = self
                    .rust_definition_for_path(trait_path)
                    .unwrap_or_else(|| trait_path.to_owned());
                if ancestral_rust_path(&identity) != ancestral_rust_path(&requested) {
                    return None;
                }
                let arguments = self.rust_owner_type_args(ty, &path)?;
                let index = info
                    .items
                    .iter()
                    .filter_map(|item| match item {
                        incan_lang::interop::RustTraitAssoc::TypeAlias { name } => Some(name),
                        _ => None,
                    })
                    .position(|name| name == member)?;
                return arguments.get(info.type_params.len() + index).cloned();
            }
            _ => return None,
        };
        let owner = self.rust_trait_owner(ty, &info, &path);
        let mut candidates = Vec::new();
        let trait_identity = self
            .rust_definition_for_path(trait_path)
            .unwrap_or_else(|| trait_path.to_owned());
        for implementation in &info.implemented_traits {
            let implementation_identity = self
                .rust_definition_for_path(&implementation.path)
                .unwrap_or_else(|| implementation.path.clone());
            if ancestral_rust_path(&implementation_identity) != ancestral_rust_path(&trait_identity) {
                continue;
            }
            if Self::rust_projection_needs_bounds(implementation, &owner)
                && !self.rust_impl_may_apply(implementation, &owner)
            {
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
            let mut value = self.rust_projected_type(&display, depth + 1)?;
            if member == "Target"
                && matches!(&value, ResolvedType::Generic(name, _) if crate::typechecker::helpers::collection_type_id(name).is_some())
                && let Some(path) = self.rust_result_item_path(&display, "")
                && let Some(metadata) = self.rust_item_metadata_for_path(&path)
                && let RustItemKind::Type(info) = &metadata.kind
            {
                let native = ResolvedType::RustPath(Self::compact_rust_display(&display));
                let storage = self.rust_storage_owner(&native, &info, &path);
                if let Some(arguments) = self.rust_owner_type_args(&storage, &path) {
                    value = ResolvedType::RustPath(render_resolved_type_as_rust_arg(&ResolvedType::Generic(
                        path, arguments,
                    )));
                }
            }
            candidates.push((implementation, value));
        }
        self.select_rust_projection(candidates, &owner)
    }

    /// Preserve unambiguous associated values and use retained obligations only to distinguish competing templates.
    /// This does not turn incomplete conditional facts into proof; rustc checks the final closed native signature.
    fn select_rust_projection(
        &self,
        candidates: Vec<(&RustImplementedTrait, ResolvedType)>,
        owner: &ResolvedType,
    ) -> Option<ResolvedType> {
        let first = &candidates.first()?.1;
        if candidates.iter().all(|(_, value)| value == first) {
            return candidates.into_iter().next().map(|(_, value)| value);
        }
        let mut selected = None;
        for (implementation, value) in candidates {
            if !self.rust_impl_may_apply(implementation, owner) {
                continue;
            }
            if selected.as_ref().is_some_and(|previous| previous != &value) {
                return None;
            }
            selected = Some(value);
        }
        selected
    }

    /// Filter borrowed owners and blanket Self binders before selecting an associated value. A bare binder can match
    /// any owner, so its bounds must exclude unrelated collections; concrete owner templates retain their existing
    /// native predicate verification rather than requiring the partial metadata cache to prove every hidden bound.
    fn rust_projection_needs_bounds(implementation: &RustImplementedTrait, owner: &ResolvedType) -> bool {
        matches!(owner, ResolvedType::Ref(_) | ResolvedType::RefMut(_))
            || implementation
                .type_params
                .iter()
                .any(|name| name == &implementation.self_type)
    }

    /// Read generic projection equalities from active bounds and their metadata-defined supertraits. A Future also
    /// has the standard blanket IntoFuture implementation with the same Self and Output.
    fn rust_bound_associated_type(
        &self,
        name: &str,
        trait_path: &str,
        member: &str,
        depth: usize,
    ) -> Option<ResolvedType> {
        if depth >= 12 {
            return None;
        }
        if ancestral_rust_path(trait_path) == ancestral_rust_path("std::future::IntoFuture") {
            let output = self.rust_bound_associated_type(name, "std::future::Future", "Output", depth + 1)?;
            return match member {
                "Output" => Some(output),
                "IntoFuture" => Some(ResolvedType::TypeVar(name.to_owned())),
                _ => None,
            };
        }
        for frame in self.current_type_param_bound_details.iter().rev() {
            for bound in frame.get(name).into_iter().flatten() {
                let path = if bound.name.contains("::") {
                    Some(bound.name.trim_start_matches("::").to_owned())
                } else {
                    self.rust_canonical_path_for_receiver_type(&ResolvedType::Named(bound.name.clone()))
                };
                let Some(path) = path else {
                    continue;
                };
                let Some(metadata) = self.rust_item_metadata_for_path(&path) else {
                    continue;
                };
                let RustItemKind::Trait(info) = &metadata.kind else {
                    continue;
                };
                let substitutions = info
                    .type_params
                    .iter()
                    .zip(&bound.type_args)
                    .map(|(parameter, value)| (parameter.as_str(), render_resolved_type_as_rust_arg(value)))
                    .collect();
                for predicate in &info.bounds {
                    if predicate.self_type != "Self"
                        || ancestral_rust_path(&predicate.path) != ancestral_rust_path(trait_path)
                    {
                        continue;
                    }
                    let Some(binding) = predicate
                        .associated_type_bindings
                        .iter()
                        .find(|binding| binding.name == member)
                    else {
                        continue;
                    };
                    let display = Self::substitute_rust_owner_display(&binding.value_path, &substitutions, name);
                    return self.rust_projected_type(&display, depth + 1);
                }
            }
        }
        None
    }

    /// Normalize qualified projections structurally, including nested receiver projections and ordinary result
    /// containers.
    pub(in crate::typechecker) fn rust_projected_type(&self, display: &str, depth: usize) -> Option<ResolvedType> {
        if depth >= 12 {
            return None;
        }
        if let Ok(syn::Type::Path(mut path)) = syn::parse_str::<syn::Type>(display) {
            if let Some(qualified) = path.qself {
                if qualified.position == 0 || path.path.segments.len() != qualified.position + 1 {
                    return None;
                }
                let owner_display = qualified.ty.to_token_stream().to_string();
                let owner = self.rust_projected_type(&owner_display, depth + 1)?;
                let owner = self.rust_projection_owner(&owner_display, owner);
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
            // Normalize nested projections before display compaction can erase the `as` token boundary.
            for segment in &mut path.path.segments {
                let syn::PathArguments::AngleBracketed(arguments) = &mut segment.arguments else {
                    continue;
                };
                for argument in &mut arguments.args {
                    let syn::GenericArgument::Type(ty) = argument else {
                        continue;
                    };
                    let display = ty.to_token_stream().to_string();
                    if display.contains(" as ") {
                        let value = self.rust_projected_type(&display, depth + 1)?;
                        *ty = syn::parse_str(&render_resolved_type_as_rust_arg(&value)).ok()?;
                    }
                }
            }
            let display = Self::compact_rust_display(&path.to_token_stream().to_string());
            return Some(self.native_rust_signature_type(&display, ""));
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

    /// Select an associated impl through its retained native item before source collection projection erases that
    /// owner's path. Declared placeholders keep their bound lookup; metadata supplies every concrete owner and arity.
    fn rust_projection_owner(&self, display: &str, owner: ResolvedType) -> ResolvedType {
        if matches!(&owner, ResolvedType::Ref(_) | ResolvedType::RefMut(_))
            || self.generic_placeholder_name(&owner).is_some()
            || self.rust_canonical_path_for_receiver_type(&owner).is_some()
        {
            return owner;
        }
        let Some(path) = self.rust_result_item_path(display, "") else {
            return owner;
        };
        if let Some(arguments) = self.rust_owner_type_args(&owner, &path) {
            return ResolvedType::Generic(path, arguments);
        }
        owner
    }
}
