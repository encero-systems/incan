//! Provider-owned generic enum templates and their closed native aliases.
//!
//! A compiled library publishes each generic enum it keeps once, as a template over its own type parameters. Every
//! closed instance the library lowered, such as `Pair[int]`, becomes an alias of that template with exact type
//! arguments, so a consumer that names `Pair[int]` reaches the provider's storage rather than a second copy of the
//! type.

use incan_mir_plan_native::{
    EnumDeclaration, GenericEnumDeclaration, GenericEnumVariant, PlanType, source_span as plan_span,
};
use incan_semantics_core::body_ir::{BodyIrModule, EnumDeclaration as SourceEnumDeclaration};

use crate::generic_layouts::{generic_field_type, referenced_templates};
use crate::layout_types::native_source_type;
use crate::native_type_identity::ConstructorIndex;
use crate::symbols::model_symbol;

/// The generic enum templates a library publishes; each closed instance's entry in `layouts` becomes their alias.
///
/// 1. Publish a template only for a generic source enum the library exports, lowers an instance of, or names from
///    another template.
/// 2. Describe its variants over its own type parameters.
/// 3. Point each closed instance at it, and give the template the derives its declaration and instances carry.
pub fn generic_enums(
    constructors: &ConstructorIndex,
    source: &BodyIrModule,
    closed: &BodyIrModule,
    layouts: &mut [EnumDeclaration],
    intrinsics: &[EnumDeclaration],
    file: &str,
) -> Result<Vec<GenericEnumDeclaration>, String> {
    let referenced = referenced_templates(source);
    let mut templates = Vec::new();
    for declaration in &source.enum_declarations {
        // 1. A private template nothing lowers or names stays in the source.
        if !published(declaration, closed, &referenced) {
            continue;
        }
        // 2. Fields keep their order and name the template's parameters, not an instance's arguments.
        let variants = template_variants(constructors, source, declaration)?;
        // 3. Aliasing changes the instances' layouts in place and reports the derives they had.
        let derives = alias_instances(constructors, declaration, closed, layouts, intrinsics)?;
        templates.push(GenericEnumDeclaration {
            name: declaration.carrier_name(),
            parameters: declaration.type_parameters.clone(),
            variants,
            derives,
            span: plan_span(file, 1, 1),
        });
    }
    Ok(templates)
}

/// Whether a source enum gets a template: it is generic, defined in Incan, and exported, lowered or referenced.
fn published(declaration: &SourceEnumDeclaration, closed: &BodyIrModule, referenced: &[String]) -> bool {
    if declaration.type_parameters.is_empty() || !declaration.native_item.is_empty() {
        return false;
    }
    if declaration.public || referenced.contains(&declaration.carrier_name()) {
        return true;
    }
    closed_instances(declaration, closed).next().is_some()
}

/// The closed instances of `declaration` the library lowered, such as `Pair[int]` for `Pair[T]`.
fn closed_instances<'a>(
    declaration: &'a SourceEnumDeclaration,
    closed: &'a BodyIrModule,
) -> impl Iterator<Item = &'a SourceEnumDeclaration> {
    closed
        .enum_declarations
        .iter()
        .filter(move |instance| instance.canonical == declaration.canonical)
}

/// Each variant of `declaration`, its fields typed over the declaration's own type parameters.
fn template_variants(
    constructors: &ConstructorIndex,
    source: &BodyIrModule,
    declaration: &SourceEnumDeclaration,
) -> Result<Vec<GenericEnumVariant>, String> {
    let mut variants = Vec::with_capacity(declaration.variants.len());
    for variant in &declaration.variants {
        let mut fields = Vec::with_capacity(variant.fields.len());
        for field in &variant.fields {
            fields.push(generic_field_type(
                constructors,
                source,
                field,
                &declaration.type_parameters,
            )?);
        }
        variants.push(GenericEnumVariant {
            name: variant.name.clone(),
            fields,
        });
    }
    Ok(variants)
}

/// Turn each closed instance's layout into an alias of the template, and return the template's derives.
///
/// The template derives what the declaration derives, except `Display`, plus whatever an instance's layout derives. An
/// alias names the template's storage with exact type arguments and has no derives of its own except `Copy`, which it
/// keeps when its own layout is Copy, as a layout naming any library item does.
fn alias_instances(
    constructors: &ConstructorIndex,
    declaration: &SourceEnumDeclaration,
    closed: &BodyIrModule,
    layouts: &mut [EnumDeclaration],
    intrinsics: &[EnumDeclaration],
) -> Result<Vec<String>, String> {
    let mut derives: Vec<String> = declaration
        .derives
        .iter()
        .filter(|derive| *derive != "Display")
        .cloned()
        .collect();
    for instance in closed_instances(declaration, closed) {
        let arguments = instance_arguments(constructors, closed, instance, intrinsics)?;
        let name = model_symbol(&instance.carrier_name());
        for layout in layouts.iter_mut() {
            if layout.name != name {
                continue;
            }
            let own = layout.derives.clone();
            for derive in &own {
                if !derives.contains(derive) {
                    derives.push(derive.clone());
                }
            }
            layout.library_item = format!("crate::{}", declaration.carrier_name());
            layout.library_type_arguments = arguments.clone();
            layout.derives = if own.iter().any(|derive| derive == "Copy") {
                vec!["Copy".to_string()]
            } else {
                Vec::new()
            };
        }
    }
    Ok(derives)
}

/// The exact native type arguments of one closed instance, which its alias applies to the template.
fn instance_arguments(
    constructors: &ConstructorIndex,
    closed: &BodyIrModule,
    instance: &SourceEnumDeclaration,
    intrinsics: &[EnumDeclaration],
) -> Result<Vec<PlanType>, String> {
    // An argument is resolved without the closed module's union carriers.
    let mut arguments = Vec::with_capacity(instance.native_type_arguments.len());
    for argument in &instance.native_type_arguments {
        arguments.push(native_source_type(
            constructors,
            &closed.nominal_declarations,
            &closed.enum_declarations,
            &[],
            argument,
            intrinsics,
            "",
        )?);
    }
    Ok(arguments)
}
