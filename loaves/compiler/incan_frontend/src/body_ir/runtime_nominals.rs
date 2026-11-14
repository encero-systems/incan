//! SDK runtime-owned surface types retain opaque native storage rather than a consumer-declared layout.

use std::collections::BTreeSet;

use incan_lang::lang::surface::types::{self as surface_types, SurfaceTypeCategory, SurfaceTypeKind};
use incan_lang::lang::types::collections::CollectionTypeId;
use incan_semantics_core::{CanonicalSymbolId, CompilerNodeId, HirSourceSpan, SemanticSourceTargetKind, SymbolOrigin};
use incan_semantics_core::{IncanPrimitiveType, SymbolNamespace};

use super::{IncanType, TypeCheckInfo, bir, semantic_type_from_resolved};

/// Retain checked task carriers, including the join error synthesized as an await result, with their SDK owner.
/// A source declaration with the same spelling remains source-owned; opaque records never authorize field access.
pub(super) fn collect(type_info: &TypeCheckInfo) -> Vec<bir::NominalDeclaration> {
    let mut names = BTreeSet::new();
    for ty in type_info
        .expressions
        .expr_types
        .values()
        .chain(type_info.calls.inferred_call_result_types.values())
    {
        named_leaves(&project_type(semantic_type_from_resolved(ty), type_info), &mut names);
    }
    for binding in type_info.declarations.function_bindings.values() {
        named_leaves(
            &project_type(semantic_type_from_resolved(&binding.return_type), type_info),
            &mut names,
        );
        for parameter in &binding.params {
            named_leaves(
                &project_type(semantic_type_from_resolved(&parameter.ty), type_info),
                &mut names,
            );
        }
    }
    // The registered native handle's Future output carries this error even when legacy leaves a Rust-path await
    // unknown. Retain the declared runtime dependency alongside the authenticated handle, not from a source spelling.
    let handle_name = surface_types::as_str(surface_types::SurfaceTypeId::JoinHandle);
    if names.contains(handle_name) && nominal(type_info, handle_name).is_some() {
        names.insert(surface_types::TASK_JOIN_ERROR_TYPE_NAME.to_owned());
    }
    names.into_iter().filter_map(|name| nominal(type_info, &name)).collect()
}

/// Retain the public native members of SDK task carriers found in this module's checked types.
/// Storage stays opaque; every call still requires the adapter to verify this contract against linked native metadata.
pub(super) fn members(facts: &TypeCheckInfo) -> Vec<bir::LoafCallable> {
    collect(facts)
        .iter()
        .flat_map(|declaration| {
            let id = surface_types::from_str(&declaration.name);
            let native_owner = id.and_then(|id| {
                let mut path = surface_types::info_for(id).ownership.runtime_rust_module_path?.to_vec();
                path.push(surface_types::as_str(id));
                Some(path.join("::"))
            });
            let Some(native_owner) = native_owner else {
                return Vec::new();
            };
            let (receiver, names): (IncanType, &[&str]) = match surface_types::from_str(&declaration.name) {
                Some(surface_types::SurfaceTypeId::TaskJoinError) => (
                    IncanType::Named(native_owner),
                    &["message", "source", "is_cancelled", "is_panic"],
                ),
                Some(surface_types::SurfaceTypeId::JoinHandle) => (
                    IncanType::Generic {
                        base: native_owner,
                        args: vec![IncanType::TypeVar("T".into())],
                    },
                    &["abort"],
                ),
                _ => return Vec::new(),
            };
            names.iter().filter_map(|name| member(facts, &receiver, name)).collect()
        })
        .collect()
}

/// Recognize SDK task and race storage whose executable call result must survive an inferred consumer binding.
/// This does not infer an output or admit an equal-spelled source type; it reuses the guarded registry ownership.
pub(super) fn retains_task_storage(facts: &TypeCheckInfo, ty: &IncanType) -> bool {
    if let IncanType::Generic { base, args } = ty
        && base == "List"
        && args.len() == 1
    {
        return retains_task_storage(facts, &args[0]);
    }
    let owner = match ty {
        IncanType::Named(owner) | IncanType::Generic { base: owner, .. } => owner,
        _ => return false,
    };
    surface_types::from_str(owner).is_some_and(|id| {
        matches!(
            surface_types::category(id),
            SurfaceTypeCategory::AsyncTask | SurfaceTypeCategory::AsyncRace
        )
    }) && nominal(facts, owner).is_some()
}

/// Bind only a registry-owned SDK task receiver to its published native method signature.
/// These contracts mirror task.rs; they grant no source field access or equal-spelled source/foreign method behavior.
pub(super) fn member(facts: &TypeCheckInfo, receiver: &IncanType, name: &str) -> Option<bir::LoafCallable> {
    let (id, owner, template, parameters) = match receiver {
        IncanType::Named(owner)
            if surface_types::from_str(owner) == Some(surface_types::SurfaceTypeId::TaskJoinError)
                || surface_types::from_runtime_rust_path(owner)
                    == Some(surface_types::SurfaceTypeId::TaskJoinError) =>
        {
            (
                surface_types::SurfaceTypeId::TaskJoinError,
                surface_types::as_str(surface_types::SurfaceTypeId::TaskJoinError),
                receiver.clone(),
                Vec::new(),
            )
        }
        IncanType::Generic { base, args }
            if (surface_types::from_str(base) == Some(surface_types::SurfaceTypeId::JoinHandle)
                || surface_types::from_runtime_rust_path(base) == Some(surface_types::SurfaceTypeId::JoinHandle))
                && args.len() == 1 =>
        {
            (
                surface_types::SurfaceTypeId::JoinHandle,
                surface_types::as_str(surface_types::SurfaceTypeId::JoinHandle),
                IncanType::Generic {
                    base: base.clone(),
                    args: vec![IncanType::TypeVar("T".into())],
                },
                vec!["T".into()],
            )
        }
        _ => return None,
    };
    if !facts.runtime_owner_admitted(id) {
        return None;
    }
    let mut path = surface_types::info_for(id)
        .ownership
        .runtime_rust_module_path?
        .iter()
        .map(|segment| (*segment).to_owned())
        .collect::<Vec<_>>();
    path.push(owner.to_owned());
    let result = match (id, name) {
        (surface_types::SurfaceTypeId::TaskJoinError, "message") => IncanType::Primitive(IncanPrimitiveType::Str),
        (surface_types::SurfaceTypeId::TaskJoinError, "source") => IncanType::builtin_collection(
            CollectionTypeId::Option,
            vec![IncanType::Primitive(IncanPrimitiveType::Str)],
        ),
        (surface_types::SurfaceTypeId::TaskJoinError, "is_cancelled" | "is_panic") => {
            IncanType::Primitive(IncanPrimitiveType::Bool)
        }
        (surface_types::SurfaceTypeId::JoinHandle, "abort") => IncanType::Primitive(IncanPrimitiveType::Unit),
        _ => return None,
    };
    path.push(name.into());
    let item = path.join("::");
    let canonical = bir::rust_item_identity(&item)?;
    Some(bir::LoafCallable {
        canonical,
        item,
        parameters: vec![IncanType::Ref(Box::new(template))],
        return_type: result,
        native_type_arguments: parameters.iter().cloned().map(IncanType::TypeVar).collect(),
        type_parameters: parameters,
        native_callable_parameters: Vec::new(),
        parameter_defaults: Vec::new(),
        declaration_frame: None,
    })
}

/// Resolve registered native vocabulary to its runtime facet; an authored declaration keeps its source layout.
fn nominal(type_info: &TypeCheckInfo, name: &str) -> Option<bir::NominalDeclaration> {
    let id = surface_types::from_str(name)?;
    if !type_info.runtime_owner_admitted(id) {
        return None;
    }
    let info = surface_types::info_for(id);
    let path = surface_types::stdlib_module_path(id)?
        .split('.')
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let library = surface_types::runtime_facet(id)?.to_owned();
    // Runtime vocabulary has no Incan declaration span. Its registry position is a stable synthetic declaration
    // address, never a consumer source location.
    let offset = surface_types::SURFACE_TYPES
        .iter()
        .position(|entry| entry.item.id == id)?;
    let mut canonical = CanonicalSymbolId::module_declaration(
        path.clone(),
        name,
        SemanticSourceTargetKind::Model,
        HirSourceSpan::new(offset, offset + 1),
    );
    canonical.origin = SymbolOrigin::Package {
        library,
        module_path: path[2..].to_vec(),
    };
    let module = incan_semantics_core::canonical_module_identity(&canonical)?;
    let type_parameters = if info.kind == SurfaceTypeKind::Generic {
        vec!["T".to_owned()]
    } else {
        Vec::new()
    };
    Some(bir::NominalDeclaration {
        record_package_identity: None,
        trait_adoptions: Vec::new(),
        native_item: String::new(),
        class_inheritance: None,
        native_type_arguments: Vec::new(),
        direct_declaration_id: CompilerNodeId::declaration_span(&module, offset, offset + 1),
        canonical,
        name: name.to_owned(),
        fields: Vec::new(),
        field_identities: Vec::new(),
        field_types: Vec::<IncanType>::new(),
        owned_callable_fields: Vec::new(),
        field_public: Vec::new(),
        field_aliases: Vec::new(),
        reflection_fields: None,
        reflection_metadata: None,
        comparison_equality: None,
        public: true,
        has_field_defaults: false,
        field_default_body: None,
        validated_constructor: None,
        method_partial_defaults: Vec::new(),
        display_method: None,
        newtype_deserialize: bir::NewtypeDeserialize::Unselected,
        newtype_string_conversion: bir::NewtypeStringConversion::Unselected,
        derives: Vec::new(),
        named_type_identities: Default::default(),
        type_parameter_count: type_parameters.len(),
        type_parameters,
    })
}

/// Copy checker-retained native storage and recursively preserve already structural source carriers.
/// An unretained native display remains opaque and cannot trigger parsing or default specialization in Body IR.
pub(super) fn project_type(ty: IncanType, facts: &TypeCheckInfo) -> IncanType {
    if let IncanType::Named(name) = &ty
        && let Some(storage) = super::checked_bridge::private_storage(name, facts)
    {
        return storage;
    }
    // Compatibility crates re-export the primitive runtime declaration; their retained Rust paths are not distinct
    // nominal owners. Normalize before checked storage can preserve the compatibility path as an opaque nominal.
    match &ty {
        IncanType::Named(name) => {
            if let Some(canonical) = surface_types::canonical_runtime_rust_path(name) {
                return IncanType::Named(canonical);
            }
        }
        IncanType::RustInteropPath(path)
            if surface_types::from_runtime_rust_path(path)
                .is_some_and(|id| surface_types::info_for(id).kind == SurfaceTypeKind::Named) =>
        {
            if let Some(canonical) = surface_types::canonical_runtime_rust_path(path) {
                return IncanType::Named(canonical);
            }
        }
        IncanType::Generic { base, args } => {
            if let Some(canonical) = surface_types::canonical_runtime_rust_path(base) {
                return IncanType::Generic {
                    base: canonical,
                    args: args.iter().cloned().map(|arg| project_type(arg, facts)).collect(),
                };
            }
        }
        _ => {}
    }
    // Function cache entries describe storage before callable passing facts are retained. Project their components
    // from the original native parameter provenance so an inline collection cannot erase its owned ABI.
    let ty = match ty {
        IncanType::Function { params, return_type } => {
            return IncanType::Function {
                params: params
                    .into_iter()
                    .map(|mut parameter| {
                        parameter.native_owned |= native_collection_parameter(&parameter.ty, facts);
                        parameter.ty = project_type(parameter.ty, facts);
                        parameter
                    })
                    .collect(),
                return_type: Box::new(project_type(*return_type, facts)),
            };
        }
        other => other,
    };
    if let Some(storage) = facts.rust.storage_types.get(&ty) {
        return storage.clone();
    }
    match ty {
        IncanType::RustInteropPath(path) => {
            super::rust_values::project(&path, facts).unwrap_or(IncanType::RustInteropPath(path))
        }
        IncanType::Generic { base, args } => {
            let base = facts.rust.item_public_paths.get(&base).cloned().unwrap_or(base);
            let base = match super::rust_values::project(&base, facts) {
                Some(IncanType::Named(owner)) => owner,
                _ => base,
            };
            let args: Vec<_> = args.into_iter().map(|arg| project_type(arg, facts)).collect();
            IncanType::Generic { base, args }
        }
        IncanType::Named(name) if bir::rust_item_identity(&name).is_some() => IncanType::Named(name),
        IncanType::Tuple(items) => IncanType::Tuple(items.into_iter().map(|item| project_type(item, facts)).collect()),
        IncanType::Ref(inner) => IncanType::Ref(Box::new(project_type(*inner, facts))),
        IncanType::NativeArray { element, length } => IncanType::NativeArray {
            element: Box::new(project_type(*element, facts)),
            length,
        },
        IncanType::RefMut(inner) => IncanType::RefMut(Box::new(project_type(*inner, facts))),
        IncanType::TypeToken(inner) => IncanType::TypeToken(Box::new(project_type(*inner, facts))),
        other => other,
    }
}

/// Retain a source native collection's owned outer ABI before its checked storage becomes a builtin collection.
/// Only an inspected Rust path projected to List, Dict or Set grants this fact; ordinary source collections borrow.
pub(super) fn native_collection_parameter(ty: &IncanType, facts: &TypeCheckInfo) -> bool {
    if !matches!(ty, IncanType::RustInteropPath(_)) {
        return false;
    }
    owned_collection_storage(&project_type(ty.clone(), facts))
}

/// Recognize an owned builtin collection carrier after the accepting checker has projected native storage.
pub(super) fn owned_collection_storage(ty: &IncanType) -> bool {
    matches!(
        ty.collection(),
        Some((
            CollectionTypeId::List | CollectionTypeId::Dict | CollectionTypeId::Set,
            _
        ))
    )
}

/// Collect the nominal names a type mentions, through generic arguments, tuples and references.
fn named_leaves(ty: &IncanType, names: &mut BTreeSet<String>) {
    match ty {
        IncanType::Named(name) => {
            names.insert(name.clone());
        }
        IncanType::Generic { base, args } => {
            names.insert(base.clone());
            for arg in args {
                named_leaves(arg, names);
            }
        }
        IncanType::Tuple(args) => {
            for arg in args {
                named_leaves(arg, names);
            }
        }
        IncanType::Ref(inner) | IncanType::RefMut(inner) | IncanType::NativeArray { element: inner, .. } => {
            named_leaves(inner, names)
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runtime compatibility paths must retain the primitive declaration as their sole nominal owner.
    #[test]
    fn runtime_reexports_project_to_canonical_owner() {
        let facts = TypeCheckInfo::default();
        let error = IncanType::RustInteropPath("incan_std_async::task::TaskJoinError".into());
        let handle = IncanType::Generic {
            base: "incan_std_async::task::JoinHandle".into(),
            args: vec![IncanType::Primitive(IncanPrimitiveType::Int)],
        };

        assert_eq!(
            project_type(error, &facts),
            IncanType::Named("incan_std_primitives::task::TaskJoinError".into())
        );
        assert_eq!(
            project_type(handle, &facts),
            IncanType::Generic {
                base: "incan_std_primitives::task::JoinHandle".into(),
                args: vec![IncanType::Primitive(IncanPrimitiveType::Int)],
            }
        );
    }

    /// Published task members bind their generic owner through the canonical Rust receiver path.
    #[test]
    fn task_member_inventory_retains_canonical_receiver() {
        let mut facts = TypeCheckInfo::default();
        facts.expressions.expr_types.insert(
            crate::ast::Span::new(0, 1),
            crate::symbols::ResolvedType::Generic("JoinHandle".into(), vec![crate::symbols::ResolvedType::Int]),
        );
        let callables = members(&facts);
        let callable = callables.iter().find(|callable| callable.item.ends_with("::abort"));

        assert!(matches!(
            callable.and_then(|callable| callable.parameters.first()),
            Some(IncanType::Ref(owner))
                if matches!(owner.as_ref(), IncanType::Generic { base, args }
                    if base == "incan_std_primitives::task::JoinHandle"
                        && args == &[IncanType::TypeVar("T".into())])
        ));
        assert_eq!(
            callable.map(|callable| callable.native_type_arguments.as_slice()),
            Some(&[IncanType::TypeVar("T".into())][..])
        );
    }
}
