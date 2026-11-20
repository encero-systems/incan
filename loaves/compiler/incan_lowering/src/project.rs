//! Shared declaration context for a project whose individual owning modules have been validated.

use std::sync::Arc;

use incan_semantics_core::body_ir::{BodyIrModule, EnumDeclaration, LoafCallable, NominalDeclaration};

use crate::parallel_ranges::{WORKER_LIMIT, balanced_module_ranges};
use crate::traits::append_specialized_project_trait_bodies;

/// One joined planning context and the source-provenance order of its owning modules.
pub fn project_context(modules: &[BodyIrModule], entry: usize) -> Result<(BodyIrModule, Vec<String>), String> {
    let mut context = selected_context(modules, entry);
    let mut projections = Vec::with_capacity(modules.len());
    for position in 0..modules.len() {
        projections.push(project_module(modules, position)?);
    }
    let owners = merge_project_modules(&mut context, projections)?;
    Ok((context, owners))
}

/// Project body-weighted contiguous module ranges concurrently, then merge their declarations in source order.
pub fn project_context_parallel(
    modules: Arc<Vec<BodyIrModule>>,
    entry: usize,
) -> Result<(BodyIrModule, Vec<String>), String> {
    let mut context = selected_context(&modules, entry);
    let weights: Vec<i64> = modules.iter().map(|module| module.bodies.len().max(1) as i64).collect();
    let ranges = balanced_module_ranges(&weights, WORKER_LIMIT);
    let results: Vec<Result<Vec<BodyIrModule>, String>> = std::thread::scope(|scope| {
        let workers: Vec<_> = ranges
            .iter()
            .map(|&(start, end)| {
                let modules = &modules;
                scope.spawn(move || (start..end).map(|position| project_module(modules, position)).collect())
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap_or_else(|payload| std::panic::resume_unwind(payload)))
            .collect()
    });
    // Workers are joined last range first, so the last failing range names the refusal.
    if let Some(Err(reason)) = results.iter().rev().find(|result| result.is_err()) {
        return Err(reason.clone());
    }
    let mut projections = Vec::with_capacity(modules.len());
    for result in results {
        projections.extend(result?);
    }
    let owners = merge_project_modules(&mut context, projections)?;
    Ok((context, owners))
}

/// The compact joined shell carrying only the selected entry module's unit-level facts.
fn selected_context(modules: &[BodyIrModule], entry: usize) -> BodyIrModule {
    let entry_module = &modules[entry];
    let mut context = BodyIrModule::declarations_only(entry_module.module_id.path().to_owned());
    context.runtime_callable_bindings = entry_module.runtime_callable_bindings.clone();
    context.runtime_binding_scopes = entry_module.runtime_binding_scopes.clone();
    context.entrypoint_effects = entry_module.entrypoint_effects.clone();
    context.program_entrypoints = entry_module.program_entrypoints.clone();
    context.string_conversion_contract = entry_module.string_conversion_contract.clone();
    context.constant_declarations = entry_module.constant_declarations.clone();
    context.type_alias_declarations = entry_module.type_alias_declarations.clone();
    context
}

/// Normalize one validated module's declarations into a mergeable source-order projection.
fn project_module(modules: &[BodyIrModule], position: usize) -> Result<BodyIrModule, String> {
    let module = &modules[position];
    let mut projection = BodyIrModule::declarations_only(module.module_id.path().to_owned());
    append_specialized_project_trait_bodies(module, modules, &mut projection.bodies)?;
    for declaration in &module.nominal_declarations {
        let mut declaration = declaration.clone();
        declaration.name = if declaration.type_parameters.is_empty() {
            declaration.carrier_name()
        } else {
            declaration.template_carrier_name()
        };
        projection.nominal_declarations.push(declaration);
    }
    for declaration in &module.enum_declarations {
        let mut declaration = declaration.clone();
        declaration.name = declaration.carrier_name();
        projection.enum_declarations.push(declaration);
    }
    projection.fieldless_enum_declarations = module.fieldless_enum_declarations.clone();
    projection.value_enum_declarations = module.value_enum_declarations.clone();
    projection.trait_declarations = module.trait_declarations.clone();
    projection.trait_implementations = module.trait_implementations.clone();
    projection.static_declarations = module.static_declarations.clone();
    projection.decorated_bindings = module.decorated_bindings.clone();
    projection.loaf_callables = module.loaf_callables.clone();
    projection.native_union_declarations = module.native_union_declarations.clone();
    Ok(projection)
}

/// Merge independently projected modules in source order under the existing collision rules.
fn merge_project_modules(context: &mut BodyIrModule, projections: Vec<BodyIrModule>) -> Result<Vec<String>, String> {
    let mut owners = Vec::with_capacity(projections.len());
    for projection in projections {
        owners.push(projection.module_id.path().to_owned());
        context.bodies.extend(projection.bodies);
        for declaration in projection.nominal_declarations {
            if !retains_nominal(context, &declaration)? {
                context.nominal_declarations.push(declaration);
            }
        }
        for declaration in projection.enum_declarations {
            if !retains_enum(context, &declaration)? {
                context.enum_declarations.push(declaration);
            }
        }
        context.fieldless_enum_declarations.extend(projection.fieldless_enum_declarations);
        context.value_enum_declarations.extend(projection.value_enum_declarations);
        context.trait_declarations.extend(projection.trait_declarations);
        context.trait_implementations.extend(projection.trait_implementations);
        context.static_declarations.extend(projection.static_declarations);
        context.decorated_bindings.extend(projection.decorated_bindings);
        for declaration in projection.loaf_callables {
            if !retains_callable(context, &declaration) {
                context.loaf_callables.push(declaration);
            }
        }
        context.native_union_declarations.extend(projection.native_union_declarations);
    }
    Ok(owners)
}

/// Reuse an exact authenticated callable record, including its native arguments, signature and declaration frame.
fn retains_callable(context: &BodyIrModule, candidate: &LoafCallable) -> bool {
    context.loaf_callables.iter().any(|callable| callable == candidate)
}

/// Reuse one exact published record; canonical duplicates with differing layout or execution facts remain refused.
fn retains_nominal(context: &BodyIrModule, candidate: &NominalDeclaration) -> Result<bool, String> {
    for declaration in &context.nominal_declarations {
        if declaration.canonical == candidate.canonical {
            if declaration != candidate {
                return Err(format!("unsupported Body IR conflicting project nominal records {}", candidate.name));
            }
            return Ok(true);
        }
        if declaration.name == candidate.name {
            return Err(format!("unsupported Body IR ambiguous project nominal carrier {}", candidate.name));
        }
    }
    Ok(false)
}

/// Keep distinct checked enum owners while refusing conflicting records or unqualified carrier collisions.
fn retains_enum(context: &BodyIrModule, candidate: &EnumDeclaration) -> Result<bool, String> {
    for declaration in &context.enum_declarations {
        if declaration.canonical == candidate.canonical {
            if declaration != candidate {
                return Err(format!("unsupported Body IR conflicting project enum records {}", candidate.name));
            }
            return Ok(true);
        }
        if declaration.carrier_name() == candidate.carrier_name() {
            return Err(format!("unsupported Body IR ambiguous project enum carrier {}", candidate.name));
        }
    }
    Ok(false)
}
