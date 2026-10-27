//! Resolve associated types from retained impl templates; native consumers never infer them from argument position.

use std::cell::Cell;
use std::collections::HashMap;

use super::TypeChecker;
use crate::resolved_type_subst::map_resolved_type_leaves;
use crate::symbols::ResolvedType;
use crate::typechecker::helpers::render_resolved_type_as_rust_arg;
use incan_lang::interop::{RustFunctionSig, RustImplementedTrait, RustItemKind, ancestral_rust_path};
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
        self.rust_associated_type_at_depth(ty, trait_path, &[], member, 0)
    }

    /// Bound recursive projection normalization and refuse competing equally specific associated values.
    /// Scalar carriers select their native primitive cache entry. Conflicting values require retained predicate
    /// evidence; a unique value still leaves complete native obligations to rustc.
    /// A named Deref target retains its native owner before source collection projection so method lookup can select
    /// that owner's metadata and preserve each executable adjustment.
    /// Unanchored Self templates require bound filtering before their associated values compete with concrete owner
    /// templates. Native signature and predicate validation remains authoritative for a selected concrete call.
    /// An unresolved blanket projection does not prevent a later concrete candidate from supplying its associated
    /// value; only successfully normalized candidates participate in specificity selection.
    fn rust_associated_type_at_depth(
        &self,
        ty: &ResolvedType,
        trait_path: &str,
        trait_arguments: &[ResolvedType],
        member: &str,
        depth: usize,
    ) -> Option<ResolvedType> {
        if depth >= 12 {
            return None;
        }
        if let Some(name) = self.generic_placeholder_name(ty) {
            if !trait_arguments.is_empty() {
                return None;
            }
            return self.rust_bound_associated_type(name, trait_path, member, depth + 1);
        }
        if let ResolvedType::RustPath(name) = ty
            && self
                .current_type_param_bound_details
                .iter()
                .any(|frame| frame.contains_key(name))
        {
            if !trait_arguments.is_empty() {
                return None;
            }
            return self.rust_bound_associated_type(name, trait_path, member, depth + 1);
        }
        let path = self.rust_bound_owner_item_path(ty)?;
        let metadata = self.rust_item_metadata_for_path(&path)?;
        let info = match &metadata.kind {
            RustItemKind::Type(info) => info,
            RustItemKind::Trait(info) => {
                if self.rust_trait_identity(&path) != self.rust_trait_identity(trait_path) {
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
        let trait_identity = self.rust_trait_identity(trait_path);
        for implementation in &info.implemented_traits {
            if self.rust_trait_identity(&implementation.path) != trait_identity {
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
            let Some(bindings) = self.rust_projection_impl_bindings(implementation, &owner, trait_arguments) else {
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
            let Some(mut value) = self.rust_projected_type(&display, depth + 1) else {
                continue;
            };
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

    /// Preserve unambiguous associated values and rank applicable competing templates by owner specificity.
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
        let mut specificity = 0;
        let mut ambiguous = false;
        for (implementation, value) in candidates {
            if !self.rust_impl_may_apply(implementation, owner) {
                continue;
            }
            let candidate_specificity = self.rust_projection_specificity(implementation);
            if selected.is_none() || candidate_specificity > specificity {
                selected = Some(value);
                specificity = candidate_specificity;
                ambiguous = false;
            } else if candidate_specificity == specificity
                && selected.as_ref().is_some_and(|previous| previous != &value)
            {
                ambiguous = true;
            }
        }
        if ambiguous { None } else { selected }
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

    /// Prefer an owner-shaped impl over an unconstrained blanket Self before reading its associated value.
    /// This is a metadata projection, not trait-obligation proof; the adapter still verifies the complete native
    /// signature and predicates against rustc's selected trait reference.
    fn rust_projection_specificity(&self, implementation: &RustImplementedTrait) -> usize {
        let template = self.native_rust_signature_type(&implementation.self_type, "");
        let template = self.rust_inference_type(&template, &implementation.type_params);
        native_template_specificity(&template)
    }

    /// Bind both Self and the requested trait arguments before reading an impl's associated value. An argument that
    /// conflicts with the closed Self template or a different trait arity grants no projection.
    fn rust_projection_impl_bindings(
        &self,
        implementation: &RustImplementedTrait,
        owner: &ResolvedType,
        arguments: &[ResolvedType],
    ) -> Option<HashMap<String, ResolvedType>> {
        if implementation.type_arguments.len() != arguments.len() {
            return None;
        }
        let mut bindings = self.rust_impl_self_bindings(implementation, owner)?;
        for (display, argument) in implementation.type_arguments.iter().zip(arguments) {
            let template = self.native_rust_signature_type(display, "");
            self.infer_type_param_bindings(
                &self.rust_inference_type(&template, &implementation.type_params),
                &self.rust_inference_type(argument, &[]),
                &mut bindings,
            );
            let closed = self.instantiate_rust_signature_type(&template, &implementation.type_params, &bindings);
            if !self.rust_trait_carriers_match(&closed, argument) {
                return None;
            }
        }
        Some(bindings)
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
        if self.rust_trait_identity(trait_path) == self.rust_trait_identity("std::future::IntoFuture") {
            let output = self.rust_bound_associated_type(name, "std::future::Future", "Output", depth + 1)?;
            return match member {
                "Output" => Some(output),
                "IntoFuture" => Some(ResolvedType::TypeVar(name.to_owned())),
                _ => None,
            };
        }
        let identity = self.rust_trait_identity(trait_path);
        for bound in self.innermost_type_param_bounds(name)? {
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
                if predicate.self_type != "Self" || self.rust_trait_identity(&predicate.path) != identity {
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
                let arguments = self.rust_projection_trait_arguments(
                    &path.path.segments.iter().nth(qualified.position - 1)?.arguments,
                    depth + 1,
                )?;
                return self.rust_associated_type_at_depth(&owner, &trait_path, &arguments, &member, depth + 1);
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

    /// Retain type arguments on a qualified trait projection; lifetimes carry no native layout, and const or
    /// associated-binding arguments remain refused until their own projection contract is retained.
    fn rust_projection_trait_arguments(
        &self,
        arguments: &syn::PathArguments,
        depth: usize,
    ) -> Option<Vec<ResolvedType>> {
        match arguments {
            syn::PathArguments::None => Some(Vec::new()),
            syn::PathArguments::AngleBracketed(arguments) => arguments
                .args
                .iter()
                .filter_map(|argument| match argument {
                    syn::GenericArgument::Lifetime(_) => None,
                    syn::GenericArgument::Type(ty) => {
                        Some(self.rust_projected_type(&ty.to_token_stream().to_string(), depth + 1))
                    }
                    _ => Some(None),
                })
                .collect(),
            _ => None,
        }
    }

    /// Select an associated impl through its retained native item before source collection projection erases that
    /// owner's path. Declared placeholders keep their bound lookup; metadata supplies every concrete owner and arity.
    fn rust_projection_owner(&self, display: &str, owner: ResolvedType) -> ResolvedType {
        let display = Self::compact_rust_display(display);
        if matches!(&owner, ResolvedType::Ref(_) | ResolvedType::RefMut(_))
            || self.generic_placeholder_name(&owner).is_some()
        {
            return owner;
        }
        let Some(path) = self.rust_result_item_path(&display, "") else {
            return owner;
        };
        if let Some(arguments) = self.rust_owner_type_args(&owner, &path) {
            return ResolvedType::Generic(path, arguments);
        }
        if let Some((base, arguments)) = Self::rust_generic_base_and_args(&display)
            && ancestral_rust_path(base) == ancestral_rust_path(&path)
        {
            let arguments = arguments
                .into_iter()
                .filter(|argument| !Self::rust_generic_arg_is_lifetime(argument))
                .map(|argument| self.native_rust_signature_type(argument, &path))
                .collect();
            return ResolvedType::Generic(path, arguments);
        }
        owner
    }
}

/// Count fixed carrier structure while leaving declared impl binders unconstrained.
fn native_template_specificity(ty: &ResolvedType) -> usize {
    match ty {
        ResolvedType::TypeVar(_) => 0,
        ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => 1 + native_template_specificity(inner),
        ResolvedType::Generic(_, arguments) | ResolvedType::Tuple(arguments) => {
            1 + arguments.iter().map(native_template_specificity).sum::<usize>()
        }
        _ => 1,
    }
}
