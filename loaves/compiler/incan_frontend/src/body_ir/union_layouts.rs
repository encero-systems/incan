//! Retain structural union payload order and callable ownership before native lowering.

use incan_semantics_core::{IncanType, body_ir as bir};

/// Collect source-owned layouts from checked storage; selected artifact layouts may replace these local owners.
pub(super) fn retain(module: &mut bir::BodyIrModule) {
    tracing::debug!(target: "incan::invocation", module = %module.module_id, "retaining union payload contracts");
    let mut types = Vec::new();
    for body in &module.bodies {
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
    for ty in types {
        let IncanType::Generic { args, .. } = &ty else { continue };
        if !args.iter().any(|ty| matches!(ty, IncanType::Function { .. })) {
            continue;
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
        module.native_union_declarations.push(bir::NativeUnionDeclaration {
            ty,
            rust_item: format!("__IncanUnion{hash:016x}"),
            members,
            closure_members,
            closure_items: Vec::new(),
        });
    }
    tracing::debug!(target: "incan::invocation", module = %module.module_id, count = module.native_union_declarations.len(), "retained union payload contracts");
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
