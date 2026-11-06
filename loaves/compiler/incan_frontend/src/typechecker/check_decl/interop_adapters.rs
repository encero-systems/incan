//! Rust newtype interop adapter candidates and signature validation.

use super::*;

impl TypeChecker {
    /// Build the resolved surface type that represents `nt` in interop signature validation.
    ///
    /// Generic rusttypes are represented as `Generic(Name, TypeVar...)` so adapter checks compare against the
    /// instantiated surface, not a monomorphic `Named`.
    pub(super) fn rusttype_decl_resolved_type(&self, nt: &NewtypeDecl) -> ResolvedType {
        if nt.type_params.is_empty() {
            return ResolvedType::Named(nt.name.clone());
        }
        let args = nt
            .type_params
            .iter()
            .map(|tp| ResolvedType::TypeVar(tp.name.clone()))
            .collect();
        ResolvedType::Generic(nt.name.clone(), args)
    }

    /// Human-readable display form for an `interop:` adapter reference expression.
    ///
    /// Used in diagnostics and duplicate-edge bookkeeping so messages show the same spelling users wrote (`name`,
    /// `Owner.member`, or a conservative `<expr>` fallback).
    pub(super) fn interop_adapter_ref_display(adapter: &Spanned<Expr>) -> String {
        match &adapter.node {
            Expr::Ident(name) => name.clone(),
            Expr::Field(base, member) => match &base.node {
                Expr::Ident(owner) => format!("{owner}.{member}"),
                _ => format!("<expr>.{member}"),
            },
            _ => "<expr>".to_string(),
        }
    }

    /// Resolve all callable adapter candidates for a short adapter name on a rusttype declaration.
    ///
    /// Candidate order is intentional:
    /// 1. Local rusttype-declared methods.
    /// 2. Backing Rust methods from metadata (when available).
    ///
    /// Returns `(candidates, backing_methods_available)`. The second flag lets callers distinguish "no match because
    /// metadata is unavailable" from "no match with authoritative metadata".
    pub(super) fn interop_adapter_candidates_for_name(
        &self,
        nt: &NewtypeDecl,
        name: &str,
    ) -> (Vec<InteropAdapterSig>, bool) {
        let mut candidates: Vec<InteropAdapterSig> = Vec::new();
        let mut backing_methods_available = false;

        if let Some(TypeInfo::Newtype(info)) = self.lookup_type_info(&nt.name) {
            if let Some(method) = info.methods.get(name) {
                candidates.push(InteropAdapterSig {
                    name: format!("{}.{}", nt.name, name),
                    receiver: method.receiver,
                    params: method.params.iter().map(|param| param.ty.clone()).collect(),
                    return_type: method.return_type.clone(),
                });
            }

            if info.is_rusttype
                && let ResolvedType::RustPath(path) = &info.underlying
                && let Some(meta) = self.rust_item_metadata_for_method_call(path, name)
                && let RustItemKind::Type(type_info) = &meta.kind
            {
                backing_methods_available = type_info.metadata_completeness.has_methods()
                    || type_info.methods.iter().any(|method| method.name == name);
                for method in type_info.methods.iter().filter(|m| m.name == name) {
                    let has_receiver = Self::rust_signature_has_receiver(&method.signature);
                    let receiver = if has_receiver { Some(Receiver::Immutable) } else { None };
                    let skip = usize::from(has_receiver);
                    let params = method
                        .signature
                        .params
                        .iter()
                        .skip(skip)
                        .map(|p| {
                            self.resolved_param_type_from_rust_display_for_owner_path(p.type_display.as_str(), path)
                        })
                        .collect();
                    let return_display = self.rust_display_for_owner_path(method.signature.return_type.as_str(), path);
                    candidates.push(InteropAdapterSig {
                        name: format!("rust::{}.{name}", path),
                        receiver,
                        params,
                        return_type: self.resolved_type_from_rust_display(return_display.as_str()),
                    });
                }
            }
        }

        (candidates, backing_methods_available)
    }

    /// Decide whether missing-adapter diagnostics should be deferred until metadata is available.
    ///
    /// For rusttypes backed by a Rust path, missing metadata should not become a hard error in the typechecker;
    /// lowering/rustc will still validate concrete callability later.
    pub(super) fn maybe_defer_interop_adapter_missing(
        &self,
        nt: &NewtypeDecl,
        backing_methods_available: bool,
    ) -> bool {
        if backing_methods_available {
            return false;
        }
        self.lookup_type_info(&nt.name).is_some_and(|info| {
            matches!(
                info,
                TypeInfo::Newtype(NewtypeInfo {
                    is_rusttype: true,
                    underlying: ResolvedType::RustPath(_),
                    ..
                })
            )
        })
    }

    /// Resolve one `interop:` adapter reference to an unambiguous callable signature.
    ///
    /// Supports short-form (`name`) and qualified (`Type.name`) references, enforces owner checks for qualified forms,
    /// and emits ambiguity/missing diagnostics when metadata is authoritative.
    pub(super) fn resolve_interop_adapter_signature(
        &mut self,
        nt: &NewtypeDecl,
        edge: &Spanned<InteropEdgeDecl>,
    ) -> Option<InteropAdapterSig> {
        match &edge.node.adapter.node {
            Expr::Ident(name) => {
                let (mut candidates, backing_methods_available) = self.interop_adapter_candidates_for_name(nt, name);
                if candidates.is_empty() {
                    if self.maybe_defer_interop_adapter_missing(nt, backing_methods_available) {
                        return None;
                    }
                    self.errors.push(errors::unknown_symbol(name, edge.node.adapter.span));
                    return None;
                }
                if candidates.len() > 1 {
                    self.errors.push(errors::ambiguous_interop_adapter_short_name(
                        &nt.name,
                        name,
                        candidates.len(),
                        edge.node.adapter.span,
                    ));
                    return None;
                }
                candidates.pop()
            }
            Expr::Field(base, method) => {
                let Expr::Ident(owner) = &base.node else {
                    self.errors.push(errors::interop_adapter_ref_must_be_name_or_member(
                        edge.node.adapter.span,
                    ));
                    return None;
                };
                if owner != &nt.name {
                    self.errors.push(errors::interop_adapter_wrong_owner(
                        &nt.name,
                        owner,
                        edge.node.adapter.span,
                    ));
                    return None;
                }
                let (candidates, backing_methods_available) = self.interop_adapter_candidates_for_name(nt, method);
                if candidates.is_empty() {
                    if self.maybe_defer_interop_adapter_missing(nt, backing_methods_available) {
                        return None;
                    }
                    self.errors
                        .push(errors::missing_method(&nt.name, method, edge.node.adapter.span));
                    return None;
                }
                // Qualified references follow ordinary lookup precedence:
                // Local rusttype methods first, then backing Rust metadata methods.
                candidates.into_iter().next()
            }
            _ => {
                self.errors.push(errors::interop_adapter_ref_must_be_name_or_member(
                    edge.node.adapter.span,
                ));
                None
            }
        }
    }

    /// Validate adapter shape against one `interop:` edge contract.
    ///
    /// This enforces:
    /// - receiver/arity constraints for `from` vs `into` edges,
    /// - input type compatibility at the boundary,
    /// - `via` (infallible) vs `try` (Result/Option) return-shape rules,
    /// - adapted output compatibility with the target edge direction.
    pub(super) fn validate_interop_adapter_signature(
        &mut self,
        nt: &NewtypeDecl,
        edge: &Spanned<InteropEdgeDecl>,
        boundary_ty: &ResolvedType,
        rusttype_ty: &ResolvedType,
        adapter: &InteropAdapterSig,
    ) {
        let (expected_input, expected_output) = match edge.node.direction {
            InteropDirection::From => (boundary_ty, rusttype_ty),
            InteropDirection::Into => (rusttype_ty, boundary_ty),
        };

        match edge.node.direction {
            InteropDirection::From if adapter.receiver.is_some() => {
                self.errors
                    .push(errors::interop_from_adapter_requires_associated_callable(
                        &nt.name,
                        adapter.name.as_str(),
                        edge.node.adapter.span,
                    ));
                return;
            }
            InteropDirection::Into if adapter.receiver.is_some() => {
                if !adapter.params.is_empty() {
                    self.errors.push(errors::interop_adapter_arity_mismatch(
                        &nt.name,
                        adapter.name.as_str(),
                        0,
                        adapter.params.len(),
                        edge.node.adapter.span,
                    ));
                    return;
                }
            }
            _ => {
                if adapter.params.len() != 1 {
                    self.errors.push(errors::interop_adapter_arity_mismatch(
                        &nt.name,
                        adapter.name.as_str(),
                        1,
                        adapter.params.len(),
                        edge.node.adapter.span,
                    ));
                    return;
                }
                let Some(found_input) = adapter.params.first() else {
                    return;
                };
                if !self.types_compatible(expected_input, found_input) {
                    self.errors.push(errors::interop_adapter_input_mismatch(
                        &nt.name,
                        adapter.name.as_str(),
                        &expected_input.to_string(),
                        &found_input.to_string(),
                        edge.node.adapter.span,
                    ));
                }
            }
        }

        let adapted_return = match edge.node.adapter_kind {
            InteropAdapterKind::Via => {
                if adapter.return_type.is_result() || adapter.return_type.is_option() {
                    self.errors.push(errors::interop_via_adapter_must_be_infallible(
                        &nt.name,
                        adapter.name.as_str(),
                        &adapter.return_type.to_string(),
                        edge.node.adapter.span,
                    ));
                }
                adapter.return_type.clone()
            }
            InteropAdapterKind::Try => {
                if adapter.return_type.is_result() {
                    adapter
                        .return_type
                        .result_ok_type()
                        .cloned()
                        .unwrap_or(ResolvedType::Unknown)
                } else if adapter.return_type.is_option() {
                    adapter
                        .return_type
                        .option_inner_type()
                        .cloned()
                        .unwrap_or(ResolvedType::Unknown)
                } else {
                    self.errors.push(errors::interop_try_adapter_requires_result_or_option(
                        &nt.name,
                        adapter.name.as_str(),
                        &adapter.return_type.to_string(),
                        edge.node.adapter.span,
                    ));
                    ResolvedType::Unknown
                }
            }
        };

        if !self.types_compatible(&adapted_return, expected_output) {
            self.errors.push(errors::interop_adapter_output_mismatch(
                &nt.name,
                adapter.name.as_str(),
                &expected_output.to_string(),
                &adapted_return.to_string(),
                edge.node.adapter.span,
            ));
        }
    }
}
