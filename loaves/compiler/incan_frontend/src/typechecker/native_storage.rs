//! Publish native storage once, while the accepting checker still owns type and declaration resolution.

use std::collections::{BTreeMap, HashMap};

use incan_lang::interop::rust_display_is_owned_string;
use incan_lang::lang::types::collections::{self, CollectionTypeId};
use incan_semantics_core::{CanonicalSymbolId, IncanType, body_ir};
use quote::ToTokens;

use super::TypeChecker;
use super::type_info::{FunctionBindingInfo, RustCheckedCall, RustCheckedSignature, semantic_type_from_resolved};
use crate::ast::{Declaration, MethodDecl, Program, Spanned, TypeParam};
use crate::resolved_type_subst::{map_resolved_type_leaves, substitute_resolved_type, type_param_subst_map};
use crate::symbols::{ResolvedType, TypeInfo};

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
    pub(super) fn publish_rust_storage_types(&mut self, program: &Program) {
        // ---- Selected callees ----
        let mut calls = std::mem::take(&mut self.type_info.rust.checked_calls);
        let mut awaits = std::mem::take(&mut self.type_info.rust.await_conversions);
        let mut arguments = std::mem::take(&mut self.type_info.rust.argument_adjustments);
        let mut conversions = std::mem::take(&mut self.type_info.rust.declared_conversions);
        let mut elements = std::mem::take(&mut self.type_info.rust.element_adjustments);
        let mut fields = std::mem::take(&mut self.type_info.rust.field_adjustments);
        let mut function_values = std::mem::take(&mut self.type_info.rust.checked_function_values);
        let mut hooks = std::mem::take(&mut self.type_info.rust.iteration_hooks);
        let mut storage = HashMap::new();
        for call in calls
            .values_mut()
            .chain(awaits.values_mut())
            .chain(arguments.values_mut().flatten())
            .chain(elements.values_mut())
            .chain(conversions.iter_mut().map(|edge| &mut edge.call))
            .chain(fields.values_mut().flatten())
            .chain(function_values.values_mut())
        {
            self.freeze_rust_call(call, &mut storage);
        }
        for hook in hooks.values_mut() {
            self.freeze_rust_call(&mut hook.iter, &mut storage);
            self.freeze_rust_call(&mut hook.next, &mut storage);
        }
        // ---- Checked values and declarations ----
        self.freeze_value_storage(program, &mut storage);
        self.freeze_source_storage(program, &mut storage);
        // ---- Native layout and default templates ----
        for layout in self.type_info.rust.checked_layouts.values() {
            for field in &layout.fields {
                self.rust_call_storage_type(&field.ty, &layout.type_parameters, &mut storage, &mut BTreeMap::new());
            }
        }
        for layout in self.type_info.rust.checked_enums.values() {
            for field in layout.variants.iter().flat_map(|(_, fields, _)| fields) {
                self.rust_call_storage_type(field, &layout.type_parameters, &mut storage, &mut BTreeMap::new());
            }
        }
        for parameters in self.type_info.rust.owner_parameters.values() {
            let names = parameters.iter().map(|(name, _)| name.clone()).collect::<Vec<_>>();
            for default in parameters.iter().filter_map(|(_, default)| default.as_ref()) {
                self.rust_call_storage_type(default, &names, &mut storage, &mut BTreeMap::new());
            }
        }
        // ---- Published checker facts ----
        self.type_info.rust.storage_types = storage;
        self.type_info.rust.checked_calls = calls;
        self.type_info.rust.await_conversions = awaits;
        self.type_info.rust.argument_adjustments = arguments;
        self.type_info.rust.declared_conversions = conversions;
        self.type_info.rust.element_adjustments = elements;
        self.type_info.rust.field_adjustments = fields;
        self.type_info.rust.checked_function_values = function_values;
        self.type_info.rust.iteration_hooks = hooks;
    }

    /// Publish accepted value types in their lexical source binder frame. Dropping the frame turns a parameter
    /// embedded in a native display into a nominal name and can contradict the same call's checked signature.
    fn freeze_value_storage(&self, program: &Program, storage: &mut HashMap<IncanType, IncanType>) {
        let frames = StorageFrames::new(program);
        let expressions = &self.type_info.expressions;
        for (span, ty) in expressions
            .expr_types
            .iter()
            .chain(&expressions.body_ir_expr_types)
            .chain(&expressions.assignment_binding_types)
            .chain(&expressions.value_destination_types)
            .chain(&self.type_info.calls.inferred_call_result_types)
            .chain(&self.type_info.calls.body_ir_constructor_types)
        {
            let parameters = frames.parameters(*span);
            self.rust_call_storage_type(ty, &parameters, storage, &mut BTreeMap::new());
        }
        for ((span, _), ty) in &self.type_info.references.resolved_write_types {
            let parameters = frames.parameters(*span);
            self.rust_call_storage_type(ty, &parameters, storage, &mut BTreeMap::new());
        }
        for (identity, ty) in expressions
            .body_ir_binding_types
            .iter()
            .chain(&self.type_info.rust.value_native_types)
        {
            let parameters = frames.parameters(identity.declaration_span);
            self.rust_call_storage_type(ty, &parameters, storage, &mut BTreeMap::new());
        }
    }

    /// Freeze declared native storage in its source binder frame, including unread fields and inherited layouts.
    /// Syntax supplies only declared binder roles and spans; every retained type comes from successful checker facts.
    fn freeze_source_storage(&self, program: &Program, storage: &mut HashMap<IncanType, IncanType>) {
        let facts = &self.type_info.declarations;
        for declaration in &program.declarations {
            let (parameters, methods) = match &declaration.node {
                Declaration::Model(owner) => (&owner.type_params, owner.methods.as_slice()),
                Declaration::Class(owner) => (&owner.type_params, owner.methods.as_slice()),
                Declaration::Enum(owner) => (&owner.type_params, owner.methods.as_slice()),
                Declaration::Trait(owner) => (&owner.type_params, owner.methods.as_slice()),
                Declaration::Newtype(owner) => (&owner.type_params, owner.methods.as_slice()),
                Declaration::Function(owner) => (&owner.type_params, &[][..]),
                Declaration::TypeAlias(owner) => (&owner.type_params, &[][..]),
                _ => continue,
            };
            let parameters = parameters
                .iter()
                .map(|parameter| parameter.name.clone())
                .collect::<Vec<_>>();
            if let Declaration::TypeAlias(owner) = &declaration.node
                && let Some(target) = facts.type_alias_targets.get(&owner.name)
            {
                self.rust_call_storage_type(target, &parameters, storage, &mut BTreeMap::new());
            }
            if let Declaration::Newtype(owner) = &declaration.node
                && owner.is_rusttype
            {
                let alias = if parameters.is_empty() {
                    ResolvedType::Named(owner.name.clone())
                } else {
                    ResolvedType::Generic(
                        owner.name.clone(),
                        parameters.iter().cloned().map(ResolvedType::TypeVar).collect(),
                    )
                };
                self.rust_call_storage_type(&alias, &parameters, storage, &mut BTreeMap::new());
            }
            for (span, ty) in facts.model_field_types.iter().chain(&facts.enum_payload_types) {
                if span.start >= declaration.span.start && span.end <= declaration.span.end {
                    self.rust_call_storage_type(ty, &parameters, storage, &mut BTreeMap::new());
                }
            }
            if let Some(binding) = facts.function_bindings_by_span.get(&declaration.span) {
                self.freeze_source_signature(binding, &parameters, storage);
            }
            for method in methods {
                let Some(binding) = facts.method_bindings_by_span.get(&method.span) else {
                    continue;
                };
                let mut frame = parameters.clone();
                frame.extend(method.node.type_params.iter().map(|parameter| parameter.name.clone()));
                self.freeze_source_signature(binding, &frame, storage);
            }
        }
        for layout in facts.class_layouts.values() {
            for field in &layout.fields {
                self.rust_call_storage_type(&field.ty, &layout.type_params, storage, &mut BTreeMap::new());
            }
        }
        for newtype in facts.newtype_construction.values() {
            self.rust_call_storage_type(&newtype.underlying, &newtype.type_params, storage, &mut BTreeMap::new());
        }
        for property in facts.body_ir_property_bindings.values() {
            self.rust_call_storage_type(
                &property.receiver_type,
                &property.owner_parameters,
                storage,
                &mut BTreeMap::new(),
            );
            self.freeze_source_signature(&property.callable, &property.owner_parameters, storage);
        }
    }

    /// Retain one checked callable's parameter and result storage without dropping its source declaration binders.
    fn freeze_source_signature(
        &self,
        binding: &FunctionBindingInfo,
        parameters: &[String],
        storage: &mut HashMap<IncanType, IncanType>,
    ) {
        for ty in binding
            .params
            .iter()
            .map(|parameter| &parameter.ty)
            .chain([&binding.return_type])
        {
            self.rust_call_storage_type(ty, parameters, storage, &mut BTreeMap::new());
        }
    }

    /// Copy a selected call's structural storage and its declaration owners, including every receiver adjustment.
    fn freeze_rust_call(&self, call: &mut RustCheckedCall, storage: &mut HashMap<IncanType, IncanType>) {
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
        storage: &mut HashMap<IncanType, IncanType>,
        identities: &mut BTreeMap<String, CanonicalSymbolId>,
    ) -> IncanType {
        self.rust_call_storage_type(ty, &[], storage, identities)
    }

    /// Preserve a call's source binder categories before projecting its fully instantiated native storage.
    fn rust_call_storage_type(
        &self,
        ty: &ResolvedType,
        parameters: &[String],
        storage: &mut HashMap<IncanType, IncanType>,
        identities: &mut BTreeMap<String, CanonicalSymbolId>,
    ) -> IncanType {
        let structural = self.rust_storage_resolved(ty, parameters);
        let mut projected = semantic_type_from_resolved(&structural);
        self.qualify_rust_storage(&mut projected, parameters, identities);
        let original = map_resolved_type_leaves(ty, &|leaf| source_binder(leaf, parameters));
        self.remember_rust_storage(&original, &projected, storage);
        projected
    }

    /// Reuse storage projection recursively, normalizing admitted associated types before freezing structural storage.
    /// A transparent alias already selects its native owner; freeze that owner before diagnostic runtime projection
    /// can translate it back into the same source alias. Rust's owned text keeps the primitive String carrier even
    /// when a lexical import contributes another public path for that same definition.
    pub(super) fn rust_storage_resolved(&self, ty: &ResolvedType, parameters: &[String]) -> ResolvedType {
        map_resolved_type_leaves(ty, &|leaf| {
            if let Some(binder) = source_binder(leaf, parameters) {
                return Some(binder);
            }
            if let Some(target) = self.rusttype_native_target(leaf) {
                return Some(match target {
                    ResolvedType::RustPath(path) if body_ir::rust_item_identity(&path).is_some() => {
                        self.rust_storage_constructor(&path, Vec::new(), parameters)
                    }
                    ResolvedType::Generic(base, arguments) if body_ir::rust_item_identity(&base).is_some() => {
                        self.rust_storage_constructor(&base, arguments, parameters)
                    }
                    target => self.rust_storage_resolved(&target, parameters),
                });
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
            if rust_display_is_owned_string(display) {
                return Some(ResolvedType::Str);
            }
            // Unsized text already has structural native storage. A public primitive re-export names the same
            // referent; its import identity must not turn `&str` into a reference to an opaque nominal.
            if display == "str" {
                return Some(leaf.clone());
            }
            // An admitted definition path has a native owner even when its final segment also names a source type.
            // Apply its public import identity before the display parser can consult that source declaration.
            let public = self.rust_public_type_path(display);
            if public != *display {
                return Some(self.rust_storage_constructor(&public, Vec::new(), parameters));
            }
            if display.trim_start().starts_with('<')
                && let Some(projected) = self.rust_projected_type(display, 0)
                && projected != *leaf
            {
                return Some(self.rust_storage_resolved(&projected, parameters));
            }
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

    /// Expand a checked transparent rusttype after classifying its underlying native binders, then substitute its
    /// actual arguments. Published Rust-path templates keep binder displays until this accepting-checker boundary;
    /// source members, conversions, and trait implementations retain their own checked admission requirements.
    pub(in crate::typechecker) fn rusttype_native_target(&self, ty: &ResolvedType) -> Option<ResolvedType> {
        let (name, arguments) = match ty {
            ResolvedType::Named(name) => (name, &[][..]),
            ResolvedType::Generic(name, arguments) => (name, arguments.as_slice()),
            _ => return None,
        };
        let TypeInfo::Newtype(info) = self.lookup_semantic_type_info(name)? else {
            return None;
        };
        if !info.is_rusttype || info.type_params.len() != arguments.len() {
            return None;
        }
        let template = self.rust_inference_type(&info.underlying, &info.type_params);
        let target = substitute_resolved_type(&template, &type_param_subst_map(&info.type_params, arguments));
        let Some((base, _, arguments, _)) = self.rust_identity_for_type(&target) else {
            return Some(target);
        };
        Some(if arguments.is_empty() {
            ResolvedType::RustPath(base)
        } else {
            ResolvedType::Generic(base, arguments)
        })
    }

    /// Complete native owner defaults from the admitted item before retaining each structural argument. Standard
    /// collections use the existing carrier only after metadata proves every hidden argument equals its default;
    /// other crates and nondefault allocators or hashers retain exact native storage.
    fn rust_storage_constructor(
        &self,
        base: &str,
        mut arguments: Vec<ResolvedType>,
        parameters: &[String],
    ) -> ResolvedType {
        let base = self.rust_public_type_path(base);
        // These internal constructors retain exact native element storage. Source frozen views additionally freeze
        // text and bytes, so reusing those public constructor names would change FrozenList<String> into a list of
        // static text. The lowering reads this storage decision rather than resolving a native owner again.
        let frozen = match (base.as_str(), arguments.len()) {
            ("incan_std_core::frozen::FrozenList", 1) => Some("$FrozenList"),
            ("incan_std_core::frozen::FrozenSet", 1) => Some("$FrozenSet"),
            ("incan_std_core::frozen::FrozenDict", 2) => Some("$FrozenDict"),
            _ => None,
        };
        if let Some(constructor) = frozen {
            return ResolvedType::Generic(
                constructor.into(),
                arguments
                    .iter()
                    .map(|argument| self.rust_storage_resolved(argument, parameters))
                    .collect(),
            );
        }
        if base == "incan_std_core::num::Decimal128" && arguments.is_empty() {
            // Decimal precision and scale constrain source expressions; all such expressions and this native
            // definition share the runtime's Decimal128 storage. Do not invent a separate nominal MIR carrier.
            return ResolvedType::Generic(
                "decimal".into(),
                vec![ResolvedType::TypeVar("38".into()), ResolvedType::TypeVar("0".into())],
            );
        }
        if body_ir::rust_item_identity(&base).is_some()
            && let Some(incan_lang::interop::RustItemMetadata {
                kind: incan_lang::interop::RustItemKind::Type(info),
                ..
            }) = self.lookup_rust_item_metadata_for_path(&base).as_deref()
        {
            self.complete_rust_owner_arguments(info, &mut arguments);
            if base.starts_with("std::") || base.starts_with("alloc::") {
                let projected = self.rust_inference_constructor(&base, &arguments, parameters);
                if matches!(&projected, ResolvedType::Generic(name, _)
                    if matches!(collections::from_str(name), Some(CollectionTypeId::List | CollectionTypeId::Dict | CollectionTypeId::Set)))
                {
                    return self.rust_storage_resolved(&projected, parameters);
                }
            }
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

    /// Retain the exact owner beside the checker's shared nominal projection. Alias targets and native builtin storage
    /// remain checker facts; an unencoded conflicting owner refuses instead of overwriting another signature's proof.
    fn qualify_rust_storage(
        &self,
        ty: &mut IncanType,
        parameters: &[String],
        identities: &mut BTreeMap<String, CanonicalSymbolId>,
    ) {
        match ty {
            IncanType::RustInteropPath(path) => {
                if let Some(object) = self.native_bounded_rust_object_storage(path) {
                    *ty = object;
                    return;
                }
                if let Some(array) = self.rust_array_storage(path, parameters, identities) {
                    *ty = array;
                }
            }
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
                    .or_else(|| self.type_info.declarations.body_ir_native_type_identities.get(name));
                let qualified = self.type_info.qualify_semantic_type(IncanType::Named(name.clone()));
                let IncanType::Named(key) = &qualified else {
                    *ty = qualified;
                    return;
                };
                if let Some(identity) = identity {
                    if let Some(builtin) = incan_semantics_core::RuntimeBuiltinType::from_identity(identity) {
                        *ty = IncanType::NativeBuiltin(builtin);
                        return;
                    }
                    if key == &identity.declaration_name
                        && self
                            .type_info
                            .declarations
                            .named_type_identities
                            .values()
                            .any(|other| &other.declaration_name == key && other != identity)
                    {
                        *ty = IncanType::Unsupported("ambiguous checked native argument owner".into());
                        return;
                    }
                    identities.insert(key.clone(), identity.clone());
                }
                *ty = qualified;
            }
            IncanType::Generic { base, args } => {
                *base = self.rust_public_type_path(base);
                let mut owner = IncanType::Named(base.clone());
                self.qualify_rust_storage(&mut owner, parameters, identities);
                let IncanType::Named(name) = owner else {
                    *ty = match owner {
                        IncanType::Unsupported(_) => owner,
                        _ => IncanType::Unsupported("checked native generic owner lacks a nominal base".into()),
                    };
                    return;
                };
                *base = name;
                for argument in args {
                    self.qualify_rust_storage(argument, parameters, identities);
                }
            }
            IncanType::Tuple(fields) => {
                for field in fields {
                    self.qualify_rust_storage(field, parameters, identities);
                }
            }
            IncanType::Ref(inner) | IncanType::RefMut(inner) | IncanType::TypeToken(inner) => {
                self.qualify_rust_storage(inner, parameters, identities);
            }
            IncanType::NativeArray { element, .. } => self.qualify_rust_storage(element, parameters, identities),
            IncanType::Function { params, return_type } => {
                for parameter in params {
                    self.qualify_rust_storage(&mut parameter.ty, parameters, identities);
                }
                self.qualify_rust_storage(return_type, parameters, identities);
            }
            _ => {}
        }
    }

    /// Freeze only arrays with an evaluated native integer length. The element uses the same source binder and
    /// canonical-owner projection as every other native signature component; unresolved const parameters refuse.
    fn rust_array_storage(
        &self,
        path: &str,
        parameters: &[String],
        identities: &mut BTreeMap<String, CanonicalSymbolId>,
    ) -> Option<IncanType> {
        let (element, length) = self.rust_array_parts(path)?;
        Some(IncanType::NativeArray {
            element: Box::new(self.rust_call_storage_type(&element, parameters, &mut HashMap::new(), identities)),
            length,
        })
    }

    /// Decode an admitted fixed-array metadata display once for receiver selection and structural publication.
    /// Only evaluated nonnegative lengths authorize a view; const parameters remain explicit refusals.
    pub(in crate::typechecker) fn rust_array_parts(&self, path: &str) -> Option<(ResolvedType, u64)> {
        let syn::Type::Array(array) = syn::parse_str::<syn::Type>(path).ok()? else {
            return None;
        };
        let syn::Expr::Lit(length) = &array.len else {
            return None;
        };
        let syn::Lit::Int(length) = &length.lit else {
            return None;
        };
        let length = length.base10_parse::<u64>().ok()?;
        Some((
            self.native_rust_signature_type(&array.elem.to_token_stream().to_string(), ""),
            length,
        ))
    }

    /// Index native values before and after checked source-owner qualification, including short import aliases whose
    /// native identity is exposed only by the projected type. A conflicting key cannot authorize either storage.
    fn remember_rust_storage(
        &self,
        original: &ResolvedType,
        projected: &IncanType,
        storage: &mut HashMap<IncanType, IncanType>,
    ) {
        let key = semantic_type_from_resolved(original);
        if has_native_storage(&key) || has_native_storage(projected) || self.rusttype_native_target(original).is_some()
        {
            for key in [key.clone(), self.type_info.qualify_semantic_type(key)] {
                let retained = storage.entry(key).or_insert_with(|| projected.clone());
                if retained != projected {
                    *retained = IncanType::Unsupported("ambiguous checked native value storage".into());
                }
            }
        }
        match (original, projected) {
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

/// The binder frames of one program's top-level declarations, in source order, collected once per publication.
///
/// A checked fact's provenance span selects its declaration's source binders. Syntax contributes only binder roles;
/// native types and declaration identities still come from the accepting checker's retained facts.
struct StorageFrames<'a> {
    /// Each binder-owning declaration's span, type parameters and methods; top-level declarations do not overlap.
    declarations: Vec<(crate::ast::Span, &'a [TypeParam], &'a [Spanned<MethodDecl>])>,
}

impl<'a> StorageFrames<'a> {
    /// Collect the binder-owning declarations of `program` in source order.
    fn new(program: &'a Program) -> Self {
        let declarations = program
            .declarations
            .iter()
            .filter_map(|declaration| {
                let (parameters, methods) = match &declaration.node {
                    Declaration::Model(owner) => (owner.type_params.as_slice(), owner.methods.as_slice()),
                    Declaration::Class(owner) => (owner.type_params.as_slice(), owner.methods.as_slice()),
                    Declaration::Enum(owner) => (owner.type_params.as_slice(), owner.methods.as_slice()),
                    Declaration::Trait(owner) => (owner.type_params.as_slice(), owner.methods.as_slice()),
                    Declaration::Newtype(owner) => (owner.type_params.as_slice(), owner.methods.as_slice()),
                    Declaration::Function(owner) => (owner.type_params.as_slice(), &[][..]),
                    Declaration::TypeAlias(owner) => (owner.type_params.as_slice(), &[][..]),
                    _ => return None,
                };
                Some((declaration.span, parameters, methods))
            })
            .collect();
        Self { declarations }
    }

    /// The type parameter names in scope at `span`: its declaration's, then its method's.
    fn parameters(&self, span: crate::ast::Span) -> Vec<String> {
        let index = self
            .declarations
            .partition_point(|(declaration, _, _)| declaration.start <= span.start);
        let Some((declaration, parameters, methods)) =
            index.checked_sub(1).and_then(|index| self.declarations.get(index))
        else {
            return Vec::new();
        };
        if span.end > declaration.end {
            return Vec::new();
        }
        let mut names = parameters
            .iter()
            .map(|parameter| parameter.name.clone())
            .collect::<Vec<_>>();
        if let Some(method) = methods
            .iter()
            .find(|method| span.start >= method.span.start && span.end <= method.span.end)
        {
            names.extend(method.node.type_params.iter().map(|parameter| parameter.name.clone()));
        }
        names
    }
}

impl TypeChecker {
    /// Freeze a nongeneric dyn principal and its explicit auto traits while native metadata still owns resolution.
    /// Associated equalities and lifetime-bearing objects require a richer storage contract and stay refused.
    fn native_bounded_rust_object_storage(&self, display: &str) -> Option<IncanType> {
        let syn::Type::TraitObject(object) = syn::parse_str::<syn::Type>(display).ok()? else {
            return None;
        };
        let mut principal = None;
        let mut send = false;
        let mut sync = false;
        for bound in object.bounds {
            let bound = match bound {
                syn::TypeParamBound::Trait(bound) => bound,
                syn::TypeParamBound::Lifetime(lifetime) if lifetime.ident == "static" => continue,
                _ => return None,
            };
            if bound
                .path
                .segments
                .iter()
                .any(|segment| !matches!(segment.arguments, syn::PathArguments::None))
            {
                return None;
            }
            let path = bound.path.to_token_stream().to_string().replace(' ', "");
            match path.as_str() {
                "std::marker::Send" | "core::marker::Send" => send = true,
                "std::marker::Sync" | "core::marker::Sync" => sync = true,
                _ if principal.is_none() => principal = Some(path),
                _ => return None,
            }
        }
        if !send && !sync {
            return None;
        }
        let path = principal?;
        let metadata = self.rust_item_metadata_for_path(&path)?;
        let incan_lang::interop::RustItemKind::Trait(info) = &metadata.kind else {
            return None;
        };
        if !info.type_params.is_empty()
            || info
                .items
                .iter()
                .any(|item| matches!(item, incan_lang::interop::RustTraitAssoc::TypeAlias { .. }))
        {
            return None;
        }
        Some(IncanType::NativeTraitObject {
            path: metadata.canonical_path.clone(),
            send,
            sync,
        })
    }
}

/// Recognize only the source frame's declared binders before any nominal or native storage projection.
fn source_binder(ty: &ResolvedType, parameters: &[String]) -> Option<ResolvedType> {
    match ty {
        ResolvedType::Named(name) | ResolvedType::RustPath(name) if parameters.contains(name) => {
            Some(ResolvedType::TypeVar(name.clone()))
        }
        _ => None,
    }
}

/// Select types containing native storage; source binders and declarations keep their own lexical facts.
fn has_native_storage(ty: &IncanType) -> bool {
    match ty {
        IncanType::RustInteropPath(_) | IncanType::NativeTraitObject { .. } => true,
        IncanType::Named(name) => body_ir::rust_item_identity(name).is_some(),
        IncanType::Generic { base, args } => {
            body_ir::rust_item_identity(base).is_some() || args.iter().any(has_native_storage)
        }
        IncanType::Tuple(fields) => fields.iter().any(has_native_storage),
        IncanType::Ref(inner) | IncanType::RefMut(inner) | IncanType::TypeToken(inner) => has_native_storage(inner),
        IncanType::Function { params, return_type } => {
            params.iter().any(|parameter| has_native_storage(&parameter.ty)) || has_native_storage(return_type)
        }
        _ => false,
    }
}
