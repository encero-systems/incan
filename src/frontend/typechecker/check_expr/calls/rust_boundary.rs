//! Rust boundary matching, Rust call validation, and coercion metadata recording.

use super::TypeChecker;
use crate::frontend::ast::{CallArg, Expr, ParamKind, Span, Spanned};
use crate::frontend::diagnostics::errors;
use crate::frontend::symbols::{CallableParam, ResolvedType, TypeInfo};
use crate::frontend::typechecker::helpers::collection_type_id;
use crate::frontend::typechecker::{RustArgCoercionInfo, RustArgCoercionKind};
use incan_core::interop::{CoercionPolicy, RustFunctionSig, RustParam, admitted_builtin_coercion};
use incan_core::lang::types::collections::CollectionTypeId;
use incan_core::lang::types::numerics;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RustArgBoundaryMatch {
    /// Argument already matches the Rust parameter shape; no lowering-time adapter is required.
    Exact,
    /// Argument is admissible if lowering inserts a boundary coercion/adapter.
    Coercion(RustArgCoercionKind),
    /// Argument cannot satisfy the Rust parameter shape.
    NoMatch,
}

struct RustCallArgBinding<'a> {
    arg: &'a CallArg,
    arg_ty: &'a ResolvedType,
    param: &'a RustParam,
}

impl TypeChecker {
    /// Remove an invalid namespace prefix from a Rust slice display.
    ///
    /// Some rust-analyzer callback-bound displays report `crate::[T]` (or its already-expanded
    /// `dependency::[T]` form). Rust slices are never namespaced; retaining that prefix makes generated Rust invalid.
    fn strip_invalid_rust_slice_namespace(display: &str) -> String {
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
    fn rust_callable_param_displays_from_bound(&self, rust_ty: &str) -> Option<Vec<String>> {
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
    fn record_rust_callable_parameter_displays(
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
                    .insert((arg_expr.span.start, arg_expr.span.end), param_displays);
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
    fn prewarm_rust_return_type_metadata(&self, ty: &ResolvedType) {
        self.prewarm_rust_type_identity_metadata(ty);
    }

    /// Reuse Rust identity metadata for nominal paths nested inside Rust display types.
    ///
    /// rust-inspect can report public signatures such as `Arc<crate::Type>` while another API returns the same type
    /// through its defining module, for example `Arc<crate::private::Type>`. The outer generic wrapper is not the
    /// semantic identity; the nested Rust path is. Reading prepared metadata for those nested paths lets compatibility
    /// use known definition aliases without doing hidden extraction from this hot path.
    fn prewarm_rust_type_identity_metadata(&self, ty: &ResolvedType) {
        match ty {
            ResolvedType::RustPath(path) => {
                let (base, args) = self.rust_path_base_and_args(path);
                if Self::rust_identity_metadata_base_should_probe(base.as_str()) {
                    let _ = self.rust_item_metadata_for_path(base.as_str());
                }
                for arg in args {
                    self.prewarm_rust_type_identity_metadata(&arg);
                }
            }
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => self.prewarm_rust_type_identity_metadata(inner),
            ResolvedType::Generic(_, args) | ResolvedType::Tuple(args) => {
                for arg in args {
                    self.prewarm_rust_type_identity_metadata(arg);
                }
            }
            ResolvedType::FrozenList(inner) | ResolvedType::FrozenSet(inner) => {
                self.prewarm_rust_type_identity_metadata(inner);
            }
            ResolvedType::FrozenDict(key, value) => {
                self.prewarm_rust_type_identity_metadata(key);
                self.prewarm_rust_type_identity_metadata(value);
            }
            ResolvedType::Function(params, ret) => {
                for param in params {
                    self.prewarm_rust_type_identity_metadata(&param.ty);
                }
                self.prewarm_rust_type_identity_metadata(ret);
            }
            _ => {}
        }
    }

    /// Return whether cache-only identity prewarm should ask rust-inspect for this Rust display base.
    ///
    /// This prewarm exists only to reuse already-known nominal metadata for returned opaque handles. Standard generic
    /// wrappers such as `Box<T>` and `Arc<T>` are not the semantic identity, and rust-analyzer sometimes renders them
    /// relative to a dependency module (`datafusion_expr::expr::Box`). Probing those wrapper bases as dependency items
    /// turns a cheap compatibility hint into a full workspace extraction. Skip the wrapper and keep recursing into its
    /// type arguments, where the actual nominal identity lives.
    pub(in crate::frontend::typechecker) fn rust_identity_metadata_base_should_probe(base: &str) -> bool {
        let normalized = Self::normalize_rust_namespace_path(base.trim());
        if !normalized.contains("::") {
            return false;
        }
        let leaf = normalized.rsplit("::").next().unwrap_or(normalized);
        !Self::is_rust_identity_wrapper_type_name(leaf)
    }

    /// Rust wrapper/container type names that do not carry the identity metadata this prewarm is looking for.
    fn is_rust_identity_wrapper_type_name(name: &str) -> bool {
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
    fn resolved_rust_return_type_from_sig(&self, sig: &RustFunctionSig, owner_path: &str) -> ResolvedType {
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
    fn resolved_rust_call_type_from_sig(&self, sig: &RustFunctionSig, owner_path: &str, span: Span) -> ResolvedType {
        let return_ty = self.resolved_rust_return_type_from_sig(sig, owner_path);
        if sig.is_async && !self.is_in_await_operand(span) {
            ResolvedType::Generic("Awaitable".to_string(), vec![return_ty])
        } else {
            return_ty
        }
    }

    /// Substitute inspected Rust generic placeholders from a compatible expected result shape.
    ///
    /// Rust metadata names unbound function generics T, U, and similar. These are not Incan type variables, but an
    /// expected result such as Result[Value, _] supplies an unambiguous binding for the matching return position.
    fn refine_unbound_rust_return_type_with_expected(actual: &ResolvedType, expected: &ResolvedType) -> ResolvedType {
        match actual {
            ResolvedType::RustPath(path) if Self::rust_display_type_var_name(path.trim()).is_some() => expected.clone(),
            ResolvedType::Ref(inner) => match expected {
                ResolvedType::Ref(expected_inner) => ResolvedType::Ref(Box::new(
                    Self::refine_unbound_rust_return_type_with_expected(inner, expected_inner),
                )),
                _ => actual.clone(),
            },
            ResolvedType::RefMut(inner) => match expected {
                ResolvedType::RefMut(expected_inner) => ResolvedType::RefMut(Box::new(
                    Self::refine_unbound_rust_return_type_with_expected(inner, expected_inner),
                )),
                _ => actual.clone(),
            },
            ResolvedType::Generic(name, args) => match expected {
                ResolvedType::Generic(expected_name, expected_args)
                    if name == expected_name && args.len() == expected_args.len() =>
                {
                    ResolvedType::Generic(
                        name.clone(),
                        args.iter()
                            .zip(expected_args)
                            .map(|(actual, expected)| {
                                Self::refine_unbound_rust_return_type_with_expected(actual, expected)
                            })
                            .collect(),
                    )
                }
                _ => actual.clone(),
            },
            ResolvedType::Tuple(items) => match expected {
                ResolvedType::Tuple(expected_items) if items.len() == expected_items.len() => ResolvedType::Tuple(
                    items
                        .iter()
                        .zip(expected_items)
                        .map(|(actual, expected)| Self::refine_unbound_rust_return_type_with_expected(actual, expected))
                        .collect(),
                ),
                _ => actual.clone(),
            },
            _ => actual.clone(),
        }
    }

    /// Record an ownership conversion for borrowed Rust scalar-like returns that Incan exposes as owned values.
    pub(in crate::frontend::typechecker) fn record_rust_return_coercion_from_display(
        &mut self,
        rust_return_type: &str,
        incan_ret: &ResolvedType,
        span: Span,
    ) {
        let normalized = rust_return_type.replace(' ', "");
        let is_borrowed_str = normalized == "&str" || (normalized.starts_with("&'") && normalized.ends_with("str"));
        let target = if is_borrowed_str && matches!(incan_ret, ResolvedType::Str) {
            Some(("String", ResolvedType::Str))
        } else {
            let is_borrowed_bytes =
                normalized == "&[u8]" || (normalized.starts_with("&'") && normalized.ends_with("[u8]"));
            if is_borrowed_bytes && matches!(incan_ret, ResolvedType::Bytes) {
                Some(("Vec<u8>", ResolvedType::Bytes))
            } else {
                None
            }
        };
        let Some((rust_target_type, target_type)) = target else {
            return;
        };
        self.type_info.rust.return_coercions.insert(
            (span.start, span.end),
            RustArgCoercionInfo {
                rust_target_type: rust_target_type.to_string(),
                target_type,
                kind: RustArgCoercionKind::Builtin(CoercionPolicy::Exact),
            },
        );
    }

    /// Return how a rusttype boundary matches an argument type.
    fn rusttype_boundary_match(&self, arg_ty: &ResolvedType, target_ty: &ResolvedType) -> Option<RustArgCoercionKind> {
        if let ResolvedType::Named(type_name) = arg_ty
            && let Some(TypeInfo::Newtype(newtype)) = self.lookup_type_info(type_name)
            && newtype.is_rusttype
        {
            if self.types_compatible(&newtype.underlying, target_ty) {
                return Some(RustArgCoercionKind::RustTypeUnwrap);
            }
            if newtype.has_interop {
                return Some(RustArgCoercionKind::RustTypeInterop);
            }
        }

        // RFC 041 (`from S ...`) edges convert non-rusttype arguments into a rusttype surface. Type checking marks
        // these as interop-capable when the target is a rusttype with declared interop, and lowering picks the concrete
        // adapter edge.
        if let ResolvedType::Named(type_name) = target_ty
            && let Some(TypeInfo::Newtype(newtype)) = self.lookup_type_info(type_name)
            && newtype.is_rusttype
            && newtype.has_interop
        {
            return Some(RustArgCoercionKind::RustTypeInterop);
        }

        None
    }

    /// Return whether a Rust type display names a generic type parameter.
    fn is_rust_generic_type_param_display(rust_ty: &str) -> bool {
        let normalized = rust_ty.trim().replace(' ', "");
        if let Some(tail) = normalized.strip_prefix("impl") {
            return !tail.is_empty()
                && (tail.contains("::") || tail.chars().next().is_some_and(|ch| ch.is_ascii_uppercase()));
        }
        let mut chars = normalized.chars();
        let Some(first) = chars.next() else {
            return false;
        };
        if !first.is_ascii_uppercase() || !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
            return false;
        }
        !matches!(
            normalized.as_str(),
            "Box" | "HashMap" | "HashSet" | "Option" | "Result" | "Self" | "String" | "Vec"
        )
    }

    /// Return whether a borrowed Rust parameter is a dynamically-dispatched trait object.
    ///
    /// Trait-object compatibility is established from rust-inspect metadata, but Rust still requires the reference
    /// syntax at the call site. Recording it here keeps typechecking and generated-Rust ownership in agreement.
    fn rust_display_is_trait_object(rust_ty: &str) -> bool {
        rust_ty.trim_start().starts_with("dyn ")
    }

    /// Render an Incan type into the canonical boundary vocabulary used by interop coercion policy lookup.
    ///
    /// Returns `None` for shapes not covered by the builtin boundary coercion matrix.
    fn incan_boundary_type_display(arg_ty: &ResolvedType) -> Option<String> {
        match arg_ty {
            ResolvedType::Int => Some("int".to_string()),
            ResolvedType::Float => Some("float".to_string()),
            ResolvedType::Numeric(id) => Some(numerics::as_str(*id).to_string()),
            ResolvedType::Bool => Some("bool".to_string()),
            ResolvedType::Str => Some("str".to_string()),
            ResolvedType::FrozenStr => Some("FrozenStr".to_string()),
            ResolvedType::Bytes => Some("bytes".to_string()),
            ResolvedType::FrozenBytes => Some("FrozenBytes".to_string()),
            ResolvedType::Unit => Some("unit".to_string()),
            ResolvedType::FrozenList(elem) => {
                let elem_display = Self::incan_boundary_type_display(elem)?;
                Some(format!("FrozenList[{elem_display}]"))
            }
            ResolvedType::FrozenDict(key, value) => {
                let key_display = Self::incan_boundary_type_display(key)?;
                let value_display = Self::incan_boundary_type_display(value)?;
                Some(format!("FrozenDict[{key_display}, {value_display}]"))
            }
            ResolvedType::FrozenSet(elem) => {
                let elem_display = Self::incan_boundary_type_display(elem)?;
                Some(format!("FrozenSet[{elem_display}]"))
            }
            ResolvedType::Generic(name, args)
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Option) && args.len() == 1 =>
            {
                let inner = Self::incan_boundary_type_display(&args[0])?;
                Some(format!("Option[{inner}]"))
            }
            ResolvedType::Generic(name, args)
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Result) && args.len() == 2 =>
            {
                let ok = Self::incan_boundary_type_display(&args[0])?;
                let err = Self::incan_boundary_type_display(&args[1])?;
                Some(format!("Result[{ok}, {err}]"))
            }
            ResolvedType::Generic(name, args)
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::List) && args.len() == 1 =>
            {
                let inner = Self::incan_boundary_type_display(&args[0])?;
                Some(format!("List[{inner}]"))
            }
            ResolvedType::Generic(name, args)
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Dict) && args.len() == 2 =>
            {
                let key = Self::incan_boundary_type_display(&args[0])?;
                let value = Self::incan_boundary_type_display(&args[1])?;
                Some(format!("Dict[{key}, {value}]"))
            }
            ResolvedType::Generic(name, args)
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Set) && args.len() == 1 =>
            {
                let inner = Self::incan_boundary_type_display(&args[0])?;
                Some(format!("Set[{inner}]"))
            }
            ResolvedType::Tuple(elems) => {
                let mut rendered = Vec::with_capacity(elems.len());
                for elem in elems {
                    rendered.push(Self::incan_boundary_type_display(elem)?);
                }
                Some(format!("Tuple[{}]", rendered.join(", ")))
            }
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => Self::incan_boundary_type_display(inner),
            _ => None,
        }
    }

    /// Whether a Rust type display string belongs to the builtin boundary coercion matrix.
    ///
    /// This intentionally includes fully-qualified std/core/alloc spellings that rust-analyzer may emit.
    fn is_builtin_rust_boundary_display(rust_ty: &str) -> bool {
        matches!(
            rust_ty,
            "bool"
                | "f32"
                | "f64"
                | "i8"
                | "i16"
                | "i32"
                | "i64"
                | "i128"
                | "isize"
                | "u8"
                | "u16"
                | "u32"
                | "u64"
                | "u128"
                | "usize"
                | "String"
                | "std::string::String"
                | "&str"
                | "Vec<u8>"
                | "std::vec::Vec<u8>"
                | "&[u8]"
                | "()"
        ) || rust_ty.starts_with("Option<")
            || rust_ty.starts_with("std::option::Option<")
            || rust_ty.starts_with("core::option::Option<")
            || rust_ty.starts_with("Result<")
            || rust_ty.starts_with("std::result::Result<")
            || rust_ty.starts_with("core::result::Result<")
            || rust_ty.starts_with("Vec<")
            || rust_ty.starts_with("std::vec::Vec<")
            || rust_ty.starts_with("alloc::vec::Vec<")
            || rust_ty.starts_with("HashMap<")
            || rust_ty.starts_with("std::collections::HashMap<")
            || rust_ty.starts_with("std::collections::hash_map::HashMap<")
            || rust_ty.starts_with("HashSet<")
            || rust_ty.starts_with("std::collections::HashSet<")
            || rust_ty.starts_with("std::collections::hash_set::HashSet<")
            || (rust_ty.starts_with('(') && rust_ty.ends_with(')'))
    }

    /// Classify whether an Incan argument type can satisfy a Rust parameter boundary.
    ///
    /// This first tries builtin coercion-matrix matches, then resolved-type compatibility, then rusttype-specific
    /// boundary adapters.
    fn rust_arg_boundary_match(&self, arg_ty: &ResolvedType, rust_param_ty: &str) -> RustArgBoundaryMatch {
        let display = Self::rust_display_without_lifetimes(rust_param_ty);
        let normalized = display.replace(' ', "");
        if let Some((is_mut, inner)) = Self::rust_display_borrow_kind(display.as_str())
            && Self::rust_display_is_trait_object(inner)
        {
            let target_inner_ty = self.resolved_type_from_rust_display(inner);
            return if self.types_compatible(arg_ty, &target_inner_ty)
                || self.rust_type_implements_trait_object(arg_ty, inner)
            {
                RustArgBoundaryMatch::Coercion(RustArgCoercionKind::TraitObjectBorrow { mutable: is_mut })
            } else {
                RustArgBoundaryMatch::NoMatch
            };
        }
        if let Some(target_ty) = self.resolved_function_type_from_rust_callable_bound_display(display.as_str()) {
            return if self.types_compatible(arg_ty, &target_ty) {
                RustArgBoundaryMatch::Exact
            } else {
                RustArgBoundaryMatch::NoMatch
            };
        }
        if Self::rust_display_type_var_name(normalized.as_str()).is_some() {
            return RustArgBoundaryMatch::Exact;
        }
        let borrowed_shared = matches!(Self::rust_display_borrow_kind(display.as_str()), Some((false, _)));
        if let Some((is_mut, inner)) = Self::rust_display_borrow_kind(display.as_str()) {
            let inner_normalized = Self::compact_rust_display(inner);
            let target_inner_ty = self.resolved_type_from_rust_display(inner_normalized.as_str());
            if matches!(arg_ty, ResolvedType::RefMut(existing) if self.types_compatible(existing, &target_inner_ty))
                || (!is_mut
                    && matches!(arg_ty, ResolvedType::Ref(existing) if self.types_compatible(existing, &target_inner_ty)))
            {
                return RustArgBoundaryMatch::Exact;
            }
            if Self::is_rust_generic_type_param_display(inner_normalized.as_str())
                && !is_mut
                && !matches!(arg_ty, ResolvedType::Ref(_) | ResolvedType::RefMut(_))
            {
                return RustArgBoundaryMatch::Exact;
            }
            if !is_mut
                && Self::incan_boundary_type_display(arg_ty).is_none()
                && self.types_compatible(arg_ty, &target_inner_ty)
            {
                return RustArgBoundaryMatch::Coercion(RustArgCoercionKind::Borrow { mutable: false });
            }
            if is_mut {
                if self.types_compatible(arg_ty, &target_inner_ty) {
                    return RustArgBoundaryMatch::Coercion(RustArgCoercionKind::Borrow { mutable: true });
                }
                if let Some(incan_display) = Self::incan_boundary_type_display(arg_ty)
                    && let Some(CoercionPolicy::Exact) =
                        admitted_builtin_coercion(incan_display.as_str(), inner_normalized.as_str())
                {
                    return RustArgBoundaryMatch::Exact;
                }
            }
        }
        if let Some(incan_display) = Self::incan_boundary_type_display(arg_ty)
            && Self::is_builtin_rust_boundary_display(normalized.as_str())
        {
            return match admitted_builtin_coercion(incan_display.as_str(), normalized.as_str()) {
                Some(CoercionPolicy::Exact) => RustArgBoundaryMatch::Exact,
                Some(policy) => RustArgBoundaryMatch::Coercion(RustArgCoercionKind::Builtin(policy)),
                None => RustArgBoundaryMatch::NoMatch,
            };
        }
        let target_ty = self.resolved_type_from_rust_display(normalized.as_str());
        let should_try_exact_type_match = !borrowed_shared || Self::incan_boundary_type_display(arg_ty).is_none();
        if should_try_exact_type_match && self.types_compatible(arg_ty, &target_ty) {
            return RustArgBoundaryMatch::Exact;
        }
        if let Some(kind) = self.rusttype_boundary_match(arg_ty, &target_ty) {
            return RustArgBoundaryMatch::Coercion(kind);
        }
        if let Some(incan_name) = Self::incan_boundary_type_display(arg_ty)
            && let Some(policy) = admitted_builtin_coercion(incan_name.as_str(), normalized.as_str())
        {
            return RustArgBoundaryMatch::Coercion(RustArgCoercionKind::Builtin(policy));
        }
        RustArgBoundaryMatch::NoMatch
    }

    /// Return whether complete Rust metadata proves that an argument type implements a trait-object boundary.
    fn rust_type_implements_trait_object(&self, arg_ty: &ResolvedType, trait_object_display: &str) -> bool {
        let Some(trait_path) = trait_object_display.trim().strip_prefix("dyn ").map(str::trim) else {
            return false;
        };
        let Some((actual_path, _, _)) = self.rust_identity_for_type(arg_ty) else {
            return false;
        };
        let Some(metadata) = self.rust_item_metadata_for_complete_type(actual_path.as_str()) else {
            return false;
        };
        let trait_definition = self.rust_definition_for_path(trait_path);
        Self::rust_type_metadata_implements_trait(&metadata, trait_path, trait_definition.as_deref())
    }

    /// Reject an immutable source binding when a planned Rust boundary borrow is exclusive.
    fn validate_rust_borrow_mutability(&mut self, kind: RustArgCoercionKind, arg_expr: &Spanned<Expr>) -> bool {
        if matches!(
            kind,
            RustArgCoercionKind::Borrow { mutable: true } | RustArgCoercionKind::TraitObjectBorrow { mutable: true }
        ) && let Expr::Ident(name) = &arg_expr.node
            && !self.mutable_bindings.contains(name)
        {
            self.errors
                .push(errors::mutable_rust_borrow_requires_mut(name, arg_expr.span));
            return false;
        }
        true
    }

    /// Validate one expression used at a Rust value boundary and record the exact coercion lowering must preserve.
    ///
    /// This is the shared Rust-boundary argument plan for free functions, methods, and named-field Rust constructors.
    /// Callers provide the Rust display from metadata; this method resolves owner-relative displays, prewarms identity
    /// metadata, and records the chosen coercion in the same artifact map lowering already consumes.
    pub(in crate::frontend::typechecker) fn validate_rust_boundary_value(
        &mut self,
        owner_path: &str,
        rust_type_display: &str,
        arg_expr: &Spanned<Expr>,
        arg_ty: &ResolvedType,
        record_exact_builtin_coercion: bool,
    ) {
        self.record_rust_callable_parameter_displays(rust_type_display, owner_path, arg_expr, arg_ty);
        let param_display = self.rust_display_for_owner_path(rust_type_display, owner_path);
        let normalized = param_display.replace(' ', "");
        let target_ty =
            self.resolved_rust_boundary_target_from_param_display_for_owner_path(rust_type_display, owner_path);
        self.prewarm_rust_type_identity_metadata(arg_ty);
        self.prewarm_rust_type_identity_metadata(&target_ty);
        match self.rust_arg_boundary_match(arg_ty, param_display.as_str()) {
            RustArgBoundaryMatch::Exact => {
                if record_exact_builtin_coercion
                    && let Some(incan_display) = Self::incan_boundary_type_display(arg_ty)
                    && admitted_builtin_coercion(incan_display.as_str(), normalized.as_str())
                        == Some(CoercionPolicy::Exact)
                {
                    self.type_info.rust.arg_coercions.insert(
                        (arg_expr.span.start, arg_expr.span.end),
                        RustArgCoercionInfo {
                            rust_target_type: normalized,
                            target_type: target_ty,
                            kind: RustArgCoercionKind::Builtin(CoercionPolicy::Exact),
                        },
                    );
                }
            }
            RustArgBoundaryMatch::Coercion(kind) => {
                if !self.validate_rust_borrow_mutability(kind, arg_expr) {
                    return;
                }
                self.type_info.rust.arg_coercions.insert(
                    (arg_expr.span.start, arg_expr.span.end),
                    RustArgCoercionInfo {
                        rust_target_type: normalized,
                        target_type: target_ty,
                        kind,
                    },
                );
            }
            RustArgBoundaryMatch::NoMatch => {
                self.errors.push(errors::type_mismatch(
                    rust_type_display,
                    &arg_ty.to_string(),
                    arg_expr.span,
                ));
            }
        }
    }

    /// Record inspected Rust parameter types so codegen can emit the same borrow shape the typechecker accepted.
    fn rust_params_as_callable_params(
        &self,
        params: &[incan_core::interop::RustParam],
        owner_path: &str,
    ) -> Vec<CallableParam> {
        let params: Vec<CallableParam> = params
            .iter()
            .map(|param| {
                let ty =
                    self.resolved_param_type_from_rust_display_for_owner_path(param.type_display.as_str(), owner_path);
                CallableParam {
                    name: param.name.clone(),
                    ty,
                    kind: ParamKind::Normal,
                    has_default: false,
                }
            })
            .collect();
        params
    }

    /// Record inspected Rust parameter types so codegen can emit the same borrow shape the typechecker accepted.
    fn record_rust_call_site_params(
        &mut self,
        span: Span,
        params: &[incan_core::interop::RustParam],
        owner_path: &str,
        force_exact: bool,
    ) {
        let params: Vec<CallableParam> = params
            .iter()
            .map(|param| {
                let ty = self.resolved_rust_boundary_target_from_param_display_for_owner_path(
                    param.type_display.as_str(),
                    owner_path,
                );
                CallableParam {
                    name: param.name.clone(),
                    ty,
                    kind: ParamKind::Normal,
                    has_default: false,
                }
            })
            .collect();
        // Plain Rust type variables carry by-value shape, but they are not ordinary borrow-boundary snapshots.
        if force_exact || params.iter().any(|param| matches!(param.ty, ResolvedType::TypeVar(_))) {
            self.type_info.record_call_site_callable_params_exact(span, &params);
        } else {
            self.type_info.record_call_site_callable_params(span, &params);
        }
    }

    /// Bind Incan call arguments to a Rust function signature.
    fn bind_rust_call_args<'a>(
        &mut self,
        callable_display: &str,
        params: &'a [RustParam],
        args: &'a [CallArg],
        arg_types: &'a [ResolvedType],
        span: Span,
    ) -> Vec<RustCallArgBinding<'a>> {
        let has_keyword_args = args
            .iter()
            .any(|arg| matches!(arg, CallArg::Named(_, _) | CallArg::KeywordUnpack(_)));
        let has_unpack_args = args
            .iter()
            .any(|arg| matches!(arg, CallArg::PositionalUnpack(_) | CallArg::KeywordUnpack(_)));
        if !has_keyword_args || has_unpack_args {
            if arg_types.len() != params.len() {
                self.errors.push(errors::builtin_arity(
                    callable_display,
                    params.len(),
                    arg_types.len(),
                    span,
                ));
                return Vec::new();
            }
            return args
                .iter()
                .zip(arg_types.iter())
                .zip(params.iter())
                .map(|((arg, arg_ty), param)| RustCallArgBinding { arg, arg_ty, param })
                .collect();
        }

        let mut params_by_name = std::collections::HashMap::new();
        for (idx, param) in params.iter().enumerate() {
            if let Some(name) = param.name.as_deref() {
                params_by_name.insert(name, idx);
            }
        }

        let mut bound_spans: Vec<Option<Span>> = vec![None; params.len()];
        let mut named_seen: std::collections::HashMap<&str, Span> = std::collections::HashMap::new();
        let mut positional_index = 0usize;
        let mut unexpected_positional = 0usize;
        let mut bindings = Vec::new();

        for (arg, arg_ty) in args.iter().zip(arg_types.iter()) {
            let arg_span = Self::call_arg_expr(arg).span;
            match arg {
                CallArg::Positional(_) => {
                    if positional_index >= params.len() {
                        unexpected_positional += 1;
                        continue;
                    }
                    let param = &params[positional_index];
                    if let Some(bound_span) = bound_spans[positional_index] {
                        let name = param.name.as_deref().unwrap_or("<positional>");
                        self.errors.push(errors::duplicate_call_argument(
                            callable_display,
                            name,
                            bound_span,
                            arg_span,
                        ));
                    } else {
                        bound_spans[positional_index] = Some(arg_span);
                        bindings.push(RustCallArgBinding { arg, arg_ty, param });
                    }
                    positional_index += 1;
                }
                CallArg::Named(name, _) => {
                    if let Some(first_span) = named_seen.insert(name.as_str(), arg_span) {
                        self.errors.push(errors::duplicate_call_argument(
                            callable_display,
                            name,
                            first_span,
                            arg_span,
                        ));
                    }
                    let Some(param_index) = params_by_name.get(name.as_str()).copied() else {
                        self.errors
                            .push(errors::unknown_keyword_argument(callable_display, name, arg_span));
                        continue;
                    };
                    if let Some(first_span) = bound_spans[param_index] {
                        self.errors.push(errors::duplicate_call_argument(
                            callable_display,
                            name,
                            first_span,
                            arg_span,
                        ));
                        continue;
                    }
                    let param = &params[param_index];
                    bound_spans[param_index] = Some(arg_span);
                    bindings.push(RustCallArgBinding { arg, arg_ty, param });
                }
                CallArg::PositionalUnpack(_) | CallArg::KeywordUnpack(_) => {}
            }
        }

        if unexpected_positional > 0 {
            self.errors.push(errors::builtin_arity(
                callable_display,
                params.len(),
                params.len() + unexpected_positional,
                span,
            ));
        }

        let mut missing_unnamed_param = false;
        for (idx, param) in params.iter().enumerate() {
            if bound_spans[idx].is_some() {
                continue;
            }
            if let Some(name) = param.name.as_deref() {
                self.errors
                    .push(errors::missing_required_argument(callable_display, name, span));
            } else {
                missing_unnamed_param = true;
            }
        }
        if missing_unnamed_param {
            self.errors
                .push(errors::builtin_arity(callable_display, params.len(), args.len(), span));
        }

        bindings
    }

    /// Return whether a lookup-style Rust method should preserve the probe argument's emitted shape.
    fn rust_lookup_probe_boundary_match(&self, arg_ty: &ResolvedType, target_ty: &ResolvedType) -> bool {
        let ResolvedType::Ref(inner) = target_ty else {
            return false;
        };
        match arg_ty {
            ResolvedType::Str | ResolvedType::FrozenStr => {
                matches!(
                    inner.as_ref(),
                    ResolvedType::Str | ResolvedType::RustPath(_) | ResolvedType::TypeVar(_)
                )
            }
            ResolvedType::Bytes | ResolvedType::FrozenBytes => {
                matches!(inner.as_ref(), ResolvedType::RustPath(_) | ResolvedType::TypeVar(_))
            }
            _ => false,
        }
    }

    /// Return whether an argument can cross a Rust boundary.
    #[cfg(test)]
    pub(in crate::frontend::typechecker) fn rust_arg_matches_boundary(
        &self,
        arg_ty: &ResolvedType,
        rust_param_ty: &str,
    ) -> bool {
        !matches!(
            self.rust_arg_boundary_match(arg_ty, rust_param_ty),
            RustArgBoundaryMatch::NoMatch
        )
    }

    /// Validate a Rust method call (`receiver.method(...)`) against metadata and record required arg coercions.
    ///
    /// The receiver is already validated by access resolution; this function validates only post-receiver parameters.
    pub(in crate::frontend::typechecker) fn validate_rust_method_call(
        &mut self,
        callable_display: &str,
        sig: &RustFunctionSig,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        preserves_lookup_arg_shape: bool,
        span: Span,
    ) -> ResolvedType {
        if sig.is_async {
            self.type_info
                .rust
                .async_call_realizations
                .insert((span.start, span.end));
        }
        if sig.is_async && !self.is_in_await_operand(span) {
            self.errors
                .push(errors::async_call_without_await(callable_display, span));
        }
        let params = if Self::rust_signature_has_receiver(sig) {
            &sig.params[1..]
        } else {
            &sig.params
        };
        let has_keyword_args = args
            .iter()
            .any(|arg| matches!(arg, CallArg::Named(_, _) | CallArg::KeywordUnpack(_)));
        self.record_rust_call_site_params(span, params, callable_display, has_keyword_args);

        let binding_errors_before = self.errors.len();
        let bindings = self.bind_rust_call_args(callable_display, params, args, arg_types, span);
        if self.errors.len() != binding_errors_before {
            return self.resolved_rust_call_type_from_sig(sig, callable_display, span);
        }

        for binding in bindings {
            let arg_expr = Self::call_arg_expr(binding.arg);
            let arg_ty = binding.arg_ty;
            let param = binding.param;
            let param_display = self.rust_display_for_owner_path(param.type_display.as_str(), callable_display);
            let normalized = param_display.replace(' ', "");
            let target_ty = self.resolved_rust_boundary_target_from_param_display_for_owner_path(
                param.type_display.as_str(),
                callable_display,
            );
            self.prewarm_rust_type_identity_metadata(arg_ty);
            self.prewarm_rust_type_identity_metadata(&target_ty);
            if preserves_lookup_arg_shape && self.rust_lookup_probe_boundary_match(arg_ty, &target_ty) {
                continue;
            }
            match self.rust_arg_boundary_match(arg_ty, param_display.as_str()) {
                RustArgBoundaryMatch::Exact => {}
                RustArgBoundaryMatch::Coercion(kind) => {
                    if !self.validate_rust_borrow_mutability(kind, arg_expr) {
                        continue;
                    }
                    self.type_info.rust.arg_coercions.insert(
                        (arg_expr.span.start, arg_expr.span.end),
                        RustArgCoercionInfo {
                            rust_target_type: normalized,
                            target_type: target_ty,
                            kind,
                        },
                    );
                }
                RustArgBoundaryMatch::NoMatch => {
                    self.errors.push(errors::type_mismatch(
                        param.type_display.as_str(),
                        &arg_ty.to_string(),
                        arg_expr.span,
                    ));
                }
            }
        }

        let ret = self.resolved_rust_call_type_from_sig(sig, callable_display, span);
        self.record_rust_return_coercion_from_display(sig.return_type.as_str(), &ret, span);
        ret
    }

    /// Validate a direct Rust function call (`rust::path::item(...)`) and record boundary coercions.
    #[cfg(test)]
    pub(in crate::frontend::typechecker::check_expr) fn validate_rust_function_call(
        &mut self,
        path: &str,
        sig: &RustFunctionSig,
        args: &[CallArg],
        span: Span,
    ) -> ResolvedType {
        self.validate_rust_function_call_with_expected(path, sig, args, span, None)
    }

    /// Validate a direct Rust function call while using an expected result type to bind return-only generics.
    pub(in crate::frontend::typechecker::check_expr) fn validate_rust_function_call_with_expected(
        &mut self,
        path: &str,
        sig: &RustFunctionSig,
        args: &[CallArg],
        span: Span,
        expected_return_ty: Option<&ResolvedType>,
    ) -> ResolvedType {
        if sig.is_async {
            self.type_info
                .rust
                .async_call_realizations
                .insert((span.start, span.end));
        }
        if sig.is_async && !self.is_in_await_operand(span) {
            self.errors.push(errors::async_call_without_await(path, span));
        }
        let expected_params = self.rust_params_as_callable_params(&sig.params, path);
        let arg_types = self.check_call_arg_types_for_params(args, &expected_params);
        let has_keyword_args = args
            .iter()
            .any(|arg| matches!(arg, CallArg::Named(_, _) | CallArg::KeywordUnpack(_)));
        self.record_rust_call_site_params(span, &sig.params, path, has_keyword_args);
        let binding_errors_before = self.errors.len();
        let bindings = self.bind_rust_call_args(path, &sig.params, args, &arg_types, span);
        if self.errors.len() != binding_errors_before {
            return self.resolved_rust_call_type_from_sig(sig, path, span);
        }

        for binding in bindings {
            let arg_expr = Self::call_arg_expr(binding.arg);
            let arg_ty = binding.arg_ty;
            let param = binding.param;
            self.validate_rust_boundary_value(path, param.type_display.as_str(), arg_expr, arg_ty, false);
        }

        let ret = self.resolved_rust_call_type_from_sig(sig, path, span);
        let ret = expected_return_ty
            .map(|expected| Self::refine_unbound_rust_return_type_with_expected(&ret, expected))
            .unwrap_or(ret);
        self.record_rust_return_coercion_from_display(sig.return_type.as_str(), &ret, span);
        ret
    }
}

#[cfg(test)]
mod validate_rust_function_call_tests {
    use super::{RustArgBoundaryMatch, TypeChecker};
    use crate::frontend::ast::{CallArg, Expr, IntLiteral, Literal, Span, Spanned};
    use crate::frontend::symbols::{
        CallableParam, NewtypeInfo, ResolvedType, ScopeKind, Symbol, SymbolKind, TypeInfo, VariableInfo,
    };
    use crate::frontend::typechecker::RustArgCoercionKind;
    use incan_core::interop::{RustFunctionSig, RustParam};
    use incan_core::lang::types::numerics::NumericTypeId;
    use std::collections::HashMap;

    #[cfg(feature = "rust_inspect")]
    fn scalar_udf_metadata(path: &str, definition_path: &str) -> incan_core::interop::RustItemMetadata {
        use incan_core::interop::{RustItemKind, RustTypeInfo, RustVisibility};

        incan_core::interop::RustItemMetadata {
            canonical_path: path.to_string(),
            definition_path: Some(definition_path.to_string()),
            visibility: RustVisibility::Public,
            kind: RustItemKind::Type(RustTypeInfo {
                alias_target: None,
                metadata_completeness: Default::default(),
                methods: Vec::new(),
                implemented_traits: Vec::new(),
                fields: Vec::new(),
                variants: Vec::new(),
            }),
        }
    }

    #[test]
    fn rust_identity_prewarm_skips_owner_relative_standard_wrappers() {
        assert!(
            !TypeChecker::rust_identity_metadata_base_should_probe("datafusion_expr::expr::Box"),
            "owner-relative Box displays are generic wrappers, not dependency metadata items"
        );
        assert!(
            !TypeChecker::rust_identity_metadata_base_should_probe("datafusion_expr::expr::Arc"),
            "owner-relative Arc displays are generic wrappers, not dependency metadata items"
        );
        assert!(
            !TypeChecker::rust_identity_metadata_base_should_probe("std::option::Option"),
            "std wrapper displays should not trigger identity metadata prewarm"
        );
        assert!(
            TypeChecker::rust_identity_metadata_base_should_probe("datafusion_expr::expr::WindowFunction"),
            "nominal dependency item displays should still reuse cache-only identity metadata"
        );
    }

    #[test]
    fn rust_concrete_reference_parameters_record_the_required_borrow_shape() {
        let checker = TypeChecker::new();
        let header = ResolvedType::RustPath("demo::Header".to_string());

        assert_eq!(
            checker.rust_arg_boundary_match(&header, "&demo::Header"),
            RustArgBoundaryMatch::Coercion(RustArgCoercionKind::Borrow { mutable: false })
        );
        assert_eq!(
            checker.rust_arg_boundary_match(&header, "&mut demo::Header"),
            RustArgBoundaryMatch::Coercion(RustArgCoercionKind::Borrow { mutable: true })
        );
        assert_eq!(
            checker.rust_arg_boundary_match(&ResolvedType::Ref(Box::new(header.clone())), "&demo::Header"),
            RustArgBoundaryMatch::Exact
        );
        assert_eq!(
            checker.rust_arg_boundary_match(&ResolvedType::RefMut(Box::new(header)), "&mut demo::Header"),
            RustArgBoundaryMatch::Exact
        );
    }

    #[test]
    fn rust_trait_object_parameters_record_the_required_borrow_shape() {
        let checker = TypeChecker::new();
        let adapter = checker.resolved_type_from_rust_display("dyn demo::Adapter");
        let adapter_mut = checker.resolved_type_from_rust_display("dyn demo::AdapterMut");

        assert_eq!(
            checker.rust_arg_boundary_match(&adapter, "&dyn demo::Adapter"),
            RustArgBoundaryMatch::Coercion(RustArgCoercionKind::TraitObjectBorrow { mutable: false })
        );
        assert_eq!(
            checker.rust_arg_boundary_match(&adapter_mut, "&mut dyn demo::AdapterMut"),
            RustArgBoundaryMatch::Coercion(RustArgCoercionKind::TraitObjectBorrow { mutable: true })
        );
    }

    #[test]
    fn mutable_rust_reference_rejects_an_immutable_binding() {
        let span = Span::new(10, 16);
        let header = ResolvedType::RustPath("demo::Header".to_string());
        let argument = Spanned::new(Expr::Ident("header".to_string()), span);

        let mut immutable_checker = TypeChecker::new();
        immutable_checker.validate_rust_boundary_value("demo::Writer", "&mut demo::Header", &argument, &header, false);
        assert!(
            immutable_checker.errors.iter().any(|error| error
                .message
                .contains("Rust parameter requires a mutable borrow of 'header'")),
            "expected an immutable-binding diagnostic, got {:?}",
            immutable_checker.errors
        );

        let mut mutable_checker = TypeChecker::new();
        mutable_checker.mutable_bindings.insert("header".to_string());
        mutable_checker.validate_rust_boundary_value("demo::Writer", "&mut demo::Header", &argument, &header, false);
        assert!(
            mutable_checker.errors.is_empty(),
            "mutable binding should satisfy a Rust mutable reference, got {:?}",
            mutable_checker.errors
        );
        assert_eq!(
            mutable_checker
                .type_info
                .rust
                .arg_coercions
                .get(&(span.start, span.end))
                .map(|coercion| coercion.kind),
            Some(RustArgCoercionKind::Borrow { mutable: true })
        );
    }

    #[test]
    fn rust_method_mutable_reference_rejects_an_immutable_binding() {
        let span = Span::new(10, 16);
        let argument = Spanned::new(Expr::Ident("header".to_string()), span);
        let args = [CallArg::Positional(argument)];
        let sig = RustFunctionSig {
            params: vec![
                RustParam {
                    name: Some("self".to_string()),
                    type_display: "&mut demo::Writer".to_string(),
                },
                RustParam {
                    name: Some("header".to_string()),
                    type_display: "&mut demo::Header".to_string(),
                },
            ],
            return_type: "()".to_string(),
            is_async: false,
            is_unsafe: false,
        };
        let mut checker = TypeChecker::new();
        let _ = checker.validate_rust_method_call(
            "rust::demo::Writer.mutate",
            &sig,
            &args,
            &[ResolvedType::RustPath("demo::Header".to_string())],
            false,
            Span::new(0, 20),
        );
        assert!(checker.errors.iter().any(|error| {
            error
                .message
                .contains("Rust parameter requires a mutable borrow of 'header'")
        }));
        assert!(checker.type_info.rust.arg_coercions.is_empty());
    }

    #[test]
    fn mutable_rust_trait_object_borrow_rejects_immutable_binding() {
        let mut checker = TypeChecker::new();
        let span = Span::new(10, 16);
        let arg = Spanned::new(Expr::Ident("output".to_string()), span);
        let adapter_mut = checker.resolved_type_from_rust_display("dyn demo::AdapterMut");

        checker.validate_rust_boundary_value(
            "demo::Processor",
            "&mut dyn demo::AdapterMut",
            &arg,
            &adapter_mut,
            false,
        );

        assert!(
            checker.errors.iter().any(|error| error
                .message
                .contains("Rust parameter requires a mutable borrow of 'output'")),
            "expected a mutable trait-object borrow diagnostic, got {:?}",
            checker.errors
        );
    }

    #[test]
    fn zero_parameter_rust_sig_rejects_extra_arguments() {
        let mut checker = TypeChecker::new();
        let span = Span::new(0, 1);
        let arg_expr = Spanned::new(Expr::Literal(Literal::Int(IntLiteral::synthetic(1))), span);
        let args = [CallArg::Positional(arg_expr)];
        let sig = RustFunctionSig {
            type_params: Vec::new(),
            params: Vec::new(),
            return_type: "()".to_string(),
            is_async: false,
            is_unsafe: false,
        };

        let _ = checker.validate_rust_function_call("rust::crate::nop", &sig, &args, span);

        assert!(
            checker.errors.iter().any(|e| {
                e.message.contains("rust::crate::nop()")
                    && e.message.contains("expects 0 argument")
                    && e.message.contains("got 1")
            }),
            "expected builtin_arity for 0-param Rust call with 1 arg, errors={:?}",
            checker.errors
        );
    }

    #[test]
    fn rust_callable_bound_rejects_by_value_function_for_borrowed_fnmut() {
        let mut checker = TypeChecker::new();
        let span = Span::new(10, 22);
        let data = ResolvedType::RustPath("demo::Data".to_string());
        let info = ResolvedType::RustPath("demo::OutputCallbackInfo".to_string());
        checker.symbols.define(Symbol {
            name: "write_output".to_string(),
            kind: SymbolKind::Variable(VariableInfo {
                ty: ResolvedType::Function(
                    vec![CallableParam::positional(data), CallableParam::positional(info)],
                    Box::new(ResolvedType::Unit),
                ),
                is_mutable: false,
                is_used: false,
            }),
            span,
            scope: 0,
        });
        let arg_expr = Spanned::new(Expr::Ident("write_output".to_string()), span);
        let args = [CallArg::Positional(arg_expr)];
        let sig = RustFunctionSig {
            type_params: Vec::new(),
            params: vec![RustParam {
                name: Some("callback".to_string()),
                type_display: "impl FnMut(&mut demo::Data, &demo::OutputCallbackInfo)".to_string(),
            }],
            return_type: "()".to_string(),
            is_async: false,
            is_unsafe: false,
        };

        let _ = checker.validate_rust_function_call("rust::demo::use_callback", &sig, &args, span);

        assert!(
            checker.errors.iter().any(|error| {
                error.message.contains("FnMut")
                    || (error.message.contains("&mut demo::Data") && error.message.contains("demo::OutputCallbackInfo"))
            }),
            "expected a borrowed callback diagnostic before codegen, got {:?}",
            checker.errors
        );
    }

    #[test]
    fn rust_callable_bound_accepts_exact_borrowed_fnmut_function() {
        let mut checker = TypeChecker::new();
        let span = Span::new(10, 22);
        let data = ResolvedType::RustPath("demo::Data".to_string());
        let info = ResolvedType::RustPath("demo::OutputCallbackInfo".to_string());
        checker.symbols.define(Symbol {
            name: "write_output".to_string(),
            kind: SymbolKind::Variable(VariableInfo {
                ty: ResolvedType::Function(
                    vec![
                        CallableParam::positional(ResolvedType::RefMut(Box::new(data))),
                        CallableParam::positional(ResolvedType::Ref(Box::new(info))),
                    ],
                    Box::new(ResolvedType::Unit),
                ),
                is_mutable: false,
                is_used: false,
            }),
            span,
            scope: 0,
        });
        let arg_expr = Spanned::new(Expr::Ident("write_output".to_string()), span);
        let args = [CallArg::Positional(arg_expr)];
        let sig = RustFunctionSig {
            type_params: Vec::new(),
            params: vec![RustParam {
                name: Some("callback".to_string()),
                type_display: "impl FnMut(&mut demo::Data, &demo::OutputCallbackInfo)".to_string(),
            }],
            return_type: "()".to_string(),
            is_async: false,
            is_unsafe: false,
        };

        let _ = checker.validate_rust_function_call("rust::demo::use_callback", &sig, &args, span);

        assert!(
            checker.errors.is_empty(),
            "exact borrowed Rust callback should typecheck, got {:?}",
            checker.errors
        );
    }

    #[test]
    fn rust_callable_bound_accepts_generic_placeholder_callback_arg() {
        let mut checker = TypeChecker::new();
        let span = Span::new(10, 22);
        checker.symbols.enter_scope(ScopeKind::Function);
        checker.symbols.define(Symbol {
            name: "TaskFn".to_string(),
            kind: SymbolKind::Type(TypeInfo::Builtin),
            span,
            scope: 1,
        });
        checker.symbols.define(Symbol {
            name: "task".to_string(),
            kind: SymbolKind::Variable(VariableInfo {
                ty: ResolvedType::Named("TaskFn".to_string()),
                is_mutable: false,
                is_used: false,
            }),
            span,
            scope: 1,
        });
        let arg_expr = Spanned::new(Expr::Ident("task".to_string()), span);
        let args = [CallArg::Positional(arg_expr)];
        let sig = RustFunctionSig {
            type_params: Vec::new(),
            params: vec![RustParam {
                name: Some("task".to_string()),
                type_display: "impl FnOnce() -> T".to_string(),
            }],
            return_type: "()".to_string(),
            is_async: false,
            is_unsafe: false,
        };

        let _ = checker.validate_rust_function_call("rust::demo::spawn_blocking", &sig, &args, span);

        assert!(
            checker.errors.is_empty(),
            "generic placeholder callbacks should keep satisfying generic Rust callable bounds, got {:?}",
            checker.errors
        );
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_function_call_matches_reexported_identity_inside_generic_wrapper() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut checker = TypeChecker::new();
        let tmp = tempfile::tempdir()?;
        std::fs::write(
            tmp.path().join("Cargo.toml"),
            "[package]\nname = \"incan_test_rust_generic_identity\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;
        let manifest_dir = tmp.path().to_path_buf();
        checker.set_rust_inspect_manifest_dir(manifest_dir.clone());
        checker.rust_inspect_cache.insert_test_item(
            &manifest_dir,
            scalar_udf_metadata("bridge::ScalarUDF", "bridge::udf::ScalarUDF"),
        )?;
        checker.rust_inspect_cache.insert_test_item(
            &manifest_dir,
            scalar_udf_metadata("bridge::udf::ScalarUDF", "bridge::udf::ScalarUDF"),
        )?;

        let span = Span::new(10, 20);
        checker.symbols.define(Symbol {
            name: "udf".to_string(),
            kind: SymbolKind::Variable(VariableInfo {
                ty: ResolvedType::RustPath("rust::Arc<bridge::udf::ScalarUDF>".to_string()),
                is_mutable: false,
                is_used: false,
            }),
            span,
            scope: 0,
        });

        let arg_expr = Spanned::new(Expr::Ident("udf".to_string()), span);
        let args = [CallArg::Positional(arg_expr)];
        let sig = RustFunctionSig {
            type_params: Vec::new(),
            params: vec![RustParam {
                name: Some("udf".to_string()),
                type_display: "Arc<bridge::ScalarUDF>".to_string(),
            }],
            return_type: "()".to_string(),
            is_async: false,
            is_unsafe: false,
        };

        let _ = checker.validate_rust_function_call("rust::bridge::consume_udf", &sig, &args, span);

        assert!(
            checker.errors.is_empty(),
            "expected Rust re-export identity to match inside generic wrapper, errors={:?}",
            checker.errors
        );
        Ok(())
    }

    #[test]
    fn zero_parameter_rust_sig_allows_no_arguments() {
        let mut checker = TypeChecker::new();
        let span = Span::new(0, 1);
        let sig = RustFunctionSig {
            type_params: Vec::new(),
            params: Vec::new(),
            return_type: "()".to_string(),
            is_async: false,
            is_unsafe: false,
        };

        let _ = checker.validate_rust_function_call("rust::crate::nop", &sig, &[], span);

        assert!(
            checker.errors.is_empty(),
            "expected no errors for arity match, got {:?}",
            checker.errors
        );
    }

    #[test]
    fn too_few_arguments_reports_arity_before_param_zip() {
        let mut checker = TypeChecker::new();
        let span = Span::new(0, 1);
        let arg_expr = Spanned::new(Expr::Literal(Literal::Int(IntLiteral::synthetic(1))), span);
        let args = [CallArg::Positional(arg_expr)];
        let sig = RustFunctionSig {
            type_params: Vec::new(),
            params: vec![
                RustParam {
                    name: Some("a".to_string()),
                    type_display: "i64".to_string(),
                },
                RustParam {
                    name: Some("b".to_string()),
                    type_display: "i64".to_string(),
                },
            ],
            return_type: "()".to_string(),
            is_async: false,
            is_unsafe: false,
        };

        let _ = checker.validate_rust_function_call("rust::crate::f", &sig, &args, span);

        assert!(
            checker
                .errors
                .iter()
                .any(|e| e.message.contains("expects 2 argument") && e.message.contains("got 1")),
            "expected arity error when call has fewer args than Rust params, errors={:?}",
            checker.errors
        );
    }

    #[test]
    fn rust_function_call_binds_keyword_args_by_inspected_param_name() {
        let mut checker = TypeChecker::new();
        let span = Span::new(0, 60);
        let text_span = Span::new(10, 20);
        let count_span = Span::new(30, 31);
        let args = [
            CallArg::Named(
                "text".to_string(),
                Spanned::new(Expr::Literal(Literal::String("demo".to_string())), text_span),
            ),
            CallArg::Named(
                "count".to_string(),
                Spanned::new(Expr::Literal(Literal::Int(IntLiteral::synthetic(3))), count_span),
            ),
        ];
        let sig = RustFunctionSig {
            type_params: Vec::new(),
            params: vec![
                RustParam {
                    name: Some("count".to_string()),
                    type_display: "i64".to_string(),
                },
                RustParam {
                    name: Some("text".to_string()),
                    type_display: "&str".to_string(),
                },
            ],
            return_type: "()".to_string(),
            is_async: false,
            is_unsafe: false,
        };

        let _ = checker.validate_rust_function_call("rust::crate::f", &sig, &args, span);

        assert!(
            checker.errors.is_empty(),
            "expected keyword Rust call args to bind by parameter name, errors={:?}",
            checker.errors
        );
        let recorded = checker
            .type_info
            .calls
            .call_site_callable_params
            .get(&(span.start, span.end))
            .expect("keyword Rust calls should record exact call-site params for lowering");
        let names: Vec<_> = recorded.iter().filter_map(|param| param.name.as_deref()).collect();
        assert_eq!(names, vec!["count", "text"]);
    }

    #[test]
    fn rust_arg_boundary_accepts_structural_list_to_vec() {
        let checker = TypeChecker::new();
        let arg_ty = ResolvedType::Generic("List".to_string(), vec![ResolvedType::Str]);
        assert!(checker.rust_arg_matches_boundary(&arg_ty, "Vec<String>"));
    }

    #[test]
    fn rust_arg_boundary_accepts_structural_option_int_to_option_i64() {
        let checker = TypeChecker::new();
        let arg_ty = ResolvedType::Generic("Option".to_string(), vec![ResolvedType::Int]);
        assert!(checker.rust_arg_matches_boundary(&arg_ty, "Option<i64>"));
    }

    #[test]
    fn rust_arg_boundary_rejects_structural_float_to_f32() {
        let checker = TypeChecker::new();
        let arg_ty = ResolvedType::FrozenDict(Box::new(ResolvedType::Str), Box::new(ResolvedType::Float));
        assert!(!checker.rust_arg_matches_boundary(&arg_ty, "std::collections::HashMap<&str, f32>"));
    }

    #[test]
    fn rust_arg_boundary_accepts_lossless_exact_width_numeric_widening() {
        let checker = TypeChecker::new();
        let arg_ty = ResolvedType::Numeric(NumericTypeId::I16);
        assert!(checker.rust_arg_matches_boundary(&arg_ty, "i64"));
    }

    #[test]
    fn rust_arg_boundary_accepts_pointer_sized_numeric_identity() {
        let checker = TypeChecker::new();
        assert!(checker.rust_arg_matches_boundary(&ResolvedType::Numeric(NumericTypeId::ISize), "isize"));
        assert!(checker.rust_arg_matches_boundary(&ResolvedType::Numeric(NumericTypeId::USize), "usize"));
    }

    #[test]
    fn rust_arg_boundary_rejects_exact_width_numeric_narrowing() {
        let checker = TypeChecker::new();
        let arg_ty = ResolvedType::Numeric(NumericTypeId::I16);
        assert!(!checker.rust_arg_matches_boundary(&arg_ty, "i8"));
    }

    #[test]
    fn rust_arg_boundary_rejects_structural_list_str_to_vec_i64() {
        let checker = TypeChecker::new();
        let arg_ty = ResolvedType::Generic("List".to_string(), vec![ResolvedType::Str]);
        assert!(!checker.rust_arg_matches_boundary(&arg_ty, "Vec<i64>"));
    }

    #[test]
    fn rust_method_call_rejects_missing_required_arguments_after_receiver() {
        let mut checker = TypeChecker::new();
        let span = Span::new(0, 1);
        let sig = RustFunctionSig {
            type_params: Vec::new(),
            params: vec![
                RustParam {
                    name: Some("self".to_string()),
                    type_display: "&Self".to_string(),
                },
                RustParam {
                    name: Some("pattern".to_string()),
                    type_display: "&str".to_string(),
                },
            ],
            return_type: "bool".to_string(),
            is_async: false,
            is_unsafe: false,
        };

        let _ = checker.validate_rust_method_call("rust::regex::Regex.is_match", &sig, &[], &[], false, span);

        assert!(
            checker.errors.iter().any(
                |e| e.message.contains("rust::regex::Regex.is_match()") && e.message.contains("expects 1 argument")
            ),
            "expected arity diagnostic for missing method arg, errors={:?}",
            checker.errors
        );
    }

    #[test]
    fn rust_method_call_rejects_type_mismatch_after_receiver() {
        let mut checker = TypeChecker::new();
        let span = Span::new(0, 1);
        let sig = RustFunctionSig {
            type_params: Vec::new(),
            params: vec![
                RustParam {
                    name: Some("self".to_string()),
                    type_display: "&Self".to_string(),
                },
                RustParam {
                    name: Some("pattern".to_string()),
                    type_display: "&str".to_string(),
                },
            ],
            return_type: "bool".to_string(),
            is_async: false,
            is_unsafe: false,
        };
        let arg_expr = Spanned::new(Expr::Literal(Literal::Int(IntLiteral::synthetic(123))), span);
        let args = [CallArg::Positional(arg_expr)];
        let arg_types = [ResolvedType::Int];

        let _ = checker.validate_rust_method_call("rust::regex::Regex.is_match", &sig, &args, &arg_types, false, span);

        assert!(
            checker
                .errors
                .iter()
                .any(|e| e.message.contains("&str") && e.message.contains("int")),
            "expected type mismatch diagnostic for method arg, errors={:?}",
            checker.errors
        );
    }

    #[test]
    fn rust_lookup_preserving_method_accepts_string_probe_for_ref_string_param() {
        let mut checker = TypeChecker::new();
        let span = Span::new(0, 1);
        let sig = RustFunctionSig {
            type_params: Vec::new(),
            params: vec![
                RustParam {
                    name: Some("self".to_string()),
                    type_display: "&Self".to_string(),
                },
                RustParam {
                    name: Some("key".to_string()),
                    type_display: "&String".to_string(),
                },
            ],
            return_type: "Option<&i64>".to_string(),
            is_async: false,
            is_unsafe: false,
        };
        let arg_expr = Spanned::new(Expr::Literal(Literal::String("the".to_string())), span);
        let args = [CallArg::Positional(arg_expr)];
        let arg_types = [ResolvedType::Str];

        let _ = checker.validate_rust_method_call(
            "rust::std::collections::HashMap.get",
            &sig,
            &args,
            &arg_types,
            true,
            span,
        );

        assert!(
            checker.errors.is_empty(),
            "expected lookup-preserving rust method call to stay permissive for string probes, errors={:?}",
            checker.errors
        );
        assert!(
            checker.type_info.rust.arg_coercions.is_empty(),
            "expected lookup-preserving rust method call to preserve arg shape without coercion, got {:?}",
            checker.type_info.rust.arg_coercions
        );
    }

    #[test]
    fn rust_lookup_preserving_method_accepts_string_probe_for_generic_ref_param() {
        let mut checker = TypeChecker::new();
        let span = Span::new(0, 1);
        let sig = RustFunctionSig {
            type_params: Vec::new(),
            params: vec![
                RustParam {
                    name: Some("self".to_string()),
                    type_display: "&Self".to_string(),
                },
                RustParam {
                    name: Some("key".to_string()),
                    type_display: "&Q".to_string(),
                },
            ],
            return_type: "Option<&i64>".to_string(),
            is_async: false,
            is_unsafe: false,
        };
        let arg_expr = Spanned::new(Expr::Ident("word".to_string()), span);
        let args = [CallArg::Positional(arg_expr)];
        let arg_types = [ResolvedType::Str];

        let _ = checker.validate_rust_method_call(
            "rust::std::collections::HashMap.get",
            &sig,
            &args,
            &arg_types,
            true,
            span,
        );

        assert!(
            checker.errors.is_empty(),
            "expected lookup-preserving generic probe to stay permissive for string probes, errors={:?}",
            checker.errors
        );
        assert!(
            checker.type_info.rust.arg_coercions.is_empty(),
            "expected lookup-preserving generic probe to preserve arg shape without coercion, got {:?}",
            checker.type_info.rust.arg_coercions
        );
    }

    #[test]
    fn rust_arg_boundary_accepts_rusttype_from_interop_target() {
        let mut checker = TypeChecker::new();
        let span = Span::new(0, 1);
        checker.symbols.define(Symbol {
            name: "Email".to_string(),
            kind: SymbolKind::Type(TypeInfo::Newtype(NewtypeInfo {
                type_params: Vec::new(),
                is_rusttype: true,
                has_interop: true,
                underlying: ResolvedType::Named("RustString".to_string()),
                constraints: Vec::new(),
                implicit_coercion_enabled: true,
                method_rebindings: HashMap::new(),
                traits: Vec::new(),
                trait_adoptions: Vec::new(),
                method_aliases: HashMap::new(),
                methods: HashMap::new(),
                method_overloads: HashMap::new(),
            })),
            span,
            scope: 0,
        });

        assert!(
            checker.rust_arg_matches_boundary(&ResolvedType::Str, "Email"),
            "expected `str` to be admitted for rusttype target boundary via `from` interop edge hint"
        );
    }

    #[test]
    fn rust_function_call_accepts_string_for_borrowed_string_param() {
        let mut checker = TypeChecker::new();
        let span = Span::new(10, 20);
        let arg_expr = Spanned::new(Expr::Literal(Literal::String("{}".to_string())), span);
        let args = [CallArg::Positional(arg_expr)];
        let sig = RustFunctionSig {
            type_params: Vec::new(),
            params: vec![RustParam {
                name: Some("value".to_string()),
                type_display: "&String".to_string(),
            }],
            return_type: "()".to_string(),
            is_async: false,
            is_unsafe: false,
        };

        let _ = checker.validate_rust_function_call("rust::demo::takes_borrowed_string", &sig, &args, span);

        assert!(
            checker.errors.is_empty(),
            "expected borrowed String boundary to admit Incan str, errors={:?}",
            checker.errors
        );
        assert!(
            checker
                .type_info
                .rust
                .arg_coercions
                .contains_key(&(span.start, span.end)),
            "expected rust arg coercion metadata for borrowed String boundary"
        );
        let expected = ResolvedType::Ref(Box::new(ResolvedType::RustPath("String".to_string())));
        assert_eq!(
            checker
                .type_info
                .rust
                .arg_coercions
                .get(&(span.start, span.end))
                .map(|coercion| &coercion.target_type),
            Some(&expected),
            "borrowed owned Rust params must preserve owned target shape in lowering metadata"
        );
    }

    #[test]
    fn rust_function_call_accepts_string_for_borrowed_str_param() {
        let mut checker = TypeChecker::new();
        let span = Span::new(10, 20);
        let arg_expr = Spanned::new(Expr::Literal(Literal::String("{}".to_string())), span);
        let args = [CallArg::Positional(arg_expr)];
        let sig = RustFunctionSig {
            type_params: Vec::new(),
            params: vec![RustParam {
                name: Some("value".to_string()),
                type_display: "&str".to_string(),
            }],
            return_type: "()".to_string(),
            is_async: false,
            is_unsafe: false,
        };

        let _ = checker.validate_rust_function_call("rust::demo::takes_borrowed_str", &sig, &args, span);

        assert!(
            checker.errors.is_empty(),
            "expected borrowed str boundary to admit Incan str, errors={:?}",
            checker.errors
        );
        let expected = ResolvedType::Ref(Box::new(ResolvedType::Str));
        assert_eq!(
            checker
                .type_info
                .rust
                .arg_coercions
                .get(&(span.start, span.end))
                .map(|coercion| &coercion.target_type),
            Some(&expected),
            "borrowed str params must stay distinct from borrowed owned String params"
        );
    }

    #[test]
    fn rust_function_call_accepts_bytes_for_borrowed_vec_param() {
        let mut checker = TypeChecker::new();
        let span = Span::new(10, 20);
        let arg_expr = Spanned::new(Expr::Literal(Literal::Bytes(b"abc".to_vec())), span);
        let args = [CallArg::Positional(arg_expr)];
        let sig = RustFunctionSig {
            type_params: Vec::new(),
            params: vec![RustParam {
                name: Some("value".to_string()),
                type_display: "&Vec<u8>".to_string(),
            }],
            return_type: "()".to_string(),
            is_async: false,
            is_unsafe: false,
        };

        let _ = checker.validate_rust_function_call("rust::demo::takes_borrowed_vec", &sig, &args, span);

        assert!(
            checker.errors.is_empty(),
            "expected borrowed Vec<u8> boundary to admit Incan bytes, errors={:?}",
            checker.errors
        );
        let expected = ResolvedType::Ref(Box::new(ResolvedType::RustPath("Vec<u8>".to_string())));
        assert_eq!(
            checker
                .type_info
                .rust
                .arg_coercions
                .get(&(span.start, span.end))
                .map(|coercion| &coercion.target_type),
            Some(&expected),
            "borrowed owned Rust byte-vector params must preserve owned target shape in lowering metadata"
        );
    }

    #[test]
    fn rust_function_call_accepts_concrete_borrowed_rust_path_param_without_ref_unknown_diagnostic() {
        let mut checker = TypeChecker::new();
        let span = Span::new(10, 20);
        checker.symbols.define(Symbol {
            name: "plan".to_string(),
            kind: SymbolKind::Variable(VariableInfo {
                ty: ResolvedType::RustPath("demo::Plan".to_string()),
                is_mutable: false,
                is_used: false,
            }),
            span,
            scope: 0,
        });

        let arg_expr = Spanned::new(Expr::Ident("plan".to_string()), span);
        let args = [CallArg::Positional(arg_expr)];
        let sig = RustFunctionSig {
            type_params: Vec::new(),
            params: vec![RustParam {
                name: Some("value".to_string()),
                type_display: "&demo::Plan".to_string(),
            }],
            return_type: "()".to_string(),
            is_async: false,
            is_unsafe: false,
        };

        let _ = checker.validate_rust_function_call("rust::demo::consume_plan", &sig, &args, span);

        assert!(
            checker.errors.is_empty(),
            "expected concrete borrowed Rust path boundary to typecheck, errors={:?}",
            checker.errors
        );
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_function_call_matches_reexported_borrowed_param_via_definition_path()
    -> Result<(), Box<dyn std::error::Error>> {
        use incan_core::interop::{RustItemKind, RustItemMetadata, RustTypeInfo, RustVisibility};
        let mut checker = TypeChecker::new();
        let tmp = tempfile::tempdir()?;
        std::fs::write(
            tmp.path().join("Cargo.toml"),
            "[package]\nname = \"incan_test_rust_identity\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;
        let manifest_dir = tmp.path().to_path_buf();
        checker.set_rust_inspect_manifest_dir(manifest_dir.clone());
        checker.rust_inspect_cache.insert_test_item(
            &manifest_dir,
            RustItemMetadata {
                canonical_path: "datafusion_substrait::substrait::proto::Plan".to_string(),
                definition_path: Some("substrait::proto::Plan".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Type(RustTypeInfo {
                    alias_target: None,
                    metadata_completeness: Default::default(),
                    methods: vec![],
                    implemented_traits: Vec::new(),
                    fields: vec![],
                    variants: vec![],
                }),
            },
        )?;
        let cached = checker
            .rust_item_metadata_for_path("rust::datafusion_substrait::substrait::proto::Plan")
            .ok_or_else(|| std::io::Error::other("expected cached metadata for rust::datafusion_substrait::..."))?;
        assert_eq!(
            cached.definition_path.as_deref(),
            Some("substrait::proto::Plan"),
            "expected cached metadata definition path to resolve through re-export"
        );
        let span = Span::new(10, 20);
        checker.symbols.define(Symbol {
            name: "plan".to_string(),
            kind: SymbolKind::Variable(VariableInfo {
                ty: ResolvedType::RustPath("rust::datafusion_substrait::substrait::proto::Plan".to_string()),
                is_mutable: false,
                is_used: false,
            }),
            span,
            scope: 0,
        });
        let arg_expr = Spanned::new(Expr::Ident("plan".to_string()), span);
        let args = [CallArg::Positional(arg_expr)];
        let sig = RustFunctionSig {
            type_params: Vec::new(),
            params: vec![RustParam {
                name: Some("value".to_string()),
                type_display: "&substrait::proto::Plan".to_string(),
            }],
            return_type: "()".to_string(),
            is_async: false,
            is_unsafe: false,
        };
        let _ = checker.validate_rust_function_call("rust::consume_plan", &sig, &args, span);
        assert!(
            checker.errors.is_empty(),
            "expected re-exported borrowed Rust path boundary to typecheck, errors={:?}",
            checker.errors
        );
        Ok(())
    }

    #[test]
    fn validate_rust_function_call_records_interop_coercion_for_rusttype_target() {
        let mut checker = TypeChecker::new();
        let span = Span::new(10, 20);
        checker.symbols.define(Symbol {
            name: "Email".to_string(),
            kind: SymbolKind::Type(TypeInfo::Newtype(NewtypeInfo {
                type_params: Vec::new(),
                is_rusttype: true,
                has_interop: true,
                underlying: ResolvedType::Named("RustString".to_string()),
                constraints: Vec::new(),
                implicit_coercion_enabled: true,
                method_rebindings: HashMap::new(),
                traits: Vec::new(),
                trait_adoptions: Vec::new(),
                method_aliases: HashMap::new(),
                methods: HashMap::new(),
                method_overloads: HashMap::new(),
            })),
            span,
            scope: 0,
        });

        let arg_expr = Spanned::new(Expr::Literal(Literal::String("alice@example.com".to_string())), span);
        let args = [CallArg::Positional(arg_expr)];
        let sig = RustFunctionSig {
            type_params: Vec::new(),
            params: vec![RustParam {
                name: Some("value".to_string()),
                type_display: "Email".to_string(),
            }],
            return_type: "()".to_string(),
            is_async: false,
            is_unsafe: false,
        };

        let _ = checker.validate_rust_function_call("rust::demo::takes_email", &sig, &args, span);

        assert!(
            checker.errors.is_empty(),
            "expected rusttype interop boundary to avoid type mismatch, errors={:?}",
            checker.errors
        );
        assert!(
            checker
                .type_info
                .rust
                .arg_coercions
                .contains_key(&(span.start, span.end)),
            "expected rust arg coercion metadata for rusttype target boundary"
        );
    }

    #[test]
    fn borrowed_generic_rust_function_param_accepts_owned_incan_value() {
        let checker = TypeChecker::new();

        assert!(checker.rust_arg_matches_boundary(&ResolvedType::Named("Payload".to_string()), "T",));
        assert!(checker.rust_arg_matches_boundary(&ResolvedType::Bytes, "impl Buf"));
        assert!(checker.rust_arg_matches_boundary(&ResolvedType::Bytes, "implBuf"));
        assert!(checker.rust_arg_matches_boundary(&ResolvedType::Named("Payload".to_string()), "&T",));
        assert!(checker.rust_arg_matches_boundary(&ResolvedType::Named("Payload".to_string()), "&TValue",));
    }

    #[test]
    fn validate_rust_method_call_records_by_value_generic_param_shape() {
        let mut checker = TypeChecker::new();
        let span = Span::new(30, 40);
        let arg_expr = Spanned::new(Expr::Ident("cursor".to_string()), span);
        let args = [CallArg::Positional(arg_expr)];
        let arg_types = [ResolvedType::RustPath("std::io::Cursor<Vec<u8>>".to_string())];
        let sig = RustFunctionSig {
            type_params: Vec::new(),
            params: vec![RustParam {
                name: Some("buf".to_string()),
                type_display: "T".to_string(),
            }],
            return_type: "demo::FileDescriptorSet".to_string(),
            is_async: false,
            is_unsafe: false,
        };

        let _ = checker.validate_rust_method_call(
            "rust::demo::FileDescriptorSet.decode",
            &sig,
            &args,
            &arg_types,
            false,
            span,
        );

        assert!(
            checker.errors.is_empty(),
            "expected by-value Rust generic param to accept the owned argument, got {:?}",
            checker.errors
        );
        assert!(
            checker
                .type_info
                .calls
                .call_site_callable_params
                .get(&(span.start, span.end))
                .is_some_and(|params| params.len() == 1 && params[0].ty == ResolvedType::TypeVar("T".to_string())),
            "expected Rust by-value generic method param shape to be recorded, got {:?}",
            checker.type_info.calls.call_site_callable_params
        );
    }

    #[test]
    fn validate_rust_method_call_records_by_value_impl_trait_param_shape() {
        let mut checker = TypeChecker::new();
        let span = Span::new(30, 40);
        let arg_expr = Spanned::new(Expr::Ident("encoded".to_string()), span);
        let args = [CallArg::Positional(arg_expr)];
        let arg_types = [ResolvedType::Bytes];
        let sig = RustFunctionSig {
            type_params: Vec::new(),
            params: vec![RustParam {
                name: Some("buf".to_string()),
                type_display: "implBuf".to_string(),
            }],
            return_type: "demo::FileDescriptorSet".to_string(),
            is_async: false,
            is_unsafe: false,
        };

        let _ = checker.validate_rust_method_call(
            "rust::demo::FileDescriptorSet.decode",
            &sig,
            &args,
            &arg_types,
            false,
            span,
        );

        assert!(
            checker.errors.is_empty(),
            "expected by-value impl Trait Rust param to accept bytes without borrow coercion, got {:?}",
            checker.errors
        );
        assert!(
            checker.type_info.rust.arg_coercions.is_empty(),
            "expected by-value impl Trait Rust param to avoid borrow coercion, got {:?}",
            checker.type_info.rust.arg_coercions
        );
        assert!(
            checker
                .type_info
                .calls
                .call_site_callable_params
                .get(&(span.start, span.end))
                .is_some_and(|params| params.len() == 1 && params[0].ty == ResolvedType::TypeVar("implBuf".to_string())),
            "expected Rust by-value impl Trait method param shape to be recorded, got {:?}",
            checker.type_info.calls.call_site_callable_params
        );
    }

    #[test]
    fn validate_rust_method_call_records_interop_coercion_for_rusttype_target() {
        let mut checker = TypeChecker::new();
        let span = Span::new(30, 40);
        checker.symbols.define(Symbol {
            name: "Email".to_string(),
            kind: SymbolKind::Type(TypeInfo::Newtype(NewtypeInfo {
                type_params: Vec::new(),
                is_rusttype: true,
                has_interop: true,
                underlying: ResolvedType::Named("RustString".to_string()),
                constraints: Vec::new(),
                implicit_coercion_enabled: true,
                method_rebindings: HashMap::new(),
                traits: Vec::new(),
                trait_adoptions: Vec::new(),
                method_aliases: HashMap::new(),
                methods: HashMap::new(),
                method_overloads: HashMap::new(),
            })),
            span,
            scope: 0,
        });

        let arg_expr = Spanned::new(Expr::Literal(Literal::String("alice@example.com".to_string())), span);
        let args = [CallArg::Positional(arg_expr)];
        let arg_types = [ResolvedType::Str];
        let sig = RustFunctionSig {
            type_params: Vec::new(),
            params: vec![
                RustParam {
                    name: Some("self".to_string()),
                    type_display: "&Self".to_string(),
                },
                RustParam {
                    name: Some("value".to_string()),
                    type_display: "Email".to_string(),
                },
            ],
            return_type: "()".to_string(),
            is_async: false,
            is_unsafe: false,
        };

        let _ = checker.validate_rust_method_call(
            "rust::demo::EmailService.set_email",
            &sig,
            &args,
            &arg_types,
            false,
            span,
        );

        assert!(
            checker.errors.is_empty(),
            "expected rusttype interop boundary to avoid type mismatch, errors={:?}",
            checker.errors
        );
        assert!(
            checker
                .type_info
                .rust
                .arg_coercions
                .contains_key(&(span.start, span.end)),
            "expected rust arg coercion metadata for rust method boundary"
        );
    }
}
