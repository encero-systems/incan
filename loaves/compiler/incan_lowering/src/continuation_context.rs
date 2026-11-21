//! Select newly resolved runtime modules and merge their specialized facts into retained lowering context.

use std::sync::Arc;

use incan_semantics_core::body_ir::{Body, BodyIrModule, EnumDeclaration, NominalDeclaration};

use crate::native_type_identity::{ConstructorIndex, extend_constructor_index};
use crate::nominal_instances::NominalInstance;

/// The refreshed entry and runtime modules that contribute facts the retained contexts do not contain.
pub struct ContinuationInputs {
    pub modules: Arc<Vec<BodyIrModule>>,
    pub sources: Vec<String>,
    pub files: Vec<String>,
    pub entry: usize,
}

/// Refresh the compact entry-owned facts the frontend can extend while resolving runtime declarations.
pub fn refresh_entry_context(context: &mut BodyIrModule, entry: &BodyIrModule) {
    context.runtime_callable_bindings = entry.runtime_callable_bindings.clone();
    context.runtime_binding_scopes = entry.runtime_binding_scopes.clone();
    context.loaf_callables = entry.loaf_callables.clone();
    context.entrypoint_effects = entry.entrypoint_effects.clone();
    context.program_entrypoints = entry.program_entrypoints.clone();
    context.string_conversion_contract = entry.string_conversion_contract.clone();
    context.constant_declarations = entry.constant_declarations.clone();
    context.type_alias_declarations = entry.type_alias_declarations.clone();
}

/// Select changed modules and their entry, preserving the entry's invocation-only contracts in the reduced slice.
pub fn continuation_inputs(
    source_context: &BodyIrModule,
    context: &BodyIrModule,
    owners: &[String],
    modules: &[BodyIrModule],
    sources: &[String],
    files: &[String],
    entry: usize,
) -> Result<ContinuationInputs, String> {
    if modules.len() != sources.len() || modules.len() != files.len() {
        return Err("invalid Body IR continuation source provenance".into());
    }
    if entry >= modules.len() {
        return Err("invalid Body IR continuation entry".into());
    }
    let changed = modules.iter().any(|module| {
        let owner = module.module_id.path();
        !owners.iter().any(|known| known == owner) || adds_checked_facts(source_context, context, module)
    });
    if !changed {
        return Ok(ContinuationInputs {
            modules: Arc::new(Vec::new()),
            sources: Vec::new(),
            files: Vec::new(),
            entry: 0,
        });
    }
    let mut additions = Vec::new();
    let mut addition_sources = Vec::new();
    let mut addition_files = Vec::new();
    let mut addition_entry = 0;
    for (position, module) in modules.iter().enumerate() {
        let owner = module.module_id.path();
        if position != entry
            && owners.iter().any(|known| known == owner)
            && !adds_checked_facts(source_context, context, module)
        {
            continue;
        }
        if position == entry {
            addition_entry = additions.len();
        }
        additions.push(module.clone());
        addition_sources.push(sources[position].clone());
        addition_files.push(files[position].clone());
    }
    Ok(ContinuationInputs {
        modules: Arc::new(additions),
        sources: addition_sources,
        files: addition_files,
        entry: addition_entry,
    })
}

/// Extend the retained source and execution contexts from the refreshed module graph.
///
/// Existing facts keep their positions. The added context has already passed the ordinary project specialization
/// path, so raw generic receiver frames never enter the retained executable context.
#[allow(clippy::too_many_arguments)]
pub fn extend_continuation_context(
    source_context: &mut BodyIrModule,
    context: &mut BodyIrModule,
    template_constructors: &mut ConstructorIndex,
    owners: &mut Vec<String>,
    retained_sources: &mut Vec<String>,
    retained_files: &mut Vec<String>,
    template_references: &mut Vec<String>,
    retained_nominals: &mut Vec<NominalInstance>,
    additions: &ContinuationInputs,
    added_source_context: &BodyIrModule,
    added_context: &BodyIrModule,
    added_template_references: &[String],
    added_nominals: &[NominalInstance],
) -> Result<(), String> {
    for (position, module) in additions.modules.iter().enumerate() {
        let owner = module.module_id.path();
        if !owners.iter().any(|known| known == owner) {
            owners.push(owner.to_owned());
            retained_sources.push(additions.sources[position].clone());
            retained_files.push(additions.files[position].clone());
        }
    }
    for reference in added_template_references {
        if !template_references.contains(reference) {
            template_references.push(reference.clone());
        }
    }
    extend_source_declarations(source_context, added_source_context)?;
    extend_execution_context(context, added_context)?;
    extend_nominal_instances(retained_nominals, added_nominals);
    extend_constructor_index(template_constructors, &source_context.nominal_declarations);
    Ok(())
}

/// Append raw nominal and enum template authority for later instance and publication work.
fn extend_source_declarations(source: &mut BodyIrModule, module: &BodyIrModule) -> Result<(), String> {
    for declaration in &module.nominal_declarations {
        let mut declaration = declaration.clone();
        declaration.name = if declaration.type_parameters.is_empty() {
            declaration.carrier_name()
        } else {
            declaration.template_carrier_name()
        };
        if !retains_source_nominal(&source.nominal_declarations, &declaration)? {
            source.nominal_declarations.push(declaration);
        }
    }
    for declaration in &module.enum_declarations {
        let mut declaration = declaration.clone();
        declaration.name = declaration.carrier_name();
        if !retains_source_enum(&source.enum_declarations, &declaration)? {
            source.enum_declarations.push(declaration);
        }
    }
    Ok(())
}

/// Append specialized executable facts while leaving every position from the prior attempt stable.
fn extend_execution_context(context: &mut BodyIrModule, module: &BodyIrModule) -> Result<(), String> {
    let prior_body_count = context.bodies.len();
    for body in &module.bodies {
        if !has_body_prefix(context, body, prior_body_count) && !has_body_specialization(context, body) {
            context.bodies.push(body.clone());
        }
    }
    append_concrete_nominals(context, module)?;
    append_concrete_enums(context, module)?;
    for declaration in &module.fieldless_enum_declarations {
        if !context.fieldless_enum_declarations.contains(declaration) {
            context.fieldless_enum_declarations.push(declaration.clone());
        }
    }
    for declaration in &module.value_enum_declarations {
        if !context.value_enum_declarations.contains(declaration) {
            context.value_enum_declarations.push(declaration.clone());
        }
    }
    for declaration in &module.trait_declarations {
        if !context.trait_declarations.contains(declaration) {
            context.trait_declarations.push(declaration.clone());
        }
    }
    for implementation in &module.trait_implementations {
        if !context.trait_implementations.contains(implementation) {
            context.trait_implementations.push(implementation.clone());
        }
    }
    append_runtime_storage(context, module);
    Ok(())
}

/// Append each newly specialized nominal key once while preserving the prior attempt's layout order.
fn extend_nominal_instances(retained: &mut Vec<NominalInstance>, additions: &[NominalInstance]) {
    for addition in additions {
        let found = retained.iter().any(|instance| {
            instance.declaration.canonical == addition.declaration.canonical && instance.arguments == addition.arguments
        });
        if !found {
            retained.push(addition.clone());
        }
    }
}

/// Detect a refreshed module whose declarations, bodies, or runtime storage are absent from retained context.
fn adds_checked_facts(source: &BodyIrModule, context: &BodyIrModule, module: &BodyIrModule) -> bool {
    module
        .nominal_declarations
        .iter()
        .any(|declaration| !has_nominal(&source.nominal_declarations, declaration))
        || module
            .enum_declarations
            .iter()
            .any(|declaration| !has_enum(&source.enum_declarations, declaration))
        || module
            .fieldless_enum_declarations
            .iter()
            .any(|d| !context.fieldless_enum_declarations.contains(d))
        || module
            .value_enum_declarations
            .iter()
            .any(|d| !context.value_enum_declarations.contains(d))
        || module
            .trait_declarations
            .iter()
            .any(|d| !context.trait_declarations.contains(d))
        || module
            .trait_implementations
            .iter()
            .any(|d| !context.trait_implementations.contains(d))
        || module.bodies.iter().any(|body| !has_body(context, body))
        || module
            .static_declarations
            .iter()
            .any(|d| !context.static_declarations.contains(d))
        || module
            .decorated_bindings
            .iter()
            .any(|d| !context.decorated_bindings.contains(d))
        || module
            .loaf_callables
            .iter()
            .any(|d| !context.loaf_callables.contains(d))
        || module
            .native_union_declarations
            .iter()
            .any(|d| !context.native_union_declarations.contains(d))
}

fn has_nominal(declarations: &[NominalDeclaration], candidate: &NominalDeclaration) -> bool {
    declarations
        .iter()
        .any(|declaration| declaration.canonical == candidate.canonical)
}

fn has_enum(declarations: &[EnumDeclaration], candidate: &EnumDeclaration) -> bool {
    declarations
        .iter()
        .any(|declaration| declaration.canonical == candidate.canonical)
}

fn has_body(context: &BodyIrModule, candidate: &Body) -> bool {
    has_body_prefix(context, candidate, context.bodies.len())
}

/// Whether the context prefix predating one merge already represents the candidate's canonical body.
fn has_body_prefix(context: &BodyIrModule, candidate: &Body, count: usize) -> bool {
    let prefix = &context.bodies[..count];
    match &candidate.canonical {
        Some(identity) => prefix
            .iter()
            .any(|retained| retained.canonical.as_ref() == Some(identity)),
        None => prefix
            .iter()
            .any(|retained| retained.canonical.is_none() && retained.name == candidate.name),
    }
}

/// Whether a body with the same canonical identity and closed frame signature entered during this merge.
fn has_body_specialization(context: &BodyIrModule, candidate: &Body) -> bool {
    context.bodies.iter().any(|retained| {
        retained.canonical == candidate.canonical
            && retained.params == candidate.params
            && retained.locals == candidate.locals
            && retained.return_type == candidate.return_type
            && retained.type_parameters == candidate.type_parameters
    })
}

/// Append only physical nominal layouts; open templates stay in the retained source context.
fn append_concrete_nominals(context: &mut BodyIrModule, module: &BodyIrModule) -> Result<(), String> {
    for declaration in &module.nominal_declarations {
        if !declaration.type_parameters.is_empty() {
            continue;
        }
        let mut declaration = declaration.clone();
        declaration.name = declaration.carrier_name();
        if !retains_concrete_nominal(&context.nominal_declarations, &declaration)? {
            context.nominal_declarations.push(declaration);
        }
    }
    Ok(())
}

/// Append only physical enum layouts; open templates stay in the retained source context.
fn append_concrete_enums(context: &mut BodyIrModule, module: &BodyIrModule) -> Result<(), String> {
    for declaration in &module.enum_declarations {
        if !declaration.type_parameters.is_empty() {
            continue;
        }
        let mut declaration = declaration.clone();
        declaration.name = declaration.carrier_name();
        if !retains_concrete_enum(&context.enum_declarations, &declaration)? {
            context.enum_declarations.push(declaration);
        }
    }
    Ok(())
}

/// Append new static, decorator, callable, and union records in refreshed module order.
fn append_runtime_storage(context: &mut BodyIrModule, module: &BodyIrModule) {
    for declaration in &module.static_declarations {
        if !context.static_declarations.contains(declaration) {
            context.static_declarations.push(declaration.clone());
        }
    }
    for binding in &module.decorated_bindings {
        if !context.decorated_bindings.contains(binding) {
            context.decorated_bindings.push(binding.clone());
        }
    }
    for callable in &module.loaf_callables {
        if !context.loaf_callables.contains(callable) {
            context.loaf_callables.push(callable.clone());
        }
    }
    for declaration in &module.native_union_declarations {
        if !context.native_union_declarations.contains(declaration) {
            context.native_union_declarations.push(declaration.clone());
        }
    }
}

fn retains_source_nominal(declarations: &[NominalDeclaration], candidate: &NominalDeclaration) -> Result<bool, String> {
    for declaration in declarations {
        if declaration.canonical != candidate.canonical {
            continue;
        }
        if declaration != candidate {
            return Err(format!(
                "unsupported Body IR conflicting continuation nominal records {}",
                candidate.name
            ));
        }
        return Ok(true);
    }
    Ok(false)
}

fn retains_source_enum(declarations: &[EnumDeclaration], candidate: &EnumDeclaration) -> Result<bool, String> {
    for declaration in declarations {
        if declaration.canonical != candidate.canonical {
            continue;
        }
        if declaration != candidate {
            return Err(format!(
                "unsupported Body IR conflicting continuation enum records {}",
                candidate.name
            ));
        }
        return Ok(true);
    }
    Ok(false)
}

fn retains_concrete_nominal(
    declarations: &[NominalDeclaration],
    candidate: &NominalDeclaration,
) -> Result<bool, String> {
    for declaration in declarations {
        if declaration.canonical != candidate.canonical
            || declaration.native_type_arguments != candidate.native_type_arguments
        {
            continue;
        }
        if declaration != candidate {
            return Err(format!(
                "unsupported Body IR conflicting continuation nominal layouts {}",
                candidate.name
            ));
        }
        return Ok(true);
    }
    Ok(false)
}

fn retains_concrete_enum(declarations: &[EnumDeclaration], candidate: &EnumDeclaration) -> Result<bool, String> {
    for declaration in declarations {
        if declaration.canonical != candidate.canonical
            || declaration.native_type_arguments != candidate.native_type_arguments
        {
            continue;
        }
        if declaration != candidate {
            return Err(format!(
                "unsupported Body IR conflicting continuation enum layouts {}",
                candidate.name
            ));
        }
        return Ok(true);
    }
    Ok(false)
}
