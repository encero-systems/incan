//! Concrete nominal layouts selected from checked Body IR types, retaining source declaration identities.

use std::collections::BTreeMap;

use incan_semantics_core::body_ir::{
    Body, BodyIrModule, EnumDeclaration, LocalOrigin, NominalDeclaration, TraitImplementation, TraitTypeArgument,
};
use incan_semantics_core::{IncanType, SemanticSourceTargetKind, SymbolOrigin};

use crate::class_inheritance::specialize_classes;
use crate::native_type_identity::{ConstructorIndex, named_positions, native_owner, representation_arguments};
use crate::nominal_frames::{SelectedMember, called_members, closed_arguments, declaration_storage, nominal_frames};
use crate::symbols::type_component;
use crate::type_parameters::{close_result_requirements, close_trait_environment, substitute};

/// A source nominal and one closed argument list occurring in checked frame or field types.
#[derive(Clone, Debug, PartialEq)]
pub struct NominalInstance {
    pub declaration: NominalDeclaration,
    pub arguments: Vec<IncanType>,
    pub ancestors: Vec<String>,
}

/// Match checked nominal uses by their retained carrier key; native owners retain their public Rust path.
pub fn template_base(declaration: &NominalDeclaration) -> String {
    declaration.template_carrier_name()
}

/// Encode a concrete nominal carrier independently of diagnostic aliases and field spellings.
pub fn nominal_symbol(base: &str, arguments: &[IncanType]) -> String {
    let mut name = format!("{base}__incan_instance");
    for argument in arguments {
        name.push_str(&type_component(argument));
    }
    name
}

/// The closed layout's retained arguments; missing specialization facts never trigger another frame traversal.
pub fn instance_arguments(declaration: &NominalDeclaration) -> Result<Vec<IncanType>, String> {
    if !declaration.native_type_arguments.is_empty() {
        return Ok(declaration.native_type_arguments.clone());
    }
    if declaration.type_parameter_count == 0 {
        return Ok(Vec::new());
    }
    Err(format!("standard library type {} has no checked type arguments", declaration.name))
}

/// Collect closed frame carriers, then append missing native layouts while preserving existing owner indices.
pub fn retain_frame_nominals(
    constructors: &ConstructorIndex,
    context: &BodyIrModule,
    templates: &BodyIrModule,
    body: &Body,
    retained_instances: &mut Vec<NominalInstance>,
) -> Result<Vec<NominalDeclaration>, String> {
    let mut instances: Vec<NominalInstance> = Vec::new();
    for local in &body.locals {
        collect_nominal_type(constructors, templates, &local.ty, &mut instances, &[])?;
    }
    collect_nominal_type(constructors, templates, &body.return_type, &mut instances, &[])?;
    let mut added: Vec<NominalDeclaration> = Vec::new();
    let mut cursor = 0;
    while cursor < instances.len() {
        let instance = instances[cursor].clone();
        cursor += 1;
        let declaration = close_layout(&instance)?;
        if retains_declaration(&context.nominal_declarations, &declaration) || retains_declaration(&added, &declaration)
        {
            continue;
        }
        if declaration.native_item.is_empty() && !package_template(&declaration) {
            return Err(format!("unsupported late source nominal instance {}", declaration.name));
        }
        let mut lineage = instance.ancestors.clone();
        lineage.push(instance.declaration.canonical.render_compact());
        if !native_owner(&declaration) {
            for field in &declaration.field_types {
                collect_nominal_type(constructors, templates, field, &mut instances, &lineage)?;
            }
        }
        added.push(declaration);
        retained_instances.push(instance);
    }
    Ok(added)
}

/// An imported source template has consumer-owned closed storage rather than a nonexistent generic Rust item.
fn package_template(declaration: &NominalDeclaration) -> bool {
    matches!(declaration.canonical.origin, SymbolOrigin::Package { .. }) && declaration.type_parameter_count != 0
}

/// Reuse only an identical canonical declaration with the same closed specialization key.
fn retains_declaration(declarations: &[NominalDeclaration], declaration: &NominalDeclaration) -> bool {
    declarations
        .iter()
        .any(|retained| retained.name == declaration.name && retained.canonical == declaration.canonical)
}

/// Retain checked instances beside concrete layouts and dispatch records for native planning.
///
/// 1. Collect checked frame, external-signature and concrete enum-payload instances.
/// 2. Close their field and method dependencies.
/// 3. Retain concrete layouts, trait dispatch records and method frames together.
pub fn specialize_nominals_with_instances(
    constructors: &ConstructorIndex,
    module: &mut BodyIrModule,
    enum_layouts: &[EnumDeclaration],
) -> Result<Vec<NominalInstance>, String> {
    let inherited = specialize_classes(module)?;
    let mut instances: Vec<NominalInstance> = Vec::new();
    let mut selected: Vec<SelectedMember> = Vec::new();
    for callable in &module.loaf_callables {
        if !callable.type_parameters.is_empty() {
            continue;
        }
        for parameter in &callable.parameters {
            collect_nominal_type(constructors, module, parameter, &mut instances, &[])?;
        }
        collect_nominal_type(constructors, module, &callable.return_type, &mut instances, &[])?;
        for argument in &callable.native_type_arguments {
            collect_nominal_type(constructors, module, argument, &mut instances, &[])?;
        }
    }
    for ty in declaration_storage(module) {
        collect_nominal_type(constructors, module, &ty, &mut instances, &[])?;
    }
    for declaration in enum_layouts {
        for variant in &declaration.variants {
            for field in &variant.fields {
                collect_nominal_type(constructors, module, field, &mut instances, &[])?;
            }
        }
    }
    for declaration in &module.static_declarations {
        collect_nominal_type(constructors, module, &declaration.ty, &mut instances, &[])?;
    }
    for body in nominal_frames(constructors, module)? {
        if owner_template(constructors, module, &body)?.is_none()
            && closed_arguments(std::slice::from_ref(&body.return_type))
        {
            retain_selected_members(&body, &mut selected);
        }
        for local in &body.locals {
            collect_nominal_type(constructors, module, &local.ty, &mut instances, &[])?;
        }
        collect_nominal_type(constructors, module, &body.return_type, &mut instances, &[])?;
    }
    let mut declarations: Vec<NominalDeclaration> = Vec::new();
    let mut implementations = nongeneric_implementations(module);
    for declaration in &module.nominal_declarations {
        if !declaration.type_parameters.is_empty() {
            continue;
        }
        declarations.push(declaration.clone());
        // A Rust layout's storage is rustc's. A field a body reads reaches the inventory through that body's checked
        // locals; an unread field must not demand layouts for the native type's private internals.
        if native_owner(declaration) {
            continue;
        }
        let lineage = [declaration.canonical.render_compact()];
        for field in &declaration.field_types {
            collect_nominal_type(constructors, module, field, &mut instances, &lineage)?;
        }
    }
    // 2. Close field and method dependencies until their checked instance inventory is complete. An instance lays out
    // its fields once; after a new member is selected, the worklist revisits only owner frames not yet closed.
    let templates = owner_templates(constructors, module)?;
    let mut pending: Vec<Vec<usize>> = Vec::new();
    let mut cursor = 0;
    while cursor < instances.len() {
        if cursor == pending.len() {
            let owned = lay_out_instance(constructors, module, &mut instances, cursor, &templates)?;
            pending.push(owned);
        }
        if pending[cursor].is_empty() {
            cursor += 1;
            continue;
        }
        let instance = instances[cursor].clone();
        let mut lineage = instance.ancestors.clone();
        lineage.push(instance.declaration.canonical.render_compact());
        let previous = selected.len();
        let waiting = std::mem::take(&mut pending[cursor]);
        pending[cursor] =
            collect_owner_body_types(constructors, module, &instance, &waiting, &mut instances, &lineage, &mut selected)?;
        cursor += 1;
        if selected.len() != previous {
            cursor = 0;
        }
    }
    // 3. Retain concrete layouts, trait dispatch records and method frames together.
    let mut retained_names: BTreeMap<String, bool> = BTreeMap::new();
    for declaration in &declarations {
        retained_names.insert(declaration.name.clone(), true);
    }
    let mut owner_instances: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (position, instance) in instances.iter().enumerate() {
        let declaration = close_layout(instance)?;
        if retained_names.contains_key(&declaration.name) {
            return Err(format!("unsupported Body IR nominal instance symbol collision {}", declaration.name));
        }
        retained_names.insert(declaration.name.clone(), true);
        declarations.push(declaration);
        append_closed_implementations(module, instance, &mut implementations)?;
        owner_instances.entry(instance.declaration.canonical.render_compact()).or_default().push(position);
    }
    let mut bodies: Vec<Body> = inherited;
    for (index, body) in module.bodies.iter().enumerate() {
        let Some(owner) = templates[index] else {
            bodies.push(body.clone());
            continue;
        };
        let canonical = &module.nominal_declarations[owner].canonical;
        if let Some(positions) = owner_instances.get(&canonical.render_compact()) {
            for position in positions {
                let instance = &instances[*position];
                if instance.declaration.canonical == *canonical {
                    let closed = close_owner_body(body, instance)?;
                    if retained_body_types(constructors, module, &closed, &instances)? {
                        bodies.push(closed);
                    }
                }
            }
        }
    }
    module.nominal_declarations = declarations;
    module.trait_implementations = implementations;
    module.bodies = bodies;
    Ok(instances)
}

/// Each body's open receiver template as a declaration index, read once.
fn owner_templates(constructors: &ConstructorIndex, module: &BodyIrModule) -> Result<Vec<Option<usize>>, String> {
    module.bodies.iter().map(|body| owner_template(constructors, module, body)).collect()
}

/// Collect one instance's field instances, then return the owner frames, in module order, it may close.
fn lay_out_instance(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    instances: &mut Vec<NominalInstance>,
    index: usize,
    templates: &[Option<usize>],
) -> Result<Vec<usize>, String> {
    let instance = instances[index].clone();
    let declaration = close_layout(&instance)?;
    let mut lineage = instance.ancestors.clone();
    lineage.push(instance.declaration.canonical.render_compact());
    if !native_owner(&declaration) {
        for field in &declaration.field_types {
            collect_nominal_type(constructors, module, field, instances, &lineage)?;
        }
    }
    Ok(owner_bodies(module, templates, &instance))
}

/// The bodies whose open receiver template is declared as the instance's own declaration.
fn owner_bodies(module: &BodyIrModule, templates: &[Option<usize>], instance: &NominalInstance) -> Vec<usize> {
    let declarations: Vec<usize> = module
        .nominal_declarations
        .iter()
        .enumerate()
        .filter(|(_, declaration)| declaration.canonical == instance.declaration.canonical)
        .map(|(position, _)| position)
        .collect();
    templates
        .iter()
        .enumerate()
        .filter(|(_, owner)| owner.is_some_and(|owner| declarations.contains(&owner)))
        .map(|(body, _)| body)
        .collect()
}

/// Keep records whose owners need no nominal substitution; generic owners are retained per checked instance.
fn nongeneric_implementations(module: &BodyIrModule) -> Vec<TraitImplementation> {
    module
        .trait_implementations
        .iter()
        .filter(|implementation| !has_generic_implementation_owner(module, implementation))
        .cloned()
        .collect()
}

fn has_generic_implementation_owner(module: &BodyIrModule, implementation: &TraitImplementation) -> bool {
    for declaration in &module.nominal_declarations {
        if declaration.canonical == implementation.owner {
            return !declaration.type_parameters.is_empty();
        }
    }
    false
}

/// Close a checked instance's trait-slot and default-body bindings with the same substitution as its fields.
fn append_closed_implementations(
    module: &BodyIrModule,
    instance: &NominalInstance,
    retained: &mut Vec<TraitImplementation>,
) -> Result<(), String> {
    let names = &instance.declaration.type_parameters;
    for implementation in &module.trait_implementations {
        if implementation.owner != instance.declaration.canonical {
            continue;
        }
        let mut closed = implementation.clone();
        closed.slot_arguments.clear();
        for original in &implementation.slot_arguments {
            closed.slot_arguments.push(substitute(original, names, &instance.arguments)?);
        }
        for index in 0..closed.type_arguments.len() {
            closed.type_arguments[index].argument =
                substitute(&implementation.type_arguments[index].argument, names, &instance.arguments)?;
        }
        retained.push(closed);
    }
    Ok(())
}

/// Retain storage from the owner frames among `pending` that a selected member calls; return the rest.
fn collect_owner_body_types(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    instance: &NominalInstance,
    pending: &[usize],
    instances: &mut Vec<NominalInstance>,
    ancestors: &[String],
    selected: &mut Vec<SelectedMember>,
) -> Result<Vec<usize>, String> {
    let mut waiting = Vec::new();
    for index in pending {
        let body = &module.bodies[*index];
        if !selected_owner_body(module, instance, body, selected) {
            waiting.push(*index);
            continue;
        }
        let closed = close_owner_body(body, instance)?;
        retain_selected_members(&closed, selected);
        for local in &closed.locals {
            collect_nominal_type(constructors, module, &local.ty, instances, ancestors)?;
        }
        collect_nominal_type(constructors, module, &closed.return_type, instances, ancestors)?;
    }
    Ok(waiting)
}

/// Match direct and adopted method identities before comparing each selected receiver's closed owner.
fn selected_owner_body(
    module: &BodyIrModule,
    instance: &NominalInstance,
    body: &Body,
    selected: &[SelectedMember],
) -> bool {
    let Some(identity) = &body.canonical else {
        return false;
    };
    if selected
        .iter()
        .any(|member| member.identity == *identity && selected_instance_receiver(&member.receiver, instance))
    {
        return true;
    }
    for implementation in &module.trait_implementations {
        if implementation.owner != instance.declaration.canonical || implementation.implementation != *identity {
            continue;
        }
        if selected.iter().any(|member| {
            implementation.method == member.identity && selected_instance_receiver(&member.receiver, instance)
        }) {
            return true;
        }
    }
    false
}

/// Keep invocation pairs distinct when the same protocol default is called on several owners.
fn retain_selected_members(body: &Body, selected: &mut Vec<SelectedMember>) {
    for member in called_members(body) {
        if !selected
            .iter()
            .any(|retained| retained.identity == member.identity && retained.receiver == member.receiver)
        {
            selected.push(member);
        }
    }
}

/// Match the exact checked receiver against a borrowed owner, stripping only reference carriers.
fn selected_instance_receiver(receiver: &IncanType, instance: &NominalInstance) -> bool {
    match receiver {
        IncanType::Ref(inner) | IncanType::RefMut(inner) => selected_instance_receiver(inner, instance),
        IncanType::Generic { base, args } => {
            *base == template_base(&instance.declaration) && *args == instance.arguments
        }
        _ => false,
    }
}

/// Collect only closed checked types; open placeholders are templates, never arguments inferred from values.
fn collect_nominal_type(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    ty: &IncanType,
    instances: &mut Vec<NominalInstance>,
    ancestors: &[String],
) -> Result<(), String> {
    match ty {
        IncanType::Generic { base, args } => {
            return collect_generic_nominal_type(constructors, module, base, args, instances, ancestors);
        }
        IncanType::Tuple(elements) => {
            for element in elements {
                collect_nominal_type(constructors, module, element, instances, ancestors)?;
            }
        }
        IncanType::Function { params, return_type } => {
            for parameter in params {
                collect_nominal_type(constructors, module, &parameter.ty, instances, ancestors)?;
            }
            collect_nominal_type(constructors, module, return_type, instances, ancestors)?;
        }
        IncanType::Ref(inner) | IncanType::RefMut(inner) => {
            return collect_nominal_type(constructors, module, inner, instances, ancestors);
        }
        _ => {}
    }
    Ok(())
}

/// Admit a closed method frame only when every nominal local and result already has retained storage.
fn retained_body_types(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    body: &Body,
    instances: &[NominalInstance],
) -> Result<bool, String> {
    for local in &body.locals {
        if !retained_type(constructors, module, &local.ty, instances)? {
            return Ok(false);
        }
    }
    retained_type(constructors, module, &body.return_type, instances)
}

/// Check one closed type recursively without extending the specialization inventory during final emission.
fn retained_type(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    ty: &IncanType,
    instances: &[NominalInstance],
) -> Result<bool, String> {
    match ty {
        IncanType::Generic { base, args } => {
            for argument in args {
                if !retained_type(constructors, module, argument, instances)? {
                    return Ok(false);
                }
            }
            if !closed_arguments(args) {
                return Ok(true);
            }
            for index in named_declarations(constructors, module, base)? {
                let declaration = &module.nominal_declarations[index];
                if !admitted_owner(declaration) {
                    continue;
                }
                if declaration.type_parameters.len() != args.len() {
                    return Err(format!("unsupported Body IR nominal type argument arity on {base}"));
                }
                let mut retained_arguments = args.clone();
                if native_owner(declaration) {
                    retained_arguments =
                        representation_arguments(&module.nominal_declarations, constructors, &retained_arguments);
                }
                return Ok(instances.iter().any(|instance| {
                    instance.declaration.canonical == declaration.canonical && instance.arguments == retained_arguments
                }));
            }
            Ok(true)
        }
        IncanType::Tuple(elements) => {
            for element in elements {
                if !retained_type(constructors, module, element, instances)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        IncanType::Function { params, return_type } => {
            for parameter in params {
                if !retained_type(constructors, module, &parameter.ty, instances)? {
                    return Ok(false);
                }
            }
            retained_type(constructors, module, return_type, instances)
        }
        IncanType::Ref(inner) | IncanType::RefMut(inner) => retained_type(constructors, module, inner, instances),
        _ => Ok(true),
    }
}

/// Resolve a closed carrier from its checked declaration; substitute every field template uniformly.
fn collect_generic_nominal_type(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    base: &str,
    arguments: &[IncanType],
    instances: &mut Vec<NominalInstance>,
    ancestors: &[String],
) -> Result<(), String> {
    for argument in arguments {
        collect_nominal_type(constructors, module, argument, instances, ancestors)?;
    }
    if !closed_arguments(arguments) {
        return Ok(());
    }
    for index in named_declarations(constructors, module, base)? {
        let declaration = &module.nominal_declarations[index];
        if !admitted_owner(declaration) {
            continue;
        }
        if declaration.type_parameters.len() != arguments.len() {
            return Err(format!("unsupported Body IR nominal type argument arity on {base}"));
        }
        return retain_nominal_instance(
            constructors,
            &module.nominal_declarations,
            declaration,
            arguments.to_vec(),
            instances,
            ancestors,
        );
    }
    Ok(())
}

/// Reuse exact source or physical native instances before rejecting a recursive field expansion of their owner.
fn retain_nominal_instance(
    constructors: &ConstructorIndex,
    declarations: &[NominalDeclaration],
    declaration: &NominalDeclaration,
    arguments: Vec<IncanType>,
    instances: &mut Vec<NominalInstance>,
    ancestors: &[String],
) -> Result<(), String> {
    let retained_arguments = if native_owner(declaration) {
        representation_arguments(declarations, constructors, &arguments)
    } else {
        arguments
    };
    if instances.iter().any(|retained| {
        retained.declaration.canonical == declaration.canonical && retained.arguments == retained_arguments
    }) {
        return Ok(());
    }
    if ancestors.contains(&declaration.canonical.render_compact()) {
        return Err(format!("unsupported Body IR recursive nominal field instantiation on {}", declaration.name));
    }
    instances.push(NominalInstance {
        declaration: declaration.clone(),
        arguments: retained_arguments,
        ancestors: ancestors.to_vec(),
    });
    Ok(())
}

/// Validate provider templates, close fields and retain source instance arguments.
pub fn close_layout(instance: &NominalInstance) -> Result<NominalDeclaration, String> {
    let template = &instance.declaration;
    let names = &template.type_parameters;
    let mut result = template.clone();
    if !result.native_item.is_empty() {
        for argument in &template.native_type_arguments {
            substitute(argument, names, &instance.arguments)?;
        }
    }
    result.native_type_arguments = instance.arguments.clone();
    result.name = nominal_symbol(&template.name, &instance.arguments);
    result.field_types.clear();
    for ty in &template.field_types {
        result.field_types.push(substitute(ty, names, &instance.arguments)?);
    }
    result.retain_owned_callable_fields(template);
    result.trait_adoptions.clear();
    for adoption in &template.trait_adoptions {
        let mut closed = adoption.clone();
        closed.arguments.clear();
        for argument in &adoption.arguments {
            closed.arguments.push(substitute(argument, names, &instance.arguments)?);
        }
        result.trait_adoptions.push(closed);
    }
    if let Some(body) = &template.field_default_body {
        result.field_default_body = Some(close_owner_body(body, instance)?.into_box());
    }
    for index in 0..result.method_partial_defaults.len() {
        let closed = close_owner_body(&template.method_partial_defaults[index].frame, instance)?;
        result.method_partial_defaults[index].frame = closed.into_box();
    }
    Ok(result)
}

/// Locate an open receiver in the native table, borrowing each declaration instead of copying it.
fn owner_template(constructors: &ConstructorIndex, module: &BodyIrModule, body: &Body) -> Result<Option<usize>, String> {
    if body.locals.is_empty() || !matches!(body.locals[0].origin, LocalOrigin::Receiver { .. }) {
        return Ok(None);
    }
    if let IncanType::Generic { base, .. } = &body.locals[0].ty {
        for index in named_declarations(constructors, module, base)? {
            let declaration = &module.nominal_declarations[index];
            if admitted_owner(declaration) && !declaration.type_parameters.is_empty() {
                return Ok(Some(index));
            }
        }
    }
    Ok(None)
}

/// Positions of the declarations with this name, read from the index that covers this module's table.
fn named_declarations(constructors: &ConstructorIndex, module: &BodyIrModule, name: &str) -> Result<Vec<usize>, String> {
    if constructors.count != module.nominal_declarations.len() {
        return Err("invalid Body IR nominal index for its declaration table".into());
    }
    Ok(named_positions(constructors, name))
}

/// Admit retained source owners and exact native carriers after the original module's layout validation.
fn admitted_owner(declaration: &NominalDeclaration) -> bool {
    !declaration.native_item.is_empty()
        || declaration.canonical.kind == SemanticSourceTargetKind::Class
        || declaration.canonical.kind == SemanticSourceTargetKind::Model
        || declaration.canonical.kind == SemanticSourceTargetKind::Newtype
}

/// Close receiver and field-dependent frame types while leaving the method's own parameters open for scheduling.
pub fn close_owner_body(body: &Body, instance: &NominalInstance) -> Result<Body, String> {
    let mut names = instance.declaration.type_parameters.clone();
    let mut arguments = instance.arguments.clone();
    for name in &body.type_parameters {
        if names.contains(name) {
            return Err(format!("unsupported Body IR shadowed nominal and method type parameter {name}"));
        }
        names.push(name.clone());
        arguments.push(IncanType::TypeVar(name.clone()));
    }
    let mut result = body.clone();
    let owner_bindings: Vec<TraitTypeArgument> = instance
        .declaration
        .type_parameters
        .iter()
        .zip(&instance.arguments)
        .map(|(parameter, argument)| TraitTypeArgument { parameter: parameter.clone(), argument: argument.clone() })
        .collect();
    result.set_nominal_instantiation(owner_bindings);
    result.trait_default_instantiation = close_trait_environment(&body.trait_default_instantiation, &names, &arguments)?;
    for local in &mut result.locals {
        local.ty = substitute(&local.ty, &names, &arguments)?;
    }
    for parameter in &mut result.params {
        parameter.ty = substitute(&parameter.ty, &names, &arguments)?;
    }
    result.return_type = substitute(&result.return_type, &names, &arguments)?;
    close_result_requirements(&mut result, &names, &arguments)?;
    Ok(result)
}
