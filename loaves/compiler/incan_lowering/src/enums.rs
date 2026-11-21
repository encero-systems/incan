//! Canonical normal-enum layouts and identity-checked native construction and pattern admission.

use incan_mir_plan_native::{EnumDeclaration as PlanEnumDeclaration, EnumVariant, PlanType, source_span};
use incan_semantics_core::IncanType;
use incan_semantics_core::body_ir::{
    BodyIrModule, Constant, EnumDeclaration as SourceEnumDeclaration, ValueEnumBacking,
};

use crate::decls::{ViewLifetimeLayout, library_enum_item, serde_propagation, with_serde_derives};
use crate::layout_types::native_source_type;
use crate::native_type_identity::ConstructorIndex;

use crate::nominal_instances::nominal_symbol;
use crate::symbols::model_symbol;

/// A retained source enum, recognized before the model and scalar resolver runs.
pub fn enum_type(declarations: &[SourceEnumDeclaration], ty: &IncanType) -> Option<PlanType> {
    let name = match ty {
        IncanType::Named(name) => name.clone(),
        IncanType::Generic { base, args } => nominal_symbol(base, args),
        _ => return None,
    };
    declarations
        .iter()
        .position(|declaration| declaration.carrier_name() == name)
        .map(|index| PlanType::Enum(index as i64, model_symbol(&name)))
}

/// Copy checked source-order variants and derives into the native declaration plan.
///
/// A source enum also takes the serde derives a serde-deriving declaration's fields require of it. Value enums retain
/// the same checked raw values for explicit and propagated serde selections. Source Display uses its checked Debug
/// path rather than a native Rust derive. Native enums retain only the checked Copy fact verified on their SDK type.
pub fn lower_enum_declarations(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    file: &str,
    intrinsics: &[PlanEnumDeclaration],
) -> Result<Vec<PlanEnumDeclaration>, String> {
    let reached = serde_propagation(module)?;
    let nominals = &module.nominal_declarations;
    let layouts = &module.enum_declarations;
    let mut declarations = Vec::with_capacity(layouts.len());
    let mut view_lifetimes = ViewLifetimeLayout::start(nominals, layouts);
    for declaration in layouts {
        let mut variants = Vec::with_capacity(declaration.variants.len());
        for variant in &declaration.variants {
            let mut fields = Vec::with_capacity(variant.fields.len());
            for ty in &variant.fields {
                fields.push(native_source_type(
                    constructors,
                    nominals,
                    layouts,
                    &[],
                    ty,
                    intrinsics,
                    "",
                )?);
            }
            variants.push(EnumVariant {
                name: variant.name.clone(),
                fields,
            });
        }
        // A stdlib enum is the SDK crate's own type; its derives and storage belong to that crate.
        let item = library_enum_item(declaration).to_owned();
        let mut native_arguments = Vec::new();
        if !item.is_empty() {
            for argument in &declaration.native_type_arguments {
                native_arguments.push(native_source_type(
                    constructors,
                    nominals,
                    layouts,
                    &[],
                    argument,
                    intrinsics,
                    "",
                )?);
            }
        }
        let selected = with_serde_derives(declaration.derives.clone(), &declaration.name, &reached);
        let mut derives: Vec<String> = Vec::new();
        if item.is_empty() {
            derives = selected.into_iter().filter(|derive| derive != "Display").collect();
        } else if declaration.derives.iter().any(|derive| derive == "Copy") {
            derives.push("Copy".into());
        }
        let (raw_kind, raw_values) = value_enum_raw_values(module, declaration)?;
        let mut view_lifetime_count = 0;
        if item.is_empty() {
            view_lifetime_count = view_lifetimes.enum_declaration(declaration, nominals, layouts);
        }
        let mut planned = PlanEnumDeclaration::new(
            model_symbol(&declaration.carrier_name()),
            declaration.public || !item.is_empty(),
            variants,
            derives,
            source_span(file, 1, 1),
        );
        planned.view_lifetime_count = view_lifetime_count;
        planned.library_item = item;
        planned.library_type_arguments = native_arguments;
        planned.raw_kind = raw_kind;
        planned.raw_values = raw_values;
        declarations.push(planned);
    }
    Ok(declarations)
}

/// A source value enum's backing and raw values in variant order; an SDK enum keeps its serialization in its crate.
fn value_enum_raw_values(
    module: &BodyIrModule,
    declaration: &SourceEnumDeclaration,
) -> Result<(String, Vec<String>), String> {
    if !library_enum_item(declaration).is_empty() {
        return Ok((String::new(), Vec::new()));
    }
    for value_enum in &module.value_enum_declarations {
        if value_enum.canonical != declaration.canonical {
            continue;
        }
        let mut raw_values = Vec::with_capacity(value_enum.variants.len());
        for variant in &value_enum.variants {
            match &variant.raw_value {
                Constant::Str(text) => raw_values.push(text.clone()),
                Constant::Int(number) => raw_values.push(number.to_string()),
                _ => return Err("invalid Body IR value enum raw value".into()),
            }
        }
        return Ok(match value_enum.backing {
            ValueEnumBacking::Str => ("str".into(), raw_values),
            ValueEnumBacking::Int => ("int".into(), raw_values),
        });
    }
    Ok((String::new(), Vec::new()))
}

/// Require an enum-plan scrutinee; scalar, Option and Result matches stay named refusals.
pub fn enum_owner(ty: &PlanType) -> Result<i64, String> {
    match ty {
        PlanType::Enum(index, _) => Ok(*index),
        _ => Err("unsupported Body IR Match outside source enums".into()),
    }
}
