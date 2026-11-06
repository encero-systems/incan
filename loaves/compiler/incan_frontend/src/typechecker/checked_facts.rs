//! Recorded expression types, value destinations, exported bindings, and source targets.

use super::*;

impl TypeChecker {
    /// Record an expression's resolved type and the identities of its completed native storage. Native defaults
    /// must contribute payload identities at the same structural positions that storage publication retains.
    pub fn record_expr_type(&mut self, span: Span, ty: ResolvedType) {
        let mut identities = std::collections::BTreeMap::new();
        let storage = self.rust_storage_resolved(&ty, &self.native_call_type_parameters());
        self.collect_expression_type_identities(&storage, &mut Vec::new(), &mut identities);
        if identities.is_empty() {
            self.type_info.expressions.expression_type_identities.remove(&span);
        } else {
            self.type_info
                .expressions
                .expression_type_identities
                .insert(span, identities);
        }
        self.refuse_unbounded_nominal_expression_type(&ty, span);
        self.type_info.expressions.expr_types.insert(span, ty);
    }

    /// Project named leaves through their checked module, provider or native binding. Structured native displays
    /// recurse through the same type conversion as signatures, so Result payload identities survive native values. A
    /// `rusttype` value retains its inspected native item identity; its source declaration identity does not authorize
    /// a distinct nominal payload.
    pub(super) fn collect_expression_type_identities(
        &self,
        ty: &ResolvedType,
        path: &mut Vec<usize>,
        identities: &mut std::collections::BTreeMap<Vec<usize>, CanonicalSymbolId>,
    ) {
        match ty {
            ResolvedType::RustPath(display) => self.collect_rust_expression_type_identities(display, path, identities),
            ResolvedType::Named(name) => {
                if matches!(self.lookup_semantic_type_info(name), Some(TypeInfo::Newtype(info)) if info.is_rusttype)
                    && let Some(native) = self.rust_canonical_path_for_receiver_type(ty)
                {
                    let public = self.type_info.rust.item_public_paths.get(&native).unwrap_or(&native);
                    if let Some(identity) = incan_semantics_core::body_ir::rust_item_identity(public) {
                        identities.insert(path.clone(), identity);
                    }
                    return;
                }
                // Resolved nominal types keep their accepted module/provider binding even when a local value
                // shadows the source alias. Provider-only return types may have no lexical symbol at all.
                let identity = match self.public_library_type_identities.get(name) {
                    Some(binding) => binding.canonical.clone(),
                    None => self
                        .symbols
                        .module_type_identity(name)
                        .or_else(|| self.type_info.declarations.named_type_identities.get(name))
                        .cloned(),
                };
                if let Some(identity) = identity {
                    identities.insert(path.clone(), identity);
                } else if incan_semantics_core::body_ir::rust_item_identity(name).is_some() {
                    self.collect_native_expression_leaf_identity(name, path, identities);
                }
            }
            ResolvedType::Generic(_, args) | ResolvedType::Tuple(args) => {
                for (index, ty) in args.iter().enumerate() {
                    path.push(index);
                    self.collect_expression_type_identities(ty, path, identities);
                    path.pop();
                }
            }
            ResolvedType::Ref(ty)
            | ResolvedType::RefMut(ty)
            | ResolvedType::TypeToken(ty)
            | ResolvedType::FrozenList(ty)
            | ResolvedType::FrozenSet(ty) => {
                path.push(0);
                self.collect_expression_type_identities(ty, path, identities);
                path.pop();
            }
            ResolvedType::FrozenDict(key, value) => {
                for (index, ty) in [key, value].into_iter().enumerate() {
                    path.push(index);
                    self.collect_expression_type_identities(ty, path, identities);
                    path.pop();
                }
            }
            // Function signatures and type variables require their own binder environment; this expression-value
            // projection does not invent one after checking. Concrete source annotations retain their own facts.
            _ => {}
        }
    }

    /// Retain native leaf identities at checking, using the same signature type walk as native call carriers.
    /// Opaque generic owners contribute their argument leaves; an exact native item query authorizes a nominal
    /// leaf. Unsupported displays confer no identity, and registered runtime builtins keep their existing authority.
    pub(super) fn collect_rust_expression_type_identities(
        &self,
        display: &str,
        path: &mut Vec<usize>,
        identities: &mut std::collections::BTreeMap<Vec<usize>, CanonicalSymbolId>,
    ) {
        let resolved = self.native_rust_signature_type(display, "");
        if resolved != ResolvedType::RustPath(display.to_owned()) {
            self.collect_expression_type_identities(&resolved, path, identities);
            return;
        }
        if let Some((base, arguments)) = Self::rust_generic_base_and_args(display) {
            for (index, argument) in arguments
                .into_iter()
                .filter(|argument| !Self::rust_generic_arg_is_lifetime(argument))
                .enumerate()
            {
                path.push(index);
                self.collect_expression_type_identities(
                    &self.native_rust_signature_type(argument, base),
                    path,
                    identities,
                );
                path.pop();
            }
            return;
        }
        self.collect_native_expression_leaf_identity(display, path, identities);
    }

    /// Retain a completed native leaf only through its registered runtime owner or inspected Rust declaration.
    /// Both structural default arguments and diagnostic native paths use this single identity admission rule.
    pub(super) fn collect_native_expression_leaf_identity(
        &self,
        display: &str,
        path: &[usize],
        identities: &mut std::collections::BTreeMap<Vec<usize>, CanonicalSymbolId>,
    ) {
        if let Some(builtin) = incan_semantics_core::RuntimeBuiltinType::from_carrier_path(display) {
            let identity = self
                .type_info
                .body_ir_runtime_types
                .values()
                .find(|identity| {
                    incan_semantics_core::RuntimeBuiltinType::from_identity(identity).as_ref() == Some(&builtin)
                })
                .cloned()
                .or_else(|| incan_semantics_core::body_ir::rust_item_identity(display));
            if let Some(identity) = identity {
                identities.insert(path.to_vec(), identity);
            }
            return;
        }
        let Some(metadata) = self.rust_item_metadata_for_path(display) else {
            return;
        };
        if matches!(metadata.kind, RustItemKind::Type(_))
            && let Some(identity) = incan_semantics_core::body_ir::rust_item_identity(&metadata.canonical_path)
        {
            identities.insert(path.to_vec(), identity);
        }
    }

    /// Record `destination` as the type the value at `value_span` is written to, when the value's type `value_ty` is
    /// compatible with it and the destination is an `Option` (after alias expansion) or the value is numeric (#1858,
    /// RFC 009).
    ///
    /// The write sites that record this (field and index assignment, a model or class constructor field, `return`,
    /// `yield`, and a `match` arm or `break` value of the construct's type) accept a value of the `Option`'s payload
    /// type, and a numeric value whose type losslessly widens to the numeric type or union member the destination
    /// holds. Lowering wraps that value in the `Some` layers the destination adds and widens it. A value already of an
    /// `Option` destination's type is recorded too; lowering leaves it as it is.
    pub(in crate::typechecker) fn record_value_destination_if_compatible(
        &mut self,
        value_span: Span,
        value_ty: &ResolvedType,
        destination: &ResolvedType,
    ) {
        let destination = self.expand_type_aliases(destination.clone());
        let materializes = !matches!(destination, ResolvedType::Ref(_) | ResolvedType::RefMut(_))
            && self.borrowed_source_value_materializes(value_ty);
        let adapts = destination.is_option()
            || (numeric_type_id_for_compat(value_ty).is_some() && *value_ty != destination)
            || materializes;
        if !adapts || !self.types_compatible(value_ty, &destination) {
            return;
        }
        self.type_info
            .expressions
            .value_destination_types
            .insert(value_span, destination);
    }

    /// Whether `borrowed` is a shared reference to a source model, or to a class that derives `Clone`: the language
    /// copies such values on demand, so a write site or call slot that needs an owned value takes a clone of it. A
    /// type imported from Rust keeps its own copy and clone semantics and is never materialized here.
    pub(in crate::typechecker) fn borrowed_source_value_materializes(&self, borrowed: &ResolvedType) -> bool {
        let ResolvedType::Ref(inner) = borrowed else {
            return false;
        };
        let (ResolvedType::Named(name) | ResolvedType::Generic(name, _)) = inner.as_ref() else {
            return false;
        };
        if self.rust_canonical_path_for_nominal_receiver(name, None).is_some() {
            return false;
        }
        match self.lookup_semantic_type_info(name) {
            Some(TypeInfo::Model(_)) => true,
            Some(TypeInfo::Class(_)) => self.type_implements_trait(name, builtin_traits::as_str(TraitId::Clone)),
            _ => false,
        }
    }

    /// Record the final checked type selected for one assignment binding.
    pub fn record_assignment_binding_type(&mut self, span: Span, ty: ResolvedType) {
        self.type_info.expressions.assignment_binding_types.insert(span, ty);
    }

    /// Record a source declaration target proven by normal typechecking.
    pub fn record_source_target(
        &mut self,
        span: Span,
        module_path: Vec<String>,
        name: impl Into<String>,
        kind: impl Into<String>,
    ) {
        self.type_info.expressions.source_targets.insert(
            span,
            SourceTargetInfo {
                module_path,
                name: name.into(),
                kind: kind.into(),
            },
        );
    }

    /// Export the RFC 120 identities and active source bindings into lowering-facing artifacts.
    ///
    /// Runs once after declaration collection. Body-local symbols are exported separately after their owner is checked.
    /// The legacy identity-only map keeps local declaration facts available to existing consumers. The
    /// checked-binding map additionally preserves imports and aliases with their target identities and all bindings
    /// introduced by a single declaration, so HIR never has to reconstruct source resolution from spellings.
    pub(super) fn export_module_declaration_bindings(&mut self) {
        for (span, identity) in self.symbols.local_declaration_identities() {
            self.type_info
                .declarations
                .declaration_identities
                .insert(span, identity.clone());
        }
        self.type_info.declarations.hir_bindings_by_span.clear();
        let bindings = self
            .symbols
            .active_module_source_bindings()
            .map(|(span, name, canonical)| {
                (
                    span,
                    CheckedSourceBinding {
                        local_name: name.to_string(),
                        canonical: canonical.cloned(),
                    },
                )
            })
            .collect::<Vec<_>>();
        for (span, binding) in bindings {
            self.type_info
                .declarations
                .hir_bindings_by_span
                .entry(span)
                .or_default()
                .push(binding);
        }
    }

    /// Retain body-owned lexical identities after body checking without republishing the module header.
    pub(super) fn export_nested_declaration_identities(&mut self) {
        for (span, identity) in self.symbols.nested_declaration_identities() {
            self.type_info
                .declarations
                .nested_declaration_identities
                .insert(span, identity.clone());
        }
    }

    /// Snapshot active checked source-import paths for consumers that do not retain the symbol table.
    pub(super) fn export_checked_import_bindings(&mut self) {
        self.type_info.import_bindings = CheckedImportBindings::from_paths(
            self.symbols
                .active_import_binding_paths()
                .map(|(name, path)| (name.to_string(), path.to_vec())),
        );
    }

    /// Retain one compiler-proven ordinary function call for checked C bridge/facade projection.
    ///
    /// The general semantic source-target map remains the authority for all codegraph targets. This narrow retained
    /// view merely records which active callable made a resolved function call so C-ABI tooling can identify a
    /// public facade without reparsing source or generated Rust.
    pub fn record_c_abi_function_call_target(&mut self, span: Span, target: SourceTargetInfo) {
        let Some(caller) = self.current_c_abi_raw_call_owner.clone() else {
            return;
        };
        self.type_info
            .c_abi
            .function_calls
            .push(CBindingFunctionCall { span, caller, target });
    }

    /// Return the canonical declaration origin for a source binding, if it is part of the supported codegraph target
    /// set.
    pub fn source_target_for_symbol(&self, name: &str, kind: &SymbolKind) -> Option<SourceTargetInfo> {
        if let Some(target) = self.source_import_targets.get(name) {
            return Some(target.clone());
        }
        if matches!(kind, SymbolKind::Function(_) | SymbolKind::FunctionOverloads(_))
            && !self.current_module_function_symbols.contains_key(name)
        {
            return None;
        }
        let current_module_path = self.current_module_path.clone()?;
        let target_kind = Self::source_target_kind_for_symbol(kind)?;
        Some(SourceTargetInfo {
            module_path: current_module_path,
            name: name.to_string(),
            kind: target_kind.to_string(),
        })
    }

    /// Return the codegraph declaration kind for source symbol targets supported by the current semantic artifacts.
    pub fn source_target_kind_for_symbol(kind: &SymbolKind) -> Option<&'static str> {
        match kind {
            SymbolKind::Function(_) | SymbolKind::FunctionOverloads(_) => Some("function"),
            SymbolKind::Type(info) => Self::source_target_kind_for_type_info(info),
            SymbolKind::Capability(_) => Some("capability"),
            _ => None,
        }
    }

    /// Return the codegraph declaration kind for type symbols supported by the current semantic artifacts.
    pub fn source_target_kind_for_type_info(info: &TypeInfo) -> Option<&'static str> {
        match info {
            TypeInfo::Model(_) => Some("model"),
            TypeInfo::Class(_) => Some("class"),
            TypeInfo::Newtype(info) => Some(if info.is_rusttype { "rusttype" } else { "newtype" }),
            TypeInfo::Enum(_) => Some("enum"),
            TypeInfo::Builtin | TypeInfo::TypeAlias => None,
        }
    }

    /// Record a typechecker-proven unpack binding plan for backend lowering.
    pub fn record_fixed_unpack_plan(&mut self, span: Span, plan: FixedUnpackPlan) {
        self.type_info.calls.fixed_unpack_plans.insert(span, plan);
    }
}
