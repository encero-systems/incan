//! Scalar, collection, and nominal layout admission from canonical frontend records.

use incan_mir_plan_native::{ListLeaf, PlanType};
use incan_semantics_core::IncanType;
use incan_semantics_core::body_ir::{NOMINAL_CARRIER_KEY_PREFIX, NominalDeclaration, nominal_carrier_key};

use crate::closures::function_pointer_type;
use crate::collections::primitive_leaf;
use crate::compiler_record_adapters::authored_plan_symbol_id;
use crate::generators::generator_type;
use crate::layout_types::native_source_type as retained_type;
use crate::native_type_identity::{ConstructorIndex, native_owner, representation_arguments};
use crate::nominal_instances::nominal_symbol;
use crate::symbols::model_symbol;
use crate::tuples::{generic_tuple_type, type_name};
use crate::types::primitive_type;

/// Translate checked types against retained declarations, preserving unsupported types in refusals.
pub fn model_type(
    constructors: &ConstructorIndex,
    declarations: &[NominalDeclaration],
    ty: &IncanType,
    source_file: &str,
) -> Result<PlanType, String> {
    if let Some(storage) = runtime_storage_marker(ty) {
        return Ok(storage);
    }
    if let Some(result) = generator_type(ty) {
        return result;
    }
    match ty {
        IncanType::Never => Ok(PlanType::Never),
        IncanType::RustInteropPath(path) => Err(format!("unsupported Body IR type RustInteropPath {path}")),
        IncanType::Named(name) => named_model_type(declarations, name),
        IncanType::Primitive(primitive) => primitive_type(primitive),
        IncanType::TypeToken(represented) => Ok(PlanType::TypeToken(vec![model_type(
            constructors,
            declarations,
            represented,
            source_file,
        )?])),
        IncanType::Trait(identity) => Ok(PlanType::Trait(authored_plan_symbol_id(
            identity,
            source_file,
            &constructors.source_files,
            &constructors.package_versions,
        )?)),
        IncanType::Function { params, return_type } => function_pointer_type(params, return_type),
        IncanType::Tuple(_) => retained_model_type(constructors, declarations, ty),
        IncanType::Decimal { .. } => Ok(PlanType::Decimal),
        IncanType::Generic { base, args } => {
            let collection = (matches!(base.as_str(), "List" | "Set") && args.len() == 1)
                || (base == "Dict" && args.len() == 2)
                || matches!(
                    base.as_str(),
                    "Tuple" | "FrozenList" | "FrozenSet" | "FrozenDict" | "$FrozenList" | "$FrozenSet" | "$FrozenDict"
                );
            if collection {
                return retained_model_type(constructors, declarations, ty);
            }
            if let Some(retained) = retained_nominal_type(constructors, declarations, base, args)? {
                return Ok(retained);
            }
            generic_tuple_type(ty)
        }
        _ => Err(format!("unsupported Body IR type {} ({ty})", type_name(ty))),
    }
}

/// A named nominal: a canonical carrier key selects its one authenticated owner; a plain name its one declaration.
fn named_model_type(declarations: &[NominalDeclaration], retained_name: &str) -> Result<PlanType, String> {
    let carrier_query = retained_name.starts_with(NOMINAL_CARRIER_KEY_PREFIX);
    let mut canonical_owner_count = 0;
    let mut authenticated_owner_count = 0;
    let mut authenticated_owner = None;
    let mut legacy_owner_count = 0;
    let mut legacy_owner = None;
    for (position, declaration) in declarations.iter().enumerate() {
        if carrier_query {
            if nominal_carrier_key(&declaration.canonical).as_deref() != Some(retained_name) {
                continue;
            }
            canonical_owner_count += 1;
            if !declaration.type_parameters.is_empty() || declaration.type_parameter_count != 0 {
                continue;
            }
            if declaration.carrier_name() != retained_name {
                continue;
            }
            authenticated_owner_count += 1;
            authenticated_owner = Some(position);
        } else if declaration.name == retained_name {
            legacy_owner_count += 1;
            legacy_owner = Some(position);
        }
    }
    let owner = if carrier_query && canonical_owner_count == 1 && authenticated_owner_count == 1 {
        authenticated_owner
    } else if !carrier_query && legacy_owner_count == 1 {
        legacy_owner
    } else {
        None
    };
    if let Some(owner) = owner {
        return Ok(PlanType::Model(owner as i64, model_symbol(&declarations[owner].name)));
    }
    let source_name = rejected_nominal_name(declarations, retained_name);
    Err(format!("unsupported Body IR nominal type {source_name}"))
}

/// Normalize one queried specialization once and name the retained sibling keys when its closed layout is missing.
fn retained_nominal_type(
    constructors: &ConstructorIndex,
    declarations: &[NominalDeclaration],
    base: &str,
    arguments: &[IncanType],
) -> Result<Option<PlanType>, String> {
    let Some(owner) = constructors.generic_owners.get(base) else {
        return Ok(None);
    };
    let template = &declarations[*owner];
    let name = if native_owner(template) {
        nominal_symbol(base, &representation_arguments(declarations, constructors, arguments))
    } else {
        nominal_symbol(base, arguments)
    };
    let mut retained: Vec<String> = Vec::new();
    for (index, declaration) in declarations.iter().enumerate() {
        if declaration.canonical != template.canonical {
            continue;
        }
        retained.push(declaration.name.clone());
        if declaration.name == name {
            return Ok(Some(PlanType::Model(index as i64, model_symbol(&declaration.name))));
        }
    }
    Err(format!(
        "unsupported Body IR unretained nominal specialization {name}; retained siblings {retained:?}"
    ))
}

/// Resolve a model carrier with empty enum, union and intrinsic contexts.
fn retained_model_type(
    constructors: &ConstructorIndex,
    declarations: &[NominalDeclaration],
    ty: &IncanType,
) -> Result<PlanType, String> {
    retained_type(constructors, declarations, &[], &[], ty, &[], "")
}

/// Preserve closed native storage without inventing source decimal precision or replacing static loans by owners.
pub fn runtime_storage_marker(ty: &IncanType) -> Option<PlanType> {
    match ty {
        IncanType::Generic { base, args } if args.is_empty() => match base.as_str() {
            "<native-static-text>" => Some(PlanType::StaticText),
            "<native-static-bytes>" => Some(PlanType::StaticBytes),
            "<native-decimal>" => Some(PlanType::Decimal),
            _ => None,
        },
        _ => None,
    }
}

/// A rejected carrier's source name, only through its retained canonical owner.
fn rejected_nominal_name(declarations: &[NominalDeclaration], name: &str) -> String {
    for declaration in declarations {
        if nominal_carrier_key(&declaration.canonical).as_deref() == Some(name) {
            return declaration.canonical.declaration_name.clone();
        }
    }
    name.to_owned()
}

/// Admit a retained model as a set element or dictionary key with derived Hash and derived or selected Eq.
pub fn hashed_element(declarations: &[NominalDeclaration], ty: &IncanType) -> Result<ListLeaf, String> {
    if let IncanType::Named(name) = ty {
        for (index, declaration) in declarations.iter().enumerate() {
            if declaration.name == *name
                && model_equality(declaration)
                && declaration.derives.iter().any(|derive| derive == "Hash")
            {
                return Ok(ListLeaf::Model(index as i64, name.clone()));
            }
        }
    }
    primitive_leaf(ty, true)
}

/// Require a checked derive or the explicit canonical source equality witness before hashed carrier admission.
fn model_equality(declaration: &NominalDeclaration) -> bool {
    declaration.derives.iter().any(|derive| derive == "Eq") || declaration.comparison_equality.is_some()
}
