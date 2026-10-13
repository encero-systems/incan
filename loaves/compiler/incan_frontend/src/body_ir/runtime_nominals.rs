//! SDK runtime-owned surface types retain opaque native storage rather than a consumer-declared layout.

use std::collections::BTreeSet;

use incan_lang::lang::stdlib::facets;
use incan_lang::lang::surface::types::{self as surface_types, SurfaceTypeCategory, SurfaceTypeKind};
use incan_semantics_core::{CanonicalSymbolId, CompilerNodeId, HirSourceSpan, SemanticSourceTargetKind, SymbolOrigin};

use super::{IncanType, TypeCheckInfo, bir, semantic_type_from_resolved};

/// Retain checked task carriers, including the join error synthesized as an await result, with their SDK owner.
/// A source declaration with the same spelling remains source-owned; opaque records never authorize field access.
pub(super) fn collect(type_info: &TypeCheckInfo) -> Vec<bir::NominalDeclaration> {
    let mut names = BTreeSet::new();
    for ty in type_info.expressions.expr_types.values() {
        super::library_calls::named_leaves(&project_type(semantic_type_from_resolved(ty), type_info), &mut names);
    }
    for binding in type_info.declarations.function_bindings.values() {
        super::library_calls::named_leaves(
            &project_type(semantic_type_from_resolved(&binding.return_type), type_info),
            &mut names,
        );
        for parameter in &binding.params {
            super::library_calls::named_leaves(
                &project_type(semantic_type_from_resolved(&parameter.ty), type_info),
                &mut names,
            );
        }
    }
    names.into_iter().filter_map(|name| nominal(type_info, &name)).collect()
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
    if let Some(identity) = type_info.declarations.named_type_identities.get(name) {
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
        public: true,
        has_field_defaults: false,
        field_default_body: None,
        method_partial_defaults: Vec::new(),
        display_method: None,
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
        IncanType::RustInteropPath(path) => project_path(&path, facts).unwrap_or(IncanType::RustInteropPath(path)),
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
    let id = surface_types::from_runtime_rust_path(path)?;
    let name = surface_types::as_str(id);
    // A source declaration may share a spelling, but never authorizes a native runtime path conversion.
    nominal(facts, name)?;
    if surface_types::info_for(id).kind == SurfaceTypeKind::Named {
        return (!path.contains('<')).then(|| IncanType::Named(name.to_owned()));
    }
    let (_, inner) = path.split_once('<')?;
    let inner = inner.strip_suffix('>')?;
    let arguments = incan_lang::interop::split_top_level_rust_args(inner);
    if arguments.len() != 1 {
        return None;
    }
    let parser = crate::typechecker::TypeChecker::new();
    let argument = parser.resolved_type_from_rust_display(arguments[0]);
    let argument = project_type(semantic_type_from_resolved(&argument), facts);
    Some(IncanType::Generic {
        base: name.to_owned(),
        args: vec![argument],
    })
}
