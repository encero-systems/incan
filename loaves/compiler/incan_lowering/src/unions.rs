//! Anonymous unions reuse native enum layouts and legacy's normalized payload order and names.

use std::collections::BTreeSet;
use std::sync::Arc;

use incan_mir_plan_native::{
    Carrier, EnumDeclaration, EnumVariant, ListLeaf, Operand, Place, PlanType, Projection, RvalueKind, SourceSpan,
    carrier_position, list_leaf_type, source_span,
};
use incan_semantics_core::IncanType;
use incan_semantics_core::body_ir::{
    Body, BodyIrModule, EnumDeclaration as SourceEnumDeclaration, NativeUnionDeclaration, NominalDeclaration, Pattern,
    Place as SourcePlace, PlaceElem, StaticInitializer,
};

use crate::compiler_record_adapters::{authored_plan_checked_type, semantic_key_matches_native};
use crate::enums::{enum_owner, validate_payload_patterns};
use crate::frame::Builder;
use crate::layout_types::native_source_type;
use crate::native_type_identity::ConstructorIndex;
use crate::nominal_frames::declaration_storage;

/// Read checked tables through shared entries and retain each union shape once without copying their bodies.
pub fn collect_unions(module: &BodyIrModule) -> Vec<IncanType> {
    let mut unions = Vec::new();
    let mut added = BTreeSet::new();
    let retained = BTreeSet::new();
    for ty in declaration_storage(module) {
        retain_union(&mut unions, &retained, &mut added, &ty);
    }
    for delegation in &module.loaf_callables {
        if !delegation.type_parameters.is_empty() {
            continue;
        }
        for parameter in &delegation.parameters {
            retain_union(&mut unions, &retained, &mut added, parameter);
        }
        retain_union(&mut unions, &retained, &mut added, &delegation.return_type);
    }
    for body in &module.bodies {
        retain_union(&mut unions, &retained, &mut added, &body.return_type);
        for local in &body.locals {
            retain_union(&mut unions, &retained, &mut added, &local.ty);
        }
    }
    for declaration in &module.nominal_declarations {
        for ty in &declaration.field_types {
            retain_union(&mut unions, &retained, &mut added, ty);
        }
    }
    for declaration in &module.enum_declarations {
        for variant in &declaration.variants {
            for ty in &variant.fields {
                retain_union(&mut unions, &retained, &mut added, ty);
            }
        }
    }
    for declaration in &module.static_declarations {
        retain_union(&mut unions, &retained, &mut added, &declaration.ty);
        if let StaticInitializer::Computed(body) = &declaration.initial {
            retain_body_union_types(&mut unions, &retained, &mut added, body);
        }
    }
    unions
}

/// Append union shapes introduced by resolved runtime facts while preserving every existing native slot.
pub fn extend_unions(unions: &mut Vec<IncanType>, module: &BodyIrModule) {
    let mut retained: BTreeSet<IncanType> = unions.iter().cloned().collect();
    for ty in collect_unions(module) {
        if !retained.contains(&ty) {
            retained.insert(ty.clone());
            unions.push(ty);
        }
    }
}

/// Index exact checked types once, independently of the list order that fixes native union slots.
pub fn union_index(unions: &[IncanType]) -> Arc<BTreeSet<IncanType>> {
    Arc::new(unions.iter().cloned().collect())
}

/// Append only closed instance shapes absent from the shared program index, preserving every existing owner slot.
pub fn retain_body_unions(unions: &mut Vec<IncanType>, retained: &BTreeSet<IncanType>, body: &Body) {
    let mut added = BTreeSet::new();
    retain_body_union_types(unions, retained, &mut added, body);
}

fn retain_body_union_types(
    unions: &mut Vec<IncanType>,
    retained: &BTreeSet<IncanType>,
    added: &mut BTreeSet<IncanType>,
    body: &Body,
) {
    retain_union(unions, retained, added, &body.return_type);
    for local in &body.locals {
        retain_union(unions, retained, added, &local.ty);
    }
}

/// Test exact borrowed keys before copying a new union into its ordered slot and the membership index.
fn retain_union(
    unions: &mut Vec<IncanType>,
    retained: &BTreeSet<IncanType>,
    added: &mut BTreeSet<IncanType>,
    ty: &IncanType,
) {
    match ty {
        IncanType::Generic { args, .. } => {
            for argument in args {
                retain_union(unions, retained, added, argument);
            }
            if ty.is_union() && !retained.contains(ty) && !added.contains(ty) {
                added.insert(ty.clone());
                unions.push(ty.clone());
            }
        }
        IncanType::Function { params, return_type } => {
            for parameter in params {
                retain_union(unions, retained, added, &parameter.ty);
            }
            retain_union(unions, retained, added, return_type);
        }
        IncanType::Tuple(elements) => {
            for element in elements {
                retain_union(unions, retained, added, element);
            }
        }
        IncanType::Ref(inner) | IncanType::RefMut(inner) => retain_union(unions, retained, added, inner),
        _ => {}
    }
}

/// Normalize by legacy Rust payload spelling, retaining only scalar and previously admitted nominal members.
pub fn union_members(
    constructors: &ConstructorIndex,
    nominals: &[NominalDeclaration],
    enums: &[SourceEnumDeclaration],
    ty: &IncanType,
    intrinsics: &[EnumDeclaration],
) -> Result<Vec<PlanType>, String> {
    let IncanType::Generic { args, .. } = ty else {
        return Err(format!("unsupported Body IR union type {ty}"));
    };
    if !ty.is_union() {
        return Err(format!("unsupported Body IR union type {ty}"));
    }
    let mut members = Vec::new();
    let mut keys: Vec<String> = Vec::new();
    for argument in args {
        let member = native_source_type(constructors, nominals, enums, &[], argument, intrinsics, "")?;
        if member == PlanType::Unit {
            return Err("unsupported Body IR optional union".into());
        }
        let key = legacy_spelling(&member)
            .map_err(|reason| format!("unsupported Body IR union member {argument}: {reason}"))?;
        if !keys.contains(&key) {
            keys.push(key);
            members.push(member);
        }
    }
    let mut sorted = keys.clone();
    sorted.sort();
    let mut ordered = Vec::with_capacity(members.len());
    for key in &sorted {
        for (index, candidate) in keys.iter().enumerate() {
            if candidate == key {
                ordered.push(members[index].clone());
            }
        }
    }
    if ordered.len() < 2 {
        return Err("unsupported Body IR degenerate union".into());
    }
    Ok(ordered)
}

/// The legacy Rust payload keys used for normalization and generated union identity.
fn legacy_spelling(ty: &PlanType) -> Result<String, String> {
    Ok(match ty {
        PlanType::Int => "i64".into(),
        PlanType::Float => "f64".into(),
        PlanType::Bool => "bool".into(),
        PlanType::String => "String".into(),
        PlanType::FrozenStr => "FrozenStr".into(),
        PlanType::U8 => "u8".into(),
        PlanType::Model(_, name) | PlanType::Enum(_, name) => name.clone(),
        PlanType::List(leaf, depth) => return list_spelling(leaf, *depth),
        _ => return Err("unsupported Body IR union member".into()),
    })
}

/// A scalar-leaf list member as legacy's nested `Vec` payload key; tuple leaves stay refused.
fn list_spelling(leaf: &ListLeaf, depth: i64) -> Result<String, String> {
    let mut spelling = legacy_spelling(&list_leaf_type(leaf))?;
    for _ in 0..depth {
        spelling = format!("Vec<{spelling}>");
    }
    Ok(spelling)
}

/// Legacy's FNV-1a name over normalized Rust payload keys.
fn union_name(members: &[PlanType]) -> Result<String, String> {
    let keys = members.iter().map(legacy_spelling).collect::<Result<Vec<_>, _>>()?;
    let mut hash: u64 = 14_695_981_039_346_656_037;
    for byte in keys.join("|").bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(1_099_511_628_211);
    }
    Ok(format!("__IncanUnion{hash:016x}"))
}

/// Publish the emitted wrapper and payload order, preserving the checker's semantic carrier and nominal owners.
pub fn publish_unions(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    intrinsics: &[EnumDeclaration],
    published: &mut Vec<NativeUnionDeclaration>,
) -> Result<(), String> {
    for ty in collect_unions(module) {
        for declaration in intrinsics {
            if !semantic_key_matches_native(&declaration.semantic_key, &ty)?
                || declaration.carrier != Carrier::Declared
                || !declaration.library_item.is_empty()
            {
                continue;
            }
            let IncanType::Generic { args, .. } = &ty else {
                return Err("invalid Body IR published union carrier".into());
            };
            let mut members: Vec<IncanType> = Vec::new();
            for variant in &declaration.variants {
                for argument in args {
                    let mut native = native_source_type(
                        constructors,
                        &module.nominal_declarations,
                        &module.enum_declarations,
                        &[],
                        argument,
                        intrinsics,
                        "",
                    )?;
                    if let Some(retained) = module
                        .native_union_declarations
                        .iter()
                        .find(|retained| retained.ty == ty)
                    {
                        native = native_union_payload(constructors, module, retained, argument, intrinsics)?;
                    }
                    if variant.fields == [native] {
                        members.push(argument.clone());
                    }
                }
            }
            if members.len() != declaration.variants.len() {
                return Err("invalid Body IR published union payloads".into());
            }
            let mut closure_members: Vec<IncanType> = Vec::new();
            for member in &members {
                for retained in &module.native_union_declarations {
                    if retained.ty == ty
                        && retained.closure_members.contains(member)
                        && !closure_members.contains(member)
                    {
                        closure_members.push(member.clone());
                    }
                }
            }
            published.push(NativeUnionDeclaration {
                ty: ty.clone(),
                rust_item: declaration.name.clone(),
                members,
                closure_members,
                closure_items: declaration.closure_items.clone(),
            });
        }
    }
    Ok(())
}

/// Require retained union carriers: checked Rust generics admit source carriers; linked Loaves need their owner.
pub fn require_native_union_signature(
    ty: &IncanType,
    intrinsics: &[EnumDeclaration],
    source_carriers: bool,
) -> Result<(), String> {
    match ty {
        IncanType::Generic { args, .. } => {
            for argument in args {
                require_native_union_signature(argument, intrinsics, source_carriers)?;
            }
            if !ty.is_union() {
                return Ok(());
            }
            for declaration in intrinsics {
                if semantic_key_matches_native(&declaration.semantic_key, ty)?
                    && (source_carriers || !declaration.library_item.is_empty())
                {
                    return Ok(());
                }
            }
            Err("unsupported Body IR Loaf union without admitted native representation".into())
        }
        IncanType::Tuple(elements) => {
            for element in elements {
                require_native_union_signature(element, intrinsics, source_carriers)?;
            }
            Ok(())
        }
        IncanType::Ref(inner) | IncanType::RefMut(inner) => {
            require_native_union_signature(inner, intrinsics, source_carriers)
        }
        _ => Ok(()),
    }
}

/// Construct a normalized union only after its nested payload layouts have been retained.
///
/// A source-owned union derives Clone and exposes Debug only when every payload supports it. An admitted Loaf alias
/// uses its owner's implementations and callable object types.
pub fn lower_union_declaration(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    ty: &IncanType,
    file: &str,
    intrinsics: &[EnumDeclaration],
) -> Result<EnumDeclaration, String> {
    let mut members: Vec<PlanType> = Vec::new();
    let mut native_item = String::new();
    let mut name = String::new();
    let mut closure_items: Vec<String> = Vec::new();
    let foreign_owner = module
        .native_union_declarations
        .iter()
        .any(|declaration| declaration.ty == *ty && declaration.rust_item.contains("::"));
    for declaration in &module.native_union_declarations {
        if declaration.ty != *ty {
            continue;
        }
        if foreign_owner && !declaration.rust_item.contains("::") {
            continue;
        }
        if !native_item.is_empty() && native_item != declaration.rust_item {
            return Err("unsupported Body IR conflicting published native union owners".into());
        }
        let ordered = published_union_members(constructors, module, declaration, intrinsics)?;
        if !native_item.is_empty() && members != ordered {
            return Err("unsupported Body IR conflicting published union payload order".into());
        }
        members = ordered;
        closure_items = declaration.closure_items.clone();
        let item = declaration.rust_item.clone();
        if item.contains("::") {
            native_item = item.clone();
        }
        name = item.rsplit("::").next().unwrap_or_default().to_owned();
    }
    if name.is_empty() {
        members = union_members(
            constructors,
            &module.nominal_declarations,
            &module.enum_declarations,
            ty,
            intrinsics,
        )?;
        name = union_name(&members)?;
    }
    let variants: Vec<EnumVariant> = members
        .iter()
        .enumerate()
        .map(|(index, member)| EnumVariant {
            name: format!("V{index}"),
            fields: vec![member.clone()],
        })
        .collect();
    if native_item.is_empty() {
        closure_items = members
            .iter()
            .enumerate()
            .filter(|(_, member)| matches!(member, PlanType::Closure(_)))
            .map(|(index, _)| format!("{name}_callable_{index}"))
            .collect();
    }
    let member_debug = native_item.is_empty() && !members.iter().any(|member| matches!(member, PlanType::Closure(_)));
    let mut declaration = EnumDeclaration::new(name, true, variants, vec!["Clone".into()], source_span(file, 1, 1));
    declaration.library_item = native_item;
    declaration.source_type = ty.to_string();
    declaration.semantic_key = Some(authored_plan_checked_type(
        ty,
        file,
        &constructors.source_files,
        &constructors.package_versions,
    )?);
    declaration.member_debug = member_debug;
    declaration.closure_items = closure_items;
    Ok(declaration)
}

/// Validate published payloads against checked semantic members without sorting them or hashing a local name.
fn published_union_members(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    declaration: &NativeUnionDeclaration,
    intrinsics: &[EnumDeclaration],
) -> Result<Vec<PlanType>, String> {
    let mut ordered = Vec::with_capacity(declaration.members.len());
    for member in &declaration.members {
        ordered.push(native_union_payload(
            constructors,
            module,
            declaration,
            member,
            intrinsics,
        )?);
    }
    let IncanType::Generic { args, .. } = &declaration.ty else {
        return Err("invalid Body IR published union carrier".into());
    };
    if !declaration.ty.is_union() || args.len() != ordered.len() {
        return Err("unsupported Body IR admitted union payload mismatch".into());
    }
    if args.iter().any(|argument| !declaration.members.contains(argument)) {
        return Err("unsupported Body IR admitted union payload mismatch".into());
    }
    Ok(ordered)
}

/// Read checked payload ownership once.
fn native_union_payload(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    declaration: &NativeUnionDeclaration,
    member: &IncanType,
    intrinsics: &[EnumDeclaration],
) -> Result<PlanType, String> {
    let payload = native_source_type(
        constructors,
        &module.nominal_declarations,
        &module.enum_declarations,
        &[],
        member,
        intrinsics,
        "",
    )?;
    if !declaration.closure_members.contains(member) {
        return Ok(payload);
    }
    match payload {
        PlanType::FunctionPointer(signature) => Ok(PlanType::Closure(signature)),
        _ => Err("invalid Body IR callable union payload".into()),
    }
}

/// Resolve a union by its exact retained layout slot, independently of neighboring Option and Result layouts.
pub fn retained_union(unions: &[IncanType], owner: i64, intrinsics: &[EnumDeclaration]) -> Result<IncanType, String> {
    let index = carrier_position(owner);
    if index < 0 || index as usize >= intrinsics.len() || intrinsics[index as usize].carrier != Carrier::Declared {
        return Err("unsupported Body IR nonunion enum isinstance".into());
    }
    let carrier = &intrinsics[index as usize];
    for ty in unions {
        if semantic_key_matches_native(&carrier.semantic_key, ty)? {
            return Ok(ty.clone());
        }
    }
    Err("invalid Body IR unretained union layout".into())
}

/// Find one exact normalized member; widening and unrepresented narrowing stay refused.
pub fn union_slot(
    unions: &[IncanType],
    owner: i64,
    member: &PlanType,
    intrinsics: &[EnumDeclaration],
) -> Result<i64, String> {
    let slot = union_test_slot(unions, owner, member, intrinsics)?;
    if slot < 0 {
        return Err("unsupported Body IR union member conversion".into());
    }
    Ok(slot)
}

/// A primitive target absent from the union is always false; source enum tests remain refused.
pub fn union_test_slot(
    unions: &[IncanType],
    owner: i64,
    member: &PlanType,
    intrinsics: &[EnumDeclaration],
) -> Result<i64, String> {
    retained_union(unions, owner, intrinsics)?;
    let variants = &intrinsics[carrier_position(owner) as usize].variants;
    for (slot, variant) in variants.iter().enumerate() {
        if variant.fields.len() == 1 && variant.fields[0] == *member {
            return Ok(slot as i64);
        }
        if let PlanType::FunctionPointer(signature) = member
            && variant.fields.len() == 1
            && variant.fields[0] == PlanType::Closure(signature.clone())
        {
            return Ok(slot as i64);
        }
    }
    Ok(-1)
}

/// Borrow the frame to map a checker-selected union member; ordinary locals never copy declaration tables or frame
/// locals just to decline this projection.
pub fn union_projection(builder: &Builder, value: &SourcePlace, span: &SourceSpan) -> Result<Option<Place>, String> {
    if value.projection.len() != 1 {
        return Ok(None);
    }
    let Some(ty) = projected_member(&value.projection[0]) else {
        return Ok(None);
    };
    let Some(local) = value.local_id() else {
        return Err("unsupported Body IR global union narrowing".into());
    };
    let index = local.index() as i64 + 1 + builder.local_offset;
    let owner = enum_owner(&builder.locals[index as usize].ty)?;
    let member = native_source_type(
        builder.constructors(),
        builder.nominals(),
        builder.enums(),
        &builder.unions,
        ty,
        &builder.intrinsics,
        "",
    )?;
    let slot = union_slot(&builder.unions, owner, &member, &builder.intrinsics)?;
    let payload = builder.intrinsics[carrier_position(owner) as usize].variants[slot as usize].fields[0].clone();
    Ok(Some(Place {
        local: index,
        projection: Projection::VariantField(slot, 0, payload),
        span: span.clone(),
    }))
}

/// Read the checker-selected payload type from one explicit union projection.
fn projected_member(projection: &PlaceElem) -> Option<&IncanType> {
    match projection {
        PlaceElem::UnionMember { ty } => Some(ty),
        _ => None,
    }
}

/// Inject an exact checked Option payload or union member at the shared assignment, argument and return boundary.
pub fn carrier_injection(
    builder: &Builder,
    operand: Operand,
    source: &PlanType,
    target: &PlanType,
) -> Result<RvalueKind, String> {
    let PlanType::Enum(owner, _) = target else {
        return Ok(RvalueKind::Use(operand));
    };
    if *owner < builder.enums().len() as i64 || source == target {
        return Ok(RvalueKind::Use(operand));
    }
    let index = carrier_position(*owner);
    if index < 0 || index as usize >= builder.intrinsics.len() {
        return Err("invalid Body IR union destination owner".into());
    }
    let declaration = &builder.intrinsics[index as usize];
    if declaration.carrier == Carrier::Option {
        if declaration.variants[1].fields.len() != 1 || declaration.variants[1].fields[0] != *source {
            return Err("unsupported Body IR implicit Option payload type".into());
        }
        return Ok(RvalueKind::Enum(*owner, 1, vec![operand]));
    }
    if declaration.carrier != Carrier::Declared {
        return Ok(RvalueKind::Use(operand));
    }
    let slot = union_slot(&builder.unions, *owner, source, &builder.intrinsics)?;
    Ok(RvalueKind::Enum(*owner, slot, vec![operand]))
}

/// Dispatch checker-retained member patterns; subset, literal and binding fallback refinements stay refused.
pub fn union_pattern_slot(builder: &Builder, owner: i64, pattern: &Pattern) -> Result<i64, String> {
    match pattern {
        Pattern::Wildcard => Ok(-1),
        Pattern::UnionMember { ty, fields } => {
            if fields.len() > 1 {
                return Err("unsupported Body IR union payload pattern arity".into());
            }
            validate_payload_patterns(fields)?;
            let member = native_source_type(
                builder.constructors(),
                builder.nominals(),
                builder.enums(),
                &builder.unions,
                ty,
                &builder.intrinsics,
                "",
            )?;
            union_slot(&builder.unions, owner, &member, &builder.intrinsics)
        }
        _ => Err("unsupported Body IR union pattern".into()),
    }
}
