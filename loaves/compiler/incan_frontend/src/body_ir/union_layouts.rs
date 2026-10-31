//! Retain structural union payload order and callable ownership before native lowering.

use incan_semantics_core::{IncanType, body_ir as bir};

/// Collect source-owned layouts from checked storage; selected artifact layouts may replace these local owners.
pub(super) fn retain(module: &mut bir::BodyIrModule) {
    tracing::debug!(target: "incan::invocation", module = %module.module_id, "retaining union payload contracts");
    let mut types = Vec::new();
    for body in &module.bodies {
        append_types(&mut types, body_types(body));
    }
    for declaration in &module.nominal_declarations {
        append_types(&mut types, storage_types(&declaration.field_types));
    }
    for declaration in &module.enum_declarations {
        for variant in &declaration.variants {
            append_types(&mut types, storage_types(&variant.fields));
        }
    }
    for declaration in &module.static_declarations {
        append_types(&mut types, storage_types([&declaration.ty]));
    }
    module
        .native_union_declarations
        .extend(types.into_iter().filter_map(layout));
    tracing::debug!(target: "incan::invocation", module = %module.module_id, count = module.native_union_declarations.len(), "retained union payload contracts");
}

/// Produce only one body's ordered union-shape contribution. Parameter storage is already present in its locals;
/// another body's locals and the module inventory are not inputs to this operation.
pub(super) fn body_types(body: &bir::Body) -> Vec<IncanType> {
    storage_types(std::iter::once(&body.return_type).chain(body.locals.iter().map(|local| &local.ty)))
}

/// Collect ordered union dependencies from one owner's checked storage types. Empty contributions are valid;
/// a record-service caller must supply schema, profile and witnesses when publishing them.
pub(super) fn storage_types<'a>(types: impl IntoIterator<Item = &'a IncanType>) -> Vec<IncanType> {
    let mut unions = Vec::new();
    for ty in types {
        collect(ty, &mut unions);
    }
    unions
}

/// Assemble independent ordered contributions without changing the compatibility module's first-occurrence order.
/// This joins type references only; canonical layouts are produced once after duplicate references are removed.
pub(super) fn append_types(retained: &mut Vec<IncanType>, contribution: impl IntoIterator<Item = IncanType>) {
    for ty in contribution {
        if !retained.contains(&ty) {
            retained.push(ty);
        }
    }
}

/// Preserve the existing canonical member ordering and native wrapper name for one structural union. A union
/// without an immediate callable payload does not require a native callable wrapper.
pub(super) fn layout(ty: IncanType) -> Option<bir::NativeUnionDeclaration> {
    let IncanType::Generic { args, .. } = &ty else {
        return None;
    };
    if !args.iter().any(|ty| matches!(ty, IncanType::Function { .. })) {
        return None;
    }
    let mut members = args.clone();
    members.sort_by_cached_key(|member| format!("{member:?}"));
    members.dedup();
    let keys = format!("{members:?}");
    let hash = keys.bytes().fold(14695981039346656037_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(1099511628211)
    });
    let closure_members = members
        .iter()
        .filter(|ty| matches!(ty, IncanType::Function { .. }))
        .cloned()
        .collect();
    Some(bir::NativeUnionDeclaration {
        ty,
        rust_item: format!("__IncanUnion{hash:016x}"),
        members,
        closure_members,
        closure_items: Vec::new(),
    })
}

/// Traverse checked carrier structure once, preserving each exact semantic union identity.
fn collect(ty: &IncanType, types: &mut Vec<IncanType>) {
    match ty {
        IncanType::Generic { base, args } => {
            for argument in args {
                collect(argument, types);
            }
            if base == "Union" && !types.contains(ty) {
                types.push(ty.clone());
            }
        }
        IncanType::Function { params, return_type } => {
            for parameter in params {
                collect(&parameter.ty, types);
            }
            collect(return_type, types);
        }
        IncanType::Tuple(elements) => {
            for element in elements {
                collect(element, types);
            }
        }
        IncanType::Ref(inner) | IncanType::RefMut(inner) => collect(inner, types),
        _ => {}
    }
}

/// Whether any checked type in `modules` is a union: a program without one selects no union layout, so the Loaves
/// that publish layouts need not be read for it.
pub fn reaches_union(modules: &[bir::BodyIrModule]) -> bool {
    let mut types = Vec::new();
    for module in modules {
        let frames = module.bodies.iter().chain(module.callable_declaration_frames());
        for body in frames {
            collect(&body.return_type, &mut types);
            for local in &body.locals {
                collect(&local.ty, &mut types);
            }
        }
        for declaration in &module.nominal_declarations {
            for ty in &declaration.field_types {
                collect(ty, &mut types);
            }
        }
        for declaration in &module.enum_declarations {
            for variant in &declaration.variants {
                for ty in &variant.fields {
                    collect(ty, &mut types);
                }
            }
        }
        for declaration in &module.static_declarations {
            collect(&declaration.ty, &mut types);
        }
        if !types.is_empty() {
            return true;
        }
    }
    false
}
