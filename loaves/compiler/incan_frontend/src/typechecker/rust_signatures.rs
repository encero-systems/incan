//! Checked types for Rust signatures, callable bounds, shapes, and Rust display spellings.

use super::*;

impl TypeChecker {
    /// Derive one checked call's loan identities from its retained Rust signature. Explicit lifetime names and type
    /// parameters keep their declaration identity; ordinary elision selects the receiver or sole reference input.
    pub(super) fn native_signature_loan_identities(signature: &RustFunctionSig) -> Vec<Vec<String>> {
        let mut slots = signature
            .params
            .iter()
            .map(|parameter| native_signature_tokens(&parameter.type_display, &signature.type_params))
            .collect::<Vec<_>>();
        let mut result = native_signature_tokens(&signature.return_type, &signature.type_params);
        if native_display_has_elided_loan(&signature.return_type, &result) {
            let source = if Self::rust_signature_has_receiver(signature) {
                Some(0)
            } else {
                let references = signature
                    .params
                    .iter()
                    .enumerate()
                    .filter(|(_, parameter)| native_display_has_reference(&parameter.type_display))
                    .map(|(index, _)| index)
                    .collect::<Vec<_>>();
                (references.len() == 1).then(|| references[0])
            };
            if let Some(source) = source {
                push_identity(&mut slots[source], "elided");
                push_identity(&mut result, "elided");
            }
        }
        slots.push(result);
        slots
    }

    /// Build a Rust function type from a signature whose displays may use `crate::` relative to `rust_path`.
    pub fn resolved_function_type_from_rust_sig_for_owner_path(
        &self,
        sig: &RustFunctionSig,
        drop_receiver: bool,
        rust_path: &str,
    ) -> ResolvedType {
        let skip = usize::from(drop_receiver && Self::rust_signature_has_receiver(sig));
        let params = sig
            .params
            .iter()
            .skip(skip)
            .map(|p| {
                CallableParam::positional(
                    self.resolved_param_type_from_rust_display_for_owner_path(p.type_display.as_str(), rust_path),
                )
            })
            .collect();
        let ret_display = self.rust_display_for_owner_path(sig.return_type.as_str(), rust_path);
        let ret = self.resolved_type_from_rust_display(ret_display.as_str());
        ResolvedType::Function(params, Box::new(ret))
    }

    /// Replace Rust metadata's `Self` placeholder with one concrete receiver type at a call site.
    pub(super) fn substitute_rust_self_type_with_resolved(ty: ResolvedType, receiver: &ResolvedType) -> ResolvedType {
        match ty {
            ResolvedType::RustPath(path) if path == "Self" => receiver.clone(),
            ResolvedType::Ref(inner) => ResolvedType::Ref(Box::new(Self::substitute_rust_self_type_with_resolved(
                *inner, receiver,
            ))),
            ResolvedType::RefMut(inner) => ResolvedType::RefMut(Box::new(
                Self::substitute_rust_self_type_with_resolved(*inner, receiver),
            )),
            ResolvedType::Generic(name, args) => ResolvedType::Generic(
                name,
                args.into_iter()
                    .map(|arg| Self::substitute_rust_self_type_with_resolved(arg, receiver))
                    .collect(),
            ),
            ResolvedType::Tuple(items) => ResolvedType::Tuple(
                items
                    .into_iter()
                    .map(|item| Self::substitute_rust_self_type_with_resolved(item, receiver))
                    .collect(),
            ),
            ResolvedType::FrozenList(inner) => ResolvedType::FrozenList(Box::new(
                Self::substitute_rust_self_type_with_resolved(*inner, receiver),
            )),
            ResolvedType::FrozenSet(inner) => ResolvedType::FrozenSet(Box::new(
                Self::substitute_rust_self_type_with_resolved(*inner, receiver),
            )),
            ResolvedType::FrozenDict(key, value) => ResolvedType::FrozenDict(
                Box::new(Self::substitute_rust_self_type_with_resolved(*key, receiver)),
                Box::new(Self::substitute_rust_self_type_with_resolved(*value, receiver)),
            ),
            ResolvedType::Function(params, ret) => ResolvedType::Function(
                params
                    .into_iter()
                    .map(|param| CallableParam {
                        ty: Self::substitute_rust_self_type_with_resolved(param.ty, receiver),
                        ..param
                    })
                    .collect(),
                Box::new(Self::substitute_rust_self_type_with_resolved(*ret, receiver)),
            ),
            other => other,
        }
    }

    /// Replace Rust metadata's `Self` placeholder with the concrete receiver path at a call site.
    pub(super) fn substitute_rust_self_type(ty: ResolvedType, rust_path: &str) -> ResolvedType {
        Self::substitute_rust_self_type_with_resolved(ty, &ResolvedType::RustPath(rust_path.to_string()))
    }

    /// Build a Rust function type and resolve `Self` return or parameter positions against `rust_path`.
    pub fn resolved_function_type_from_rust_sig_for_path(
        &self,
        sig: &RustFunctionSig,
        drop_receiver: bool,
        rust_path: &str,
    ) -> ResolvedType {
        Self::substitute_rust_self_type(
            self.resolved_function_type_from_rust_sig_for_owner_path(sig, drop_receiver, rust_path),
            rust_path,
        )
    }

    /// Render `path` with generic arguments as `path<A, B, ...>` for embedding in [`ResolvedType::RustPath`].
    ///
    /// When `args` is empty, returns `path` unchanged (no angle brackets).
    pub(super) fn render_rust_shape_path(path: &str, args: &[RustTypeShape]) -> String {
        render_rust_type_shape_path(path, args)
    }

    /// Detect whether a Rust display type starts with `&T` or `&mut T`.
    ///
    /// Returns the mutability flag plus the remaining inner type spelling so [`Self::resolved_type_from_rust_display`]
    /// can preserve borrow semantics for Rust-backed values instead of collapsing them into plain path types.
    /// Rust permits a tuple or slice delimiter immediately after `mut`; identifier continuations stay part of a name.
    pub(super) fn rust_display_borrow_kind(display: &str) -> Option<(bool, &str)> {
        let after_amp = display.trim().strip_prefix('&')?.trim_start();
        if let Some(inner) = after_amp.strip_prefix("mut")
            && inner
                .chars()
                .next()
                .is_none_or(|next| next.is_whitespace() || matches!(next, '(' | '[' | '&' | '*'))
        {
            return Some((true, inner.trim_start()));
        }
        Some((false, after_amp))
    }

    /// Remove trailing callable marker bounds such as `+ Send + Sync` from a Rust callable-bound display.
    pub(super) fn trim_top_level_rust_bound_suffix(text: &str) -> &str {
        let mut angle = 0usize;
        let mut paren = 0usize;
        let mut bracket = 0usize;
        for (idx, ch) in text.char_indices() {
            match ch {
                '<' => angle += 1,
                '>' => angle = angle.saturating_sub(1),
                '(' => paren += 1,
                ')' => paren = paren.saturating_sub(1),
                '[' => bracket += 1,
                ']' => bracket = bracket.saturating_sub(1),
                '+' if angle == 0 && paren == 0 && bracket == 0 => return text[..idx].trim(),
                _ => {}
            }
        }
        text.trim()
    }

    /// Return the body of a Rust callable bound display such as `impl FnMut(A) -> B`.
    pub(in crate::typechecker) fn rust_callable_bound_body(display: &str) -> Option<&str> {
        let trimmed = Self::trim_top_level_rust_bound_suffix(display.trim());
        for prefix in ["impl ", "dyn "] {
            if let Some(rest) = trimmed.strip_prefix(prefix)
                && matches!(rest, r if r.starts_with("Fn") || r.starts_with("std::ops::Fn") || r.starts_with("core::ops::Fn"))
            {
                return Some(rest.trim());
            }
        }
        for compact_prefix in ["impl", "dyn"] {
            if let Some(rest) = trimmed.strip_prefix(compact_prefix)
                && matches!(rest, r if r.starts_with("Fn") || r.starts_with("std::ops::Fn") || r.starts_with("core::ops::Fn"))
            {
                return Some(rest.trim());
            }
        }
        if trimmed.starts_with("Fn") || trimmed.starts_with("std::ops::Fn") || trimmed.starts_with("core::ops::Fn") {
            Some(trimmed)
        } else {
            None
        }
    }

    /// Return the byte index of the matching close paren for an opening paren at the start of `text`.
    pub(in crate::typechecker) fn rust_callable_args_end(text: &str) -> Option<usize> {
        let mut depth = 0usize;
        for (idx, ch) in text.char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return Some(idx);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// Resolve a safe Rust function pointer or callable bound display into an Incan function type.
    ///
    /// Rust generic callback parameters often surface as `impl FnMut(&mut T, &U)` or through a generic bound that is
    /// normalized to that display. Treating those as ordinary type variables lets by-value Incan function values reach
    /// rustc and fail there. This resolver preserves the callback input borrow shape so the frontend can reject
    /// incompatible named callbacks before emission.
    pub(super) fn resolved_function_type_from_rust_callable_bound_display(
        &self,
        rust_ty: &str,
    ) -> Option<ResolvedType> {
        let after_name = if let Some(pointer) = rust_ty.trim().strip_prefix("fn") {
            pointer.trim_start()
        } else {
            let body = Self::rust_callable_bound_body(rust_ty)?;
            let body = body
                .strip_prefix("std::ops::")
                .or_else(|| body.strip_prefix("core::ops::"))
                .unwrap_or(body);
            ["FnOnce", "FnMut", "Fn"]
                .into_iter()
                .find_map(|name| body.strip_prefix(name))?
                .trim_start()
        };
        if !after_name.starts_with('(') {
            return None;
        }
        let args_end = Self::rust_callable_args_end(after_name)?;
        let args_inner = &after_name[1..args_end];
        let params = Self::split_top_level_generic_args(args_inner)
            .into_iter()
            .map(|arg| CallableParam::positional(self.resolved_param_type_from_rust_display(arg)))
            .collect::<Vec<_>>();
        let after_args = after_name[args_end + 1..].trim_start();
        let ret = if let Some(ret_display) = after_args.strip_prefix("->") {
            let ret_display = Self::trim_top_level_rust_bound_suffix(ret_display);
            self.resolved_type_from_rust_display(ret_display)
        } else {
            ResolvedType::Unit
        };
        Some(ResolvedType::Function(params, Box::new(ret)))
    }

    /// Remove Rust lifetime labels that decorate borrowed display types.
    ///
    /// rust-analyzer commonly prints borrowed method parameters as `&'h str` or `&'a [u8]`. The typechecker only needs
    /// the ownership shape and payload type, so erase the lifetime after `&` before whitespace normalization turns it
    /// into an unparseable token such as `&'hstr`.
    pub(super) fn strip_borrow_lifetimes(rust_ty: &str) -> String {
        strip_rust_borrow_lifetimes(rust_ty)
    }

    /// Return Rust display text with lifetime parameters removed.
    pub(super) fn rust_display_without_lifetimes(rust_ty: &str) -> String {
        Self::strip_borrow_lifetimes(rust_ty)
            .replace("'static ", "")
            .replace("<>", "")
            .trim_start_matches("::")
            .to_string()
    }

    /// Return compact Rust display text for comparison. Bring a Rust type display into the one normal form every
    /// lookup and comparison in the checker uses.
    ///
    /// Lifetimes and formatting whitespace are removed while preserving keyword separation around `as` and after `dyn`,
    /// `impl` or `mut`. These spaces keep qualified associated types, trait objects and exclusive borrows parseable.
    /// Every route that compacts a display must use this normalization; different parameter and argument normalization
    /// previously made `Arc<dyn TableProvider>` fail to match itself (#1229).
    pub(super) fn compact_rust_display(rust_ty: &str) -> String {
        let display = Self::rust_display_without_lifetimes(rust_ty);
        let display = display.as_str();
        let mut out = String::with_capacity(display.len());
        let mut word = String::new();
        let flush = |word: &mut String, out: &mut String| {
            if !word.is_empty() {
                if word == "as" && !out.ends_with('#') {
                    out.push(' ');
                }
                out.push_str(word);
                word.clear();
            }
        };
        let mut chars = display.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == ' ' {
                let keep = matches!(word.as_str(), "dyn" | "impl" | "mut" | "as")
                    && chars
                        .peek()
                        .is_some_and(|next| next.is_alphanumeric() || *next == '_' || *next == ':');
                flush(&mut word, &mut out);
                if keep {
                    out.push(' ');
                }
                continue;
            }
            if ch.is_alphanumeric() || ch == '_' {
                word.push(ch);
            } else {
                flush(&mut word, &mut out);
                out.push(ch);
            }
        }
        flush(&mut word, &mut out);
        out
    }

    /// Expand a Rust type alias applied to `args` into its target display, or `None` when `base` names no alias.
    ///
    /// The alias item records its target with the alias's own parameters; substituting the written arguments gives the
    /// aliased type exactly, including arms the alias spelling omits. Shipped caches and check-time extraction expose
    /// the same alias contract; a dependency without a shipped cache must retain its target too.
    pub(super) fn rust_alias_expansion(&self, base: &str, args: &[&str]) -> Option<String> {
        if !base.contains("::") {
            return None;
        }
        let lookup_path = Self::rust_metadata_lookup_path(base)?;
        let alias = self.known_rust_type_alias_for_path(lookup_path)?;
        self.record_rust_metadata_query(base);
        let alias = alias.into_alias()?;
        if alias.type_params.len() != args.len() {
            return None;
        }
        Some(Self::substitute_rust_display_params(
            &alias.target,
            &alias.type_params,
            args,
        ))
    }

    /// Replace each binder of `display` that names one of `params` with the matching argument display, through
    /// [`TypeChecker::substitute_rust_binders`].
    pub(super) fn substitute_rust_display_params(display: &str, params: &[String], args: &[&str]) -> String {
        let binder = |word: &str| {
            params
                .iter()
                .position(|param| param == word)
                .and_then(|index| args.get(index).copied())
        };
        Self::substitute_rust_binders(display, binder, None)
    }

    /// The one binder-substitution tokenizer for Rust type displays.
    ///
    /// Each identifier that `binder` maps is replaced by its display, unless `::` or `'` precedes it: a path segment
    /// (`a::T`) or a lifetime (`'a`) is not a binder, while the owner of a projection (`T::Item`) is. `Self` becomes
    /// `receiver` when the caller passes one.
    pub(in crate::typechecker) fn substitute_rust_binders<'a>(
        display: &str,
        binder: impl Fn(&str) -> Option<&'a str>,
        receiver: Option<&'a str>,
    ) -> String {
        let is_word = |ch: char| ch.is_alphanumeric() || ch == '_';
        let mut out = String::with_capacity(display.len());
        let mut rest = display;
        while let Some(start) = rest.find(is_word) {
            out.push_str(&rest[..start]);
            let tail = &rest[start..];
            let end = tail.find(|ch: char| !is_word(ch)).unwrap_or(tail.len());
            let word = &tail[..end];
            let replacement = match (receiver, word) {
                _ if out.ends_with("::") || out.ends_with('\'') => None,
                (Some(receiver), "Self") => Some(receiver),
                _ => binder(word),
            };
            out.push_str(replacement.unwrap_or(word));
            rest = &tail[end..];
        }
        out.push_str(rest);
        out
    }

    /// Split a Rust generic display type into base and arguments.
    pub(super) fn rust_generic_base_and_args(normalized: &str) -> Option<(&str, Vec<&str>)> {
        let start = normalized.find('<')?;
        if !normalized.ends_with('>') {
            return None;
        }
        let base = normalized[..start].trim();
        let inner = &normalized[start + 1..normalized.len() - 1];
        Some((base, Self::split_top_level_generic_args(inner)))
    }

    /// Classify collection vocabulary only when a qualified storage owner has a canonical native collection identity.
    /// A foreign ADT named `Vec` remains nominal; its spelling grants neither list storage nor list method lookup.
    pub(super) fn rust_collection_id_from_display_base(base: &str) -> Option<CollectionTypeId> {
        let collection = incan_lang::lang::types::collections::from_rust_display_base(base)?;
        let native_owner = match collection {
            CollectionTypeId::List => {
                incan_lang::interop::ancestral_rust_path(base) == incan_lang::interop::ancestral_rust_path("Vec")
            }
            CollectionTypeId::Dict | CollectionTypeId::Set => {
                incan_lang::interop::RustCollectionFamily::for_canonical_path(base).is_some()
            }
            _ => true,
        };
        (!base.contains("::") || native_owner).then_some(collection)
    }

    /// Return the resolved Rust display type for a structural parameter.
    pub(super) fn resolved_structural_rust_param_display<F>(
        &self,
        normalized: &str,
        mut resolve_arg: F,
    ) -> Option<ResolvedType>
    where
        F: FnMut(&Self, &str) -> ResolvedType,
    {
        let (base, arg_displays) = Self::rust_generic_base_and_args(normalized)?;
        let collection_id = Self::rust_collection_id_from_display_base(base)?;
        let args = arg_displays
            .into_iter()
            .map(|arg| resolve_arg(self, arg))
            .collect::<Vec<_>>();
        Self::resolved_structural_rust_arguments(collection_id, args).ok()
    }

    /// Construct the source view of a canonical native collection from already resolved arguments. A rejected arity
    /// returns the original arguments so native inference can retain its nominal owner without copying or rendering
    /// checked source identities back through Rust display text.
    pub(super) fn resolved_structural_rust_arguments(
        collection_id: CollectionTypeId,
        mut args: Vec<ResolvedType>,
    ) -> Result<ResolvedType, Vec<ResolvedType>> {
        match collection_id {
            CollectionTypeId::List if args.len() == 1 => Ok(ResolvedType::Generic(
                collection_name(CollectionTypeId::List).to_string(),
                args,
            )),
            CollectionTypeId::Dict if args.len() == 2 => Ok(ResolvedType::Generic(
                collection_name(CollectionTypeId::Dict).to_string(),
                args,
            )),
            CollectionTypeId::Set if args.len() == 1 => Ok(ResolvedType::Generic(
                collection_name(CollectionTypeId::Set).to_string(),
                args,
            )),
            CollectionTypeId::Option if args.len() == 1 => Ok(ResolvedType::Generic(
                collection_name(CollectionTypeId::Option).to_string(),
                args,
            )),
            CollectionTypeId::Result if args.len() <= 2 => {
                let ok = args.first().cloned().unwrap_or(ResolvedType::Unknown);
                let err = args.get(1).cloned().unwrap_or(ResolvedType::Unknown);
                Ok(ResolvedType::Generic(
                    collection_name(CollectionTypeId::Result).to_string(),
                    vec![ok, err],
                ))
            }
            CollectionTypeId::Tuple => Ok(ResolvedType::Tuple(args)),
            CollectionTypeId::FrozenList if args.len() == 1 => Ok(ResolvedType::FrozenList(Box::new(args.remove(0)))),
            CollectionTypeId::FrozenSet if args.len() == 1 => Ok(ResolvedType::FrozenSet(Box::new(args.remove(0)))),
            CollectionTypeId::FrozenDict if args.len() == 2 => {
                let value = args.pop().unwrap_or(ResolvedType::Unknown);
                let key = args.pop().unwrap_or(ResolvedType::Unknown);
                Ok(ResolvedType::FrozenDict(Box::new(key), Box::new(value)))
            }
            CollectionTypeId::Generator if args.len() == 1 => Ok(ResolvedType::Generic(
                collection_name(CollectionTypeId::Generator).to_string(),
                args,
            )),
            _ => Err(args),
        }
    }

    /// Map structured Rust metadata [`RustTypeShape`] into a [`ResolvedType`] for field access and pattern typing.
    ///
    /// `Option`/`Result` become [`ResolvedType::Generic`] with constructor names `Option` and `Result`. Concrete paths
    /// are rendered through the ordinary Rust-display resolver so a structured metadata path receives the same
    /// primitive classification as its textual fallback (notably owned Rust strings).
    pub fn resolved_type_from_rust_shape(&self, shape: &RustTypeShape) -> ResolvedType {
        match shape {
            RustTypeShape::Never => ResolvedType::Never,
            RustTypeShape::Bool => ResolvedType::Bool,
            RustTypeShape::Float => ResolvedType::Float,
            RustTypeShape::Int => ResolvedType::Int,
            RustTypeShape::Str => ResolvedType::Str,
            RustTypeShape::Bytes => ResolvedType::Bytes,
            RustTypeShape::Unit => ResolvedType::Unit,
            RustTypeShape::Option(inner) => {
                ResolvedType::Generic("Option".to_string(), vec![self.resolved_type_from_rust_shape(inner)])
            }
            RustTypeShape::Result(ok, err) => ResolvedType::Generic(
                "Result".to_string(),
                vec![
                    self.resolved_type_from_rust_shape(ok),
                    self.resolved_type_from_rust_shape(err),
                ],
            ),
            RustTypeShape::Tuple(items) => ResolvedType::Tuple(
                items
                    .iter()
                    .map(|item| self.resolved_type_from_rust_shape(item))
                    .collect(),
            ),
            RustTypeShape::Ref(inner) => ResolvedType::Ref(Box::new(self.resolved_type_from_rust_shape(inner))),
            RustTypeShape::RustPath { path, args } => {
                // `String` has an implementation allocator argument in some rustc metadata views. That argument
                // does not change the language-level string value contract, so retain `str` semantics from the
                // canonical nominal path instead of rendering it as an opaque generic Rust display.
                if rust_display_is_owned_string(path) {
                    return ResolvedType::Str;
                }
                self.resolved_type_from_rust_display(Self::render_rust_shape_path(path, args).as_str())
            }
            RustTypeShape::TypeParam(name) => ResolvedType::TypeVar(name.clone()),
            RustTypeShape::Unknown => ResolvedType::Unknown,
        }
    }

    /// Return checked Rust metadata for one enum variant constructor's callable parameters.
    ///
    /// Rust enum variants are callable constructors at the source surface, but they are not ordinary inherent
    /// functions. Named fields retain their metadata names and declaration order for ordinary argument binding;
    /// tuple payloads remain positional. The same payload contract directs downstream ownership planning.
    pub fn rust_variant_callable_params(&self, rust_path: &str, variant: &str) -> Option<Vec<CallableParam>> {
        let metadata = self.rust_item_metadata_for_path(rust_path)?;
        let RustItemKind::Type(info) = &metadata.kind else {
            return None;
        };
        let variant = info.variants.iter().find(|candidate| candidate.name == variant)?;
        Some(
            variant
                .fields
                .iter()
                .enumerate()
                .map(|(index, field)| {
                    let ty = self.resolved_type_from_rust_shape(field);
                    match variant.field_names.get(index) {
                        Some(name) => CallableParam::named(name, ty, ParamKind::Normal),
                        None => CallableParam::positional(ty),
                    }
                })
                .collect(),
        )
    }

    /// Return the storage carrier of each payload field of one Rust enum variant from its metadata.
    ///
    /// The callable parameters above are the semantic payload types; this is the parallel fact lowering needs to
    /// emit the Rust constructor, such as the `Box<T>` prost writes around a recursive `oneof` payload.
    pub fn rust_variant_payload_carriers(&self, rust_path: &str, variant: &str) -> Option<Vec<RustPayloadCarrier>> {
        let metadata = self.rust_item_metadata_for_path(rust_path)?;
        let RustItemKind::Type(info) = &metadata.kind else {
            return None;
        };
        let variant = info.variants.iter().find(|candidate| candidate.name == variant)?;
        Some(
            (0..variant.fields.len())
                .map(|index| variant.field_carrier(index))
                .collect(),
        )
    }

    /// Resolve a Rust-origin method signature from cached metadata.
    pub fn rust_method_signature(&self, rust_path: &str, method: &str) -> Option<RustFunctionSig> {
        let metadata = self.rust_item_metadata_for_path(rust_path)?;
        if let RustItemKind::Type(info) = &metadata.kind {
            return info
                .methods
                .iter()
                .find(|m| m.name == method)
                .map(|m| m.signature.clone());
        }
        None
    }

    /// Resolve a Rust-origin associated function signature (must not take `self`) from cached metadata.
    pub fn rust_associated_function_signature(&self, rust_path: &str, method: &str) -> Option<RustFunctionSig> {
        let sig = self.rust_method_signature(rust_path, method)?;
        if Self::rust_signature_has_receiver(&sig) {
            return None;
        }
        Some(sig)
    }

    /// Convert a Rust display type string into a conservative [`ResolvedType`].
    ///
    /// RFC 041: intentionally best-effort. Rust's diverging `!` return is preserved as [`ResolvedType::Never`]. Common
    /// primitive spellings and `Option` / `Result` wrappers are recognized, including namespaced aliases whose
    /// trailing segment is `Option` or `Result`. Nested generics, lifetimes, and crate paths otherwise become
    /// [`ResolvedType::RustPath`] (or [`ResolvedType::Unknown`] when empty); lowering relies on rustc for fidelity.
    /// Native scalar names precede source import lookup, and metadata-selected transparent aliases retain their
    /// target carrier rather than inventing a nominal layout for a public primitive path.
    ///
    /// ## `Result<T, E>` parsing
    ///
    /// `Result<…>` uses top-level generic splitting, so nested generic or tuple commas stay inside the appropriate
    /// argument. Prefer precise typing from Incan surfaces over relying on this heuristic for arbitrary Rust paths.
    pub fn resolved_type_from_rust_display(&self, rust_ty: &str) -> ResolvedType {
        let trimmed = rust_ty.trim();
        let display = Self::rust_display_without_lifetimes(trimmed);
        let normalized = Self::compact_rust_display(display.as_str());
        if normalized == RUST_NEVER_TYPE_DISPLAY {
            return ResolvedType::Never;
        }
        if let Some(source) = self.resolved_source_from_rust_carrier(&normalized) {
            return source;
        }
        // Native scalar names are metadata vocabulary, independent of same-spelled source imports.
        if let Some(id) = numerics::from_str(&normalized)
            && numerics::as_str(id) == normalized
        {
            return match id {
                NumericTypeId::I64 => ResolvedType::Int,
                NumericTypeId::F64 => ResolvedType::Float,
                NumericTypeId::Bool => ResolvedType::Bool,
                id => ResolvedType::Numeric(id),
            };
        }
        // Public primitive re-exports are transparent aliases. A nominal alias keeps its public spelling: its expansion
        // can include default arguments that no public path names.
        if !normalized.contains('<')
            && let Some(expanded) = self.rust_alias_expansion(&normalized, &[])
            && expanded != normalized
            && expanded.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            return self.resolved_type_from_rust_display(&expanded);
        }
        if let Some(callable) = self.resolved_function_type_from_rust_callable_bound_display(display.as_str()) {
            return callable;
        }
        if let Some(Symbol {
            kind: SymbolKind::RustItem(info),
            ..
        }) = self.lookup_symbol(normalized.as_str())
            && !matches!(info.binding, RustImportBindingKind::CrateRoot)
        {
            return ResolvedType::RustPath(info.path.clone());
        }
        self.resolved_nested_type_from_rust_display(display, normalized)
    }

    /// Recover an admitted source declaration when Rust returns its native carrier. Published layouts are indexed
    /// by canonical identity even when the consumer imports only a function; no lexical type binding is introduced.
    pub(super) fn resolved_source_from_rust_carrier(&self, normalized: &str) -> Option<ResolvedType> {
        if !normalized.starts_with(incan_semantics_core::body_ir::NOMINAL_CARRIER_KEY_PREFIX) {
            return None;
        }
        if let Some((_, name, _)) = self.symbols.active_module_source_bindings().find(|(_, name, _)| {
            self.symbols
                .module_type_identity(name)
                .and_then(incan_semantics_core::body_ir::nominal_carrier_key)
                .as_deref()
                == Some(normalized)
        }) {
            return Some(ResolvedType::Named(name.to_owned()));
        }
        self.transitive_pub_types
            .contains_key(normalized)
            .then(|| ResolvedType::Named(normalized.to_owned()))
    }

    /// Resolve borrowed, structural and nominal Rust spellings after scalar and callable forms have been admitted.
    /// Nested arguments reenter the same conversion so retained source carriers keep their identity.
    pub(super) fn resolved_nested_type_from_rust_display(&self, display: String, normalized: String) -> ResolvedType {
        match normalized.as_str() {
            "&str" => return ResolvedType::Str,
            "&[u8]" => return ResolvedType::Bytes,
            _ => {}
        }
        if let Some((is_mut, inner)) = Self::rust_display_borrow_kind(display.as_str()) {
            let inner_ty = self.resolved_type_from_rust_display(inner);
            return if is_mut {
                ResolvedType::RefMut(Box::new(inner_ty))
            } else {
                ResolvedType::Ref(Box::new(inner_ty))
            };
        }
        if rust_display_is_owned_string(normalized.as_str()) {
            return ResolvedType::Str;
        }
        if let Some(inner) = display.strip_prefix('(').and_then(|inner| inner.strip_suffix(')'))
            && inner.contains(',')
        {
            return ResolvedType::Tuple(
                Self::split_top_level_generic_args(inner)
                    .into_iter()
                    .map(|element| self.resolved_type_from_rust_display(element))
                    .collect(),
            );
        }
        match normalized.as_str() {
            "{unknown}" => ResolvedType::Unknown,
            "str" | "&str" => ResolvedType::Str,
            "Vec<u8>" | "std::vec::Vec<u8>" | "alloc::vec::Vec<u8>" | "&[u8]" => ResolvedType::Bytes,
            "()" => ResolvedType::Unit,
            _ if normalized.ends_with('>') => self.resolved_parameterized_rust_display(display, normalized),
            _ if !normalized.is_empty() => {
                if self.lookup_type_info(normalized.as_str()).is_some() {
                    ResolvedType::Named(normalized)
                } else if let Some(path) = self.unique_imported_rust_item_path_for_display_tail(normalized.as_str()) {
                    ResolvedType::RustPath(path)
                } else {
                    ResolvedType::RustPath(normalized)
                }
            }
            _ => ResolvedType::Unknown,
        }
    }

    /// Resolve parameterized Rust displays using wrapper aliases, structural facts and source nominal declarations.
    ///
    /// Consume the already normalized strings so the opaque-path fallback keeps the original display without another
    /// copy.
    pub(super) fn resolved_parameterized_rust_display(&self, display: String, normalized: String) -> ResolvedType {
        if let Some((base, args)) = Self::rust_generic_base_and_args(display.as_str()) {
            let tail = base.rsplit("::").next().unwrap_or(base);
            match collection_type_id(tail) {
                Some(CollectionTypeId::Option) => {
                    let inner = args.first().copied().unwrap_or("");
                    return ResolvedType::Generic(
                        collection_name(CollectionTypeId::Option).to_string(),
                        vec![self.resolved_type_from_rust_display(inner)],
                    );
                }
                Some(CollectionTypeId::Result) => {
                    // A `Result` alias (`std::thread::Result<T>`, `std::io::Result<T>`) fixes the error arm its
                    // display omits; the alias item's recorded target names it.
                    if args.len() != 2
                        && let Some(expanded) = self.rust_alias_expansion(base, &args)
                    {
                        return self.resolved_type_from_rust_display(expanded.as_str());
                    }
                    let ok_ty = args
                        .first()
                        .map(|p| self.resolved_type_from_rust_display(p))
                        .unwrap_or(ResolvedType::Unknown);
                    // Without alias metadata (`datafusion_common::error::Result<T>` outside the inspected
                    // graph) keep the success path semantic and degrade only the
                    // missing error arm.
                    let err_ty = args
                        .get(1)
                        .map(|p| self.resolved_type_from_rust_display(p))
                        .unwrap_or(ResolvedType::Unknown);
                    return ResolvedType::Generic(
                        collection_name(CollectionTypeId::Result).to_string(),
                        vec![ok_ty, err_ty],
                    );
                }
                _ => {}
            }
            if let Some(structural) = self.resolved_structural_rust_param_display(display.as_str(), |checker, arg| {
                checker.resolved_type_from_rust_display(arg)
            }) {
                return structural;
            }
            if let Some(source) = self.resolved_source_generic_from_rust_display(base, &args) {
                return source;
            }
        }
        if self.lookup_type_info(normalized.as_str()).is_some() {
            ResolvedType::Named(normalized)
        } else {
            ResolvedType::RustPath(display)
        }
    }

    /// Name a source model, class, enum or newtype instance that a Rust display spells with angle brackets
    /// (`Node<T>`), so the checked type keeps the declaration instead of an opaque Rust path. An argument that names a
    /// type parameter of the enclosing declaration stays that parameter.
    pub(super) fn resolved_source_generic_from_rust_display(&self, base: &str, args: &[&str]) -> Option<ResolvedType> {
        if !matches!(
            self.lookup_semantic_type_info(base),
            Some(TypeInfo::Model(_) | TypeInfo::Class(_) | TypeInfo::Enum(_) | TypeInfo::Newtype(_))
        ) {
            return None;
        }
        let arguments = args
            .iter()
            .map(|argument| {
                let parameter = ResolvedType::Named(argument.trim().to_owned());
                if self.active_type_param_name(&parameter).is_some() {
                    parameter
                } else {
                    self.resolved_type_from_rust_display(argument)
                }
            })
            .collect();
        Some(ResolvedType::Generic(base.to_owned(), arguments))
    }

    /// Return a Rust generic parameter display when rust-analyzer reports a by-value generic boundary.
    ///
    /// Plain type parameters appear as `T` or `U`. Anonymous `impl Trait` parameters arrive as `impl Buf` in the
    /// checker's normal form, or with the whitespace erased (`implBuf`) when a caller compacted the display itself;
    /// both still carry by-value shape and must not be treated as borrowed Rust boundary targets.
    pub fn rust_display_type_var_name(normalized: &str) -> Option<&str> {
        if let Some(tail) = normalized.strip_prefix("impl").map(str::trim_start)
            && !tail.is_empty()
            && (tail.contains("::") || tail.chars().next().is_some_and(|ch| ch.is_ascii_uppercase()))
        {
            return Some(normalized);
        }
        if normalized.len() == 1 && normalized.chars().next().is_some_and(|ch| ch.is_ascii_uppercase()) {
            Some(normalized)
        } else {
            None
        }
    }

    /// Convert a Rust parameter display type into a [`ResolvedType`] while preserving borrow shape.
    ///
    /// `resolved_type_from_rust_display()` intentionally maps borrowed scalar-like returns such as `&str` and `&[u8]`
    /// into owned Incan value types. Parameters need the opposite treatment: the callable signature must remember the
    /// borrowed Rust boundary so emission can pass `&arg` instead of moving an owned `String` or `Vec<u8>`.
    pub fn resolved_param_type_from_rust_display(&self, rust_ty: &str) -> ResolvedType {
        let trimmed = rust_ty.trim();
        let display = Self::rust_display_without_lifetimes(trimmed);
        let normalized = Self::compact_rust_display(&display);
        if let Some(callable) = self.resolved_function_type_from_rust_callable_bound_display(display.as_str()) {
            return callable;
        }
        if let Some(name) = Self::rust_display_type_var_name(normalized.as_str()) {
            return ResolvedType::TypeVar(name.to_string());
        }
        if let Some((is_mut, inner)) = Self::rust_display_borrow_kind(display.as_str()) {
            let inner_normalized = Self::compact_rust_display(inner);
            let inner_ty = match inner {
                "str" => ResolvedType::Str,
                "[u8]" => ResolvedType::Bytes,
                _ if inner_normalized.starts_with('[') && inner_normalized.ends_with(']') => {
                    let elem = &inner_normalized[1..inner_normalized.len() - 1];
                    ResolvedType::Generic(
                        collection_name(CollectionTypeId::List).to_string(),
                        vec![self.resolved_type_from_rust_display(elem)],
                    )
                }
                _ => self.resolved_type_from_rust_display(inner_normalized.as_str()),
            };
            return if is_mut {
                ResolvedType::RefMut(Box::new(inner_ty))
            } else {
                ResolvedType::Ref(Box::new(inner_ty))
            };
        }
        if let Some(structural) = self.resolved_structural_rust_param_display(display.as_str(), |checker, arg| {
            checker.resolved_param_type_from_rust_display(arg)
        }) {
            return structural;
        }
        self.resolved_type_from_rust_display(display.as_str())
    }

    /// Convert a Rust parameter display into a resolved type after expanding crate-relative paths for its owner.
    pub fn resolved_param_type_from_rust_display_for_owner_path(
        &self,
        rust_ty: &str,
        owner_path: &str,
    ) -> ResolvedType {
        let display = self.rust_display_for_owner_path(rust_ty, owner_path);
        self.resolved_param_type_from_rust_display(display.as_str())
    }

    /// Convert a Rust parameter display type into the typed target carried by Rust-boundary coercion metadata.
    ///
    /// This preserves the semantic difference between slice borrow targets such as `&str`/`&[u8]` and borrowed owned
    /// Rust targets such as `&String`/`&Vec<u8>`. Ordinary parameter typing still maps Rust scalar displays onto Incan
    /// value types, but coercion metadata is a backend contract: lowering and emission must be able to choose borrow
    /// versus materialize-then-borrow behavior from this typed target without re-decoding Rust display strings.
    pub fn resolved_rust_boundary_target_from_param_display(&self, rust_ty: &str) -> ResolvedType {
        let trimmed = rust_ty.trim();
        let display = Self::rust_display_without_lifetimes(trimmed);
        if let Some(callable) = self.resolved_function_type_from_rust_callable_bound_display(display.as_str()) {
            return callable;
        }
        if let Some((is_mut, inner)) = Self::rust_display_borrow_kind(display.as_str()) {
            let inner_ty = match inner {
                "str" => ResolvedType::Str,
                "[u8]" => ResolvedType::Bytes,
                "String" | "std::string::String" | "alloc::string::String" => ResolvedType::RustPath(inner.to_string()),
                "Vec<u8>" | "std::vec::Vec<u8>" | "alloc::vec::Vec<u8>" => ResolvedType::RustPath(inner.to_string()),
                _ => self.resolved_type_from_rust_display(inner),
            };
            return if is_mut {
                ResolvedType::RefMut(Box::new(inner_ty))
            } else {
                ResolvedType::Ref(Box::new(inner_ty))
            };
        }
        if let Some(structural) = self.resolved_structural_rust_param_display(display.as_str(), |checker, arg| {
            checker.resolved_rust_boundary_target_from_param_display(arg)
        }) {
            return structural;
        }
        self.resolved_param_type_from_rust_display(display.as_str())
    }

    /// Convert a Rust boundary target after expanding crate-relative paths for its owner.
    pub fn resolved_rust_boundary_target_from_param_display_for_owner_path(
        &self,
        rust_ty: &str,
        owner_path: &str,
    ) -> ResolvedType {
        let display = self.rust_display_for_owner_path(rust_ty, owner_path);
        self.resolved_rust_boundary_target_from_param_display(display.as_str())
    }
}

/// Collect named lifetime and type-parameter identities from one portable Rust display. Static lifetimes deliberately
/// contribute no identity; an anonymous lifetime is retained as the elision marker.
fn native_signature_tokens(display: &str, type_parameters: &[String]) -> Vec<String> {
    let characters = display.chars().collect::<Vec<_>>();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < characters.len() {
        if characters[index] == '\'' {
            let start = index + 1;
            index = start;
            while index < characters.len() && (characters[index].is_alphanumeric() || characters[index] == '_') {
                index += 1;
            }
            let name = characters[start..index].iter().collect::<String>();
            match name.as_str() {
                "" | "static" => {}
                "_" => push_identity(&mut tokens, "elided"),
                _ => push_identity(&mut tokens, &format!("lifetime:{name}")),
            }
            continue;
        }
        if characters[index].is_alphabetic() || characters[index] == '_' {
            let start = index;
            index += 1;
            while index < characters.len() && (characters[index].is_alphanumeric() || characters[index] == '_') {
                index += 1;
            }
            let name = characters[start..index].iter().collect::<String>();
            if name == "Self" || type_parameters.contains(&name) {
                push_identity(&mut tokens, &format!("type:{name}"));
            }
            continue;
        }
        index += 1;
    }
    tokens
}

/// Whether an output display contains an anonymous caller-bounded loan after excluding explicit named and static
/// lifetime contracts. Bare references are precisely the Rust lifetime-elision surface.
fn native_display_has_elided_loan(display: &str, tokens: &[String]) -> bool {
    tokens.iter().any(|token| token == "elided")
        || (native_display_has_reference(display)
            && !display.contains("'static")
            && !tokens.iter().any(|token| token.starts_with("lifetime:")))
}

/// Whether a Rust display structurally names a reference. Callable displays are already selected and validated by
/// the native boundary; this scan only decides the language-defined elision source.
fn native_display_has_reference(display: &str) -> bool {
    display.chars().any(|character| character == '&')
}

/// Append one signature-local identity once, preserving its first structural occurrence for deterministic facts.
fn push_identity(tokens: &mut Vec<String>, identity: &str) {
    if !tokens.iter().any(|token| token == identity) {
        tokens.push(identity.to_owned());
    }
}
