//! SDK runtime-owned surface types retain opaque native storage rather than a consumer-declared layout.

use std::collections::BTreeSet;

use incan_lang::lang::stdlib::facets;
use incan_lang::lang::surface::types::{self as surface_types, SurfaceTypeCategory, SurfaceTypeKind};
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
            let (receiver, names): (IncanType, &[&str]) = match surface_types::from_str(&declaration.name) {
                Some(surface_types::SurfaceTypeId::TaskJoinError) => (
                    IncanType::Named(declaration.name.clone()),
                    &["message", "source", "is_cancelled", "is_panic"],
                ),
                Some(surface_types::SurfaceTypeId::JoinHandle) => (
                    IncanType::Generic {
                        base: declaration.name.clone(),
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
    let (owner, template, parameters) = match receiver {
        IncanType::Named(owner)
            if surface_types::from_str(owner) == Some(surface_types::SurfaceTypeId::TaskJoinError) =>
        {
            (owner, receiver.clone(), Vec::new())
        }
        IncanType::Generic { base, args }
            if surface_types::from_str(base) == Some(surface_types::SurfaceTypeId::JoinHandle) && args.len() == 1 =>
        {
            (
                base,
                IncanType::Generic {
                    base: base.clone(),
                    args: vec![IncanType::TypeVar("T".into())],
                },
                vec!["T".into()],
            )
        }
        _ => return None,
    };
    let declared = nominal(facts, owner)?;
    let mut canonical = declared.canonical.clone();
    canonical.namespace = SymbolNamespace::Member;
    canonical.declaration_name = name.into();
    canonical.kind = SemanticSourceTargetKind::Method;
    let SymbolOrigin::Package { library, module_path } = declared.canonical.origin else {
        return None;
    };
    let mut path = std::iter::once(library)
        .chain(module_path)
        .chain(std::iter::once(owner.clone()))
        .collect::<Vec<_>>();
    let id = surface_types::from_runtime_rust_path(&path.join("::"))?;
    let result = match (id, name) {
        (surface_types::SurfaceTypeId::TaskJoinError, "message") => IncanType::Primitive(IncanPrimitiveType::Str),
        (surface_types::SurfaceTypeId::TaskJoinError, "source") => IncanType::Generic {
            base: "Option".into(),
            args: vec![IncanType::Primitive(IncanPrimitiveType::Str)],
        },
        (surface_types::SurfaceTypeId::TaskJoinError, "is_cancelled" | "is_panic") => {
            IncanType::Primitive(IncanPrimitiveType::Bool)
        }
        (surface_types::SurfaceTypeId::JoinHandle, "abort") => IncanType::Primitive(IncanPrimitiveType::Unit),
        _ => return None,
    };
    path.push(name.into());
    let target = CanonicalSymbolId {
        namespace: SymbolNamespace::Member,
        origin: SymbolOrigin::RustCrate(path),
        declaration_name: name.into(),
        kind: SemanticSourceTargetKind::RustItem,
        scope_discriminant: None,
        declaration_span: HirSourceSpan::new(0, 0),
    };
    Some(bir::LoafCallable {
        canonical,
        target,
        parameters: vec![IncanType::Ref(Box::new(template))],
        return_type: result,
        type_parameters: parameters,
        native_type_arguments: Vec::new(),
        parameter_defaults: Vec::new(),
    })
}

/// Resolve registered task, race and synchronization vocabulary to its runtime facet; source-owned names stay out.
fn nominal(type_info: &TypeCheckInfo, name: &str) -> Option<bir::NominalDeclaration> {
    let id = surface_types::from_str(name)?;
    if !matches!(
        surface_types::category(id),
        SurfaceTypeCategory::AsyncTask | SurfaceTypeCategory::AsyncRace | SurfaceTypeCategory::AsyncSync
    ) {
        return None;
    }
    let info = surface_types::info_for(id);
    let path = surface_types::stdlib_module_path(id)?
        .split('.')
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if let Some(identity) = type_info
        .declarations
        .named_type_identities
        .get(name)
        .or_else(|| type_info.declarations.body_ir_native_type_identities.get(name))
    {
        match &identity.origin {
            SymbolOrigin::Module(owner) if owner == &path => {}
            SymbolOrigin::Package { library, module_path }
                if library == "incan_stdlib_async" && module_path == &path[1..] => {}
            SymbolOrigin::RustCrate(owner) if surface_types::from_runtime_rust_path(&owner.join("::")) == Some(id) => {}
            _ => return None,
        }
    }
    let namespace = path.get(1)?;
    let library = facets::for_namespace(namespace).to_owned();
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
        field_public: Vec::new(),
        field_aliases: Vec::new(),
        reflection_fields: None,
        reflection_metadata: None,
        comparison_equality: None,
        public: true,
        has_field_defaults: false,
        field_default_body: None,
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

/// Project only registry-owned native Rust paths onto the same SDK carrier that source annotations name.
/// Argument splitting uses the existing Rust interop parser; unregistered paths and unsupported arguments retain their
/// refusal.
pub(super) fn project_type(ty: IncanType, facts: &TypeCheckInfo) -> IncanType {
    match ty {
        IncanType::RustInteropPath(path) => project_path(&path, facts)
            .or_else(|| super::rust_values::project(&path, facts))
            .unwrap_or(IncanType::RustInteropPath(path)),
        IncanType::Generic { base, args } => IncanType::Generic {
            base,
            args: args.into_iter().map(|arg| project_type(arg, facts)).collect(),
        },
        IncanType::Tuple(items) => IncanType::Tuple(items.into_iter().map(|item| project_type(item, facts)).collect()),
        IncanType::Ref(inner) => IncanType::Ref(Box::new(project_type(*inner, facts))),
        IncanType::RefMut(inner) => IncanType::RefMut(Box::new(project_type(*inner, facts))),
        other => other,
    }
}

/// Preserve checked SDK ownership and exact argument types when the provider's signature uses native Rust spelling.
fn project_path(path: &str, facts: &TypeCheckInfo) -> Option<IncanType> {
    let ty = project_resolved_path(path, facts)?;
    Some(project_type(semantic_type_from_resolved(&ty), facts))
}

/// Share registry and source-ownership admission with private call inference, retaining the native argument spelling.
/// Named carriers reject generic syntax, and consumer declarations cannot authorize an equal-spelled SDK carrier.
pub(crate) fn project_resolved_path(path: &str, facts: &TypeCheckInfo) -> Option<crate::symbols::ResolvedType> {
    let id = surface_types::from_runtime_rust_path(path)?;
    let name = surface_types::as_str(id);
    // A source declaration may share a spelling, but never authorizes a native runtime path conversion.
    nominal(facts, name)?;
    if surface_types::info_for(id).kind == SurfaceTypeKind::Named {
        return (!path.contains('<')).then(|| crate::symbols::ResolvedType::Named(name.to_owned()));
    }
    let (_, inner) = path.split_once('<')?;
    let inner = inner.strip_suffix('>')?;
    let arguments = incan_lang::interop::split_top_level_rust_args(inner);
    if arguments.len() != 1 {
        return None;
    }
    let parser = crate::typechecker::TypeChecker::new();
    let argument = parser.resolved_type_from_rust_display(arguments[0]);
    Some(crate::symbols::ResolvedType::Generic(name.to_owned(), vec![argument]))
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
        IncanType::Ref(inner) | IncanType::RefMut(inner) => named_leaves(inner, names),
        _ => {}
    }
}
