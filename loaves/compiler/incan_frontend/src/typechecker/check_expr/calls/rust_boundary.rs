//! Rust boundary matching, Rust call validation, and coercion metadata recording.

use std::collections::HashMap;

use super::TypeChecker;
use crate::ast::{CallArg, Expr, ParamKind, Span, Spanned};
use crate::diagnostics::errors;
use crate::resolved_type_subst::{map_resolved_type_leaves, substitute_resolved_type};
use crate::symbols::{CallableParam, ResolvedType, TypeInfo};
use crate::typechecker::helpers::{collection_type_id, render_resolved_type_as_rust_arg};
use crate::typechecker::{RustArgCoercionInfo, RustArgCoercionKind};
use incan_lang::interop::{CoercionPolicy, RustFunctionSig, RustItemKind, RustParam, admitted_builtin_coercion};
use incan_lang::lang::types::collections::CollectionTypeId;
use incan_lang::lang::types::numerics;
use quote::ToTokens;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RustArgBoundaryMatch {
    /// Argument already matches the Rust parameter shape; no lowering-time adapter is required.
    Exact,
    /// Argument is admissible if lowering inserts a boundary coercion/adapter.
    Coercion(RustArgCoercionKind),
    /// Argument cannot satisfy the Rust parameter shape.
    NoMatch,
}

/// A written argument paired with its inspected parameter and declaration-order slot before specialization.
struct RustCallArgBinding<'a> {
    arg: &'a CallArg,
    arg_ty: &'a ResolvedType,
    param: &'a RustParam,
    parameter_index: usize,
}

impl TypeChecker {
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
    fn native_rust_object_type(&self, display: &str) -> Option<ResolvedType> {
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
    /// form). Rust slices are never namespaced; retaining that prefix makes generated Rust invalid.
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
    fn prewarm_rust_return_type_metadata(&self, ty: &ResolvedType) {
        self.prewarm_rust_type_identity_metadata(ty);
    }

    /// Visit nominal Rust paths nested inside a resolved type with one metadata probe policy.
    fn prewarm_rust_type_identity_metadata_with(&self, ty: &ResolvedType, probe: &impl Fn(&Self, &str)) {
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
    fn prewarm_rust_type_identity_metadata(&self, ty: &ResolvedType) {
        self.prewarm_rust_type_identity_metadata_with(ty, &|checker, path| {
            let _ = checker.rust_item_metadata_for_path(path);
        });
    }

    /// Prewarm Rust identity metadata for a type explicitly named by a Rust call boundary.
    ///
    /// The ordinary compatibility path must remain metadata-light, but a public Rust API can name a type that is
    /// re-exported through macro-generated source. This permits blocking inspection on a cache miss and is invoked
    /// only while validating an explicit call argument, never from [`Self::types_compatible`].
    fn prewarm_rust_boundary_type_identity_metadata(&self, ty: &ResolvedType) {
        self.prewarm_rust_type_identity_metadata_with(ty, &|checker, path| {
            let _ = checker.rust_item_metadata_for_path(path);
        });
    }

    /// Return whether cache-only identity prewarm should ask rust-inspect for this Rust display base.
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

    /// Select the native argument carrier after an already approved boundary conversion, preserving the checked borrow
    /// or numeric conversion when a call is retained. A source range keeps ordinary storage; a half-open literal
    /// constructs Rust's public Range only at this boundary.
    pub(in crate::typechecker::check_expr) fn rust_argument_native_type(
        &self,
        expression: &Spanned<Expr>,
    ) -> Option<ResolvedType> {
        if let Some(conversion) = self.type_info.rust.element_adjustments.get(&expression.span) {
            return Some(ResolvedType::Generic(
                "List".into(),
                vec![conversion.return_type.clone()],
            ));
        }
        if let Some(coercion) = self.type_info.rust_arg_coercion(expression.span) {
            // Builtin boundary types retain unsized text and slices. The source-facing target intentionally
            // normalizes those to source str/List, which would change a second inference pass to &String/&Vec.
            if matches!(
                coercion.kind,
                RustArgCoercionKind::Builtin(_) | RustArgCoercionKind::Borrow { mutable: false }
            ) {
                return Some(self.native_rust_signature_type(&coercion.rust_target_type, ""));
            }
            return Some(coercion.target_type.clone());
        }
        match &expression.node {
            Expr::Paren(inner) => self.rust_argument_native_type(inner),
            Expr::Range { inclusive: false, .. } => Some(self.native_rust_signature_type("std::ops::Range<i64>", "")),
            _ => self.rust_native_value_type(expression),
        }
    }

    /// Project declared Rust parameters and canonical native constructors into ordinary structural inference.
    /// Re-exported owners normalize once, and checked generic placeholders retain their declared binders.
    /// Source aliases expand before native inference; lowering receives their structural carrier, never an alias
    /// name embedded inside a native generic argument. Native aliases expand from retained templates; lifetime-only
    /// or defaulted owners with no effective arguments share their metadata self template's bare identity.
    /// Structured arguments already carry checked bindings, so only a rendered owner's base enters the constructor.
    /// Anonymous native binders keep the declaration's exact key even when a signature parser normalizes whitespace.
    pub(in crate::typechecker) fn rust_inference_type(&self, ty: &ResolvedType, parameters: &[String]) -> ResolvedType {
        map_resolved_type_leaves(ty, &|leaf| {
            if let Some(target) = self.rusttype_native_target(leaf) {
                return Some(self.rust_inference_type(&target, parameters));
            }
            if let ResolvedType::Named(name) | ResolvedType::TypeVar(name) | ResolvedType::RustPath(name) = leaf
                && let Some(parameter) = parameters.iter().find(|parameter| {
                    parameter
                        .chars()
                        .filter(|ch| !ch.is_whitespace())
                        .eq(name.chars().filter(|ch| !ch.is_whitespace()))
                })
            {
                return Some(ResolvedType::TypeVar(parameter.clone()));
            }
            if let Some(name) = self.generic_placeholder_name(leaf) {
                return Some(ResolvedType::TypeVar(name.to_owned()));
            }
            if let ResolvedType::Named(name) = leaf
                && matches!(self.lookup_semantic_type_info(name), Some(TypeInfo::Newtype(info)) if info.is_rusttype)
                && let Some(path) = self.rust_canonical_path_for_receiver_type(leaf)
            {
                return Some(self.rust_inference_type(&ResolvedType::RustPath(path), parameters));
            }
            if matches!(leaf, ResolvedType::Named(_) | ResolvedType::Generic(_, _)) {
                let expanded = self.expand_type_aliases(leaf.clone());
                if expanded != *leaf {
                    return Some(self.rust_inference_type(&expanded, parameters));
                }
            }
            if let ResolvedType::Generic(base, arguments) = leaf {
                let path = self.rust_canonical_path_for_receiver_type(leaf);
                let canonical = path
                    .as_ref()
                    .map(|path| self.type_info.rust.item_public_paths.get(path).unwrap_or(path))
                    .map_or(base.as_str(), String::as_str);
                let canonical = Self::rust_generic_base_and_args(canonical).map_or(canonical, |(base, _)| base);
                return Some(self.rust_inference_constructor(canonical, arguments, parameters));
            }
            if *leaf == ResolvedType::Bytes {
                return Some(ResolvedType::Generic(
                    "List".into(),
                    vec![ResolvedType::Numeric(numerics::NumericTypeId::U8)],
                ));
            }
            let ResolvedType::RustPath(path) = leaf else {
                return None;
            };
            if self
                .generic_placeholder_name(&ResolvedType::Named(path.clone()))
                .is_some()
            {
                return Some(ResolvedType::TypeVar(path.trim().to_owned()));
            }
            let source_alias = ResolvedType::Named(path.clone());
            let expanded = self.expand_type_aliases(source_alias.clone());
            if expanded != source_alias {
                return Some(self.rust_inference_type(&expanded, parameters));
            }
            let (base, arguments) = Self::rust_generic_base_and_args(path).unwrap_or((path, Vec::new()));
            if let Some(target) = self.rust_alias_expansion(base, &arguments) {
                return Some(self.rust_inference_type(&self.native_rust_signature_type(&target, ""), parameters));
            }
            if arguments.is_empty() {
                return None;
            }
            if let Some(structural) = self.resolved_structural_rust_param_display(path, |checker, argument| {
                checker.resolved_type_from_rust_display(argument)
            }) {
                return Some(self.rust_inference_type(&structural, parameters));
            }
            let base = self
                .type_info
                .rust
                .item_public_paths
                .get(base)
                .map_or(base, String::as_str);
            let arguments = arguments
                .into_iter()
                .filter(|argument| !Self::rust_generic_arg_is_lifetime(argument))
                .map(|argument| self.native_rust_signature_type(argument, ""))
                .collect::<Vec<_>>();
            Some(self.rust_inference_constructor(base, &arguments, parameters))
        })
    }

    /// Normalize native defaults before projecting an ordinary collection shape, preserving the checked public
    /// owner path for storage. Builtin tuple applications become structural tuples before shared inference matches
    /// Rust return signatures; a nondefault allocator remains an exact native argument.
    pub(in crate::typechecker) fn rust_inference_constructor(
        &self,
        base: &str,
        arguments: &[ResolvedType],
        parameters: &[String],
    ) -> ResolvedType {
        let displays = arguments
            .iter()
            .map(render_resolved_type_as_rust_arg)
            .collect::<Vec<_>>();
        let references = displays.iter().map(String::as_str).collect::<Vec<_>>();
        if let Some(target) = self.rust_alias_expansion(base, &references) {
            return self.rust_inference_type(&self.native_rust_signature_type(&target, ""), parameters);
        }
        let arguments = arguments
            .iter()
            .map(|argument| self.rust_inference_type(argument, parameters))
            .collect();
        if collection_type_id(base) == Some(CollectionTypeId::Tuple) {
            return ResolvedType::Tuple(arguments);
        }
        if !base.contains("::") {
            return ResolvedType::Generic(base.to_owned(), arguments);
        }
        let arguments = match self.rust_item_metadata_for_path(base).as_deref() {
            Some(incan_lang::interop::RustItemMetadata {
                kind: RustItemKind::Type(info),
                ..
            }) => self.rust_storage_owner_arguments(arguments, info, base),
            _ => arguments,
        };
        if arguments.is_empty() {
            return ResolvedType::RustPath(base.to_owned());
        }
        let Some(collection) = Self::rust_collection_id_from_display_base(base) else {
            return ResolvedType::Generic(base.to_owned(), arguments);
        };
        match Self::resolved_structural_rust_arguments(collection, arguments) {
            Ok(structural) => self.native_frozen_owner(base, "", structural),
            Err(arguments) => ResolvedType::Generic(base.to_owned(), arguments),
        }
    }

    /// The canonical Rust path a generic template names: its own path, or the item an imported Rust name stands for
    /// (`Box` imported from `std::boxed`), so a template written with the import unifies with a native owner.
    pub(super) fn rust_template_canonical_path(&self, name: &str) -> Option<String> {
        if name.contains("::") {
            return Some(name.to_owned());
        }
        self.rust_canonical_path_for_nominal_receiver(name, None)
    }

    /// Expand a source collection into an exact native owner only when a native template requires its hidden
    /// default parameters. A whole type variable still binds the original source type through ordinary inference.
    pub(in crate::typechecker) fn rust_inference_owner_for_template(
        &self,
        base: &str,
        actual: &ResolvedType,
    ) -> Option<ResolvedType> {
        if !base.contains("::") {
            return None;
        }
        let mut arguments = self.rust_owner_type_args(actual, base)?;
        let metadata = self.rust_item_metadata_for_path(base)?;
        let RustItemKind::Type(info) = &metadata.kind else {
            return None;
        };
        if !self.complete_rust_owner_arguments(&info, &mut arguments) {
            return None;
        }
        let expanded = ResolvedType::Generic(base.to_owned(), arguments);
        (&expanded != actual).then_some(expanded)
    }

    /// Infer declared function or impl arguments from bound inputs and the expected output, using the same
    /// structural unifier as Incan functions. References describe passing mode and do not prevent an owned argument
    /// from fixing the referent's type. An inconsistent binding remains unknown and cannot authorize a native call.
    pub(in crate::typechecker::check_expr) fn infer_rust_signature_bindings(
        &self,
        sig: &RustFunctionSig,
        owner_path: &str,
        parameters: &[String],
        args: &[CallArg],
        arg_types: &[ResolvedType],
        expected: Option<&ResolvedType>,
    ) -> HashMap<String, ResolvedType> {
        let mut bound = HashMap::new();
        let params = if Self::rust_signature_has_receiver(sig) {
            &sig.params[1..]
        } else {
            &sig.params
        };
        for (index, (argument, actual)) in args.iter().zip(arg_types).enumerate() {
            let parameter = match argument {
                CallArg::Positional(_) => params.get(index),
                CallArg::Named(name, _) => params.iter().find(|param| param.name.as_ref() == Some(&name.node)),
                _ => None,
            };
            let Some(parameter) = parameter else {
                continue;
            };
            let target = self.resolved_rust_boundary_target_from_param_display_for_owner_path(
                parameter.type_display.as_str(),
                owner_path,
            );
            let target = self.rust_inference_type(&target, parameters);
            let native_actual = self.rust_argument_native_type(Self::call_arg_expr(argument));
            let actual = self.rust_inference_type(native_actual.as_ref().unwrap_or(actual), parameters);
            let (target, actual) = match (&target, &actual) {
                (ResolvedType::Ref(_) | ResolvedType::RefMut(_), ResolvedType::Ref(_) | ResolvedType::RefMut(_)) => {
                    (&target, &actual)
                }
                (ResolvedType::Ref(inner) | ResolvedType::RefMut(inner), _) => (inner.as_ref(), &actual),
                _ => (&target, &actual),
            };
            self.infer_type_param_bindings(target, actual, &mut bound);
            if let Some(callable) = self.rust_bound_callable_type(sig, &parameter.type_display) {
                self.infer_type_param_bindings(&self.rust_inference_type(&callable, parameters), actual, &mut bound);
            }
        }
        if let Some(expected) = expected {
            let result = self.resolved_rust_return_type_from_sig(sig, owner_path);
            self.infer_type_param_bindings(
                &self.rust_inference_type(&result, parameters),
                &self.rust_inference_type(expected, parameters),
                &mut bound,
            );
        }
        self.infer_rust_predicate_bindings(sig, parameters, &mut bound);
        self.prefer_borrowed_bindings(sig, parameters, &mut bound);
        bound
    }

    /// Instantiate signature displays from inferred types before recording boundary and result facts.
    fn specialize_inferred_rust_signature(
        &self,
        sig: &RustFunctionSig,
        owner: &str,
        bound: &HashMap<String, ResolvedType>,
    ) -> RustFunctionSig {
        let substitutions = bound
            .iter()
            .filter(|(_, ty)| !matches!(ty, ResolvedType::Unknown | ResolvedType::CallSiteInfer))
            .map(|(name, ty)| (name.as_str(), render_resolved_type_as_rust_arg(ty)))
            .collect();
        self.rust_signature_projections(Self::specialize_rust_signature(sig, &substitutions, owner))
    }

    /// Instantiate a checked Rust signature structurally so Incan argument identities survive the foreign boundary.
    /// Rendering an Incan model into a Rust display must not replace its retained declaration with a bare name.
    pub(in crate::typechecker::check_expr) fn instantiate_rust_signature_type(
        &self,
        ty: &ResolvedType,
        parameters: &[String],
        bindings: &HashMap<String, ResolvedType>,
    ) -> ResolvedType {
        substitute_resolved_type(&self.rust_inference_type(ty, parameters), bindings)
    }

    /// Retain unsized native text, slices, and trait-object principals structurally before generic substitution in
    /// parameters and results. `[]` is an internal generic constructor unavailable as a source identifier. A generic
    /// `&T` instantiated with source text borrows its owned String carrier instead of being reclassified as an
    /// unsized string.
    ///
    /// A display that names a nominal carrier key resolves through the active module's bindings, which the cache key
    /// does not hold, so it bypasses the cache.
    pub(crate) fn native_rust_signature_type(&self, display: &str, owner: &str) -> ResolvedType {
        if display.contains(incan_semantics_core::body_ir::NOMINAL_CARRIER_KEY_PREFIX) {
            return self.resolve_native_rust_signature_type(display, owner);
        }
        let key = (display.to_owned(), owner.to_owned());
        if let Some(resolved) = self.native_signature_types.borrow().get(&key) {
            return resolved.clone();
        }
        let resolved = self.resolve_native_rust_signature_type(display, owner);
        self.native_signature_types.borrow_mut().insert(key, resolved.clone());
        resolved
    }

    /// Resolve one native signature display for `native_rust_signature_type`, which caches the result; nested
    /// displays recurse through the cached entry point. Metadata-proven aliases of unsized text retain the native
    /// referent before ordinary source projection can turn them into owned string values.
    fn resolve_native_rust_signature_type(&self, display: &str, owner: &str) -> ResolvedType {
        let display = self.rust_display_for_owner_path(display, owner);
        let display = Self::rust_display_without_lifetimes(&display);
        // Parsed qualified owners render namespace punctuation with surrounding whitespace. Normalize that
        // punctuation before metadata lookup while preserving word boundaries in `dyn`, `mut`, and `as`.
        let display = display.split("::").map(str::trim).collect::<Vec<_>>().join("::");
        let display = display.trim();
        // Rust's associated values can parenthesize a dynamic referent with an explicit object lifetime.
        // Parentheses change no carrier; the object parser retains its principal and associated equalities.
        if display.starts_with('(')
            && let Ok(syn::Type::Paren(parenthesized)) = syn::parse_str::<syn::Type>(display)
        {
            return self.native_rust_signature_type(&parenthesized.elem.to_token_stream().to_string(), owner);
        }
        // Qualified projections must keep the `as` token separated until the associated-type query normalizes them.
        // Compact path rendering would merge its owner and trait names into an unresolvable nominal identity.
        if display.starts_with('<') {
            return ResolvedType::RustPath(display.to_owned());
        }
        if display == "str" || self.rust_alias_expansion(display, &[]).as_deref() == Some("str") {
            return ResolvedType::RustPath("str".into());
        }
        if display.starts_with("dyn ")
            && let Some(object) = self.native_rust_object_type(display)
        {
            return object;
        }
        // The impossible source identifier `[]` is the structural native slice constructor, never a Vec alias.
        if let Some(element) = display.strip_prefix('[').and_then(|inner| inner.strip_suffix(']'))
            && !element.contains(';')
        {
            return ResolvedType::Generic("[]".into(), vec![self.native_rust_signature_type(element, owner)]);
        }
        if let Some((mutable, inner)) = Self::rust_display_borrow_kind(display) {
            let inner = Box::new(self.native_rust_signature_type(inner, owner));
            return if mutable {
                ResolvedType::RefMut(inner)
            } else {
                ResolvedType::Ref(inner)
            };
        }
        if let Some(inner) = display.strip_prefix('(').and_then(|inner| inner.strip_suffix(')'))
            && inner.contains(',')
        {
            return ResolvedType::Tuple(
                Self::split_top_level_generic_args(inner)
                    .into_iter()
                    .map(|element| self.native_rust_signature_type(element, owner))
                    .collect(),
            );
        }
        if let Some(structural) = self.resolved_structural_rust_param_display(display, |checker, argument| {
            checker.native_rust_signature_type(argument, owner)
        }) {
            return self.native_frozen_owner(display, owner, structural);
        }
        let ty = self.resolved_rust_boundary_target_from_param_display_for_owner_path(display, owner);
        let ty = map_resolved_type_leaves(&ty, &|leaf| {
            matches!(leaf, ResolvedType::Ref(inner) if **inner == ResolvedType::Str)
                .then(|| ResolvedType::Ref(Box::new(ResolvedType::RustPath("str".into()))))
        });
        self.native_frozen_owner(display, owner, self.native_signature_storage_owner(ty))
    }

    /// Preserve a metadata-proven frozen nominal's owner alongside its inferred arguments. Source frozen views are
    /// useful for expression checking but cannot unify with a native impl Self template after erasing its owner.
    pub(in crate::typechecker::check_expr) fn native_frozen_owner(
        &self,
        display: &str,
        owner: &str,
        ty: ResolvedType,
    ) -> ResolvedType {
        let arguments = match &ty {
            ResolvedType::FrozenList(element) | ResolvedType::FrozenSet(element) => vec![*element.clone()],
            ResolvedType::FrozenDict(key, value) => vec![*key.clone(), *value.clone()],
            _ => return ty,
        };
        match self.rust_result_item_path(display, owner) {
            Some(path) => ResolvedType::Generic(path, arguments),
            None => ty,
        }
    }

    /// Canonicalize metadata-defaulted native storage at signature intake, as receiver selection already does. A
    /// payload layout and a call signature must not retain different arities for the same defaulted native owner.
    pub(in crate::typechecker::check_expr) fn native_signature_storage_owner(&self, ty: ResolvedType) -> ResolvedType {
        let normalized = map_resolved_type_leaves(&ty, &|leaf| self.short_native_storage_owner(leaf));
        let normalized = self.short_native_storage_owner(&normalized).unwrap_or(normalized);
        if normalized != ty {
            return self.native_signature_storage_owner(normalized);
        }
        normalized
    }

    /// Shorten one exact native owner from its metadata defaults. A changed display may reveal a structural
    /// collection or nested owner, so the caller walks the resulting structure before publishing its storage type.
    fn short_native_storage_owner(&self, ty: &ResolvedType) -> Option<ResolvedType> {
        let path = match ty {
            ResolvedType::RustPath(path) if path.contains('<') => self.rust_path_base_and_args(path).0,
            ResolvedType::Generic(base, _) if base.contains("::") => base.clone(),
            _ => return None,
        };
        if let Some(metadata) = self.known_rust_metadata_for_path(&path)
            && let RustItemKind::Type(info) = &metadata.kind
        {
            return Some(self.rust_storage_owner(ty, &info, &path));
        }
        None
    }

    /// Retain a declaration-ordered inference result for Body IR; conflicting constraints are source errors.
    fn record_inferred_rust_arguments(
        &mut self,
        path: &str,
        parameters: &[String],
        bindings: &HashMap<String, ResolvedType>,
        span: Span,
    ) {
        for parameter in parameters {
            if bindings.get(parameter) == Some(&ResolvedType::Unknown) {
                self.errors
                    .push(errors::call_site_type_inference_unresolved(path, parameter, span));
            }
        }
        if parameters.is_empty() {
            return;
        }
        let arguments = parameters
            .iter()
            .map(|parameter| bindings.get(parameter).cloned())
            .collect::<Option<Vec<_>>>();
        if let Some(arguments) = arguments {
            self.type_info
                .calls
                .call_site_monomorph_type_args
                .insert(span, arguments);
        }
    }

    /// Retain a safe call whose Rust signature and native instantiation were fully selected. Async calls retain a
    /// future of the declared output, which the existing native constructor boxes without polling its body.
    fn retain_checked_rust_function(
        &mut self,
        path: &str,
        signature: &RustFunctionSig,
        bindings: &HashMap<String, ResolvedType>,
        span: Span,
    ) {
        if signature.is_unsafe {
            return;
        }
        self.retain_native_callback_parameters(path, signature, &signature.type_params);
        let arguments = signature
            .type_params
            .iter()
            .map(|parameter| bindings.get(parameter).cloned())
            .collect::<Option<Vec<_>>>();
        let Some(arguments) = arguments else {
            return;
        };
        let mut parameters = self.type_info.call_site_callable_params(span).unwrap_or(&[]).to_vec();
        for (parameter, declared) in parameters.iter_mut().zip(&signature.params) {
            let template = self.native_rust_signature_type(&declared.type_display, path);
            parameter.ty = self.instantiate_rust_signature_type(&template, &signature.type_params, bindings);
        }
        let result = self.instantiate_rust_signature_type(
            &self.native_rust_signature_type(&signature.return_type, path),
            &signature.type_params,
            bindings,
        );
        let result = if signature.return_type.contains(" as ") {
            let selected = self.specialize_inferred_rust_signature(signature, path, bindings);
            self.native_rust_signature_type(&selected.return_type, path)
        } else {
            result
        };
        let result = if signature.is_async {
            ResolvedType::Generic("Awaitable".into(), vec![result])
        } else {
            result
        };
        self.type_info.rust.checked_calls.insert(
            span,
            crate::typechecker::type_info::RustCheckedCall {
                storage: None,
                source_type_parameters: self.native_call_type_parameters(),
                receiver_adjustments: Vec::new(),
                path: path.trim_start_matches("rust::").to_owned(),
                type_arguments: arguments,
                parameters,
                return_type: result,
                result_item_path: self.rust_result_item_path(&signature.return_type, path),
                has_receiver: false,
            },
        );
    }

    /// Retain a safe monomorphic free-function read using the same signature selection as its direct application.
    pub(in crate::typechecker::check_expr) fn retain_rust_function_value(
        &mut self,
        path: &str,
        signature: &RustFunctionSig,
        span: Span,
    ) -> Option<ResolvedType> {
        if signature.is_async || signature.is_unsafe || !signature.type_params.is_empty() {
            return None;
        }
        self.record_rust_call_site_params(span, &signature.params, path);
        self.retain_checked_rust_function(path, signature, &HashMap::new(), span);
        self.publish_rust_function_value(span)
    }

    /// Keep the selected callable's native parameters and result under its function-value role, so the read never
    /// supplies an application's result provenance.
    pub(in crate::typechecker::check_expr) fn publish_rust_function_value(
        &mut self,
        span: Span,
    ) -> Option<ResolvedType> {
        let key = span;
        let selected = self.type_info.rust.checked_calls.remove(&key)?;
        let ty = ResolvedType::Function(selected.parameters.clone(), Box::new(selected.return_type.clone()));
        self.type_info.rust.checked_function_values.insert(key, selected);
        Some(ty)
    }

    /// Record an ownership conversion for borrowed Rust scalar-like returns that Incan exposes as owned values.
    pub(in crate::typechecker) fn record_rust_return_coercion_from_display(
        &mut self,
        rust_return_type: &str,
        incan_ret: &ResolvedType,
        span: Span,
    ) {
        if self.type_info.rust.native_return_consumers.contains(&span) {
            return;
        }
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
            span,
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
        let normalized = Self::compact_rust_display(rust_ty);
        if let Some(tail) = normalized.strip_prefix("impl").map(str::trim_start) {
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
    /// boundary adapters. Nested borrows keep their token boundaries throughout type resolution.
    /// Exact canonical carrier equality also recognizes transparent aliases nested inside owned
    /// containers without changing their passing mode.
    fn rust_arg_boundary_match(&self, arg_ty: &ResolvedType, rust_param_ty: &str) -> RustArgBoundaryMatch {
        if matches!(arg_ty, ResolvedType::Function(..))
            && let Some(signature) = self.rust_boxed_callable_signature(rust_param_ty)
        {
            return if self.rust_trait_carriers_match(arg_ty, &signature) {
                RustArgBoundaryMatch::Exact
            } else {
                RustArgBoundaryMatch::NoMatch
            };
        }
        let display = Self::rust_display_without_lifetimes(rust_param_ty);
        let normalized = Self::compact_rust_display(&display);
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
        // A by-value trait object reaches an argument through a specialized generic — `Arc::new(v)` checked
        // against `Arc<dyn TableProvider>` asks whether `v` is a `dyn TableProvider`. Rust admits that by
        // unsized coercion when `v`'s type implements the trait; the borrowed `&dyn Trait` branch above already
        // proves exactly that from metadata, so reuse it rather than fall through to a display-string check.
        if Self::rust_display_is_trait_object(display.as_str()) {
            return if self.rust_type_implements_trait_object(arg_ty, display.as_str()) {
                RustArgBoundaryMatch::Exact
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
            let target_inner_ty = self.resolved_type_from_rust_display(inner);
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
            if !is_mut && self.types_compatible(arg_ty, &target_inner_ty) {
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
            if Self::rust_display_borrow_kind(display.as_str()).is_none() {
                let target_ty = self.resolved_type_from_rust_display(display.as_str());
                if self.types_compatible(arg_ty, &target_ty) || self.rust_trait_carriers_match(arg_ty, &target_ty) {
                    return RustArgBoundaryMatch::Exact;
                }
            }
            return match admitted_builtin_coercion(incan_display.as_str(), normalized.as_str()) {
                Some(CoercionPolicy::Exact) => RustArgBoundaryMatch::Exact,
                Some(policy) => RustArgBoundaryMatch::Coercion(RustArgCoercionKind::Builtin(policy)),
                None => RustArgBoundaryMatch::NoMatch,
            };
        }
        // Keep token boundaries inside native arguments: compacting `&mut T` into `&mutT` changes its referent.
        let target_ty = self.resolved_type_from_rust_display(display.as_str());
        let should_try_exact_type_match = !borrowed_shared || Self::incan_boundary_type_display(arg_ty).is_none();
        if should_try_exact_type_match
            && (self.types_compatible(arg_ty, &target_ty) || self.rust_trait_carriers_match(arg_ty, &target_ty))
        {
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
        let Some((actual_path, _, _, _)) = self.rust_identity_for_type(arg_ty) else {
            return false;
        };
        let Some(metadata) = self.rust_item_metadata_for_complete_type(actual_path.as_str()) else {
            return false;
        };
        let trait_definition = self.rust_definition_for_path(trait_path);
        Self::rust_type_metadata_implements_trait(&metadata, trait_path, trait_definition.as_deref())
    }

    /// Reject an immutable source binding when a planned Rust boundary borrow is exclusive, and record the exclusive
    /// use of a caller-visible `mut` parameter as a change to it.
    pub(in crate::typechecker) fn validate_rust_borrow_mutability(
        &mut self,
        kind: RustArgCoercionKind,
        arg_expr: &Spanned<Expr>,
    ) -> bool {
        if matches!(
            kind,
            RustArgCoercionKind::Borrow { mutable: true } | RustArgCoercionKind::TraitObjectBorrow { mutable: true }
        ) {
            self.note_mut_param_exclusive_use(arg_expr);
        }
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
    pub(in crate::typechecker) fn validate_rust_boundary_value(
        &mut self,
        owner_path: &str,
        rust_type_display: &str,
        arg_expr: &Spanned<Expr>,
        arg_ty: &ResolvedType,
        record_exact_builtin_coercion: bool,
    ) {
        self.validate_rust_boundary_value_with_target(
            owner_path,
            rust_type_display,
            arg_expr,
            arg_ty,
            record_exact_builtin_coercion,
            None,
        );
    }

    /// Preserve an instantiated checked parameter alongside its display when validating a generic argument. An exact
    /// structural match needs no conversion; all other arguments still require the ordinary native boundary plan.
    /// The optional target never authorizes a new borrow, numeric widening, or nominal-name equivalence.
    fn validate_rust_boundary_value_with_target(
        &mut self,
        owner_path: &str,
        rust_type_display: &str,
        arg_expr: &Spanned<Expr>,
        arg_ty: &ResolvedType,
        record_exact_builtin_coercion: bool,
        instantiated: Option<&ResolvedType>,
    ) {
        self.record_rust_callable_parameter_displays(rust_type_display, owner_path, arg_expr, arg_ty);
        let param_display = self.rust_display_for_owner_path(rust_type_display, owner_path);
        let normalized = Self::compact_rust_display(&param_display);
        let target_ty =
            self.resolved_rust_boundary_target_from_param_display_for_owner_path(rust_type_display, owner_path);
        if self.retain_rust_range_argument(arg_expr, &target_ty) {
            return;
        }
        self.prewarm_rust_type_identity_metadata(arg_ty);
        self.prewarm_rust_boundary_type_identity_metadata(&target_ty);
        if self.retain_native_list_conversion(arg_expr, arg_ty, &target_ty) {
            return;
        }
        let boundary = match self.rust_arg_boundary_match(arg_ty, param_display.as_str()) {
            RustArgBoundaryMatch::NoMatch
                if instantiated.is_some_and(|target| {
                    self.rust_inference_type(arg_ty, &[]) == self.rust_inference_type(target, &[])
                }) =>
            {
                RustArgBoundaryMatch::Exact
            }
            matched => matched,
        };
        match boundary {
            RustArgBoundaryMatch::Exact => {
                if Self::rust_display_type_var_name(normalized.as_str()).is_some() {
                    // A borrowed Rust result passed directly into a generic Rust parameter must keep its native
                    // reference type. Owning it first changes generic selection and can add an unnecessary clone
                    // (for example `String::from(match_.as_str())`). Record the consumption as well as clearing an
                    // existing coercion because return and argument artifacts can be discovered in either order.
                    // The Rust bound remains the final authority.
                    self.type_info.rust.native_return_consumers.insert(arg_expr.span);
                    self.type_info.rust.return_coercions.remove(&arg_expr.span);
                }
                if record_exact_builtin_coercion
                    && let Some(incan_display) = Self::incan_boundary_type_display(arg_ty)
                    && admitted_builtin_coercion(incan_display.as_str(), normalized.as_str())
                        == Some(CoercionPolicy::Exact)
                {
                    self.type_info.rust.arg_coercions.insert(
                        arg_expr.span,
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
                    arg_expr.span,
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

    /// Retain an explicit owned-list mapping only when native From metadata proves each element conversion.
    /// Canonically identical or open element types keep ordinary argument inference; borrowed collections are not
    /// converted.
    fn retain_native_list_conversion(
        &mut self,
        expression: &Spanned<Expr>,
        source: &ResolvedType,
        target: &ResolvedType,
    ) -> bool {
        // ---- Owned collection shape ----
        let (ResolvedType::Generic(source_name, source_args), ResolvedType::Generic(target_name, target_args)) =
            (source, target)
        else {
            return false;
        };
        if collection_type_id(source_name) != Some(CollectionTypeId::List)
            || collection_type_id(target_name) != Some(CollectionTypeId::List)
        {
            return false;
        }
        let ([source], [target]) = (source_args.as_slice(), target_args.as_slice()) else {
            return false;
        };

        // ---- Written literal elements or one checked mapping ----
        if self.retain_native_literal_conversions(expression, target) {
            return true;
        }
        if self.rust_trait_carriers_match(source, target)
            || matches!(source, ResolvedType::Unknown | ResolvedType::CallSiteInfer)
        {
            return false;
        }
        let Some(conversion) = self.native_element_conversion(source, target) else {
            return false;
        };
        self.type_info
            .rust
            .element_adjustments
            .insert(expression.span, conversion);
        true
    }

    /// Preserve native conversions for a contextually typed list literal before its expected element type hides
    /// the written values' types. Mixed compatible values keep their ordinary storage, and a spread stays outside
    /// this fixed-element mapping. Aliases of the same native carrier require no conversion, and frozen text uses the
    /// language's ordinary owned-string boundary.
    fn retain_native_literal_conversions(&mut self, expression: &Spanned<Expr>, target: &ResolvedType) -> bool {
        let Expr::List(entries) = &expression.node else {
            return false;
        };

        // ---- Complete literal conversion selection ----
        let mut conversions = Vec::new();
        for entry in entries {
            let crate::ast::ListEntry::Element(value) = entry else {
                return false;
            };
            let Some(actual) = self.type_info.expr_type(value.span) else {
                return false;
            };
            let source = if *actual == ResolvedType::FrozenStr {
                &ResolvedType::Str
            } else {
                actual
            };
            if self.rust_trait_carriers_match(source, target) {
                continue;
            }
            let Some(conversion) = self.native_element_conversion(source, target) else {
                return false;
            };
            conversions.push((value.span, vec![conversion]));
        }

        // ---- Retain only a fully selected literal ----
        if conversions.is_empty() {
            return false;
        }
        self.type_info.rust.argument_adjustments.extend(conversions);
        true
    }

    /// Preserve direct Rust-return arguments when the receiving Rust callable has no inspected signature.
    ///
    /// Without parameter metadata the compiler cannot justify converting a borrowed Rust result into its owned Incan
    /// surface representation before handing it straight back to Rust. Retain the native result and leave the unknown
    /// boundary to rustc. The expression marker also makes this independent of return/argument traversal order.
    pub(in crate::typechecker) fn preserve_unresolved_rust_call_argument_returns(&mut self, args: &[CallArg]) {
        for arg in args {
            let expr = Self::call_arg_expr(arg);
            let key = expr.span;
            self.type_info.rust.native_return_consumers.insert(key);
            self.type_info.rust.return_coercions.remove(&key);
        }
    }

    /// Contextualize Rust arguments from inspected signatures, including noncapturing boxed callbacks. Executable
    /// boxing still requires its separately retained boundary operation.
    fn rust_params_as_callable_params(
        &self,
        params: &[incan_lang::interop::RustParam],
        owner_path: &str,
    ) -> Vec<CallableParam> {
        let params: Vec<CallableParam> = params
            .iter()
            .map(|param| {
                let ty = self
                    .rust_boxed_callable_signature(&param.type_display)
                    .unwrap_or_else(|| {
                        self.resolved_param_type_from_rust_display_for_owner_path(
                            param.type_display.as_str(),
                            owner_path,
                        )
                    });
                CallableParam {
                    name: param.name.clone(),
                    ty,
                    kind: ParamKind::Normal,
                    has_default: false,
                    is_partial_preset: false,
                    is_mut: false,
                }
            })
            .collect();
        params
    }

    /// Record inspected Rust parameter types so codegen can emit the same borrow shape the typechecker accepted.
    pub(in crate::typechecker) fn record_rust_call_site_params(
        &mut self,
        span: Span,
        params: &[incan_lang::interop::RustParam],
        owner_path: &str,
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
                    is_partial_preset: false,
                    is_mut: false,
                }
            })
            .collect();
        // An inspected Rust signature is the boundary authority, including by-value shapes such as `String` that the
        // emitter cannot safely infer from the argument alone. Preserve the whole selected parameter list rather than
        // retaining only explicit borrows and later falling back to a name/type heuristic.
        self.type_info.record_call_site_callable_params_exact(span, &params);
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
            return self.bind_positional_rust_call_args(callable_display, params, args, arg_types, span);
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
                        bindings.push(RustCallArgBinding {
                            arg,
                            arg_ty,
                            param,
                            parameter_index: positional_index,
                        });
                    }
                    positional_index += 1;
                }
                CallArg::Named(name, _) => {
                    if let Some(first_span) = named_seen.insert(name.node.as_str(), name.span) {
                        self.errors.push(errors::duplicate_call_argument(
                            callable_display,
                            &name.node,
                            first_span,
                            name.span,
                        ));
                    }
                    let Some(param_index) = params_by_name.get(name.node.as_str()).copied() else {
                        self.errors.push(errors::unknown_keyword_argument(
                            callable_display,
                            &name.node,
                            name.span,
                        ));
                        continue;
                    };
                    if let Some(first_span) = bound_spans[param_index] {
                        self.errors.push(errors::duplicate_call_argument(
                            callable_display,
                            &name.node,
                            first_span,
                            name.span,
                        ));
                        continue;
                    }
                    let param = &params[param_index];
                    bound_spans[param_index] = Some(arg_span);
                    bindings.push(RustCallArgBinding {
                        arg,
                        arg_ty,
                        param,
                        parameter_index: param_index,
                    });
                }
                CallArg::PositionalUnpack(_) | CallArg::KeywordUnpack(_) => {}
            }
        }

        self.report_unbound_rust_call_args(
            callable_display,
            params,
            &bound_spans,
            unexpected_positional,
            args.len(),
            span,
        );
        bindings
    }

    /// Bind the position-only Rust path after requiring its complete inspected arity. The selected slot travels with
    /// each argument so later signature specialization cannot change its original callback predicate.
    fn bind_positional_rust_call_args<'a>(
        &mut self,
        callable_display: &str,
        params: &'a [RustParam],
        args: &'a [CallArg],
        arg_types: &'a [ResolvedType],
        span: Span,
    ) -> Vec<RustCallArgBinding<'a>> {
        if arg_types.len() != params.len() {
            self.errors.push(errors::builtin_arity(
                callable_display,
                params.len(),
                arg_types.len(),
                span,
            ));
            return Vec::new();
        }
        args.iter()
            .zip(arg_types.iter())
            .zip(params.iter().enumerate())
            .map(|((arg, arg_ty), (parameter_index, param))| RustCallArgBinding {
                arg,
                arg_ty,
                param,
                parameter_index,
            })
            .collect()
    }

    /// Report extra positional arguments and every unbound inspected parameter after named binding completes. This
    /// preserves the original arity and missing-keyword diagnostics without changing which arguments own slots.
    fn report_unbound_rust_call_args(
        &mut self,
        callable_display: &str,
        params: &[RustParam],
        bound_spans: &[Option<Span>],
        unexpected_positional: usize,
        argument_count: usize,
        span: Span,
    ) {
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
            self.errors.push(errors::builtin_arity(
                callable_display,
                params.len(),
                argument_count,
                span,
            ));
        }
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
    pub(in crate::typechecker) fn rust_arg_matches_boundary(&self, arg_ty: &ResolvedType, rust_param_ty: &str) -> bool {
        !matches!(
            self.rust_arg_boundary_match(arg_ty, rust_param_ty),
            RustArgBoundaryMatch::NoMatch
        )
    }

    /// Return whether a call argument typed without context is a bare Rust owner that the parameter's generic
    /// spelling of the same owner would specialize.
    ///
    /// Arguments are typed before the callee resolves, so a `Self`-returning constructor such as `Box.new(v)`
    /// arrives as the bare owner `std::boxed::Box` while the parameter reads `Box<T>`. Re-checking that one
    /// argument against its parameter lets the receiver reconcile — the same contextual path literals and closures
    /// already take. Projected source collections also carry their parameter expectation into the nested constructor.
    /// Native owners are compared in the ancestral namespace so every re-export spelling matches.
    fn rust_call_arg_wants_owner_expectation(arg_ty: &ResolvedType, target_ty: &ResolvedType) -> bool {
        if let (ResolvedType::Generic(actual, _), ResolvedType::Generic(expected, _)) = (arg_ty, target_ty) {
            return actual == expected;
        }
        // The uncontextual type may already carry the owner's own unresolved parameters (`RefCell<T>` from a
        // `RefCell<T>`-returning `new`), so compare owners by base, not whole display.
        let arg_owner = match arg_ty {
            ResolvedType::RustPath(path) | ResolvedType::Named(path) => path.split('<').next().unwrap_or(path),
            _ => return false,
        };
        let ResolvedType::RustPath(target) = target_ty else {
            return false;
        };
        let Some((base, _)) = target.split_once('<') else {
            return false;
        };
        incan_lang::interop::ancestral_rust_path(base) == incan_lang::interop::ancestral_rust_path(arg_owner)
    }

    /// Validate a Rust method call (`receiver.method(...)`) against metadata and record required arg coercions.
    ///
    /// The receiver is already validated by access resolution; this function validates only post-receiver parameters.
    pub(in crate::typechecker) fn validate_rust_method_call(
        &mut self,
        callable_display: &str,
        sig: &RustFunctionSig,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        preserves_lookup_arg_shape: bool,
        span: Span,
    ) -> ResolvedType {
        self.validate_rust_method_call_with_expected(
            callable_display,
            sig,
            args,
            arg_types,
            preserves_lookup_arg_shape,
            span,
            None,
        )
    }

    /// Validate a Rust method call, letting the type its result is expected to have bind method binders the
    /// arguments leave open (`parse::<F>()` reads `F` from `Result<i64, _>`). Literal aggregates are checked against
    /// the specialized parameter before their concrete storage is retained for Body IR.
    pub(in crate::typechecker) fn validate_rust_method_call_with_expected(
        &mut self,
        callable_display: &str,
        sig: &RustFunctionSig,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        preserves_lookup_arg_shape: bool,
        span: Span,
        expected_return_ty: Option<&ResolvedType>,
    ) -> ResolvedType {
        let owner_path = Self::rust_method_owner_path(callable_display);
        let contextual_types =
            self.contextualize_rust_bound_closures(sig, owner_path, args, arg_types, expected_return_ty);
        let arg_types = contextual_types.as_slice();
        if let Some(contract) = sig.receiver_contract {
            self.type_info.rust.receiver_contracts.insert(span, contract);
        }
        if sig.is_async {
            self.type_info.rust.async_call_realizations.insert(span);
        }
        if sig.is_async && !self.is_in_await_operand(span) {
            self.errors
                .push(errors::async_call_without_await(callable_display, span));
        }
        let callable_returns =
            self.infer_rust_signature_bindings(sig, owner_path, &sig.type_params, args, arg_types, expected_return_ty);
        self.record_inferred_rust_arguments(callable_display, &sig.type_params, &callable_returns, span);
        let declared_sig = sig;
        let declared_params = if Self::rust_signature_has_receiver(sig) {
            &sig.params[1..]
        } else {
            &sig.params
        };
        let effective_sig = self.specialize_inferred_rust_signature(sig, owner_path, &callable_returns);
        let sig = &effective_sig;
        let params = if Self::rust_signature_has_receiver(sig) {
            &sig.params[1..]
        } else {
            &sig.params
        };
        self.record_rust_call_site_params(span, params, owner_path);

        let binding_errors_before = self.errors.len();
        let bindings = self.bind_rust_call_args(callable_display, params, args, arg_types, span);
        if self.errors.len() != binding_errors_before {
            return self.resolved_rust_call_type_from_sig(sig, owner_path, span);
        }

        for binding in bindings {
            let arg_expr = Self::call_arg_expr(binding.arg);
            let param = binding.param;
            if let Some(declared) = declared_params.get(binding.parameter_index)
                && self.validate_rust_bound_callable_argument(
                    declared_sig,
                    &declared.type_display,
                    &callable_returns,
                    arg_expr,
                    binding.arg_ty,
                )
            {
                continue;
            }
            let target_ty = self.resolved_rust_boundary_target_from_param_display_for_owner_path(
                param.type_display.as_str(),
                owner_path,
            );
            let contextual_arg_ty = if (matches!(arg_expr.node, Expr::Closure(_, _))
                && matches!(&target_ty, ResolvedType::Function(_, _)))
                || super::super::ops::is_unsuffixed_integer_literal(arg_expr)
                || matches!(
                    arg_expr.node,
                    Expr::Literal(_)
                        | Expr::Unary(_, _)
                        | Expr::List(_)
                        | Expr::Dict(_)
                        | Expr::Set(_)
                        | Expr::Tuple(_)
                )
                || (matches!(arg_expr.node, Expr::Call(..) | Expr::MethodCall(..))
                    && Self::rust_call_arg_wants_owner_expectation(binding.arg_ty, &target_ty))
            {
                Some(self.check_expr_with_expected(arg_expr, Some(&target_ty)))
            } else {
                None
            };
            let arg_ty = contextual_arg_ty.as_ref().unwrap_or(binding.arg_ty);
            if preserves_lookup_arg_shape && self.rust_lookup_probe_boundary_match(arg_ty, &target_ty) {
                continue;
            }
            let instantiated = declared_params.get(binding.parameter_index).map(|declared| {
                self.instantiate_rust_signature_type(
                    &self.native_rust_signature_type(&declared.type_display, owner_path),
                    &declared_sig.type_params,
                    &callable_returns,
                )
            });
            self.validate_rust_boundary_value_with_target(
                owner_path,
                param.type_display.as_str(),
                arg_expr,
                arg_ty,
                false,
                instantiated.as_ref(),
            );
        }

        let ret = self.resolved_rust_call_type_from_sig(sig, owner_path, span);
        self.record_rust_return_coercion_from_display(sig.return_type.as_str(), &ret, span);
        ret
    }

    /// Validate a direct Rust function call (`rust::path::item(...)`) and record boundary coercions.
    #[cfg(test)]
    pub(in crate::typechecker::check_expr) fn validate_rust_function_call(
        &mut self,
        path: &str,
        sig: &RustFunctionSig,
        args: &[CallArg],
        span: Span,
    ) -> ResolvedType {
        self.validate_rust_function_call_with_expected(path, sig, args, span, None)
    }

    /// Validate a Rust function from its metadata, instantiate its parameters from inputs and expected output, and
    /// retain the selected callee for Body IR when validation succeeds. Unsafe signatures are refused independently
    /// of checked C acknowledgment regions: RFC 005 provides no Rust safety opt-in.
    pub(in crate::typechecker::check_expr) fn validate_rust_function_call_with_expected(
        &mut self,
        path: &str,
        sig: &RustFunctionSig,
        args: &[CallArg],
        span: Span,
        expected_return_ty: Option<&ResolvedType>,
    ) -> ResolvedType {
        let errors_before = self.errors.len();
        if sig.is_unsafe {
            self.check_call_args(args);
            self.errors.push(errors::rust_unsafe_call_not_supported(path, span));
            return ResolvedType::Unknown;
        }
        if sig.is_async {
            self.type_info.rust.async_call_realizations.insert(span);
        }
        if sig.is_async && !self.is_in_await_operand(span) {
            self.errors.push(errors::async_call_without_await(path, span));
        }
        let expected_params = self.rust_params_as_callable_params(&sig.params, path);
        let arg_types = self.check_call_arg_types_for_params(args, &expected_params);
        // A closure passed to a callable-bounded binder takes its parameter types from the bound, as for a method.
        let arg_types = self.contextualize_rust_bound_closures(sig, path, args, &arg_types, expected_return_ty);
        self.record_rust_call_site_params(span, &sig.params, path);
        let binding_errors_before = self.errors.len();
        self.bind_rust_call_args(path, &sig.params, args, &arg_types, span);
        if self.errors.len() != binding_errors_before {
            return self.resolved_rust_call_type_from_sig(sig, path, span);
        }

        let callable_returns =
            self.infer_rust_signature_bindings(sig, path, &sig.type_params, args, &arg_types, expected_return_ty);
        self.record_inferred_rust_arguments(path, &sig.type_params, &callable_returns, span);
        let effective_sig = self.specialize_inferred_rust_signature(sig, path, &callable_returns);
        self.record_rust_call_site_params(span, &effective_sig.params, path);
        let bindings = self.bind_rust_call_args(path, &effective_sig.params, args, &arg_types, span);
        for binding in bindings {
            let arg_expr = Self::call_arg_expr(binding.arg);
            let arg_ty = binding.arg_ty;
            let param = binding.param;
            if let Some(declared) = sig.params.get(binding.parameter_index)
                && self.validate_rust_bound_callable_argument(
                    sig,
                    &declared.type_display,
                    &callable_returns,
                    arg_expr,
                    arg_ty,
                )
            {
                continue;
            }
            let instantiated = sig.params.get(binding.parameter_index).map(|declared| {
                self.instantiate_rust_signature_type(
                    &self.native_rust_signature_type(&declared.type_display, path),
                    &sig.type_params,
                    &callable_returns,
                )
            });
            self.validate_rust_boundary_value_with_target(
                path,
                param.type_display.as_str(),
                arg_expr,
                arg_ty,
                false,
                instantiated.as_ref(),
            );
        }

        let ret = self.resolved_rust_call_type_from_sig(&effective_sig, path, span);
        let ret = self.instantiate_rust_signature_type(&ret, &sig.type_params, &callable_returns);
        self.record_rust_return_coercion_from_display(sig.return_type.as_str(), &ret, span);
        if self.errors.len() == errors_before {
            self.retain_checked_rust_function(path, sig, &callable_returns, span);
        }
        ret
    }
}

#[cfg(test)]
mod validate_rust_function_call_tests {
    use super::{RustArgBoundaryMatch, TypeChecker};
    use crate::ast::{CallArg, Expr, IntLiteral, Literal, Span, Spanned};
    use crate::symbols::{
        CallableParam, NewtypeInfo, ResolvedType, RustImportBindingKind, RustItemInfo, ScopeKind, Symbol, SymbolKind,
        TypeInfo, VariableInfo,
    };
    use crate::typechecker::{RustArgCoercionInfo, RustArgCoercionKind};
    use incan_lang::interop::{
        CoercionPolicy, RustAssociatedTypeBinding, RustFunctionSig, RustItemKind, RustItemMetadata, RustParam,
        RustTraitBound, RustTypeInfo, RustVisibility,
    };
    use incan_lang::lang::types::collections::{self, CollectionTypeId};
    use incan_lang::lang::types::numerics::{self, NumericTypeId};
    use std::collections::HashMap;

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
    fn exact_generic_rust_boundary_preserves_nested_borrowed_return() {
        let mut checker = TypeChecker::new();
        let span = Span::new(10, 20);
        let argument = Spanned::new(Expr::Ident("borrowed".to_string()), span);
        checker.type_info.rust.return_coercions.insert(
            span,
            RustArgCoercionInfo {
                rust_target_type: "String".to_string(),
                target_type: ResolvedType::Str,
                kind: RustArgCoercionKind::Builtin(CoercionPolicy::Exact),
            },
        );

        checker.validate_rust_boundary_value("std::string::String::from", "T", &argument, &ResolvedType::Str, false);

        assert!(
            checker.type_info.rust.return_coercions.is_empty(),
            "a direct generic Rust consumer must receive the original borrowed Rust result"
        );
        checker.record_rust_return_coercion_from_display("&str", &ResolvedType::Str, span);
        assert!(
            checker.type_info.rust.return_coercions.is_empty(),
            "later return inspection must preserve the generic consumer's native Rust value"
        );
        assert_eq!(
            checker.type_info.rust.native_return_consumers,
            std::collections::HashSet::from([span])
        );
    }

    #[test]
    fn unresolved_rust_call_preserves_direct_native_return() {
        let mut checker = TypeChecker::new();
        let span = Span::new(30, 45);
        let argument = Spanned::new(Expr::Ident("borrowed".to_string()), span);
        checker.type_info.rust.return_coercions.insert(
            span,
            RustArgCoercionInfo {
                rust_target_type: "String".to_string(),
                target_type: ResolvedType::Str,
                kind: RustArgCoercionKind::Builtin(CoercionPolicy::Exact),
            },
        );

        checker.preserve_unresolved_rust_call_argument_returns(&[CallArg::Positional(argument)]);

        assert!(checker.type_info.rust.return_coercions.is_empty());
        assert!(checker.type_info.rust.native_return_consumers.contains(&span));
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

    /// Native collection namespaces keep structural identity, public nominal owners remain import-independent, and
    /// metadata aliases preserve unsized text only at the native boundary without changing source string values.
    #[test]
    fn canonical_rust_collection_namespaces_keep_structural_boundary_identity() {
        let mut loaves = crate::loaf_index::LoafIndex::default();
        loaves.add_rust_facts(
            [("std::primitive::str", "str"), ("native::Text", "str")]
                .into_iter()
                .map(|(path, target)| {
                    (
                        path.to_owned(),
                        RustItemMetadata {
                            canonical_path: path.to_owned(),
                            definition_path: None,
                            visibility: RustVisibility::Public,
                            kind: RustItemKind::Type(RustTypeInfo {
                                alias_target: Some(target.to_owned()),
                                ..RustTypeInfo::default()
                            }),
                        },
                    )
                })
                .collect(),
        );
        let mut checker = TypeChecker::new();
        checker.set_loaves(std::sync::Arc::new(loaves));
        let strings = ResolvedType::Generic("List".to_string(), vec![ResolvedType::Str]);

        assert_eq!(
            checker.rust_arg_boundary_match(&strings, "alloc::vec::Vec<alloc::string::String>"),
            RustArgBoundaryMatch::Exact
        );
        assert_eq!(
            checker.rust_arg_boundary_match(&ResolvedType::Bytes, "alloc::vec::Vec<u8>"),
            RustArgBoundaryMatch::Exact
        );
        for display in ["str", "std::primitive::str", "native::Text"] {
            assert_eq!(
                checker.native_rust_signature_type(display, ""),
                ResolvedType::RustPath("str".into()),
                "{display} retains the metadata-proven unsized native referent"
            );
            assert_eq!(checker.resolved_type_from_rust_display(display), ResolvedType::Str);
        }
        assert_eq!(
            checker.native_rust_signature_type("&std::primitive::str", ""),
            ResolvedType::Ref(Box::new(ResolvedType::RustPath("str".into())))
        );
        for display in ["String", "std::string::String"] {
            assert_eq!(checker.native_rust_signature_type(display, ""), ResolvedType::Str);
        }
        verify_native_owner_is_import_independent();
    }

    /// Import aliases cannot replace a producer-authenticated public owner. Private definitions and another bound
    /// extern remain behind their original public projections, even when their underlying metadata is available.
    fn verify_native_owner_is_import_independent() {
        let owner = "native::types::Token";
        let first = "native::facade::Token";
        let second = "native::other::Token";
        let hidden = "native::private::Secret";
        let public_secret = "native::Secret";
        let other_extern = "renamed::Token";
        let definitions = [
            (owner, owner, RustVisibility::Public),
            (first, owner, RustVisibility::Public),
            (second, owner, RustVisibility::Public),
            (hidden, hidden, RustVisibility::Restricted),
            (public_secret, hidden, RustVisibility::Public),
            (other_extern, owner, RustVisibility::Public),
        ];
        for imports in [&[first][..], &[second][..], &[first, second][..]] {
            let mut loaves = crate::loaf_index::LoafIndex::default();
            loaves.add_rust_facts(
                definitions
                    .iter()
                    .map(|(path, definition, visibility)| {
                        (
                            (*path).to_owned(),
                            RustItemMetadata {
                                canonical_path: (*path).to_owned(),
                                definition_path: (*path != *definition).then(|| (*definition).to_owned()),
                                visibility: *visibility,
                                kind: RustItemKind::Type(RustTypeInfo::default()),
                            },
                        )
                    })
                    .collect(),
            );
            let mut checker = TypeChecker::new();
            checker.set_loaves(std::sync::Arc::new(loaves));
            for (index, path) in imports.iter().copied().chain([public_secret, other_extern]).enumerate() {
                checker.symbols.define(Symbol {
                    name: format!("Imported{index}"),
                    kind: SymbolKind::RustItem(RustItemInfo {
                        crate_name: path.split("::").next().unwrap_or("").to_owned(),
                        path: path.to_owned(),
                        binding: RustImportBindingKind::FromImport,
                        metadata: checker.rust_item_metadata_for_path(path),
                    }),
                    span: Span::new(index, index + 1),
                    scope: 0,
                });
            }
            let paths = checker.checked_rust_item_public_paths();
            for imported in imports {
                assert_eq!(paths.get(*imported).map(String::as_str), Some(owner));
            }
            assert_eq!(paths.get(public_secret).map(String::as_str), Some(public_secret));
            assert_eq!(paths.get(other_extern).map(String::as_str), Some(other_extern));
        }
    }

    #[test]
    fn by_value_trait_object_inside_a_generic_matches_itself_on_every_route() {
        // #1229: the argument side reaches `resolved_type_from_rust_display` with the raw display while the
        // parameter side is compacted first. Both must land on one normal form, or `Arc<dyn Trait>` fails to match
        // itself because one route kept the space after `dyn` and the other erased it.
        let checker = TypeChecker::new();
        let arg = checker.resolved_type_from_rust_display("alloc::sync::Arc<dyn demo::Adapter>");

        assert_eq!(
            checker.rust_arg_boundary_match(&arg, "alloc::sync::Arc<dyn demo::Adapter>"),
            RustArgBoundaryMatch::Exact
        );
        assert_eq!(
            checker.rust_arg_boundary_match(&arg, "alloc::sync::Arc< dyn demo::Adapter >"),
            RustArgBoundaryMatch::Exact
        );
        assert_eq!(
            TypeChecker::compact_rust_display("alloc::sync::Arc< dyn demo::Adapter >"),
            "alloc::sync::Arc<dyn demo::Adapter>"
        );
    }

    #[test]
    fn anonymous_impl_trait_parameters_keep_by_value_shape_in_both_spellings() {
        for display in ["impl demo::Buf", "implBuf", "impl Buf"] {
            assert!(
                TypeChecker::is_rust_generic_type_param_display(display),
                "{display} must be a by-value generic boundary"
            );
            assert_eq!(TypeChecker::rust_display_type_var_name(display), Some(display));
        }
        assert_eq!(TypeChecker::rust_display_type_var_name("dyn demo::Adapter"), None);
        assert_eq!(TypeChecker::rust_display_type_var_name("T"), Some("T"));
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
                .get(&span)
                .map(|coercion| coercion.kind),
            Some(RustArgCoercionKind::Borrow { mutable: true })
        );
    }

    /// #1561: a `mut` parameter or local belongs to the body that declares it. Once a module whose function declares a
    /// `mut header` parameter and a `mut` local is checked, neither name is mutable anywhere else, so an immutable
    /// `header` passed to a Rust `&mut` parameter is still refused.
    #[test]
    fn mutable_bindings_of_one_body_do_not_reach_another_issue1561() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def first(mut header: list[int]) -> None:\n    mut local: list[int] = [1]\n    local.append(1)\n    header.append(1)\n\nclass Writer:\n    lines: list[int]\n\n    def write(mut self, mut header: list[int]) -> None:\n        header.append(1)\n        self.lines.append(1)\n";
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("lex failed: {errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("parse failed: {errors:?}"))?;
        let mut checker = TypeChecker::new();
        checker
            .check_program(&program)
            .map_err(|errors| format!("the module must check: {errors:?}"))?;
        assert!(
            checker.mutable_bindings.is_empty(),
            "each body's `mut` bindings end with the body, got {:?}",
            checker.mutable_bindings
        );

        let span = Span::new(10, 16);
        let header = ResolvedType::RustPath("demo::Header".to_string());
        let argument = Spanned::new(Expr::Ident("header".to_string()), span);
        checker.validate_rust_boundary_value("demo::Writer", "&mut demo::Header", &argument, &header, false);
        assert!(
            checker.errors.iter().any(|error| error
                .message
                .contains("Rust parameter requires a mutable borrow of 'header'")),
            "another body's `mut header` does not make this `header` mutable, got {:?}",
            checker.errors
        );
        Ok(())
    }

    /// A method's mutable-reference parameter rejects an immutable binding without recording a coercion.
    #[test]
    fn rust_method_mutable_reference_rejects_an_immutable_binding() {
        let span = Span::new(10, 16);
        let argument = Spanned::new(Expr::Ident("header".to_string()), span);
        let args = [CallArg::Positional(argument)];
        let sig = RustFunctionSig {
            receiver_contract: None,
            bounds: Vec::new(),
            type_params: Vec::new(),
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

    /// A parameterless Rust function refuses an extra argument with an arity diagnostic.
    #[test]
    fn zero_parameter_rust_sig_rejects_extra_arguments() {
        let mut checker = TypeChecker::new();
        let span = Span::new(0, 1);
        let arg_expr = Spanned::new(Expr::Literal(Literal::Int(IntLiteral::synthetic(1))), span);
        let args = [CallArg::Positional(arg_expr)];
        let sig = RustFunctionSig {
            receiver_contract: None,
            bounds: Vec::new(),
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

    /// A callback taking owned values cannot satisfy a Rust FnMut bound requiring borrowed parameters.
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
            receiver_contract: None,
            bounds: Vec::new(),
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

    /// A callback with exactly the inspected borrowed parameters satisfies its Rust FnMut bound.
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
            receiver_contract: None,
            bounds: Vec::new(),
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
        verify_structured_generic_callback_binding();
    }

    /// Keep nested source identities through generic callback substitution and named argument ordering, while
    /// rejecting a callback whose input loses the native predicate's exclusive borrow.
    fn verify_structured_generic_callback_binding() {
        let mut checker = TypeChecker::new();
        let span = Span::new(100, 180);
        let native_map = ResolvedType::Generic(
            "std::collections::HashMap".into(),
            vec![ResolvedType::Str, ResolvedType::Named("SourceEntry".into())],
        );
        let projected_map = ResolvedType::Generic(
            "Dict".into(),
            vec![ResolvedType::Str, ResolvedType::Named("SourceEntry".into())],
        );
        assert_eq!(checker.rust_inference_type(&native_map, &[]), projected_map);
        let callback = ResolvedType::Function(
            vec![CallableParam::positional(ResolvedType::RefMut(Box::new(
                native_map.clone(),
            )))],
            Box::new(ResolvedType::Bool),
        );
        for (name, ty) in [("entries", native_map.clone()), ("predicate", callback)] {
            checker.symbols.define(Symbol {
                name: name.into(),
                kind: SymbolKind::Variable(VariableInfo {
                    ty,
                    is_mutable: false,
                    is_used: false,
                }),
                span,
                scope: 0,
            });
        }
        let signature = RustFunctionSig {
            receiver_contract: None,
            type_params: vec!["T".into(), "F".into()],
            bounds: vec![RustTraitBound {
                self_type: "F".into(),
                path: "std::ops::FnMut".into(),
                type_arguments: vec!["(&mut T,)".into()],
                associated_type_bindings: vec![RustAssociatedTypeBinding {
                    name: "Output".into(),
                    value_path: "bool".into(),
                }],
            }],
            params: vec![
                RustParam {
                    name: Some("entries".into()),
                    type_display: "T".into(),
                },
                RustParam {
                    name: Some("predicate".into()),
                    type_display: "F".into(),
                },
            ],
            return_type: "bool".into(),
            is_async: false,
            is_unsafe: false,
        };
        let args = [
            CallArg::Named(
                Spanned::new("predicate".into(), span),
                Spanned::new(Expr::Ident("predicate".into()), span),
            ),
            CallArg::Named(
                Spanned::new("entries".into(), span),
                Spanned::new(Expr::Ident("entries".into()), span),
            ),
        ];
        let _ = checker.validate_rust_function_call("rust::demo::visit", &signature, &args, span);
        assert!(
            checker.errors.is_empty(),
            "nested source callback must stay structural: {:?}",
            checker.errors
        );
        let wrong_input = ResolvedType::Function(
            vec![CallableParam::positional(native_map.clone())],
            Box::new(ResolvedType::Bool),
        );
        let bindings = HashMap::from([("T".into(), native_map)]);
        assert!(checker.validate_rust_bound_callable_argument(
            &signature,
            "F",
            &bindings,
            &Spanned::new(Expr::Ident("predicate".into()), span),
            &wrong_input,
        ));
        assert!(
            !checker.errors.is_empty(),
            "an owned callback cannot satisfy an exclusive input"
        );
    }

    /// A generic callback placeholder remains admissible while Rust supplies its concrete callable implementation.
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
            receiver_contract: None,
            bounds: Vec::new(),
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

    /// A parameterless Rust function accepts an empty argument list.
    #[test]
    fn zero_parameter_rust_sig_allows_no_arguments() {
        let mut checker = TypeChecker::new();
        let span = Span::new(0, 1);
        let sig = RustFunctionSig {
            receiver_contract: None,
            bounds: Vec::new(),
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

    /// Missing Rust arguments produce an arity error before parameter-wise validation can truncate the list.
    #[test]
    fn too_few_arguments_reports_arity_before_param_zip() {
        let mut checker = TypeChecker::new();
        let span = Span::new(0, 1);
        let arg_expr = Spanned::new(Expr::Literal(Literal::Int(IntLiteral::synthetic(1))), span);
        let args = [CallArg::Positional(arg_expr)];
        let sig = RustFunctionSig {
            receiver_contract: None,
            bounds: Vec::new(),
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

    /// Keyword arguments bind by inspected Rust parameter names even when written in another order.
    #[test]
    fn rust_function_call_binds_keyword_args_by_inspected_param_name() {
        let mut checker = TypeChecker::new();
        let span = Span::new(0, 60);
        let text_span = Span::new(10, 20);
        let count_span = Span::new(30, 31);
        let args = [
            CallArg::Named(
                Spanned::new("text".to_string(), text_span),
                Spanned::new(Expr::Literal(Literal::String("demo".to_string())), text_span),
            ),
            CallArg::Named(
                Spanned::new("count".to_string(), count_span),
                Spanned::new(Expr::Literal(Literal::Int(IntLiteral::synthetic(3))), count_span),
            ),
        ];
        let sig = RustFunctionSig {
            receiver_contract: None,
            bounds: Vec::new(),
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
            .get(&span)
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

    /// A Rust receiver does not satisfy a missing required value parameter.
    #[test]
    fn rust_method_call_rejects_missing_required_arguments_after_receiver() {
        let mut checker = TypeChecker::new();
        let span = Span::new(0, 1);
        let sig = RustFunctionSig {
            receiver_contract: None,
            bounds: Vec::new(),
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

    /// Rust method validation checks value parameter types after excluding the receiver.
    #[test]
    fn rust_method_call_rejects_type_mismatch_after_receiver() {
        let mut checker = TypeChecker::new();
        let span = Span::new(0, 1);
        let sig = RustFunctionSig {
            receiver_contract: None,
            bounds: Vec::new(),
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
            checker.errors.iter().any(|error| {
                error.span == span
                    && error.expected() == Some("&str")
                    && error.actual() == Some(numerics::descriptor(NumericTypeId::I64))
            }),
            "expected type mismatch diagnostic for method arg, errors={:?}",
            checker.errors
        );
    }

    /// A lookup-preserving method accepts a string probe for an inspected String reference without coercing it.
    #[test]
    fn rust_lookup_preserving_method_accepts_string_probe_for_ref_string_param() {
        let mut checker = TypeChecker::new();
        let span = Span::new(0, 1);
        let sig = RustFunctionSig {
            receiver_contract: None,
            bounds: Vec::new(),
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

    /// A lookup-preserving generic reference parameter keeps a string probe's original argument shape, and native
    /// retention preserves validated written method arguments independently of the receiver's impl binders.
    #[test]
    fn rust_lookup_preserving_method_accepts_string_probe_for_generic_ref_param() {
        let mut checker = TypeChecker::new();
        let span = Span::new(0, 1);
        let sig = RustFunctionSig {
            receiver_contract: None,
            bounds: Vec::new(),
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
        TypeChecker::verify_written_native_method_arguments();
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
                derives: Vec::new(),
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

    /// A string value is accepted at a Rust function's borrowed String boundary.
    #[test]
    fn rust_function_call_accepts_string_for_borrowed_string_param() {
        let mut checker = TypeChecker::new();
        let span = Span::new(10, 20);
        let arg_expr = Spanned::new(Expr::Literal(Literal::String("{}".to_string())), span);
        let args = [CallArg::Positional(arg_expr)];
        let sig = RustFunctionSig {
            receiver_contract: None,
            bounds: Vec::new(),
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
            checker.type_info.rust.arg_coercions.contains_key(&span),
            "expected rust arg coercion metadata for borrowed String boundary"
        );
        let expected = ResolvedType::Ref(Box::new(ResolvedType::RustPath("String".to_string())));
        assert_eq!(
            checker
                .type_info
                .rust
                .arg_coercions
                .get(&span)
                .map(|coercion| &coercion.target_type),
            Some(&expected),
            "borrowed owned Rust params must preserve owned target shape in lowering metadata"
        );
    }

    /// A string value is accepted at a Rust function's borrowed str boundary.
    #[test]
    fn rust_function_call_accepts_string_for_borrowed_str_param() {
        let mut checker = TypeChecker::new();
        let span = Span::new(10, 20);
        let arg_expr = Spanned::new(Expr::Literal(Literal::String("{}".to_string())), span);
        let args = [CallArg::Positional(arg_expr)];
        let sig = RustFunctionSig {
            receiver_contract: None,
            bounds: Vec::new(),
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
                .get(&span)
                .map(|coercion| &coercion.target_type),
            Some(&expected),
            "borrowed str params must stay distinct from borrowed owned String params"
        );
    }

    /// Bytes satisfy an inspected borrowed byte-vector parameter.
    #[test]
    fn rust_function_call_accepts_bytes_for_borrowed_vec_param() {
        let mut checker = TypeChecker::new();
        let span = Span::new(10, 20);
        let arg_expr = Spanned::new(Expr::Literal(Literal::Bytes(b"abc".to_vec())), span);
        let args = [CallArg::Positional(arg_expr)];
        let sig = RustFunctionSig {
            receiver_contract: None,
            bounds: Vec::new(),
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
                .get(&span)
                .map(|coercion| &coercion.target_type),
            Some(&expected),
            "borrowed owned Rust byte-vector params must preserve owned target shape in lowering metadata"
        );
    }

    /// A concrete borrowed Rust path keeps its nominal type instead of becoming an unknown-reference diagnostic.
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
            receiver_contract: None,
            bounds: Vec::new(),
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

    /// An accepted rusttype function argument records the interop conversion required by lowering.
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
                derives: Vec::new(),
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
            receiver_contract: None,
            bounds: Vec::new(),
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
            checker.type_info.rust.arg_coercions.contains_key(&span),
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

    /// A by-value generic Rust method parameter retains its checked concrete argument shape for lowering.
    #[test]
    fn validate_rust_method_call_records_by_value_generic_param_shape() {
        let mut checker = TypeChecker::new();
        let span = Span::new(30, 40);
        let arg_expr = Spanned::new(Expr::Ident("cursor".to_string()), span);
        let args = [CallArg::Positional(arg_expr)];
        let arg_types = [ResolvedType::RustPath("std::io::Cursor<Vec<u8>>".to_string())];
        let sig = RustFunctionSig {
            receiver_contract: None,
            bounds: Vec::new(),
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
            checker.type_info.rust.arg_coercions.is_empty(),
            "an owned generic argument must not acquire a borrow coercion: {:?}",
            checker.type_info.rust.arg_coercions
        );
        assert!(
            checker
                .type_info
                .calls
                .call_site_callable_params
                .get(&span)
                .is_some_and(|params| params.len() == 1 && params[0].ty == arg_types[0]),
            "expected the concrete owned Rust generic method parameter, got {:?}",
            checker.type_info.calls.call_site_callable_params
        );
    }

    /// An inspected owned String method parameter remains by value in the recorded callable signature.
    #[test]
    fn validate_rust_method_call_records_owned_string_param_shape() {
        let mut checker = TypeChecker::new();
        let span = Span::new(30, 40);
        let arg_expr = Spanned::new(Expr::Ident("value".to_string()), span);
        let args = [CallArg::Positional(arg_expr)];
        let arg_types = [ResolvedType::Str];
        let sig = RustFunctionSig {
            receiver_contract: None,
            bounds: Vec::new(),
            type_params: Vec::new(),
            params: vec![RustParam {
                name: Some("value".to_string()),
                type_display: "String".to_string(),
            }],
            return_type: "demo::Value".to_string(),
            is_async: false,
            is_unsafe: false,
        };

        let _ = checker.validate_rust_method_call("rust::demo::Value.string", &sig, &args, &arg_types, false, span);

        assert!(
            checker.errors.is_empty(),
            "unexpected Rust boundary errors: {:?}",
            checker.errors
        );
        assert!(
            checker
                .type_info
                .calls
                .call_site_callable_params
                .get(&span)
                .is_some_and(|params| params.len() == 1 && params[0].ty == ResolvedType::Str),
            "an inspected by-value String parameter must reach lowering exactly"
        );
    }

    /// A by-value impl Trait method parameter retains its inferred byte-vector storage without a borrow coercion.
    #[test]
    fn validate_rust_method_call_records_by_value_impl_trait_param_shape() {
        let mut checker = TypeChecker::new();
        let span = Span::new(30, 40);
        let arg_expr = Spanned::new(Expr::Ident("encoded".to_string()), span);
        let args = [CallArg::Positional(arg_expr)];
        let arg_types = [ResolvedType::Bytes];
        let sig = RustFunctionSig {
            receiver_contract: None,
            bounds: Vec::new(),
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
                .get(&span)
                .is_some_and(|params| params.len() == 1
                    && params[0].ty
                        == ResolvedType::Generic(
                            collections::as_str(CollectionTypeId::List).to_owned(),
                            vec![ResolvedType::Numeric(NumericTypeId::U8)],
                        )),
            "expected the concrete owned byte-vector method parameter, got {:?}",
            checker.type_info.calls.call_site_callable_params
        );
    }

    /// A borrowed-slice callback preserves the exact inspected Rust parameter displays for lowering.
    #[test]
    fn validate_rust_method_call_records_borrowed_slice_callback_displays_issue835() {
        let mut checker = TypeChecker::new();
        let span = Span::new(30, 40);
        let callback_span = Span::new(12, 25);
        let callback_expr = Spanned::new(Expr::Ident("write_silence".to_string()), callback_span);
        let args = [CallArg::Positional(callback_expr)];
        let callback_ty = ResolvedType::Function(
            vec![
                CallableParam::positional(ResolvedType::RefMut(Box::new(ResolvedType::Generic(
                    "List".to_string(),
                    vec![ResolvedType::Numeric(NumericTypeId::F32)],
                )))),
                CallableParam::positional(ResolvedType::Ref(Box::new(ResolvedType::RustPath(
                    "demo::OutputCallbackInfo".to_string(),
                )))),
            ],
            Box::new(ResolvedType::Unit),
        );
        let sig = RustFunctionSig {
            receiver_contract: None,
            bounds: Vec::new(),
            type_params: Vec::new(),
            params: vec![
                RustParam {
                    name: Some("self".to_string()),
                    type_display: "&self".to_string(),
                },
                RustParam {
                    name: Some("callback".to_string()),
                    type_display: "impl FnMut(&mut [f32], &demo::OutputCallbackInfo)".to_string(),
                },
            ],
            return_type: "()".to_string(),
            is_async: false,
            is_unsafe: false,
        };

        let _ = checker.validate_rust_method_call("rust::demo::Device.run", &sig, &args, &[callback_ty], false, span);

        assert!(
            checker.errors.is_empty(),
            "expected the borrowed-slice callback to satisfy the Rust method bound, got {:?}",
            checker.errors
        );
        assert_eq!(
            checker.type_info.rust.function_param_type_displays.get("write_silence"),
            Some(&vec!["&mut [f32]".to_string(), "&demo::OutputCallbackInfo".to_string(),]),
            "expected method validation to preserve exact callback parameter displays"
        );
    }

    /// An accepted rusttype method argument records the same interop conversion as a function argument.
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
                derives: Vec::new(),
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
            receiver_contract: None,
            bounds: Vec::new(),
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
            checker.type_info.rust.arg_coercions.contains_key(&span),
            "expected rust arg coercion metadata for rust method boundary"
        );
    }
}
