//! Publish native storage once, while the accepting checker still owns type and declaration resolution.

use std::collections::{BTreeMap, HashMap};

use incan_semantics_core::{CanonicalSymbolId, IncanType, body_ir};

use super::TypeChecker;
use super::type_info::{RustCheckedCall, RustCheckedSignature, semantic_type_from_resolved};
use crate::resolved_type_subst::map_resolved_type_leaves;
use crate::symbols::ResolvedType;

impl TypeChecker {
    /// Retain source binder roles while the accepting function or method's lexical environment is still active.
    pub(in crate::typechecker) fn native_call_type_parameters(&self) -> Vec<String> {
        self.current_type_param_bound_details
            .iter()
            .flat_map(|frame| frame.keys())
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// Freeze all selected native signatures and diagnostic displays before publishing this successful check.
    /// Body IR receives structural types and exact owners; it never creates another checker or resolves a name.
    pub(super) fn publish_rust_storage_types(&mut self) {
        // ---- Selected callees ----
        let mut calls = std::mem::take(&mut self.type_info.rust.checked_calls);
        let mut hooks = std::mem::take(&mut self.type_info.rust.iteration_hooks);
        let mut storage = HashMap::new();
        for call in calls.values_mut() {
            self.freeze_rust_call(call, &mut storage);
        }
        for hook in hooks.values_mut() {
            self.freeze_rust_call(&mut hook.iter, &mut storage);
            self.freeze_rust_call(&mut hook.next, &mut storage);
        }
        // ---- Checked values and declarations ----
        let expressions = &self.type_info.expressions;
        let declarations = &self.type_info.declarations;
        for ty in expressions
            .expr_types
            .values()
            .chain(expressions.body_ir_expr_types.values())
            .chain(expressions.body_ir_binding_types.values())
            .chain(expressions.assignment_binding_types.values())
            .chain(expressions.value_destination_types.values())
            .chain(self.type_info.references.resolved_write_types.values())
            .chain(declarations.type_alias_targets.values())
            .chain(self.type_info.rust.value_native_types.values())
            .chain(self.type_info.calls.inferred_call_result_types.values())
            .chain(self.type_info.calls.body_ir_constructor_types.values())
        {
            self.rust_storage_type(ty, &mut storage, &mut BTreeMap::new());
        }
        for binding in declarations.function_bindings.values() {
            for ty in binding
                .params
                .iter()
                .map(|parameter| &parameter.ty)
                .chain([&binding.return_type])
            {
                self.rust_storage_type(ty, &mut storage, &mut BTreeMap::new());
            }
        }
        // ---- Native layout and default templates ----
        for layout in self.type_info.rust.checked_layouts.values() {
            for field in &layout.fields {
                self.rust_storage_type(&field.ty, &mut storage, &mut BTreeMap::new());
            }
        }
        for layout in self.type_info.rust.checked_enums.values() {
            for field in layout.variants.iter().flat_map(|(_, fields)| fields) {
                self.rust_storage_type(field, &mut storage, &mut BTreeMap::new());
            }
        }
        for parameters in self.type_info.rust.owner_parameters.values() {
            for default in parameters.iter().filter_map(|(_, default)| default.as_ref()) {
                self.rust_storage_type(default, &mut storage, &mut BTreeMap::new());
            }
        }
        // ---- Published checker facts ----
        self.type_info.rust.storage_types = storage;
        self.type_info.rust.checked_calls = calls;
        self.type_info.rust.iteration_hooks = hooks;
    }

    /// Copy a selected call's structural storage and its declaration owners, including every receiver adjustment.
    fn freeze_rust_call(&self, call: &mut RustCheckedCall, storage: &mut HashMap<String, IncanType>) {
        for adjustment in &mut call.receiver_adjustments {
            self.freeze_rust_call(adjustment, storage);
        }
        let mut identities = BTreeMap::new();
        call.storage = Some(RustCheckedSignature {
            parameters: call
                .parameters
                .iter()
                .map(|parameter| {
                    self.rust_call_storage_type(&parameter.ty, &call.source_type_parameters, storage, &mut identities)
                })
                .collect(),
            return_type: self.rust_call_storage_type(
                &call.return_type,
                &call.source_type_parameters,
                storage,
                &mut identities,
            ),
            type_arguments: call
                .type_arguments
                .iter()
                .map(|argument| {
                    self.rust_call_storage_type(argument, &call.source_type_parameters, storage, &mut identities)
                })
                .collect(),
            identities,
        });
    }

    /// Resolve diagnostic Rust leaves in the original checker and retain source owners before erasing aliases.
    /// Structural recursion uses the shared resolved-type walk; generic display splitting stays in the checker.
    fn rust_storage_type(
        &self,
        ty: &ResolvedType,
        storage: &mut HashMap<String, IncanType>,
        identities: &mut BTreeMap<String, CanonicalSymbolId>,
    ) -> IncanType {
        self.rust_call_storage_type(ty, &[], storage, identities)
    }

    /// Preserve a call's source binder categories before projecting its fully instantiated native storage.
    fn rust_call_storage_type(
        &self,
        ty: &ResolvedType,
        parameters: &[String],
        storage: &mut HashMap<String, IncanType>,
        identities: &mut BTreeMap<String, CanonicalSymbolId>,
    ) -> IncanType {
        let structural = self.rust_storage_resolved(ty, parameters);
        let mut projected = semantic_type_from_resolved(&structural);
        self.qualify_rust_storage(&mut projected, identities);
        self.remember_rust_storage(ty, &projected, storage);
        projected
    }

    /// Reuse storage projection recursively without allowing diagnostic displays into a structural signature.
    fn rust_storage_resolved(&self, ty: &ResolvedType, parameters: &[String]) -> ResolvedType {
        map_resolved_type_leaves(ty, &|leaf| {
            if let ResolvedType::Named(name) | ResolvedType::RustPath(name) = leaf
                && parameters.contains(name)
            {
                return Some(ResolvedType::TypeVar(name.clone()));
            }
            if let ResolvedType::Generic(base, arguments) = leaf {
                return Some(self.rust_storage_constructor(base, arguments.clone(), parameters));
            }
            if let ResolvedType::Named(name) = leaf
                && body_ir::rust_item_identity(name).is_some()
            {
                return Some(self.rust_storage_constructor(name, Vec::new(), parameters));
            }
            let ResolvedType::RustPath(display) = leaf else {
                return None;
            };
            let parsed = self.native_rust_signature_type(display, "");
            if parsed != *leaf {
                return Some(self.rust_storage_resolved(&parsed, parameters));
            }
            if let Some((base, arguments)) = Self::rust_generic_base_and_args(display) {
                return Some(
                    self.rust_storage_constructor(
                        base,
                        arguments
                            .into_iter()
                            .filter(|argument| !Self::rust_generic_arg_is_lifetime(argument))
                            .map(|argument| {
                                self.rust_storage_resolved(&self.native_rust_signature_type(argument, base), parameters)
                            })
                            .collect(),
                        parameters,
                    ),
                );
            }
            body_ir::rust_item_identity(display).map(|_| self.rust_storage_constructor(display, Vec::new(), parameters))
        })
    }

    /// Complete native owner defaults from the admitted item before retaining each structural argument.
    fn rust_storage_constructor(
        &self,
        base: &str,
        mut arguments: Vec<ResolvedType>,
        parameters: &[String],
    ) -> ResolvedType {
        let base = self.rust_storage_name(base);
        if body_ir::rust_item_identity(&base).is_some()
            && let Some(incan_lang::interop::RustItemMetadata {
                kind: incan_lang::interop::RustItemKind::Type(info),
                ..
            }) = self.lookup_rust_item_metadata_for_path(&base)
        {
            self.complete_rust_owner_arguments(&info, &mut arguments);
        }
        let arguments = arguments
            .iter()
            .map(|argument| self.rust_storage_resolved(argument, parameters))
            .collect::<Vec<_>>();
        if arguments.is_empty() {
            ResolvedType::Named(base)
        } else {
            ResolvedType::Generic(base, arguments)
        }
    }

    /// Select a native item's admitted public path, leaving source and builtin names to their own identities.
    fn rust_storage_name(&self, name: &str) -> String {
        self.type_info
            .rust
            .item_public_paths
            .get(name)
            .cloned()
            .unwrap_or_else(|| name.to_owned())
    }

    /// Encode source owners from this checker's exact type bindings; Rust items and builtins keep native storage.
    fn qualify_rust_storage(&self, ty: &mut IncanType, identities: &mut BTreeMap<String, CanonicalSymbolId>) {
        match ty {
            IncanType::Named(name) => {
                if let Some(target) = self.type_info.type_alias_target(name) {
                    *ty = self.rust_storage_type(target, &mut HashMap::new(), identities);
                    return;
                }
                if let Some(builtin) = incan_semantics_core::RuntimeBuiltinType::from_carrier_path(name) {
                    *ty = IncanType::NativeBuiltin(builtin);
                    return;
                }
                let identity = self
                    .type_info
                    .declarations
                    .named_type_identities
                    .get(name)
                    .or_else(|| self.type_info.declarations.body_ir_native_type_identities.get(name))
                    .or_else(|| self.symbols.module_type_identity(name));
                if let Some(identity) = identity {
                    if let Some(builtin) = incan_semantics_core::RuntimeBuiltinType::from_identity(identity) {
                        *ty = IncanType::NativeBuiltin(builtin);
                        return;
                    }
                    let key = body_ir::nominal_carrier_key(identity)
                        .or_else(|| body_ir::native_opaque_name(identity))
                        .unwrap_or_else(|| identity.declaration_name.clone());
                    if key == identity.declaration_name
                        && self
                            .type_info
                            .declarations
                            .named_type_identities
                            .values()
                            .any(|other| other.declaration_name == key && other != identity)
                    {
                        *ty = IncanType::Unsupported("ambiguous checked native argument owner".into());
                        return;
                    }
                    identities.insert(key.clone(), identity.clone());
                    *name = key;
                }
            }
            IncanType::Generic { base, args } => {
                *base = self.rust_storage_name(base);
                let mut owner = IncanType::Named(base.clone());
                self.qualify_rust_storage(&mut owner, identities);
                if let IncanType::Named(name) = owner {
                    *base = name;
                }
                for argument in args {
                    self.qualify_rust_storage(argument, identities);
                }
            }
            IncanType::Tuple(fields) => {
                for field in fields {
                    self.qualify_rust_storage(field, identities);
                }
            }
            IncanType::Ref(inner) | IncanType::RefMut(inner) | IncanType::TypeToken(inner) => {
                self.qualify_rust_storage(inner, identities);
            }
            IncanType::Function { params, return_type } => {
                for parameter in params {
                    self.qualify_rust_storage(&mut parameter.ty, identities);
                }
                self.qualify_rust_storage(return_type, identities);
            }
            _ => {}
        }
    }

    /// Index diagnostic display spellings by the structure already retained while checking them.
    fn remember_rust_storage(
        &self,
        original: &ResolvedType,
        projected: &IncanType,
        storage: &mut HashMap<String, IncanType>,
    ) {
        match (original, projected) {
            (ResolvedType::RustPath(display), _) => {
                storage.insert(display.clone(), projected.clone());
            }
            (
                ResolvedType::Generic(_, arguments) | ResolvedType::Tuple(arguments),
                IncanType::Generic { args, .. } | IncanType::Tuple(args),
            ) => {
                for (original, projected) in arguments.iter().zip(args) {
                    self.remember_rust_storage(original, projected, storage);
                }
            }
            (
                ResolvedType::Ref(original) | ResolvedType::RefMut(original) | ResolvedType::TypeToken(original),
                IncanType::Ref(projected) | IncanType::RefMut(projected) | IncanType::TypeToken(projected),
            ) => {
                self.remember_rust_storage(original, projected, storage);
            }
            _ => {}
        }
    }
}
