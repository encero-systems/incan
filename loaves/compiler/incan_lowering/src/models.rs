//! Scalar, collection, and nominal layout admission from canonical frontend records.

use incan_mir_plan_native::{
    Carrier, EnumDeclaration, FieldProjection, ListLeaf, Local, Place, PlanType, Projection, SourceSpan,
    carrier_position, is_carrier_index, tuple_element_type,
};
use incan_semantics_core::body_ir::{
    ArgumentBinding, ConstructorTarget, EnumDeclaration as SourceEnumDeclaration, NOMINAL_CARRIER_KEY_PREFIX,
    NominalDeclaration, Place as SourcePlace, PlaceElem, nominal_carrier_key,
};
use incan_semantics_core::{CanonicalSymbolId, IncanType};

use crate::closures::function_pointer_type;
use crate::collections::primitive_leaf;
use crate::compiler_record_adapters::authored_plan_symbol_id;
use crate::generators::generator_type;
use crate::layout_types::{native_source_type as retained_type, stored_field_type};
use crate::native_type_identity::{ConstructorIndex, native_owner, representation_arguments};
use crate::nominal_instances::nominal_symbol;
use crate::ranges::range_field;
use crate::symbols::model_symbol;
use crate::tuples::{generic_tuple_type, tuple_slot, type_name};
use crate::types::primitive_type;
use crate::unions::union_slot;

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

/// The declaration tables a checked place projects through.
#[derive(Clone, Copy)]
pub struct PlaceTables<'a> {
    pub constructors: &'a ConstructorIndex,
    pub declarations: &'a [NominalDeclaration],
    pub enums: &'a [SourceEnumDeclaration],
    pub unions: &'a [IncanType],
    pub intrinsics: &'a [EnumDeclaration],
}

/// Borrow the frame's tables while requiring each selected field's canonical layout; an unprojected local needs no
/// declaration-table copy. `offset` is the frame's first slot in `locals`; the returned place stays frame-relative.
pub fn model_place(
    tables: PlaceTables<'_>,
    locals: &[Local],
    value: &SourcePlace,
    span: &SourceSpan,
    offset: i64,
) -> Result<Place, String> {
    model_prefix(tables, locals, value, value.projection.len(), span, offset)
}

/// Borrow retained tables to project a checked leading field path without copying or changing the frontend place.
/// Slots are read at `offset` in the shared locals table; the returned place stays frame-relative.
pub fn model_prefix(
    tables: PlaceTables<'_>,
    locals: &[Local],
    value: &SourcePlace,
    count: usize,
    span: &SourceSpan,
    offset: i64,
) -> Result<Place, String> {
    if count > value.projection.len() {
        return Err("invalid Body IR model field prefix length".into());
    }
    let Some(local) = value.local_id() else {
        return Err("unsupported Body IR global place".into());
    };
    let index = local.index() as i64 + 1;
    let place = |projection: Projection| {
        Ok(Place {
            local: index,
            projection,
            span: span.clone(),
        })
    };
    let slot_type = &locals[(index + offset) as usize].ty;
    // RFC 164: a projection of what a reference-valued local borrows (`(*x).field`) is a projection of the local,
    // which reaches its referent's fields itself.
    if count > 1 && matches!(value.projection.first(), Some(PlaceElem::Deref)) {
        let mut through = value.clone();
        through.projection.remove(0);
        return model_prefix(tables, locals, &through, count - 1, span, offset);
    }
    // RFC 164: a whole reference-valued local read or written through its reference (`*x`) names its referent.
    if count == 1 && matches!(value.projection.first(), Some(PlaceElem::Deref)) {
        let (referent, _) = crate::explicit_arguments::reference_contract(slot_type)?;
        return place(Projection::Deref(referent));
    }
    if let Some((owner, name)) = receiver_owner(slot_type) {
        if count == 0 {
            return place(Projection::Deref(PlanType::Model(owner, name)));
        }
        let root = PlanType::Model(owner, tables.declarations[owner as usize].name.clone());
        let fields = field_path(tables, root, &value.projection, count)?;
        if fields.len() == 1 {
            return place(Projection::DerefField(fields[0].slot, fields[0].ty.clone()));
        }
        return place(Projection::DerefFields(fields));
    }
    // A `mut str` or enum parameter, an in-place binding or an in-place loop item reads through its reference.
    if let Some(referent) = exclusive_referent(slot_type) {
        if count == 0 {
            return place(Projection::Deref(referent));
        }
        let fields = field_path(tables, referent, &value.projection, count)?;
        return place(Projection::DerefFields(fields));
    }
    // An enum method's shared receiver reads the whole enum through its reference.
    match slot_type {
        PlanType::TupleRef(elements) => {
            if count == 0 {
                return place(Projection::Whole);
            }
            let fields = field_path(tables, PlanType::Tuple(elements.clone()), &value.projection, count)?;
            return place(Projection::DerefFields(fields));
        }
        PlanType::EnumRef(owner, name) => {
            if count == 0 {
                return place(Projection::Deref(PlanType::Enum(*owner, name.clone())));
            }
            return Err("unsupported Body IR projection through an enum receiver".into());
        }
        _ => {}
    }
    if count == 0 {
        return place(Projection::Whole);
    }
    let mut fields = field_path(tables, slot_type.clone(), &value.projection, count)?;
    if fields.len() == 1 && fields[0].dereferences == 0 {
        let Some(field) = fields.pop() else {
            return Err("invalid Body IR empty field path".into());
        };
        if field.variant >= 0 {
            return place(Projection::VariantField(field.variant, field.slot, field.ty));
        }
        return place(Projection::Field(field.slot, field.ty));
    }
    place(Projection::Fields(fields))
}

/// Resolve ordered checked carrier payloads and canonical fields; indices and slices keep their separate lowering.
pub fn field_path(
    tables: PlaceTables<'_>,
    root: PlanType,
    projections: &[PlaceElem],
    count: usize,
) -> Result<Vec<FieldProjection>, String> {
    let mut ty = root;
    let mut fields: Vec<FieldProjection> = Vec::new();
    for projection in &projections[..count] {
        let steps = field_steps(tables, &ty, projection)?;
        if let Some(last) = steps.last() {
            ty = last.ty.clone();
        }
        fields.extend(steps);
    }
    Ok(fields)
}

/// Select one source field or payload as stored, ending at the value the source names.
///
/// A boxed class field adds a dereference through its box, so reading, borrowing, assigning and moving all reach the
/// stored value; every single-step field projection goes through here.
pub fn field_steps(
    tables: PlaceTables<'_>,
    root: &PlanType,
    projection: &PlaceElem,
) -> Result<Vec<FieldProjection>, String> {
    Ok(stored_steps(field_projection(tables, root, projection)?))
}

/// Continue a projected stored field to the value the source names, dereferencing a boxed class field's box.
pub fn stored_steps(field: FieldProjection) -> Vec<FieldProjection> {
    if let PlanType::Boxed(referent) = &field.ty
        && let Some(inner) = referent.first()
    {
        let mut boxed = FieldProjection::new(-1, inner.clone());
        boxed.dereferences = 1;
        return vec![field, boxed];
    }
    vec![field]
}

/// The type of the value one source field step names, through a boxed class field's box.
pub fn field_step_type(tables: PlaceTables<'_>, root: &PlanType, projection: &PlaceElem) -> Result<PlanType, String> {
    let steps = field_steps(tables, root, projection)?;
    steps
        .last()
        .map(|step| step.ty.clone())
        .ok_or_else(|| "invalid Body IR empty field step".to_string())
}

/// Validate one ordered payload or canonical field through the typed Rust enum boundary required by #459.
fn field_projection(
    tables: PlaceTables<'_>,
    root: &PlanType,
    projection: &PlaceElem,
) -> Result<FieldProjection, String> {
    if let Some(referent) = field_referent(root) {
        let mut field = field_projection(tables, &referent, projection)?;
        field.dereferences += 1;
        return Ok(field);
    }
    let resolve = |ty: &IncanType| {
        retained_type(
            tables.constructors,
            tables.declarations,
            tables.enums,
            tables.unions,
            ty,
            tables.intrinsics,
            "",
        )
    };
    match projection {
        PlaceElem::OptionPayload {
            option_type,
            payload_type,
        } => {
            if resolve(option_type)? != *root {
                return Err("invalid Body IR optional projection owner".into());
            }
            let PlanType::Enum(owner, _) = root else {
                return Err("invalid Body IR optional projection carrier".into());
            };
            if !is_carrier_index(*owner) || carrier_position(*owner) as usize >= tables.intrinsics.len() {
                return Err("invalid Body IR optional projection carrier".into());
            }
            let carrier = &tables.intrinsics[carrier_position(*owner) as usize];
            let ty = resolve(payload_type)?;
            if carrier.carrier != Carrier::Option
                || carrier.variants.len() != 2
                || carrier.variants[1].fields.len() != 1
                || carrier.variants[1].fields[0] != ty
            {
                return Err("invalid Body IR optional projection payload".into());
            }
            let mut field = FieldProjection::new(0, ty);
            field.variant = 1;
            return Ok(field);
        }
        PlaceElem::UnionMember { ty: member } => {
            let PlanType::Enum(owner, _) = root else {
                return Err("invalid Body IR union projection carrier".into());
            };
            let ty = resolve(member)?;
            let variant = union_slot(tables.unions, *owner, &ty, tables.intrinsics)?;
            let mut field = FieldProjection::new(0, ty);
            field.variant = variant;
            return Ok(field);
        }
        _ => {}
    }
    match root {
        PlanType::Model(owner, _) => {
            // Borrow the layout: each field step of every place reaches here, so a copied declaration dominates.
            let declaration = &tables.declarations[*owner as usize];
            let slot = field_slot(projection, declaration)?;
            let ty = stored_field_type(
                tables.constructors,
                tables.declarations,
                tables.enums,
                tables.unions,
                declaration,
                slot,
                tables.intrinsics,
            )?;
            Ok(FieldProjection::new(slot as i64, ty))
        }
        PlanType::Tuple(elements) => {
            let slot = tuple_slot(projection, elements.len() as i64)?;
            Ok(FieldProjection::new(slot, tuple_element_type(&elements[slot as usize])))
        }
        PlanType::RangeValue => range_field(projection),
        _ => Err(format!(
            "unsupported Body IR model index or slice projection at root {root:?}; projection {projection:?}"
        )),
    }
}

/// Retain each reference layer traversed before a canonical field, including references stored in tuple slots.
fn field_referent(ty: &PlanType) -> Option<PlanType> {
    match ty {
        PlanType::ModelRef(owner, name) | PlanType::ModelMutRef(owner, name) => {
            Some(PlanType::Model(*owner, name.clone()))
        }
        PlanType::TupleRef(elements) | PlanType::TupleMutRef(elements) => Some(PlanType::Tuple(elements.clone())),
        PlanType::ScalarRef(elements) | PlanType::ScalarMutRef(elements) if elements.len() == 1 => {
            Some(elements[0].clone())
        }
        _ => None,
    }
}

/// Require an exact stored field or its checked trait requirement binding; never resolve a missing identity by name.
pub fn field_slot(value: &PlaceElem, declaration: &NominalDeclaration) -> Result<usize, String> {
    let PlaceElem::Field {
        name,
        canonical,
        structural,
    } = value
    else {
        return Err("unsupported Body IR model index or slice projection".into());
    };
    if *structural {
        return Err("unsupported Body IR structural model projection".into());
    }
    if let Some(identity) = canonical {
        let selected = checked_field_identity(declaration, identity)?;
        for slot in 0..declaration.fields.len() {
            if declaration.fields[slot] == *name && declaration.field_identities[slot] == selected {
                return Ok(slot);
            }
        }
    }
    Err(format!(
        "unsupported Body IR unproven model field; projection {value:?}; selected owner {:?}; name {:?}; fields {:?}; field identities {:?}; field types {:?}",
        declaration.canonical,
        declaration.name,
        declaration.fields,
        declaration.field_identities,
        declaration.field_types
    ))
}

/// Apply an exact required-field witness once, refusing conflicting bindings instead of selecting by field name.
fn checked_field_identity(
    declaration: &NominalDeclaration,
    identity: &CanonicalSymbolId,
) -> Result<CanonicalSymbolId, String> {
    let mut selected = identity.clone();
    let mut bound = false;
    for adoption in &declaration.trait_adoptions {
        for field in &adoption.fields {
            if field.requirement != *identity {
                continue;
            }
            if bound && selected != field.implementation {
                return Err("invalid Body IR conflicting required field bindings".into());
            }
            selected = field.implementation.clone();
            bound = true;
        }
    }
    Ok(selected)
}

/// The text, enum or tuple value an exclusive reference names; model references keep their receiver projection.
fn exclusive_referent(ty: &PlanType) -> Option<PlanType> {
    match ty {
        PlanType::StringMutRef => Some(PlanType::String),
        PlanType::EnumMutRef(owner, name) => Some(PlanType::Enum(*owner, name.clone())),
        PlanType::TupleMutRef(elements) => Some(PlanType::Tuple(elements.clone())),
        _ => None,
    }
}

/// Identify the nominal owner behind either checked receiver-reference shape.
fn receiver_owner(ty: &PlanType) -> Option<(i64, String)> {
    match ty {
        PlanType::ModelRef(owner, name) | PlanType::ModelMutRef(owner, name) => Some((*owner, name.clone())),
        _ => None,
    }
}

/// Prove the constructor's retained declaration identity, canonical layout, and complete field binding.
pub fn constructor_index(
    declarations: &[NominalDeclaration],
    target: &ConstructorTarget,
    count: usize,
    expected: &PlanType,
) -> Result<usize, String> {
    let Some(identity) = &target.direct_declaration_id else {
        return Err("unsupported Body IR nonlocal model constructor".into());
    };
    for (index, declaration) in declarations.iter().enumerate() {
        match expected {
            PlanType::Model(owner, _) => {
                if index as i64 != *owner {
                    continue;
                }
            }
            _ => return Err("unsupported Body IR nonnominal constructor destination".into()),
        }
        if *identity != declaration.direct_declaration_id {
            continue;
        }
        if target.canonical.as_ref() != Some(&declaration.canonical)
            || target.canonical_field_layout.as_ref() != Some(&declaration.fields)
        {
            return Err("invalid Body IR model constructor identity or layout".into());
        }
        validate_constructor_binding(&target.binding, count, declaration.fields.len())?;
        return Ok(index);
    }
    Err("unsupported Body IR unretained model constructor".into())
}

/// Require every field slot exactly once; Body IR supplies elements in canonical field order.
fn validate_constructor_binding(binding: &ArgumentBinding, count: usize, fields: usize) -> Result<(), String> {
    let ArgumentBinding::Resolved {
        arguments,
        defaulted_slots,
    } = binding
    else {
        return Err("unsupported Body IR unresolved model constructor".into());
    };
    if arguments.len() != count || arguments.len() + defaulted_slots.len() != fields {
        return Err("unsupported Body IR incomplete model constructor".into());
    }
    let mut slots: Vec<usize> = Vec::new();
    for argument in arguments {
        if argument.slot >= fields || slots.contains(&argument.slot) {
            return Err("invalid Body IR model constructor slot map".into());
        }
        slots.push(argument.slot);
    }
    for slot in defaulted_slots {
        if *slot >= fields || slots.contains(slot) {
            return Err("invalid Body IR model default slot map".into());
        }
        slots.push(*slot);
    }
    // Aliases change diagnostic spelling, never the retained constructor identity or physical layout.
    Ok(())
}
