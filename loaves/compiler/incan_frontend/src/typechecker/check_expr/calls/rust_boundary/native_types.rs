//! Native Rust object and result types, callable parameter displays, and metadata prewarming.

use super::*;
use incan_lang::lang::types::collections;

impl TypeChecker {
    /// Project a signature-proven native reference result onto Incan's ordinary borrowed-value surface while the
    /// checked call keeps its physical Rust carrier for Body IR. Native owner types remain nominal dependency holders.
    pub(in crate::typechecker::check_expr) fn native_borrowed_source_result(
        &self,
        span: Span,
        source: ResolvedType,
    ) -> ResolvedType {
        let explicit_body = self.current_body_identity().is_some_and(|owner| {
            self.type_info.declarations.ownership_contracts.contains_key(owner)
                || self
                    .type_info
                    .protocols
                    .explicit_local_modes
                    .values()
                    .any(|mode| &mode.owner == owner)
        });
        if !explicit_body {
            return source;
        }
        let Some(call) = self.type_info.rust.checked_calls.get(&span) else {
            return source;
        };
        let carries_result_loan = call.signature_loans.len() == call.parameters.len() + 1
            && self.type_holds_native_signature_loan(&call.return_type);
        if carries_result_loan {
            native_borrowed_surface_type(source)
        } else {
            source
        }
    }

    /// Preserve the ordinary source scalar/collection projection while restoring source owners from structural
    /// native substitution. Rendering an inferred source model to a Rust display must not turn it into a foreign
    /// item; native references and collection views keep the source projection already selected for this call.
    pub(in crate::typechecker::check_expr) fn rust_source_result_type(
        &self,
        native: &ResolvedType,
        source: &ResolvedType,
    ) -> ResolvedType {
        match (native, source) {
            (ResolvedType::Named(_) | ResolvedType::TypeVar(_), _) => native.clone(),
            (ResolvedType::Generic(base, _), ResolvedType::RustPath(_))
                if base == "[]"
                    || self.lookup_semantic_type_info(base).is_some()
                    || matches!(
                        Self::rust_collection_id_from_display_base(base),
                        Some(CollectionTypeId::FrozenList | CollectionTypeId::FrozenDict | CollectionTypeId::FrozenSet)
                    ) =>
            {
                native.clone()
            }
            (ResolvedType::Generic(base, native), ResolvedType::Generic(projected, source))
                if base == projected && native.len() == source.len() =>
            {
                ResolvedType::Generic(
                    base.clone(),
                    native
                        .iter()
                        .zip(source)
                        .map(|(native, source)| self.rust_source_result_type(native, source))
                        .collect(),
                )
            }
            (ResolvedType::Tuple(native), ResolvedType::Tuple(source)) if native.len() == source.len() => {
                ResolvedType::Tuple(
                    native
                        .iter()
                        .zip(source)
                        .map(|(native, source)| self.rust_source_result_type(native, source))
                        .collect(),
                )
            }
            (ResolvedType::Ref(native), ResolvedType::Ref(source)) => {
                ResolvedType::Ref(Box::new(self.rust_source_result_type(native, source)))
            }
            (ResolvedType::RefMut(native), ResolvedType::RefMut(source)) => {
                ResolvedType::RefMut(Box::new(self.rust_source_result_type(native, source)))
            }
            _ => source.clone(),
        }
    }

    /// Retain a single native object's principal arguments followed by its declared associated equalities. Trait
    /// metadata fixes the associated slot order; source spelling never chooses a native layout or invents an Item.
    pub(super) fn native_rust_object_type(&self, display: &str) -> Option<ResolvedType> {
        let syn::Type::TraitObject(object) = syn::parse_str::<syn::Type>(display).ok()? else {
            return None;
        };
        let traits = object
            .bounds
            .iter()
            .filter_map(|bound| match bound {
                syn::TypeParamBound::Trait(bound) => Some(bound),
                _ => None,
            })
            .collect::<Vec<_>>();
        let [principal] = traits.as_slice() else {
            return None;
        };
        let mut path = principal.path.clone();
        let segment = path.segments.last_mut()?;
        let supplied = std::mem::replace(&mut segment.arguments, syn::PathArguments::None);
        let path = path.to_token_stream().to_string().replace(' ', "");
        let metadata = self.rust_item_metadata_for_path(&path)?;
        let RustItemKind::Trait(info) = &metadata.kind else {
            return None;
        };
        let mut arguments = Vec::new();
        let mut equalities = HashMap::new();
        if let syn::PathArguments::AngleBracketed(supplied) = supplied {
            for argument in supplied.args {
                match argument {
                    syn::GenericArgument::Type(ty) => {
                        arguments.push(self.native_rust_signature_type(&ty.to_token_stream().to_string(), ""))
                    }
                    syn::GenericArgument::AssocType(binding) if binding.generics.is_none() => {
                        equalities.insert(
                            binding.ident.to_string(),
                            self.native_rust_signature_type(&binding.ty.to_token_stream().to_string(), ""),
                        );
                    }
                    syn::GenericArgument::Lifetime(_) => {}
                    _ => return None,
                }
            }
        }
        if arguments.len() != info.type_params.len() {
            return None;
        }
        for item in &info.items {
            if let incan_lang::interop::RustTraitAssoc::TypeAlias { name } = item {
                arguments.push(equalities.remove(name)?);
            }
        }
        if !equalities.is_empty() {
            return None;
        }
        Some(if arguments.is_empty() {
            ResolvedType::RustPath(path)
        } else {
            ResolvedType::Generic(path, arguments)
        })
    }

    /// Remove an invalid namespace prefix from a Rust slice display.
    ///
    /// Some rust-analyzer callback-bound displays report `crate::[T]` (or its already-expanded `dependency::[T]`
    /// form). Rust slices are never namespaced; retaining that prefix names no Rust type.
    pub(super) fn strip_invalid_rust_slice_namespace(display: &str) -> String {
        let mut normalized = String::with_capacity(display.len());
        let mut remaining = display;
        while let Some(index) = remaining.find("::[") {
            let before = &remaining[..index];
            let prefix_start = before
                .rfind(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == ':'))
                .map_or(0, |index| index + 1);
            normalized.push_str(&before[..prefix_start]);
            normalized.push('[');
            remaining = &remaining[index + "::[".len()..];
        }
        normalized.push_str(remaining);
        normalized
    }

    /// Return the exact Rust input displays carried by a callable bound.
    ///
    /// The resolved Incan function type preserves ownership semantics for checking, but it cannot distinguish a
    /// borrowed slice from a borrowed `Vec`. Emission needs the original Rust spelling for callback parameters.
    pub(super) fn rust_callable_param_displays_from_bound(&self, rust_ty: &str) -> Option<Vec<String>> {
        let body = Self::rust_callable_bound_body(rust_ty)?;
        let body = body
            .strip_prefix("std::ops::")
            .or_else(|| body.strip_prefix("core::ops::"))
            .unwrap_or(body);
        let after_name = ["FnOnce", "FnMut", "Fn"]
            .into_iter()
            .find_map(|name| body.strip_prefix(name))?
            .trim_start();
        let args_end = Self::rust_callable_args_end(after_name)?;
        if !after_name.starts_with('(') {
            return None;
        }
        Some(
            Self::split_top_level_generic_args(&after_name[1..args_end])
                .into_iter()
                // Keep the token boundary after `mut`: `&mut[f32]` is not valid Rust even though it is useful for
                // type-display comparisons. These strings are emitted as Rust parameter types.
                .map(|display| {
                    let display = Self::rust_display_without_lifetimes(display);
                    Self::strip_invalid_rust_slice_namespace(display.trim())
                })
                .collect(),
        )
    }

    /// Preserve exact callable parameter displays for an inline closure or a direct named source function.
    pub(super) fn record_rust_callable_parameter_displays(
        &mut self,
        rust_type_display: &str,
        owner_path: &str,
        arg_expr: &Spanned<Expr>,
        arg_ty: &ResolvedType,
    ) {
        let display = self.rust_display_for_owner_path(rust_type_display, owner_path);
        let Some(param_displays) = self.rust_callable_param_displays_from_bound(display.as_str()) else {
            return;
        };
        let ResolvedType::Function(params, _) = arg_ty else {
            return;
        };
        if params.len() != param_displays.len()
            || !params.iter().zip(&param_displays).all(|(param, display)| {
                self.types_compatible(&param.ty, &self.resolved_param_type_from_rust_display(display))
            })
        {
            return;
        }

        match &arg_expr.node {
            Expr::Closure(_, _) => {
                self.type_info
                    .rust
                    .closure_param_type_displays
                    .insert(arg_expr.span, param_displays);
            }
            Expr::Ident(name) => {
                self.type_info
                    .rust
                    .function_param_type_displays
                    .entry(name.clone())
                    .or_insert(param_displays);
            }
            _ => {}
        }
    }

    /// Reuse already-prepared metadata for Rust path types returned by inspected Rust calls.
    ///
    /// Chained registry APIs often return helper types that users never import directly. When metadata for those types
    /// is already available, reusing it lets the next method call validate against its real signature. This must stay
    /// cache-only: forcing fresh rust-analyzer extraction for every opaque return handle turns ordinary downstream
    /// builds into dependency metadata crawls.
    pub(super) fn prewarm_rust_return_type_metadata(&self, ty: &ResolvedType) {
        self.prewarm_rust_type_identity_metadata(ty);
    }

    /// Visit nominal Rust paths nested inside a resolved type with one metadata probe policy.
    pub(super) fn prewarm_rust_type_identity_metadata_with(&self, ty: &ResolvedType, probe: &impl Fn(&Self, &str)) {
        match ty {
            ResolvedType::RustPath(path) => {
                let (base, args) = self.rust_path_base_and_args(path);
                if Self::rust_identity_metadata_base_should_probe(base.as_str()) {
                    probe(self, base.as_str());
                }
                for arg in args {
                    self.prewarm_rust_type_identity_metadata_with(&arg, probe);
                }
            }
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => {
                self.prewarm_rust_type_identity_metadata_with(inner, probe)
            }
            ResolvedType::Generic(_, args) | ResolvedType::Tuple(args) => {
                for arg in args {
                    self.prewarm_rust_type_identity_metadata_with(arg, probe);
                }
            }
            ResolvedType::FrozenList(inner) | ResolvedType::FrozenSet(inner) => {
                self.prewarm_rust_type_identity_metadata_with(inner, probe);
            }
            ResolvedType::FrozenDict(key, value) => {
                self.prewarm_rust_type_identity_metadata_with(key, probe);
                self.prewarm_rust_type_identity_metadata_with(value, probe);
            }
            ResolvedType::Function(params, ret) => {
                for param in params {
                    self.prewarm_rust_type_identity_metadata_with(&param.ty, probe);
                }
                self.prewarm_rust_type_identity_metadata_with(ret, probe);
            }
            _ => {}
        }
    }

    /// Reuse cache-only Rust identity metadata while checking ordinary compatibility.
    pub(super) fn prewarm_rust_type_identity_metadata(&self, ty: &ResolvedType) {
        self.prewarm_rust_type_identity_metadata_with(ty, &|checker, path| {
            let _ = checker.rust_item_metadata_for_path(path);
        });
    }

    /// Prewarm Rust identity metadata for a type explicitly named by a Rust call boundary.
    ///
    /// The ordinary compatibility path must remain metadata-light, but a public Rust API can name a type that is
    /// re-exported through macro-generated source. This permits blocking inspection on a cache miss and is invoked
    /// only while validating an explicit call argument, never from [`Self::types_compatible`].
    pub(super) fn prewarm_rust_boundary_type_identity_metadata(&self, ty: &ResolvedType) {
        self.prewarm_rust_type_identity_metadata_with(ty, &|checker, path| {
            let _ = checker.rust_item_metadata_for_path(path);
        });
    }

    /// Return whether cache-only identity prewarm should query Rust metadata for this display base.
    ///
    /// This prewarm exists only to reuse already-known nominal metadata for returned opaque handles. Standard generic
    /// wrappers such as `Box<T>` and `Arc<T>` are not the semantic identity, and rust-analyzer sometimes renders them
    /// relative to a dependency module (`datafusion_expr::expr::Box`). Probing those wrapper bases as dependency items
    /// turns a cheap compatibility hint into a full workspace extraction. Skip the wrapper and keep recursing into its
    /// type arguments, where the actual nominal identity lives.
    pub(in crate::typechecker) fn rust_identity_metadata_base_should_probe(base: &str) -> bool {
        let normalized = Self::normalize_rust_namespace_path(base.trim());
        if !normalized.contains("::") {
            return false;
        }
        let leaf = normalized.rsplit("::").next().unwrap_or(normalized);
        !Self::is_rust_identity_wrapper_type_name(leaf)
    }

    /// Rust wrapper/container type names that do not carry the identity metadata this prewarm is looking for.
    pub(super) fn is_rust_identity_wrapper_type_name(name: &str) -> bool {
        matches!(
            name,
            "Arc"
                | "Box"
                | "BTreeMap"
                | "BTreeSet"
                | "Cow"
                | "HashMap"
                | "HashSet"
                | "Option"
                | "PhantomData"
                | "Pin"
                | "Rc"
                | "Result"
                | "Self"
                | "String"
                | "Vec"
        )
    }

    /// Resolve an inspected Rust return display and cache any returned Rust receiver metadata.
    pub(super) fn resolved_rust_return_type_from_sig(&self, sig: &RustFunctionSig, owner_path: &str) -> ResolvedType {
        let return_display = self.rust_display_for_owner_path(sig.return_type.as_str(), owner_path);
        let return_ty = self.resolved_type_from_rust_display(return_display.as_str());
        self.prewarm_rust_return_type_metadata(&return_ty);
        return_ty
    }

    /// Type exposed for an async Rust call when it is not the direct operand of `await`.
    ///
    /// In an `await` operand we keep the existing source-async behavior: the inner call checks to its output type and
    /// `check_await` returns that type. Outside `await`, expose the pending future as `Awaitable[T]` so consumers
    /// cannot accidentally match or unwrap `T` before awaiting the Rust future.
    pub(super) fn resolved_rust_call_type_from_sig(
        &self,
        sig: &RustFunctionSig,
        owner_path: &str,
        span: Span,
    ) -> ResolvedType {
        let return_ty = self.resolved_rust_return_type_from_sig(sig, owner_path);
        if sig.is_async && !self.is_in_await_operand(span) {
            ResolvedType::Generic("Awaitable".to_string(), vec![return_ty])
        } else {
            return_ty
        }
    }
}

/// Project native references only inside builtin Option and Result payloads. A direct reference remains a borrowed
/// value, and other generic owners, including `Take` and `RefMut`, keep their imported nominal source type.
fn native_borrowed_surface_type(ty: ResolvedType) -> ResolvedType {
    match ty {
        ResolvedType::Generic(name, arguments)
            if collections::from_str(&name)
                .is_some_and(|id| matches!(id, CollectionTypeId::Option | CollectionTypeId::Result)) =>
        {
            ResolvedType::Generic(
                name,
                arguments
                    .into_iter()
                    .map(native_borrowed_carrier_payload_type)
                    .collect(),
            )
        }
        other => other,
    }
}

/// Read through a reference payload while preserving any nested builtin carrier shape.
fn native_borrowed_carrier_payload_type(ty: ResolvedType) -> ResolvedType {
    match ty {
        ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => native_borrowed_carrier_payload_type(*inner),
        other => native_borrowed_surface_type(other),
    }
}
